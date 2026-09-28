//! Module 2 — copy-trading bot.
//!
//! Watches a set of tracked wallets ("leaders") and mirrors their pump.fun /
//! PumpSwap / Raydium trades. A leader's confirmed swap is decoded into a
//! [`WalletTrade`] by the feeds, lifted into a [`event::LeaderTradeEvent`]
//! and walked through the staged pipeline [`CopyBot::process_event`]
//! (`event.rs`): shape validation, leader lookup, the one authoritative
//! dedup, ordering, policy, sizing, the shared risk engine, cross-replica
//! ownership, and finally the shared executor (`mirror.rs`). If the leader
//! exits and `mirror_exits` is on, we close our mirrored position too.
//!
//! Like every module it is **paper-trading by default**: trades are constructed
//! and simulated, never broadcast, until `execution.mode = "live"` *and*
//! `execution.allow_live_trading = true`.
//!
//! Module map (TASK 3):
//!
//! | module | responsibility |
//! |---|---|
//! | [`leader`] | leader registry + lifecycle (follow / pause / resume / unfollow) |
//! | [`event`] | canonical event, shape validation, stage / rejection vocabulary, **the staged pipeline** (`process_event`) |
//! | [`event_dedup`] | the one authoritative dedup (`AppState::mark_copy_event_seen`) |
//! | [`event_ordering`] | per-leader slot cursors, gap detection |
//! | [`policy`] | mirror / skip decision (pure) |
//! | [`sizing`] | leader size → requested size (pure) |
//! | [`intent`] | deterministic entry ids (unchanged) + hardened exit ids + write-ahead journal records |
//! | [`mirror`] | the legacy `mirror_trade` door and the entry execution paths |
//! | [`exit`] | our own TP/SL sweeper and the shared sell path |
//! | [`reconcile`] | leader ↔ follower reconciliation |
//! | [`recovery`] | durable journal ([`recovery::CopyStore`]) + restart recovery |
//! | [`metrics`] / [`audit`] | `copy_*` series and `copy.*` audit records |
//! | [`feeds`] | PumpPortal / polling / Geyser feeds (behaviour unchanged; dedup contract documented) |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audit;
pub mod event;
pub mod event_dedup;
pub mod event_ordering;
pub mod exit;
pub mod feeds;
pub mod intent;
pub mod leader;
pub mod metrics;
pub mod mirror;
pub mod policy;
pub mod reconcile;
pub mod recovery;
pub mod sizing;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use bot_core::config::Config;
use bot_core::config::CopyWallet;
use bot_core::error::BotResult;
use bot_core::models::{BotModule, ExecutionMode, PositionSide, PositionStatus, WalletTrade};
use bot_core::risk::RiskEngine;
use bot_core::state::Shared;

use solana_kit::execute::{ExecPolicy, Executor};
use solana_kit::layout::LayoutStore;
use solana_kit::rpc::Rpc;
use solana_kit::signer::SignerRegistry;
use solana_kit::tokens::Wallet;

use event::{CopyStage, EventSource, LeaderTradeEvent};
use event_ordering::OrderingTracker;
use feeds::CopyFeed;
use leader::{LeaderRegistry, LeaderTransition};
use reconcile::{ReconAction, ReconInputs, ReconReport};
use recovery::{CopyStore, MemoryCopyStore, RecoveryAction, RecoveryInputs, RecoveryPlan};

/// The copy-trading bot.
pub struct CopyBot {
    state: Shared,
    rpc: Rpc,
    wallet: Arc<Wallet>,
    executor: Executor,
    risk: RiskEngine,
    layouts: Arc<RwLock<LayoutStore>>,
    /// Signer registry for transactions that need signers beyond the wallet.
    /// `None` keeps wallet-only behaviour; required-but-missing signers then
    /// fail the build explicitly (see `solana_kit::tx`).
    signers: Option<Arc<SignerRegistry>>,
    /// Write-ahead intent journal (§I crash point C), injected by the server
    /// when `[recovery] intent_journal` is on. `None` = legacy behaviour.
    intents: Option<Arc<dyn bot_core::recovery::IntentSink>>,
    /// Distributed execution ownership (Prompt 3 §B/§F), injected by the
    /// server. `None` = single-instance/legacy behaviour.
    ownership: Option<Arc<bot_core::ownership::OwnershipRegistry>>,
    /// Leader registry (TASK 3 §01), seeded from `[copy].wallets` and kept in
    /// sync with config hot reloads.
    leaders: Arc<RwLock<LeaderRegistry>>,
    /// Per-leader ordering cursors (TASK 3 §04).
    ordering: OrderingTracker,
    /// Durable journal (TASK 3 §09): leaders, processed events, links. The
    /// server injects the Postgres store; the in-memory store is the default.
    store: Arc<dyn CopyStore>,
    /// Monotonic delivery counter handed to events as `source_sequence`.
    sequence: u64,
}

