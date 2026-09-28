//! Deterministic replay (TASK 2 §M).
//!
//! A [`ReplayFixture`] is a recorded sequence of launch events, each paired
//! with the market snapshot the chain would have returned (or none, to model
//! an unreadable venue) and the outcome the pipeline is expected to reach.
//! [`ReplayEngine::run`] drives **the same validation code as the live path**
//! — `pipeline::precheck`, the authoritative dedup in `AppState`, the risk
//! engine's screening and entry decision, `gates::evaluate`,
//! `slippage::decide`, `select_route`, `pipeline::check_fee_budget` (against
//! the fee policy derived from the fixture's `[execution]` section), the
//! deterministic intent id — and stops at `EXECUTION_READY`.
//!
//! What replay never does: open a websocket, call an RPC, build, sign or
//! submit a transaction, touch the execution ledger. The furthest a step can
//! get is "would submit intent `int_…` on route `x`". This is enforced by
//! construction (the engine holds no executor, wallet or RPC handle), not by
//! a flag.
//!
//! Time is fixture-controlled: every step evaluates at
//! `event.observed_at + now_offset_ms`, so a fixture recorded months ago
//! produces the same verdicts today.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use bot_core::config::{ExecutionConfig, RiskConfig, SniperConfig};
use bot_core::error::{BotError, BotResult};
use bot_core::models::BotModule;
use bot_core::risk::{EntryRequest, RiskEngine};
use bot_core::state::Shared;

use crate::entry::entry_intent_id;
use crate::event::LaunchEvent;
use crate::gates::{self, MarketSnapshot};
use crate::pipeline::{
    check_fee_budget, precheck, select_route, EntryRoute, Lifecycle, PrecheckContext, RejectReason,
    Rejection, SniperStage,
};
use crate::slippage::{self, SlippageInputs, SlippageMode};

/// What a step is expected to produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expectation {
    pub stage: SniperStage,
    #[serde(default)]
    pub reason: Option<RejectReason>,
}

/// One recorded observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStep {
    pub event: LaunchEvent,
    /// What the venue looked like; `None` models an unreadable venue
    /// (`EXECUTION_UNAVAILABLE`).
    #[serde(default)]
    pub snapshot: Option<MarketSnapshot>,
    /// Whether the pump.fun curve was already complete when read (drives
    /// route selection exactly like the live loader).
    #[serde(default)]
    pub curve_complete: bool,
    /// Evaluate at `event.observed_at + now_offset_ms`.
    #[serde(default)]
    pub now_offset_ms: i64,
    /// Engage/release the kill switch before this step.
    #[serde(default)]
    pub kill_switch: Option<bool>,
    /// Block/unblock the event's symbol (reconciliation gate) before this step.
    #[serde(default)]
    pub symbol_gated: Option<bool>,
    /// Set/clear `risk.sniper_emergency_disable` before this step — the
    /// operator's sniper-only stop, distinct from the global kill switch.
    #[serde(default)]
    pub emergency_disable: Option<bool>,
    /// Book this realised sniper PnL (SOL; negative = loss) before this step,
    /// so the daily-loss controls can be replayed without a database.
    #[serde(default)]
    pub book_realized_sol: Option<f64>,
    pub expect: Expectation,
}

/// A recorded scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayFixture {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Wallet balance in SOL the risk engine sizes against.
    pub wallet_sol: f64,
    /// Sniper config for the scenario (defaults when omitted).
    #[serde(default)]
    pub sniper: SniperConfig,
    /// Risk config for the scenario (defaults when omitted).
    #[serde(default)]
    pub risk: RiskConfig,
    /// Execution config the fee budget (check 15) is priced from — fee
    /// policy bounds, priority fee, compute-unit limit, retries, Jito tip.
    /// Omitted = the engine's current `[execution]` section. Replay never
    /// executes, so `mode` / `allow_live_trading` in here are inert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionConfig>,
    pub steps: Vec<ReplayStep>,
}

impl ReplayFixture {
    pub fn from_json(text: &str) -> BotResult<Self> {
        serde_json::from_str(text).map_err(|e| BotError::encoding(format!("replay fixture: {e}")))
    }

