//! Entry path for Module 1: turn a normalised [`LaunchEvent`] into a position
//! through one deterministic, staged pipeline (TASK 2 §C/§F/§I/§J/§K).
//!
//! ```text
//!  DETECTED ──► VALIDATED ──► RISK_APPROVED ──► EXECUTION_READY ──► SUBMITTED ──► CONFIRMED
//!     │             │               │                  │                 │
//!     └── REJECTED ─┴───────────────┴──────────────────┘                 └── FAILED
//! ```
//!
//! Checks, in order (each one names the machine-readable reason it emits):
//!
//! | # | check | reason |
//! |---|-------|--------|
//! | 1 | event shape (pubkeys, signature, identity, timestamps) | `INVALID_EVENT` |
//! | 2 | protocol routable under the live config | `INVALID_ROUTE` |
//! | 3 | kill switch / emergency halt | `KILL_SWITCH` |
//! | 4 | module enabled, `risk.sniper_emergency_disable` off | `STRATEGY_DISABLED` |
//! | 5 | launch age ≤ `sniper.max_launch_age_secs` | `STALE_EVENT` |
//! | 6 | symbol not gated by unresolved reconciliation | `SYMBOL_GATED` |
//! | 7 | authoritative dedup (`AppState::mark_launch_seen`) | `DUPLICATE_EVENT` |
//! | 8 | static screening (`RiskEngine::check_launch_with_lists`) | `RISK_REJECTED` |
//! | 9 | RPC provider pool healthy | `EXECUTION_UNAVAILABLE` |
//! | 10 | wallet balance readable | `EXECUTION_UNAVAILABLE` |
//! | 11 | venue readable, route resolvable, pool exists | `EXECUTION_UNAVAILABLE` / `INVALID_ROUTE` / `POOL_NOT_READY` |
//! | 12 | safety gates (`gates::evaluate`) | `POOL_NOT_READY` / `TOKEN_STATE_INVALID` / `INSUFFICIENT_LIQUIDITY` / `CONCENTRATION_LIMIT` / `STALE_EVENT` |
//! | 13 | slippage engine finds an allowed tolerance | `SLIPPAGE_LIMIT` |
//! | 14 | modelled price impact ≤ `sniper.max_price_impact_bps` | `PRICE_IMPACT_LIMIT` |
//! | 15 | fee budget: executor fee policy accepts the configured priority fee; worst-case tx fee ≤ `sniper.max_entry_fee_lamports` | `FEE_LIMIT` |
//! | 16 | `RiskEngine::check_entry` (sizing, exposure, cooldowns, caps) | `EXPOSURE_LIMIT` / `SLIPPAGE_LIMIT` / `KILL_SWITCH` / `STRATEGY_DISABLED` / `RISK_REJECTED` |
//! | 17 | distributed ownership claim | `OWNERSHIP_LOST` |
//! | 18 | transaction built for the route | `EXECUTION_UNAVAILABLE` |
//! | 19 | entry latency budget + snapshot freshness at submit time | `STALE_EVENT` |
//!
//! Execution always goes through the hardened engine
//! ([`solana_kit::execute::Executor`] — ledger, deterministic intent ids, fee
//! policy, reconciliation) with the write-ahead intent journal and the
//! ownership permit around it, exactly as before this pipeline existed.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::maths;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, TokenLaunch, Trade, TradeSource, Venue,
};
use bot_core::risk::EntryRequest;

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::{ExecStatus, ExecutionResult};
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::pump::{self, BuildOptions};
use solana_kit::pumpswap;
use solana_kit::tx::TxRequest;

use crate::event::{raw_hash_of, LaunchEvent};
use crate::gates::{self, MarketSnapshot};
use crate::market::{load_market, MarketData, VenueData};
use crate::pipeline::{
    check_fee_budget, count_event, count_rejection, count_stage, observe_slippage, precheck,
    EntryRoute, LatencyTimeline, Lifecycle, PrecheckContext, RejectReason, Rejection, SniperStage,
};
use crate::slippage::{self, SlippageDecision, SlippageInputs, SlippageMode};
use crate::{available_sol, Sniper};

/// Sequence numbers for events that enter through the legacy
/// [`TokenLaunch`] door (manual / server-injected launches).
static MANUAL_SEQ: AtomicU64 = AtomicU64::new(0);

/// What one pass through the pipeline produced. Returned to callers and
/// tests; the run loop only needs to know whether an infrastructure error
/// occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryOutcome {
    pub event_id: String,
    pub mint: String,
    pub stage: SniperStage,
    pub rejection: Option<Rejection>,
    pub route: Option<EntryRoute>,
    /// Deterministic execution-intent id, once a route was chosen.
    pub intent_id: Option<String>,
    pub position_id: Option<String>,
    pub slippage_bps: Option<u64>,
    pub price_impact_bps: Option<u64>,
    /// Worst-case transaction fee the entry was budgeted at (check 15), in
    /// lamports, once the route was known.
    pub fee_estimate_lamports: Option<u64>,
    pub timeline: LatencyTimeline,
    /// `gate=outcome,…` for the audit trail.
    pub gates: String,
}

impl EntryOutcome {
    pub fn accepted(&self) -> bool {
        self.rejection.is_none()
            && matches!(self.stage, SniperStage::Submitted | SniperStage::Confirmed)
    }

    /// True when the refusal was an infrastructure problem (RPC, balance,
    /// venue read, build, execution engine) rather than a decision — the
    /// run loop records these against the module.
    pub fn infra_failure(&self) -> bool {
        matches!(
            &self.rejection,
            Some(r) if r.reason == RejectReason::ExecutionUnavailable
        )
    }
}

/// Everything the build step needs, gathered by the validation steps.
struct Approved {
    market: MarketData,
    slippage: SlippageDecision,
    sized_lamports: u64,
}