/// Fresh write-ahead intent record for one copy-trade broadcast (§I).
pub(crate) fn copy_intent(
    state: &Shared,
    wallet: &Arc<Wallet>,
    symbol: &str,
    side: &str,
    qty: &str,
) -> bot_core::db::repo::IntentRecord {
    intent::journal_record(state, wallet, symbol, side, qty)
}

impl CopyBot {
    /// Build a copy bot from shared state plus per-run infrastructure.
    pub async fn new(
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<SignerRegistry>>,
    ) -> Self {
        let cfg = state.config_snapshot().await;
        let mut executor = Executor::new(
            rpc.clone(),
            wallet.clone(),
            solana_kit::execute::exec_policy_from_config(&cfg),
        )
        .with_fee_policy(solana_kit::execute::fee_policy_from_config(&cfg));
        if let Some(reg) = &signers {
            executor = executor.with_signer_registry(Arc::clone(reg));
        }
        let layouts = Arc::new(RwLock::new(
            LayoutStore::load(&cfg.sniper.pump_layout_file).await,
        ));
        let risk = RiskEngine::new(state.clone());
        let leaders = LeaderRegistry::from_config(&cfg.copy.wallets);
        CopyBot {
            state,
            rpc,
            wallet,
            executor,
            risk,
            layouts,
            signers,
            intents: None,
            ownership: None,
            leaders: Arc::new(RwLock::new(leaders)),
            ordering: OrderingTracker::new(cfg.copy.strict_ordering),
            store: Arc::new(MemoryCopyStore::new()),
            sequence: 0,
        }
    }

    /// Attach the write-ahead intent journal (§I crash point C). Broadcasts
    /// are journaled BEFORE they leave the process and linked to their
    /// signature (or abandoned) immediately after; orphans gate their symbol
    /// at the next startup reconciliation — they are never resubmitted.
    #[must_use]
    pub fn with_intent_sink(mut self, sink: Arc<dyn bot_core::recovery::IntentSink>) -> Self {
        self.intents = Some(sink);
        self
    }

    /// Attach the distributed execution-ownership registry (Prompt 3
    /// §B/§F/§P): mirrored entries claim `copy:{wallet}:{mint}` (whale
    /// dedup across replicas, §H) and exits claim `exit:{position}:{rule}`.
    #[must_use]
    pub fn with_ownership(mut self, reg: Arc<bot_core::ownership::OwnershipRegistry>) -> Self {
        self.ownership = Some(reg);
        self
    }

    /// Attach the durable copy journal (TASK 3 §09). Without it the engine
    /// keeps leaders, processed events and links in memory for the lifetime
    /// of the process.
    #[must_use]
    pub fn with_copy_store(mut self, store: Arc<dyn CopyStore>) -> Self {
        self.store = store;
        self
    }

    /// The leader registry (shared with the API / Telegram for read-only
    /// snapshots).
    pub fn leaders(&self) -> Arc<RwLock<LeaderRegistry>> {
        self.leaders.clone()
    }

    /// The durable journal in use.
    pub fn store(&self) -> Arc<dyn CopyStore> {
        self.store.clone()
    }

    /// Per-leader ordering cursors.
    pub fn ordering(&self) -> &OrderingTracker {
        &self.ordering
    }

    /// Next delivery number for events built by this bot.
    pub(crate) fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    /// Lift a raw feed trade into an event numbered by this bot.
    pub fn event_from_trade(&mut self, trade: &WalletTrade, feed: &str) -> LeaderTradeEvent {
        let sequence = self.next_sequence();
        LeaderTradeEvent::from_wallet_trade(trade, EventSource::from_feed(feed), sequence)
    }