    pub fn to_json(&self) -> BotResult<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| BotError::encoding(format!("replay fixture: {e}")))
    }

    pub fn load(path: &Path) -> BotResult<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| BotError::encoding(format!("replay fixture {}: {e}", path.display())))?;
        Self::from_json(&text)
    }

    /// Every `*.json` fixture in a directory, sorted by file name.
    pub fn load_dir(dir: &Path) -> BotResult<Vec<(String, Self)>> {
        let mut out = BTreeMap::new();
        let entries = std::fs::read_dir(dir).map_err(|e| {
            BotError::encoding(format!("replay fixture dir {}: {e}", dir.display()))
        })?;
        for entry in entries {
            let entry =
                entry.map_err(|e| BotError::encoding(format!("replay fixture dir: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            out.insert(name, Self::load(&path)?);
        }
        Ok(out.into_iter().collect())
    }
}

/// What one step produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayOutcome {
    pub step: usize,
    pub event_id: String,
    pub mint: String,
    pub stage: SniperStage,
    pub rejection: Option<Rejection>,
    pub route: Option<EntryRoute>,
    /// The exact intent id the live path would submit under.
    pub intent_id: Option<String>,
    pub slippage_bps: Option<u64>,
    pub price_impact_bps: Option<u64>,
    /// Worst-case entry fee the step was budgeted at (check 15).
    pub fee_estimate_lamports: Option<u64>,
    pub sized_sol: Option<f64>,
    pub gates: String,
    pub path: String,
    pub expected: Expectation,
    pub matched: bool,
}

/// Summary of a whole fixture run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub fixture: String,
    pub outcomes: Vec<ReplayOutcome>,
}

impl ReplayReport {
    pub fn all_matched(&self) -> bool {
        self.outcomes.iter().all(|o| o.matched)
    }

    pub fn mismatches(&self) -> Vec<&ReplayOutcome> {
        self.outcomes.iter().filter(|o| !o.matched).collect()
    }

    /// One line per step, for CLI/test output.
    pub fn render(&self) -> String {
        let mut s = format!("fixture {}\n", self.fixture);
        for o in &self.outcomes {
            let verdict = match &o.rejection {
                Some(r) => format!("{} {}", r.reason, r.detail),
                None => format!(
                    "would submit intent {} on {}",
                    o.intent_id.as_deref().unwrap_or("-"),
                    o.route.map(|r| r.as_str()).unwrap_or("-")
                ),
            };
            s.push_str(&format!(
                "  #{} {} {} -> {} [{}] expected {}{} {}\n",
                o.step,
                o.event_id,
                o.mint,
                o.stage,
                verdict,
                o.expected.stage,
                o.expected
                    .reason
                    .map(|r| format!("/{r}"))
                    .unwrap_or_default(),
                if o.matched { "OK" } else { "MISMATCH" }
            ));
        }
        s
    }
}

/// Drives fixtures through the validation pipeline. Holds no executor,
/// wallet or RPC: it cannot submit.
pub struct ReplayEngine {
    state: Shared,
    risk: RiskEngine,
}

impl ReplayEngine {
    pub fn new(state: Shared) -> Self {
        let risk = RiskEngine::new(state.clone());
        ReplayEngine { state, risk }
    }

    pub fn state(&self) -> &Shared {
        &self.state
    }