/// A transaction ready for the execution engine.
enum Prepared {
    /// Instructions the executor builds, signs and lifecycles itself.
    Request {
        req: TxRequest,
        expected_out_raw: u64,
        spend_lamports: u64,
        base_decimals: u8,
    },
    /// A Jupiter-signed transaction handed to the executor's lifecycle
    /// (`send_prebuilt`), or nothing at all in paper mode.
    Jupiter {
        built: Option<solana_kit::tx::BuiltTx>,
        label: String,
        intent_id: String,
        expected_out_raw: u64,
        spend_lamports: u64,
        base_decimals: u8,
    },
}

impl Sniper {
    /// Legacy entry point: wrap a [`TokenLaunch`] (pump.fun protocol) into a
    /// [`LaunchEvent`] and run the pipeline. Kept so existing callers and
    /// manual launches keep working.
    pub async fn consider_launch(&mut self, launch: TokenLaunch) -> BotResult<()> {
        let raw = serde_json::to_vec(&launch)
            .map(|b| raw_hash_of(&b))
            .unwrap_or_else(|_| raw_hash_of(launch.mint.as_bytes()));
        let seq = MANUAL_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let event = LaunchEvent::from_token_launch(launch, seq, raw);
        let outcome = self.consider_event(event).await;
        match outcome.rejection {
            Some(r) if outcome.infra_failure() => Err(BotError::rpc(format!(
                "snipe {}: {}",
                outcome.mint, r.detail
            ))),
            _ => Ok(()),
        }
    }

    /// Evaluate one normalised launch event and, if it passes every gate,
    /// buy it. Never fails: every refusal — decision or infrastructure — is
    /// described by the returned outcome (`EntryOutcome::infra_failure`
    /// tells the two apart).
    pub async fn consider_event(&mut self, event: LaunchEvent) -> EntryOutcome {
        let now = Utc::now();
        count_event(event.protocol, &event.source.to_string());
        let mut lc = Lifecycle::start(
            &event.event_id,
            event.protocol,
            event.effective_ts(),
            event.observed_at,
            now,
        );
        count_stage(SniperStage::Detected, event.protocol);

        let mut route = None;
        let mut intent_id = None;
        let mut slippage_bps = None;
        let mut price_impact_bps = None;
        let mut fee_estimate_lamports = None;
        let mut gates_summary = String::new();
        let mut position_id = None;

        let result = self
            .run_pipeline(
                &event,
                &mut lc,
                &mut route,
                &mut intent_id,
                &mut slippage_bps,
                &mut price_impact_bps,
                &mut fee_estimate_lamports,
                &mut gates_summary,
                &mut position_id,
            )
            .await;

        let rejection = result.err();
        lc.timeline.record_metrics(event.protocol);

        let outcome = EntryOutcome {
            event_id: event.event_id.clone(),
            mint: event.mint.clone(),
            stage: lc.stage,
            rejection: rejection.clone(),
            route,
            intent_id,
            position_id,
            slippage_bps,
            price_impact_bps,
            fee_estimate_lamports,
            timeline: lc.timeline.clone(),
            gates: gates_summary,
        };
        self.publish_audit(&event, &lc, &outcome).await;
        outcome
    }