    /// Reconcile the leader registry with `[copy].wallets` (config hot
    /// reload): follows, rule changes, pause/resume toggles and unfollows are
    /// journaled, audited and metered. Returns the transitions performed.
    pub async fn sync_leaders(&mut self, cfg: &Config) -> Vec<LeaderTransition> {
        let transitions = {
            let mut reg = self.leaders.write().await;
            reg.sync_from_config(&cfg.copy.wallets)
        };
        self.ordering.set_strict(cfg.copy.strict_ordering);
        for t in &transitions {
            if t.event == leader::LeaderEvent::Unfollowed {
                self.ordering.forget(&t.address);
            }
            metrics::count_leader_event(t.event.as_str());
            audit::publish_leader(
                &self.state,
                &t.address,
                t.event.as_str(),
                t.reason.as_deref(),
            );
            if !self
                .store
                .append_leader_event(t.record(self.state.replica_id()))
                .await
            {
                metrics::count_journal_error("append_leader_event");
            }
            self.persist_leader(&t.address).await;
            info!(leader = %t.address, event = %t.event, reason = ?t.reason, "copy leader lifecycle");
        }
        self.publish_leader_gauges().await;
        transitions
    }

    /// Write one leader's current row to the journal (best effort).
    pub(crate) async fn persist_leader(&self, address: &str) {
        let rec = {
            let reg = self.leaders.read().await;
            reg.get(address).map(|l| l.record())
        };
        if let Some(rec) = rec {
            if !self.store.upsert_leader(rec).await {
                metrics::count_journal_error("upsert_leader");
            }
        }
    }

    async fn publish_leader_gauges(&self) {
        let (active, paused, removed) = self.leaders.read().await.counts();
        metrics::set_leader_gauge("active", active);
        metrics::set_leader_gauge("paused", paused);
        metrics::set_leader_gauge("removed", removed);
    }