    /// Run every step of `fixture` in order against this engine's state.
    /// The fixture's sniper/risk config is installed first (the module is
    /// enabled so `STRATEGY_DISABLED` only appears when a step asks for it).
    pub async fn run(&self, fixture: &ReplayFixture) -> ReplayReport {
        self.state
            .update_config(|c| {
                c.sniper = fixture.sniper.clone();
                c.sniper.enabled = true;
                c.risk = fixture.risk.clone();
                if let Some(execution) = &fixture.execution {
                    c.execution = execution.clone();
                }
            })
            .await;
        self.state.set_enabled(BotModule::Sniper, true).await;
        self.state
            .set_balances(Some(fixture.wallet_sol), None)
            .await;

        let mut outcomes = Vec::with_capacity(fixture.steps.len());
        for (i, step) in fixture.steps.iter().enumerate() {
            if let Some(kill) = step.kill_switch {
                self.state.set_kill_switch(kill, "replay fixture").await;
            }
            if let Some(gated) = step.symbol_gated {
                if gated {
                    self.state.block_symbol(&step.event.mint).await;
                } else {
                    self.state.unblock_symbol(&step.event.mint).await;
                }
            }
            if let Some(disable) = step.emergency_disable {
                self.state
                    .update_config(|c| c.risk.sniper_emergency_disable = disable)
                    .await;
            }
            if let Some(pnl) = step.book_realized_sol {
                self.state.add_realized(BotModule::Sniper, pnl).await;
            }
            let mut outcome = self.run_step(i, step, fixture.wallet_sol).await;
            outcome.matched = outcome.stage == step.expect.stage
                && outcome.rejection.as_ref().map(|r| r.reason) == step.expect.reason;
            outcomes.push(outcome);
        }
        ReplayReport {
            fixture: fixture.name.clone(),
            outcomes,
        }
    }