    /// The staged checks. Every `Err` is a terminal rejection already
    /// applied to `lc`; `Ok` means the execution engine was handed the
    /// transaction (the result of which is reflected in `lc.stage`).
    #[allow(clippy::too_many_arguments)]
    async fn run_pipeline(
        &mut self,
        event: &LaunchEvent,
        lc: &mut Lifecycle,
        route_out: &mut Option<EntryRoute>,
        intent_out: &mut Option<String>,
        slippage_out: &mut Option<u64>,
        impact_out: &mut Option<u64>,
        fee_out: &mut Option<u64>,
        gates_out: &mut String,
        position_out: &mut Option<String>,
    ) -> Result<(), Rejection> {
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let risk_cfg = cfg.risk.clone();
        let now = Utc::now();

        // ---- DETECTED → VALIDATED --------------------------------------
        // 1.–6. Shape, route, kill switch, enabled/emergency, age, symbol
        //       gate — the pure `precheck` shared with the replay engine.
        //       The symbol gate (§H) refuses NEW entries while the symbol has
        //       unresolved reconciliation claims and runs before dedup so a
        //       gated symbol does not consume its one-shot launch slot. The
        //       kill/enabled checks are cheap early exits; the risk engine
        //       re-checks them authoritatively in step 16.
        let precheck_ctx = PrecheckContext {
            kill_switch: self.state.kill_switch(),
            module_enabled: self.state.is_enabled(BotModule::Sniper).await,
            emergency_disable: risk_cfg.sniper_emergency_disable,
            symbol_gated: self.state.is_symbol_blocked(&event.mint).await,
        };
        if let Err((reason, detail)) = precheck(event, &sniper, precheck_ctx, now) {
            if reason == RejectReason::SymbolGated {
                bot_core::obs::metrics::global()
                    .counter(
                        "bot_symbol_gated_entries_total",
                        "Entries refused because the symbol is gated by unresolved reconciliation.",
                        &[("module", "sniper")],
                    )
                    .inc();
            }
            return Err(self.reject(lc, event, reason, detail));
        }
        // 7. The one authoritative dedup: feeds race to report the same
        //    launch; only the first observation wins.
        if !self.state.mark_launch_seen(&event.dedup_key()).await {
            return Err(self.reject(
                lc,
                event,
                RejectReason::DuplicateEvent,
                format!("launch {} already seen", event.dedup_key()),
            ));
        }
        // 8. Static screening (denylists, creator buy, market cap, age).
        let age_secs = (event.age_ms(now) / 1_000) as i64;
        let screen = self.risk.check_launch_with_lists(
            &event.launch,
            &risk_cfg,
            &sniper.creator_denylist,
            &sniper.keyword_denylist,
            &[], // known-bad-creator list is not persisted yet
            age_secs,
            sniper.max_launch_age_secs,
        );
        let (accepted, reason) = match &screen {
            Ok(()) => (true, None),
            Err(r) => (false, Some(r.clone())),
        };
        self.state.events.publish(AppEvent::Launch {
            ts: Utc::now(),
            launch: Box::new(event.launch.clone()),
            accepted,
            reason: reason.clone(),
        });
        if let Some(r) = reason {
            return Err(self.reject(
                lc,
                event,
                RejectReason::RiskRejected,
                format!("screening: {r}"),
            ));
        }
        self.advance(lc, event, SniperStage::Validated)?;
        info!(
            event = %event.event_id,
            protocol = %event.protocol,
            mint = %event.mint,
            symbol = %event.launch.symbol,
            name = %event.launch.name,
            cap_sol = event.launch.market_cap_sol,
            feed = %event.source,
            age_ms = event.age_ms(now),
            "launch validated — evaluating entry"
        );

        // ---- VALIDATED → RISK_APPROVED ---------------------------------
        let approved = self
            .validate_market_and_risk(
                event,
                lc,
                &cfg,
                gates_out,
                route_out,
                slippage_out,
                impact_out,
                fee_out,
            )
            .await?;
        self.advance(lc, event, SniperStage::RiskApproved)?;

        // ---- RISK_APPROVED → EXECUTION_READY ---------------------------
        // 17. Distributed execution ownership (§B/§F): exactly one replica
        //     may execute this launch. Claimed AFTER risk so a rejected
        //     entry never consumes a claim; an unavailable ownership store
        //     fails closed (§K).
        let mint = event.mint_pubkey().ok_or_else(|| {
            self.reject(lc, event, RejectReason::InvalidEvent, "mint unparseable")
        })?;
        let mut permit = match bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("snipe:{}", event.mint),
            "entry",
            "sniper",
            "launch",
            &event.launch.symbol,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                return Err(self.reject(
                    lc,
                    event,
                    RejectReason::ExecutionUnavailable,
                    format!("ownership store: {e}"),
                ))
            }
        };
        if !permit.proceed() {
            return Err(self.reject(
                lc,
                event,
                RejectReason::OwnershipLost,
                "snipe owned by another replica",
            ));
        }

        // 18. Build for the chosen route.
        let route = approved.market.route;
        let intent = snipe_intent_id(event, &mint, route.as_str());
        *intent_out = Some(intent.clone());
        let prepared = match self.prepare(event, &approved, &mint, &intent, &cfg).await {
            Ok(p) => p,
            Err(e) => {
                permit.finish(false).await;
                return Err(self.reject(
                    lc,
                    event,
                    RejectReason::ExecutionUnavailable,
                    format!("build ({route}): {e}"),
                ));
            }
        };

        // 19. Latency budget + snapshot freshness, measured right before the
        //     hand-off: chasing a launch we are already late on, or acting
        //     on numbers older than the operator allows, is refused here.
        let pre_submit = Utc::now();
        let held = event.held_ms(pre_submit);
        if sniper.max_entry_latency_ms > 0 && held > sniper.max_entry_latency_ms {
            permit.finish(false).await;
            return Err(self.reject(
                lc,
                event,
                RejectReason::StaleEvent,
                format!(
                    "entry latency {held} ms exceeds budget {} ms",
                    sniper.max_entry_latency_ms
                ),
            ));
        }
        let snapshot_age = approved.market.snapshot.age_ms(pre_submit);
        if snapshot_age > sniper.max_snapshot_age_ms {
            permit.finish(false).await;
            return Err(self.reject(
                lc,
                event,
                RejectReason::StaleEvent,
                format!(
                    "market snapshot {snapshot_age} ms old at submit (max {} ms)",
                    sniper.max_snapshot_age_ms
                ),
            ));
        }
        if let Err(r) = self.advance(lc, event, SniperStage::ExecutionReady) {
            permit.finish(false).await;
            return Err(r);
        }

        // ---- EXECUTION_READY → SUBMITTED → CONFIRMED / FAILED ----------
        // The kill switch is re-read at the last moment: one engaged while
        // the market was read or the transaction built must stop the
        // hand-off — the execution engine itself has no view of it.
        if self.state.kill_switch() {
            permit.finish(false).await;
            return Err(self.reject(
                lc,
                event,
                RejectReason::KillSwitch,
                "kill switch engaged before submission",
            ));
        }
        // An attempt is about to leave the process: record it for the
        // per-token cooldown whatever the outcome.
        self.state.note_entry_attempt(&event.mint).await;
        // Honour runtime mode/gate changes made since startup.
        self.refresh_policy().await;
        // Fencing (§E): ownership must still be ours at broadcast time.
        if let Err(e) = permit.fence().await {
            return Err(self.reject(
                lc,
                event,
                RejectReason::OwnershipLost,
                format!("fenced before broadcast: {e}"),
            ));
        }
        if let Err(r) = self.advance(lc, event, SniperStage::Submitted) {
            permit.finish(false).await;
            return Err(r);
        }

        let (result, expected_out_raw, spend_lamports, base_decimals, mode_live) = match prepared {
            Prepared::Request {
                req,
                expected_out_raw,
                spend_lamports,
                base_decimals,
            } => {
                // Write-ahead intent (§I crash point C): journaled BEFORE
                // broadcast, linked to the signature (or abandoned) after.
                let journal = self.intent_rec(&event.mint, "buy", &expected_out_raw.to_string());
                let res = bot_core::recovery::with_intent(
                    self.intents.as_ref(),
                    journal,
                    self.executor.run(req),
                    |r| r.broadcast_signature(),
                )
                .await;
                match res {
                    Ok(r) => (r, expected_out_raw, spend_lamports, base_decimals, true),
                    Err(e) => {
                        permit.finish(false).await;
                        self.state.note_failed_entry(&event.mint).await;
                        return Err(self.fail(lc, event, format!("execution engine: {e}")));
                    }
                }
            }
            Prepared::Jupiter {
                built,
                label,
                intent_id,
                expected_out_raw,
                spend_lamports,
                base_decimals,
            } => {
                let mode = self.state.execution_mode().await;
                let res: BotResult<ExecutionResult> = match (mode, built) {
                    (ExecutionMode::Paper, _) | (_, None) => {
                        let mut r = ExecutionResult::empty(
                            &label,
                            &intent_id,
                            mode == ExecutionMode::Paper,
                        );
                        if mode == ExecutionMode::Paper {
                            r.status = ExecStatus::PaperFilled;
                            r.state = bot_core::execution::ExecutionState::Confirmed;
                        } else {
                            // Simulate-success: nothing broadcast; counts as
                            // `Sent` for bookkeeping, as before.
                            r.status = ExecStatus::Sent;
                            r.state = bot_core::execution::ExecutionState::Validated;
                        }
                        r.attempts = 1;
                        Ok(r)
                    }
                    (_, Some(built)) => match self.state.may_broadcast().await {
                        Err(e) => Err(e),
                        Ok(()) => {
                            let journal =
                                self.intent_rec(&event.mint, "buy", &spend_lamports.to_string());
                            bot_core::recovery::with_intent(
                                self.intents.as_ref(),
                                journal,
                                self.executor.send_prebuilt(&built),
                                |r| r.broadcast_signature(),
                            )
                            .await
                        }
                    },
                };
                match res {
                    Ok(r) => (
                        r,
                        expected_out_raw,
                        spend_lamports,
                        base_decimals,
                        mode == ExecutionMode::Live,
                    ),
                    Err(e) => {
                        permit.finish(false).await;
                        self.state.note_failed_entry(&event.mint).await;
                        return Err(self.fail(lc, event, format!("jupiter execution: {e}")));
                    }
                }
            }
        };

        // Ownership terminal (§I/§M): an unproven outcome (Sent/SendUnknown)
        // hands the execution to reconciliation — the claim stays blocked
        // for the grace window so no replica resubmits; a determinate
        // outcome releases it. Paper/simulate never moved money → release.
        permit
            .finish(mode_live && result.status.is_ambiguous())
            .await;

        let latency_ms = event.held_ms(Utc::now());
        if !result.succeeded() {
            self.state.note_failed_entry(&event.mint).await;
            self.state.inc_orders_sent(BotModule::Sniper).await;
            self.state.inc_orders_failed(BotModule::Sniper).await;
            let detail = format!(
                "{:?}{}: {}",
                result.status,
                result
                    .failure
                    .map(|f| format!(" ({})", f.as_str()))
                    .unwrap_or_default(),
                result.error.as_deref().unwrap_or("order did not fill")
            );
            self.state.record_error(BotModule::Sniper, &detail).await;
            self.state.events.publish(AppEvent::Error {
                ts: Utc::now(),
                module: Some(BotModule::Sniper),
                message: format!("snipe {} {detail}", event.launch.symbol),
                fatal: false,
            });
            warn!(symbol = %event.launch.symbol, %detail, "snipe did not fill");
            return Err(self.fail(lc, event, detail));
        }

        if matches!(
            result.status,
            ExecStatus::Confirmed | ExecStatus::PaperFilled
        ) {
            self.advance(lc, event, SniperStage::Confirmed)?;
        }
        // Ambiguous outcomes (Sent / SendUnknown) stay SUBMITTED: the
        // position is booked (the transaction may land) and reconciliation
        // settles the lifecycle record.

        let pos = self
            .record_execution(
                event,
                route,
                &result,
                expected_out_raw,
                spend_lamports,
                base_decimals,
                latency_ms,
                mint,
                approved.market.snapshot.pool.clone(),
                approved.slippage.bps,
            )
            .await
            .map_err(|e| self.fail(lc, event, format!("bookkeeping: {e}")))?;
        *position_out = Some(pos);
        Ok(())
    }

    /// Steps 9–15: readiness, market data, gates, slippage, price impact,
    /// risk. Returns everything the build step needs; the `*_out` slots are
    /// filled as soon as each value is known so a rejection still reports
    /// the route, tolerance and impact it was decided on.
    #[allow(clippy::too_many_arguments)]
    async fn validate_market_and_risk(
        &mut self,
        event: &LaunchEvent,
        lc: &mut Lifecycle,
        cfg: &bot_core::config::Config,
        gates_out: &mut String,
        route_out: &mut Option<EntryRoute>,
        slippage_out: &mut Option<u64>,
        impact_out: &mut Option<u64>,
        fee_out: &mut Option<u64>,
    ) -> Result<Approved, Rejection> {
        let sniper = &cfg.sniper;
        let risk_cfg = &cfg.risk;
        // 9. RPC readiness: every provider tripped means nothing below can
        //    succeed — say so instead of burning the latency budget.
        if self.rpc.unhealthy() {
            return Err(self.reject(
                lc,
                event,
                RejectReason::ExecutionUnavailable,
                "all RPC providers are tripped",
            ));
        }
        // 10. Balance (risk needs the real number).
        let available = match available_sol(&self.state, &self.wallet, &self.rpc).await {
            Ok(v) => v,
            Err(e) => {
                return Err(self.reject(
                    lc,
                    event,
                    RejectReason::ExecutionUnavailable,
                    format!("balance: {e}"),
                ))
            }
        };
        // 11. Market data for the route. The Jupiter route quotes the size
        //     we intend to spend (capped by the risk position cap so the
        //     quote is for a realistic amount).
        let intended_sol = sniper.buy_sol.min(risk_cfg.sniper_position_cap());
        let intended_lamports = maths::sol_to_lamports(intended_sol);
        let fetched_at = Utc::now();
        let market = match load_market(
            &self.rpc,
            &self.wallet.pubkey,
            event,
            sniper,
            intended_lamports,
            fetched_at,
        )
        .await
        {
            Ok(m) => m,
            Err(e) => return Err(self.reject(lc, event, e.reason, e.detail)),
        };
        *route_out = Some(market.route);
        // 12. Safety gates.
        let report = gates::evaluate(&market.snapshot, sniper, Utc::now());
        report.record_metrics();
        *gates_out = report.summary();
        if let Some((gate, detail)) = report.first_failure(sniper.strict_gates) {
            return Err(self.reject(
                lc,
                event,
                gates::reason_for_gate(gate),
                format!("gate {gate}: {detail}"),
            ));
        }
        // 13. Slippage engine.
        let slippage = match slippage::decide(&slippage_inputs(
            sniper,
            risk_cfg,
            &market.snapshot,
            market.route,
            &event.mint,
            intended_lamports,
        )) {
            Ok(d) => d,
            Err(e) => {
                return Err(self.reject(lc, event, RejectReason::SlippageLimit, e.to_string()))
            }
        };
        observe_slippage(slippage.bps, slippage.mode.as_str());
        *slippage_out = Some(slippage.bps);
        *impact_out = Some(slippage.price_impact_bps);
        // 14. Price impact.
        if sniper.max_price_impact_bps > 0
            && slippage.price_impact_bps > sniper.max_price_impact_bps
        {
            return Err(self.reject(
                lc,
                event,
                RejectReason::PriceImpactLimit,
                format!(
                    "modelled price impact {} bps exceeds max {} bps",
                    slippage.price_impact_bps, sniper.max_price_impact_bps
                ),
            ));
        }
        // 15. Fee budget. The policy and attempt count are derived from the
        //     same config snapshot `refresh_policy` installs on the executor
        //     right before the hand-off, so this pre-check and the engine's
        //     own `decide` see identical inputs; the engine stays the
        //     authority at submission time.
        let fee_policy = solana_kit::execute::fee_policy_from_config(cfg);
        let attempts = u32::from(crate::exec_policy(cfg).max_attempts);
        let fee = match check_fee_budget(
            &fee_policy,
            &cfg.execution,
            market.route,
            attempts,
            sniper.max_entry_fee_lamports,
        ) {
            Ok(estimate) => estimate,
            Err((reason, detail)) => return Err(self.reject(lc, event, reason, detail)),
        };
        *fee_out = Some(fee.total_lamports);
        // 16. The authoritative risk decision.
        let decision = self
            .risk
            .check_entry(&EntryRequest {
                module: BotModule::Sniper,
                venue: market.route.venue(),
                symbol: event.mint.clone(),
                symbol_display: event.launch.symbol.clone(),
                requested_quote: sniper.buy_sol,
                available_quote: available,
                slippage_bps: slippage.bps,
                price: None,
                fair_value: None,
                liquidity: None,
                // TASK 5 — attribution for the global layer.
                wallet: self.wallet.pubkey.to_string(),
                strategy: bot_core::global_risk::strategy_label(BotModule::Sniper, None),
            })
            .await;
        if !decision.allowed() {
            self.state.inc_risk_rejected(BotModule::Sniper).await;
            self.state.events.publish(AppEvent::RiskRejected {
                ts: Utc::now(),
                module: BotModule::Sniper,
                symbol: event.launch.symbol.clone(),
                reason: decision.reason.clone(),
            });
            return Err(self.reject(
                lc,
                event,
                RejectReason::from_risk_code(decision.code),
                format!(
                    "risk[{}]: {}",
                    decision.code.map(|c| c.as_str()).unwrap_or("none"),
                    decision.reason
                ),
            ));
        }
        self.state.inc_signals(BotModule::Sniper).await;
        let sized_lamports = maths::sol_to_lamports(decision.sized_quote);
        if sized_lamports == 0 {
            return Err(self.reject(
                lc,
                event,
                RejectReason::RiskRejected,
                "sized SOL rounds to zero lamports",
            ));
        }
        // A reduced size can only lower the impact; re-decide so the
        // recorded tolerance matches what is actually sent.
        let slippage = if sized_lamports < intended_lamports {
            match slippage::decide(&slippage_inputs(
                sniper,
                risk_cfg,
                &market.snapshot,
                market.route,
                &event.mint,
                sized_lamports,
            )) {
                Ok(d) => d,
                Err(e) => {
                    return Err(self.reject(lc, event, RejectReason::SlippageLimit, e.to_string()))
                }
            }
        } else {
            slippage
        };
        *slippage_out = Some(slippage.bps);
        *impact_out = Some(slippage.price_impact_bps);
        debug!(
            symbol = %event.launch.symbol,
            route = %market.route,
            requested = sniper.buy_sol,
            sized = decision.sized_quote,
            verdict = ?decision.verdict,
            slippage_bps = slippage.bps,
            impact_bps = slippage.price_impact_bps,
            fee_estimate_lamports = fee.total_lamports,
            "entry sized"
        );
        Ok(Approved {
            market,
            slippage,
            sized_lamports,
        })
    }

    /// Step 18: build the transaction for the chosen route. Nothing here
    /// touches the network except the Jupiter swap build (which needs the
    /// aggregator) and the ATA existence checks.
    async fn prepare(
        &mut self,
        event: &LaunchEvent,
        approved: &Approved,
        mint: &Pubkey,
        intent_id: &str,
        cfg: &bot_core::config::Config,
    ) -> BotResult<Prepared> {
        let sniper = &cfg.sniper;
        let lamports = approved.sized_lamports;
        let slip_pct = slippage::bps_to_pct(approved.slippage.bps);
        let with_common = |req: TxRequest| -> TxRequest {
            let mut req = req
                .priority_fee(cfg.execution.priority_fee_micro_lamports)
                .compute_units(cfg.execution.compute_unit_limit)
                // Deterministic lifecycle identity: one launch → one buy
                // intent, so a replayed detection or a post-crash retry maps
                // onto the same ledger record and is refused while the first
                // attempt is live.
                .with_intent_id(intent_id)
                .attributed("sniper", mint.to_string());
            if cfg.execution.use_jito {
                req = req.jito_tip(cfg.execution.jito_tip_lamports);
            }
            req
        };

        match (&approved.market.venue, approved.market.route) {
            (VenueData::PumpCurve(ctx), EntryRoute::PumpCurve) => {
                let fee_bps = ctx.global_state.fee_basis_points;
                let (amount, max_sol_cost) =
                    pump::plan_buy(&ctx.curve, lamports, slip_pct, fee_bps)?;
                if amount == 0 {
                    return Err(BotError::solana(
                        "bonding curve returns zero tokens for this buy — curve may be complete",
                    ));
                }
                let opts = BuildOptions {
                    extra_accounts: sniper.pump_extra_accounts.clone(),
                    append_bonding_curve_v2: sniper.pump_append_bonding_curve_v2,
                    ..BuildOptions::default()
                };
                let buy_ix = {
                    let store = self.layouts.read().await;
                    pump::build_buy_ix(ctx, &store, &opts, amount, max_sol_cost)?
                };
                let req = with_common(
                    TxRequest::new(format!("snipe-{}", event.launch.symbol))
                        .with_instruction(buy_ix),
                );
                Ok(Prepared::Request {
                    req,
                    expected_out_raw: amount,
                    spend_lamports: lamports,
                    base_decimals: 6,
                })
            }
            (VenueData::PumpSwap(ctx), EntryRoute::PumpSwapDirect) => {
                let (quote_in, min_base_out, max_quote_in) =
                    pumpswap::plan_buy(ctx, lamports, slip_pct)?;
                let expected_out = ctx.quote_buy(quote_in)?;
                let (_ata, create_ata) = self.wallet.ensure_ata(&self.rpc, mint).await?;
                let buy_ix = {
                    let store = self.layouts.read().await;
                    pumpswap::build_buy_ix(
                        ctx,
                        &store,
                        pumpswap::BuyKind::ExactQuoteIn,
                        quote_in,
                        min_base_out,
                        false,
                    )?
                };
                let mut req = TxRequest::new(format!("snipe-ps-{}", event.launch.symbol));
                if let Some(ix) = create_ata {
                    req = req.with_instruction(ix);
                }
                // Wrap enough SOL to cover the swap plus protocol/creator
                // fees; the remainder is swept back by `unwrap_sol`.
                let mut req = with_common(req.with_instruction(buy_ix)).wrap_sol(max_quote_in);
                req.unwrap_sol = true;
                Ok(Prepared::Request {
                    req,
                    expected_out_raw: expected_out,
                    spend_lamports: quote_in,
                    base_decimals: ctx.base_decimals,
                })
            }
            (VenueData::Raydium(pool), EntryRoute::RaydiumV4Direct) => {
                let expected_out = pool.quote(&WSOL_MINT, lamports)?;
                let min_out = maths::minus_pct_u64(expected_out, slip_pct);
                if min_out == 0 {
                    return Err(BotError::solana(
                        "slippage tolerance would allow zero tokens out",
                    ));
                }
                let (token_ata, create_ata) = self.wallet.ensure_ata(&self.rpc, mint).await?;
                let wsol_ata = pump::associated_user(
                    &WSOL_MINT,
                    &self.wallet.pubkey,
                    &solana_kit::consts::TOKEN_PROGRAM,
                );
                let swap_ix = {
                    let store = self.layouts.read().await;
                    pool.swap_base_in_learned(
                        &store,
                        &self.wallet.pubkey,
                        &wsol_ata,
                        &token_ata,
                        lamports,
                        min_out,
                        pool.market.is_none(),
                    )?
                };
                let mut req = TxRequest::new(format!("snipe-ray-{}", event.launch.symbol));
                if let Some(ix) = create_ata {
                    req = req.with_instruction(ix);
                }
                let mut req = with_common(req.with_instruction(swap_ix)).wrap_sol(lamports);
                req.unwrap_sol = true;
                Ok(Prepared::Request {
                    req,
                    expected_out_raw: expected_out,
                    spend_lamports: lamports,
                    base_decimals: approved.market.snapshot.base_decimals,
                })
            }
            (VenueData::Jupiter(quote), EntryRoute::Jupiter) => {
                let out_amount = quote.out_amount_u64()?;
                if out_amount == 0 {
                    return Err(BotError::solana(
                        "jupiter found no output for the graduated buy",
                    ));
                }
                let label = format!("snipe-jup-{}", event.launch.symbol);
                let mode = self.state.execution_mode().await;
                // Paper never builds/sends: the quote is the fill. Simulate
                // builds + simulates only. Live hands the Jupiter-signed
                // transaction to the executor's lifecycle.
                let built = if mode == ExecutionMode::Paper {
                    None
                } else {
                    let jupiter = Jupiter::new();
                    let request = QuoteRequest::new(*WSOL_MINT, *mint, lamports)
                        .slippage_bps(approved.slippage.bps);
                    let recent = self.rpc.latest_blockhash(true).await?;
                    let (_q, tx, last_valid) = jupiter
                        .build_swap(
                            &self.wallet,
                            &request,
                            Some(recent.blockhash),
                            Some(cfg.execution.priority_fee_micro_lamports),
                        )
                        .await?;
                    if mode == ExecutionMode::Simulate {
                        let sim = self.rpc.simulate(&tx).await?;
                        if let Some(err) = sim.value.err {
                            return Err(BotError::solana(format!(
                                "jupiter simulate failed: {err}"
                            )));
                        }
                        None
                    } else {
                        Some(
                            solana_kit::tx::BuiltTx::from_signed(
                                &label,
                                tx,
                                recent.blockhash,
                                last_valid.or(Some(recent.last_valid_block_height)),
                            )?
                            .with_intent_id(intent_id)
                            .attributed("sniper", mint.to_string()),
                        )
                    }
                };
                Ok(Prepared::Jupiter {
                    built,
                    label,
                    intent_id: intent_id.to_string(),
                    expected_out_raw: out_amount,
                    spend_lamports: lamports,
                    base_decimals: approved.market.snapshot.base_decimals,
                })
            }
            (venue, route) => Err(BotError::invalid(format!(
                "route {route} does not match the loaded venue {}",
                venue_name(venue)
            ))),
        }
    }

    /// Shared post-execution bookkeeping: position, trade, events, counters.
    ///
    /// `amount_raw` is the base tokens expected (in `base_decimals`),
    /// `sol_in` the lamports spent. For paper/simulate the plan values *are*
    /// the fill; for live they are the intended fill (a confirmed-transaction
    /// decode can refine this later). Returns the position id.
    #[allow(clippy::too_many_arguments)]
    async fn record_execution(
        &mut self,
        event: &LaunchEvent,
        route: EntryRoute,
        result: &ExecutionResult,
        amount_raw: u64,
        sol_in: u64,
        base_decimals: u8,
        latency_ms: u64,
        mint: Pubkey,
        pool: Option<String>,
        slippage_bps: u64,
    ) -> BotResult<String> {
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let mode = self.state.execution_mode().await;
        let venue: Venue = route.venue();
        let launch = &event.launch;

        self.state.inc_orders_sent(BotModule::Sniper).await;

        // Compute the fill economics in human units.
        let qty = maths::from_raw_amount(amount_raw, base_decimals);
        let cost_sol = maths::lamports_to_sol(sol_in);
        let price = if qty > 0.0 { cost_sol / qty } else { 0.0 }; // SOL per token

        let signature = if result.signature.is_empty() {
            None
        } else {
            Some(result.signature.clone())
        };

        self.state.events.publish(AppEvent::OrderSent {
            ts: Utc::now(),
            module: BotModule::Sniper,
            symbol: launch.symbol.clone(),
            venue: venue.as_str().to_string(),
            mode: mode.as_str().to_string(),
            quote_amount: cost_sol,
            signature: signature.clone(),
            signer: Some(self.wallet.pubkey.to_string()),
            attempts: Some(result.attempts),
            latency_ms: Some(latency_ms),
        });

        // ---- Trade record -------------------------------------------------
        let trade = Trade {
            id: self.state.next_id("t"),
            ts: Utc::now(),
            source: TradeSource::Sniper,
            venue,
            mode,
            side: PositionSide::Long,
            symbol: launch.mint.clone(),
            symbol_display: launch.symbol.clone(),
            amount_in: cost_sol,
            amount_out: qty,
            quote_symbol: "SOL".into(),
            price,
            fee: 0.0,
            slippage_bps,
            signature: signature.clone(),
            position_id: None,
            note: Some(format!(
                "feed={} protocol={} route={} event={} {}",
                launch.feed, event.protocol, route, event.event_id, launch.name
            )),
            latency_ms: Some(latency_ms),
        };
        let trade_id = trade.id.clone();
        // TASK 5 — the typed accounting event for this fill, built from the
        // same trade record (signature = reference for live fills, a
        // paper reference otherwise; the deterministic snipe intent id is
        // the correlation). Submitted after the position exists so the
        // event carries the position id.
        let mut ledger_event = bot_core::accounting::fill_event_for_trade(
            &trade,
            self.wallet.pubkey.to_string(),
            bot_core::global_risk::strategy_label(BotModule::Sniper, None),
            None,
            Some(snipe_intent_id(event, &mint, route.as_str())),
        );
        self.state.record_trade(trade.clone()).await;
        self.state.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade),
        });

        // ---- Position -----------------------------------------------------
        let pos_id = self.state.next_id("p");
        ledger_event.position_id = Some(pos_id.clone());
        let mut position = Position::new(
            pos_id.clone(),
            TradeSource::Sniper,
            venue,
            mode,
            launch.mint.clone(),
            launch.symbol.clone(),
            "SOL".into(),
        );
        position.apply_buy(qty, price, cost_sol);
        position.entry_signature = signature.clone();
        position.entry_latency_ms = Some(latency_ms);
        // The venue the position lives on (curve / AMM pool address) so the
        // exit path can mark and sell without rediscovering it.
        position.market_id = pool;

        // Exit parameters from the sniper config, expressed as absolute price
        // levels derived from the entry price (risk.check_exit prefers these).
        if let Some(tp) = sniper.take_profit_pct {
            position.take_profit = Some(price * (1.0 + tp));
        }
        if let Some(sl) = sniper.stop_loss_pct {
            position.stop_loss = Some(price * (1.0 - sl));
        }
        position.trailing_stop = sniper.trailing_stop_pct;
        position.max_hold_secs = sniper.max_hold_secs;

        self.state.upsert_position(position.clone()).await;
        self.state.events.publish(AppEvent::PositionUpdate {
            ts: Utc::now(),
            position: Box::new(position),
        });
        // The global ledger is the only mutator of global accounting state;
        // the module hands over the typed event and keeps its own record.
        self.state.ledger().submit(ledger_event).await;

        // Persist the learned layout store if a live buy confirmed (the layout
        // that just worked is worth keeping). Best-effort, never fatal.
        if sniper.pump_learn_account_layout && !sniper.pump_layout_file.trim().is_empty() {
            if let Err(e) = self.maybe_learn_layout(mint, signature.as_deref()).await {
                debug!(error = %e, "layout learning skipped");
            }
        }

        info!(
            symbol = %launch.symbol,
            route = %route,
            qty,
            cost_sol,
            price,
            latency_ms,
            status = ?result.status,
            intent = %result.intent_id,
            trade_id,
            pos_id,
            "SNIPED"
        );
        Ok(pos_id)
    }

    /// After a confirmed live buy, learn the account layout from the on-chain
    /// transaction so future builds reuse exactly what worked. No-op in paper
    /// mode (there is no confirmed transaction to learn from).
    async fn maybe_learn_layout(&mut self, mint: Pubkey, signature: Option<&str>) -> BotResult<()> {
        let mode = self.state.execution_mode().await;
        if mode == ExecutionMode::Paper {
            return Ok(());
        }
        let Some(sig) = signature else { return Ok(()) };
        let sig = sig
            .parse::<solana_sdk::signature::Signature>()
            .map_err(|e| BotError::encoding(format!("signature: {e}")))?;
        let Some(confirmed) = self.rpc.get_transaction(&sig).await? else {
            return Ok(());
        };
        // Decode the account keys and find the pump buy instruction's metas.
        let decoded = solana_kit::decode::decode_swap(
            &self.wallet.pubkey,
            &sig.to_string(),
            confirmed.slot,
            confirmed.block_time,
            &confirmed.transaction.transaction,
            confirmed
                .transaction
                .meta
                .as_ref()
                .ok_or_else(|| BotError::solana("confirmed tx has no meta"))?,
        )?;
        if decoded.is_none() {
            debug!(%mint, "confirmed snipe did not decode to a swap; nothing to learn");
        }
        // The heavy lifting (mapping metas back to named accounts) lives in the
        // layout store's `learn`; we only trigger a save here so a good template
        // is not lost on restart.
        let path = self
            .state
            .config_snapshot()
            .await
            .sniper
            .pump_layout_file
            .clone();
        let store = self.layouts.read().await;
        store.save(&path).await?;
        Ok(())
    }

    // ---- lifecycle helpers ----------------------------------------------

    fn reject(
        &self,
        lc: &mut Lifecycle,
        event: &LaunchEvent,
        reason: RejectReason,
        detail: impl Into<String>,
    ) -> Rejection {
        let rejection = lc.reject(reason, detail, Utc::now());
        count_rejection(&rejection, event.protocol);
        count_stage(lc.stage, event.protocol);
        debug!(
            event = %event.event_id,
            mint = %event.mint,
            reason = %rejection.reason,
            stage = %rejection.stage,
            detail = %rejection.detail,
            "launch rejected"
        );
        rejection
    }

    fn fail(
        &self,
        lc: &mut Lifecycle,
        event: &LaunchEvent,
        detail: impl Into<String>,
    ) -> Rejection {
        let rejection = lc.fail(detail, Utc::now());
        count_rejection(&rejection, event.protocol);
        count_stage(lc.stage, event.protocol);
        rejection
    }

    fn advance(
        &self,
        lc: &mut Lifecycle,
        event: &LaunchEvent,
        next: SniperStage,
    ) -> Result<(), Rejection> {
        match lc.advance(next, Utc::now()) {
            Ok(()) => {
                count_stage(next, event.protocol);
                Ok(())
            }
            Err(r) => {
                // An illegal transition is a programming error surfaced as a
                // rejection; make sure the lifecycle still terminates.
                let rejection = lc.reject(r.reason, r.detail.clone(), Utc::now());
                count_rejection(&rejection, event.protocol);
                Err(rejection)
            }
        }
    }

    /// One audit record per event: stage path, route, intent and the
    /// rejection (if any). Persisted by the server's audit sink like every
    /// other `AppEvent::Audit`.
    async fn publish_audit(&self, event: &LaunchEvent, lc: &Lifecycle, outcome: &EntryOutcome) {
        let outcome_text = match &outcome.rejection {
            Some(r) => format!(
                "{} route={} reason={} detail={} path={} gates=[{}]",
                lc.stage,
                outcome.route.map(|r| r.as_str()).unwrap_or("-"),
                r.reason,
                r.detail,
                lc.path(),
                outcome.gates
            ),
            None => format!(
                "{} route={} intent={} position={} slippage_bps={} impact_bps={} fee_est_lamports={} total_ms={} path={} gates=[{}]",
                lc.stage,
                outcome.route.map(|r| r.as_str()).unwrap_or("-"),
                outcome.intent_id.as_deref().unwrap_or("-"),
                outcome.position_id.as_deref().unwrap_or("-"),
                outcome.slippage_bps.unwrap_or(0),
                outcome.price_impact_bps.unwrap_or(0),
                outcome.fee_estimate_lamports.unwrap_or(0),
                lc.timeline.total_ms().unwrap_or(0),
                lc.path(),
                outcome.gates
            ),
        };
        self.state.events.publish(AppEvent::Audit {
            ts: Utc::now(),
            actor: "sniper".into(),
            action: format!("sniper.entry.{}", lc.stage.as_str().to_ascii_lowercase()),
            target: Some(format!("{}:{}", event.protocol, event.mint)),
            outcome: outcome_text,
        });
    }
}