    /// Restart recovery (TASK 3 §09): restore leader counters, re-seed the
    /// dedup facade and ordering cursors from the durable journal, hold
    /// ambiguous entries, clean up entries that provably never landed and
    /// repair links against the position book. Returns the plan applied.
    pub async fn recover_after_restart(&mut self, cfg: &Config) -> RecoveryPlan {
        if let Some(rows) = self.store.load_leaders().await {
            let applied = self.leaders.write().await.restore(&rows);
            if applied > 0 {
                info!(
                    leaders = applied,
                    "copy leader counters restored from journal"
                );
            }
        } else {
            warn!("copy journal unavailable — leader counters start from zero");
        }
        let lookback = cfg.copy.recovery_lookback_hours.max(0);
        let journal = if lookback > 0 {
            self.store
                .events_since(Utc::now() - Duration::hours(lookback), 10_000)
                .await
                .unwrap_or_else(|| {
                    warn!("copy journal unavailable — dedup is not re-seeded after this restart");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let links = self.store.open_links().await.unwrap_or_default();
        let positions = self.copy_positions().await;
        let ledger: Vec<bot_core::execution::ExecutionRecord> = bot_core::execution::ledger()
            .list(5_000)
            .await
            .into_iter()
            .filter(|r| r.module == BotModule::Copy.as_str())
            .collect();
        let plan = recovery::plan_recovery(&RecoveryInputs {
            journal,
            links,
            positions,
            ledger,
        });
        for action in &plan.actions {
            metrics::count_recovery_action(action.as_str());
            match action {
                RecoveryAction::SeedDedup { keys } => {
                    let n = event_dedup::seed(&self.state, keys.iter()).await;
                    info!(
                        seeded = n,
                        journaled = keys.len(),
                        "copy dedup re-seeded from journal"
                    );
                    audit::publish_recovery(
                        &self.state,
                        action.as_str(),
                        "dedup",
                        &format!("{n} keys marked from {} journaled events", keys.len()),
                    );
                }
                RecoveryAction::SeedCursor {
                    leader,
                    slot,
                    signature,
                } => {
                    self.ordering.seed(leader, *slot, signature);
                }
                RecoveryAction::HoldAmbiguous {
                    intent_id,
                    position_id,
                    mint,
                } => {
                    warn!(
                        intent = %intent_id,
                        position = ?position_id,
                        %mint,
                        "mirrored entry still live in the execution ledger — held, never resubmitted"
                    );
                    audit::publish_recovery(
                        &self.state,
                        action.as_str(),
                        mint,
                        &format!(
                            "intent {intent_id} position {} — ledger/reconciliation own the outcome",
                            position_id.as_deref().unwrap_or("-")
                        ),
                    );
                }
                RecoveryAction::CleanupFailedEntry {
                    position_id,
                    signature,
                } => {
                    self.state
                        .with_position(position_id, |p| {
                            p.qty = 0.0;
                            p.cost_basis = 0.0;
                        })
                        .await;
                    self.state
                        .close_position(
                            position_id,
                            PositionStatus::Failed,
                            "entry never landed (execution ledger: failed/expired)",
                        )
                        .await;
                    self.store
                        .close_link(
                            position_id,
                            "closed",
                            None,
                            Some("entry never landed; cleaned up at restart"),
                        )
                        .await;
                    audit::publish_recovery(
                        &self.state,
                        action.as_str(),
                        position_id,
                        &format!("closed as failed; entry signature {signature}"),
                    );
                    info!(position = %position_id, "failed copy entry cleaned up (no tokens were ever received)");
                }
                RecoveryAction::RestoreLink { link } => {
                    if !self.store.upsert_link((**link).clone()).await {
                        metrics::count_journal_error("upsert_link");
                    }
                    audit::publish_recovery(
                        &self.state,
                        action.as_str(),
                        &link.position_id,
                        &format!("leader {} mint {}", link.leader, link.mint),
                    );
                }
                RecoveryAction::CloseLink {
                    position_id,
                    status,
                    note,
                } => {
                    self.store
                        .close_link(position_id, status, None, Some(note))
                        .await;
                    audit::publish_recovery(&self.state, action.as_str(), position_id, note);
                }
            }
        }
        if !plan.is_empty() {
            info!(
                actions = plan.actions.len(),
                "copy restart recovery applied"
            );
        }
        plan
    }

    /// One reconciliation pass (TASK 3 §08): compare links, the position
    /// book and journaled leader activity; close / refresh links; flag
    /// drift; optionally mirror a leader's exit (`copy.reconcile_auto_exit`)
    /// through the normal exit path. Returns the report.
    pub async fn reconcile_once(&mut self, cfg: &Config) -> ReconReport {
        let links = self.store.open_links().await.unwrap_or_default();
        let lookback = cfg.copy.recovery_lookback_hours.max(1);
        let activity = self
            .store
            .events_since(Utc::now() - Duration::hours(lookback), 10_000)
            .await
            .unwrap_or_default();
        let positions = self.copy_positions().await;
        let leader_status: HashMap<String, leader::LeaderStatus> = self
            .leaders
            .read()
            .await
            .all()
            .into_iter()
            .map(|l| (l.address.clone(), l.status))
            .collect();
        let ledger: Vec<bot_core::execution::ExecutionRecord> = bot_core::execution::ledger()
            .list(5_000)
            .await
            .into_iter()
            .filter(|r| r.module == BotModule::Copy.as_str())
            .collect();
        let report = reconcile::reconcile(&ReconInputs {
            links,
            positions,
            activity,
            leader_status,
            ledger,
            auto_exit: cfg.copy.reconcile_auto_exit && cfg.copy.mirror_exits,
        });
        for finding in &report.findings {
            metrics::count_recon_finding(finding.kind.as_str());
            audit::publish_recon(
                &self.state,
                finding.kind.as_str(),
                &finding.leader,
                &finding.mint,
                &format!("{} action={}", finding.detail, finding.action.as_str()),
            );
            match &finding.action {
                ReconAction::Flag => {}
                ReconAction::CloseLink {
                    position_id,
                    status,
                } => {
                    self.store
                        .close_link(position_id, status, None, Some(&finding.detail))
                        .await;
                }
                ReconAction::UpdateLink {
                    position_id,
                    follower_qty,
                } => {
                    if let Some(mut link) = self
                        .store
                        .open_links()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .find(|l| &l.position_id == position_id)
                    {
                        link.follower_qty = *follower_qty;
                        link.last_reconciled_at = Some(Utc::now());
                        link.updated_at = Utc::now();
                        if !self.store.upsert_link(link).await {
                            metrics::count_journal_error("upsert_link");
                        }
                    }
                }
                ReconAction::MirrorExit {
                    position_id,
                    fraction,
                } => {
                    self.reconcile_exit(position_id, *fraction, finding).await;
                }
            }
        }
        // Per-leader exposure gauges ride along with every pass.
        let leaders: Vec<String> = self
            .leaders
            .read()
            .await
            .all()
            .into_iter()
            .map(|l| l.address.clone())
            .collect();
        for address in leaders {
            let (exposure, _) = self.risk.leader_exposure(&address).await;
            metrics::set_leader_exposure(&address, exposure);
        }
        report
    }

    /// Sell a position whose leader exited, through the same ownership +
    /// exit path a live mirrored exit uses.
    async fn reconcile_exit(
        &mut self,
        position_id: &str,
        fraction: f64,
        finding: &reconcile::Finding,
    ) {
        let Some(position) = self
            .state
            .open_positions_for(BotModule::Copy)
            .await
            .into_iter()
            .find(|p| p.id == position_id)
        else {
            return;
        };
        let mut permit = match bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("exit:{}:recon_exit", position.id),
            "exit",
            "copy",
            "recon_exit",
            &position.symbol,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!(position = %position.id, error = %e, "reconciliation exit ownership unavailable — failing closed");
                return;
            }
        };
        if !permit.proceed() {
            return;
        }
        let res = exit::sell_position(
            &self.state,
            &self.rpc,
            &self.wallet,
            &mut self.executor,
            &self.layouts,
            &self.risk,
            &position,
            fraction,
            "leader exited (reconciliation)",
            None,
            self.intents.as_ref(),
            &mut permit,
        )
        .await;
        match res {
            Ok(()) => {
                if self
                    .state
                    .find_open(BotModule::Copy, &position.symbol)
                    .await
                    .is_none()
                {
                    self.store
                        .close_link(&position.id, "closed", None, Some(&finding.detail))
                        .await;
                }
                audit::publish_recon(
                    &self.state,
                    "mirror_exit_done",
                    &finding.leader,
                    &finding.mint,
                    &format!("position {} sold ({fraction:.2} of holding)", position.id),
                );
            }
            Err(e) => {
                permit.finish(false).await;
                warn!(position = %position.id, error = %e, "reconciliation exit failed");
                self.state
                    .record_error(
                        BotModule::Copy,
                        &format!("recon exit {}: {e}", position.symbol),
                    )
                    .await;
            }
        }
    }

    /// Every copy position in the book (open and closed).
    async fn copy_positions(&self) -> Vec<bot_core::models::Position> {
        self.state
            .all_positions()
            .await
            .into_iter()
            .filter(|p| p.source == bot_core::models::TradeSource::Copy)
            .collect()
    }

    /// Run the copy bot until the task is aborted.
    pub async fn run(&mut self, mut feeds: mpsc::Receiver<WalletTrade>) -> BotResult<()> {
        self.state.set_running(BotModule::Copy, true, true).await;
        self.state.set_detail(BotModule::Copy, "starting").await;
        self.state.heartbeat(BotModule::Copy).await;

        let cfg = self.state.config_snapshot().await;
        // Restart recovery before anything can act on the book.
        self.sync_leaders(&cfg).await;
        let plan = self.recover_after_restart(&cfg).await;
        if !plan.is_empty() {
            self.state
                .set_detail(
                    BotModule::Copy,
                    &format!("recovered ({} actions)", plan.actions.len()),
                )
                .await;
        }

        // Independent exit sweeper for our own TP/SL on mirrored positions.
        let mut sweeper = exit::ExitSweeper::new(
            self.state.clone(),
            self.rpc.clone(),
            self.wallet.clone(),
            self.layouts.clone(),
            self.signers.clone(),
            self.intents.clone(),
            self.ownership.clone(),
        )
        .await;
        tokio::spawn(async move { sweeper.run().await });

        info!(wallets = cfg.copy.wallets.len(), "copy-trading bot running");
        let recon_secs = cfg.copy.reconcile_interval_secs.max(1);
        let mut recon = tokio::time::interval(std::time::Duration::from_secs(recon_secs));
        recon.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        recon.tick().await; // the first tick completes immediately

        loop {
            // Shutdown-aware receive (mirrors module 1) plus the periodic
            // reconciliation tick.
            let trade = tokio::select! {
                t = feeds.recv() => t,
                _ = recon.tick() => {
                    let cfg = self.state.config_snapshot().await;
                    if cfg.copy.enabled && cfg.copy.reconcile_interval_secs > 0 {
                        self.sync_leaders(&cfg).await;
                        let report = self.reconcile_once(&cfg).await;
                        if !report.is_clean() {
                            info!(findings = report.findings.len(), "copy reconciliation findings");
                        }
                    }
                    self.state.heartbeat(BotModule::Copy).await;
                    continue;
                }
                _ = self.state.wait_shutdown() => None,
            };
            let Some(trade) = trade else {
                info!("module 2 (copy) stopping (shutdown or feed closed)");
                break;
            };
            // Decision-queue backlog (cheap atomic read of the mpsc length).
            bot_core::obs::metrics::global()
                .gauge(
                    "bot_module_queue_depth",
                    "Pending items in the module's decision queue.",
                    &[("module", "copy")],
                )
                .set(feeds.len() as i64);
            let cfg = self.state.config_snapshot().await;
            if !cfg.copy.enabled || self.state.kill_switch() {
                continue;
            }
            if !self.state.is_enabled(BotModule::Copy).await {
                continue;
            }
            // Config hot reload: keep the registry in step with [copy].wallets.
            self.sync_leaders(&cfg).await;
            // The pipeline owns de-duplication (event_dedup): the feed's
            // signature mark is fetch suppression only and is NOT consulted
            // here — re-marking it dropped every polled/Geyser event before.
            let event = self.event_from_trade(&trade, &cfg.copy.feed);
            let outcome = self.process_event(&event, &cfg).await;
            if outcome.stage == CopyStage::Failed {
                let detail = outcome
                    .rejection
                    .as_ref()
                    .map(|r| r.detail.clone())
                    .unwrap_or_default();
                warn!(
                    wallet = %trade.wallet,
                    mint = %trade.mint,
                    error = %detail,
                    "copy mirror failed"
                );
                self.state
                    .record_error(BotModule::Copy, &format!("{}: {detail}", trade.mint))
                    .await;
            }
            self.state.heartbeat(BotModule::Copy).await;
        }

        self.state.set_running(BotModule::Copy, false, false).await;
        Ok(())
    }

    /// Spawn the merged copy feed (PumpPortal account trades + RPC polling).
    pub async fn spawn_feed(&self) -> BotResult<mpsc::Receiver<WalletTrade>> {
        CopyFeed::spawn(self.state.clone(), self.rpc.clone()).await
    }

    /// The tracked wallets (empty when copy trading is not configured).
    pub async fn tracked_wallets(&self) -> Vec<CopyWallet> {
        self.state.config_snapshot().await.copy.wallets
    }

    /// Re-read config and refresh the executor policy (picks up hot reloads).
    pub async fn refresh_policy(&mut self) {
        let cfg = self.state.config_snapshot().await;
        self.executor
            .set_policy(solana_kit::execute::exec_policy_from_config(&cfg));
        let fees = solana_kit::execute::fee_policy_from_config(&cfg);
        if *self.executor.fee_policy() != fees {
            self.executor.set_fee_policy(fees);
        }
    }

    /// SOL we can spend. In paper mode we use the configured demo balance.
    pub async fn available_sol(&self) -> BotResult<f64> {
        let cached = self.state.balances().await.sol;
        if self.state.execution_mode().await == ExecutionMode::Paper && cached > 0.0 {
            return Ok(cached);
        }
        let bal = self.rpc.get_balance(&self.wallet.pubkey).await?;
        Ok(bot_core::maths::lamports_to_sol(bal))
    }

    /// Convenience accessor used by tests.
    pub fn trade_side(trade: &WalletTrade) -> PositionSide {
        trade.side
    }
}

/// Build the execution policy from config (re-exported for convenience).
pub fn exec_policy(cfg: &Config) -> ExecPolicy {
    solana_kit::execute::exec_policy_from_config(cfg)
}