    async fn run_step(&self, index: usize, step: &ReplayStep, wallet_sol: f64) -> ReplayOutcome {
        let event = &step.event;
        let now: DateTime<Utc> = event.observed_at + Duration::milliseconds(step.now_offset_ms);
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let risk_cfg = cfg.risk.clone();
        let mut lc = Lifecycle::start(
            &event.event_id,
            event.protocol,
            event.effective_ts(),
            event.observed_at,
            now,
        );
        let mut route = None;
        let mut intent_id = None;
        let mut slippage_bps = None;
        let mut price_impact_bps = None;
        let mut fee_estimate_lamports = None;
        let mut sized_sol = None;
        let mut gates_summary = String::new();

        let verdict: Result<(), (RejectReason, String)> = async {
            // 1–6.
            let ctx = PrecheckContext {
                kill_switch: self.state.kill_switch(),
                module_enabled: self.state.is_enabled(BotModule::Sniper).await,
                emergency_disable: risk_cfg.sniper_emergency_disable,
                symbol_gated: self.state.is_symbol_blocked(&event.mint).await,
            };
            precheck(event, &sniper, ctx, now)?;
            // 7. Authoritative dedup.
            if !self.state.mark_launch_seen(&event.dedup_key()).await {
                return Err((
                    RejectReason::DuplicateEvent,
                    format!("launch {} already seen", event.dedup_key()),
                ));
            }
            // 8. Screening.
            let age_secs = (event.age_ms(now) / 1_000) as i64;
            self.risk
                .check_launch_with_lists(
                    &event.launch,
                    &risk_cfg,
                    &sniper.creator_denylist,
                    &sniper.keyword_denylist,
                    &[],
                    age_secs,
                    sniper.max_launch_age_secs,
                )
                .map_err(|r| (RejectReason::RiskRejected, format!("screening: {r}")))?;
            lc.advance(SniperStage::Validated, now)
                .map_err(|r| (r.reason, r.detail))?;

            // 11. Market data comes from the fixture.
            let snapshot = step.snapshot.as_ref().ok_or_else(|| {
                (
                    RejectReason::ExecutionUnavailable,
                    "venue unreadable (fixture provides no snapshot)".to_string(),
                )
            })?;
            let chosen = select_route(event.protocol, &sniper, step.curve_complete)
                .map_err(|d| (RejectReason::InvalidRoute, d))?;
            route = Some(chosen);
            // 12. Gates.
            let report = gates::evaluate(snapshot, &sniper, now);
            gates_summary = report.summary();
            if let Some((gate, detail)) = report.first_failure(sniper.strict_gates) {
                return Err((
                    gates::reason_for_gate(gate),
                    format!("gate {gate}: {detail}"),
                ));
            }
            // 13. Slippage.
            let intended_sol = sniper.buy_sol.min(risk_cfg.sniper_position_cap());
            let intended_lamports = bot_core::maths::sol_to_lamports(intended_sol);
            let protocol_bps = match chosen {
                EntryRoute::PumpSwapDirect => {
                    sniper.pumpswap_slippage_pct.map(slippage::pct_to_bps)
                }
                EntryRoute::RaydiumV4Direct => {
                    sniper.raydium_slippage_pct.map(slippage::pct_to_bps)
                }
                _ => None,
            };
            let decision = slippage::decide(&SlippageInputs {
                mode: SlippageMode::parse(&sniper.slippage_mode).unwrap_or_default(),
                strategy_bps: slippage::pct_to_bps(sniper.slippage_pct),
                protocol_bps,
                token_bps: sniper.slippage_overrides_bps.get(&event.mint).copied(),
                hard_max_bps: risk_cfg.max_slippage_bps,
                quote_reserve_lamports: snapshot.pricing_quote_reserve_lamports,
                trade_lamports: intended_lamports,
            })
            .map_err(|e| (RejectReason::SlippageLimit, e.to_string()))?;
            slippage_bps = Some(decision.bps);
            price_impact_bps = Some(decision.price_impact_bps);
            // 14. Price impact.
            if sniper.max_price_impact_bps > 0
                && decision.price_impact_bps > sniper.max_price_impact_bps
            {
                return Err((
                    RejectReason::PriceImpactLimit,
                    format!(
                        "modelled price impact {} bps exceeds max {} bps",
                        decision.price_impact_bps, sniper.max_price_impact_bps
                    ),
                ));
            }
            // 15. Fee budget — the policy and attempt count the executor
            //     would derive from this same config.
            let fee_policy = solana_kit::execute::fee_policy_from_config(&cfg);
            let attempts =
                u32::from(solana_kit::execute::exec_policy_from_config(&cfg).max_attempts);
            let fee = check_fee_budget(
                &fee_policy,
                &cfg.execution,
                chosen,
                attempts,
                sniper.max_entry_fee_lamports,
            )?;
            fee_estimate_lamports = Some(fee.total_lamports);
            // 16. Risk.
            let risk = self
                .risk
                .check_entry(&EntryRequest {
                    module: BotModule::Sniper,
                    venue: chosen.venue(),
                    symbol: event.mint.clone(),
                    symbol_display: event.launch.symbol.clone(),
                    requested_quote: sniper.buy_sol,
                    available_quote: wallet_sol,
                    slippage_bps: decision.bps,
                    price: None,
                    fair_value: None,
                    liquidity: None,
                    // Replays hold no wallet; the global layer attributes
                    // the request to the module name.
                    wallet: String::new(),
                    strategy: bot_core::global_risk::strategy_label(BotModule::Sniper, None),
                })
                .await;
            if !risk.allowed() {
                return Err((
                    RejectReason::from_risk_code(risk.code),
                    format!(
                        "risk[{}]: {}",
                        risk.code.map(|c| c.as_str()).unwrap_or("none"),
                        risk.reason
                    ),
                ));
            }
            sized_sol = Some(risk.sized_quote);
            lc.advance(SniperStage::RiskApproved, now)
                .map_err(|r| (r.reason, r.detail))?;
            // 18./19. Intent id and the pre-submit freshness checks; no
            //         transaction is ever built here.
            intent_id = entry_intent_id(event, chosen);
            let held = event.held_ms(now);
            if sniper.max_entry_latency_ms > 0 && held > sniper.max_entry_latency_ms {
                return Err((
                    RejectReason::StaleEvent,
                    format!(
                        "entry latency {held} ms exceeds budget {} ms",
                        sniper.max_entry_latency_ms
                    ),
                ));
            }
            lc.advance(SniperStage::ExecutionReady, now)
                .map_err(|r| (r.reason, r.detail))?;
            Ok(())
        }
        .await;

        let rejection = match verdict {
            Ok(()) => None,
            Err((reason, detail)) => Some(lc.reject(reason, detail, now)),
        };
        ReplayOutcome {
            step: index,
            event_id: event.event_id.clone(),
            mint: event.mint.clone(),
            stage: lc.stage,
            rejection,
            route,
            intent_id,
            slippage_bps,
            price_impact_bps,
            fee_estimate_lamports,
            sized_sol,
            gates: gates_summary,
            path: lc.path(),
            expected: step.expect.clone(),
            matched: false,
        }
    }
}