/// Assemble the slippage-engine inputs for one route/size.
fn slippage_inputs(
    sniper: &bot_core::config::SniperConfig,
    risk: &bot_core::config::RiskConfig,
    snapshot: &MarketSnapshot,
    route: EntryRoute,
    mint: &str,
    trade_lamports: u64,
) -> SlippageInputs {
    let protocol_bps = match route {
        EntryRoute::PumpSwapDirect => sniper.pumpswap_slippage_pct.map(slippage::pct_to_bps),
        EntryRoute::RaydiumV4Direct => sniper.raydium_slippage_pct.map(slippage::pct_to_bps),
        EntryRoute::PumpCurve | EntryRoute::Jupiter => None,
    };
    SlippageInputs {
        mode: SlippageMode::parse(&sniper.slippage_mode).unwrap_or_default(),
        strategy_bps: slippage::pct_to_bps(sniper.slippage_pct),
        protocol_bps,
        token_bps: sniper.slippage_overrides_bps.get(mint).copied(),
        hard_max_bps: risk.max_slippage_bps,
        quote_reserve_lamports: snapshot.pricing_quote_reserve_lamports,
        trade_lamports,
    }
}

fn venue_name(v: &VenueData) -> &'static str {
    match v {
        VenueData::PumpCurve(_) => "pump curve",
        VenueData::PumpSwap(_) => "pumpswap pool",
        VenueData::Raydium(_) => "raydium pool",
        VenueData::Jupiter(_) => "jupiter quote",
    }
}

/// Deterministic execution-intent id for one launch → one buy. The launch is
/// identified by its creation signature (falling back to the slot, then to
/// the observation time) so a replayed feed event or a post-crash retry maps
/// onto the same ledger record. The route is part of the id: a curve buy and
/// a Jupiter buy of the same launch are different transactions.
pub(crate) fn snipe_intent_id(event: &LaunchEvent, mint: &Pubkey, route: &str) -> String {
    let launch_ref = event
        .signature
        .clone()
        .or_else(|| event.slot.map(|s| s.to_string()))
        .unwrap_or_else(|| event.observed_at.timestamp_millis().to_string());
    bot_core::execution::intent_id(&["sniper", &mint.to_string(), "buy", route, &launch_ref])
}

/// Public wrapper so tests and the replay engine compute the exact intent
/// id the live path would use.
pub fn entry_intent_id(event: &LaunchEvent, route: EntryRoute) -> Option<String> {
    let mint = event.mint_pubkey()?;
    Some(snipe_intent_id(event, &mint, route.as_str()))
}

/// Timestamp helper shared with tests: `now` as the pipeline sees it.
pub fn pipeline_now() -> DateTime<Utc> {
    Utc::now()
}
