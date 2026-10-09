#![forbid(unsafe_code)]
//! Module 1 — the new-launch sniper.
//!
//! ## What it does
//! Watches pump.fun (bonding-curve creations), PumpSwap (pool creations) and
//! Raydium AMM v4 (pool initialisations) for brand-new tokens, runs every
//! observation through one deterministic validation pipeline and buys within
//! roughly a second of launch, then manages the exit (take-profit /
//! stop-loss / trailing / time / stale-position).
//!
//! ## How it is wired
//! ```text
//!  PumpPortal ──────┐
//!  logsSubscribe ───┼─► detect::LaunchDetector ─► mpsc<event::LaunchEvent> ─► Sniper::run
//!  transactionSubscribe ┘   (normalise + sequence + raw hash)                  │
//!                                                                              ▼
//!   entry::consider_event — pipeline::SniperStage lifecycle
//!     DETECTED  ─ shape · route · kill/enabled · age · symbol gate · dedup · screening
//!     VALIDATED ─ rpc ready · balance · market::load_market · gates::evaluate
//!                 · slippage::decide · price impact · risk.check_entry
//!     RISK_APPROVED ─ ownership permit · build (curve | pumpswap | raydium | jupiter)
//!                 · latency budget · snapshot freshness
//!     EXECUTION_READY ─► SUBMITTED (Executor: ledger, intent id, fee policy) ─► CONFIRMED
//!                                                                              │
//!                                              state.upsert_position + EventBus + Audit
//!                                                                              │
//!   exit::sweep — failed-entry cleanup · ambiguous hold · venue mark · risk.check_exit
//!                 · stale rule · retry backoff ─► sell (same engine, exit intent id)
//! ```
//!
//! `replay::ReplayEngine` drives the same validation code over recorded
//! fixtures without any network or submission.
//!
//! ## Safety
//! Every buy and sell flows through [`bot_core::risk::RiskEngine`] and the
//! shared kill switch, and is executed by [`solana_kit::execute::Executor`] in
//! the configured [`ExecutionMode`]. The default is `Paper`, which builds the
//! real transaction against live chain data but never broadcasts it. Nothing
//! is sent to the network unless `execution.mode = "live"` *and*
//! `execution.allow_live_trading = true`.
//!
//! The pump.fun account layout has changed several times without notice; the
//! [`solana_kit::layout::LayoutStore`] lets a working transaction repair a
//! broken build without a code change (see `sniper.pump_layout_file`).

use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use bot_core::config::Config;
use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::risk::RiskEngine;
use bot_core::state::Shared;

use solana_kit::execute::{ExecPolicy, Executor};
use solana_kit::layout::LayoutStore;
use solana_kit::rpc::Rpc;
use solana_kit::tenant_broadcast_guard::TenantBroadcastGuard;
use solana_kit::tenant_signing_context::TenantSigningContext;
use solana_kit::tenant_transaction::TenantTransactionMeta;
use solana_kit::tokens::Wallet;

pub mod backtest;
pub mod dca;
pub mod detect;
pub mod entry;
pub mod event;
pub mod exit;
pub mod exit_policy;
pub mod gates;
pub mod limit_orders;
pub mod market;
pub mod pipeline;
pub mod replay;
pub mod risk_intel;
pub mod slippage;
pub mod tenant_context;
pub mod tenant_executor;
pub mod tenant_state;

pub use detect::LaunchDetector;
pub use entry::EntryOutcome;
pub use event::{LaunchEvent, LaunchProtocol};
pub use pipeline::{EntryRoute, RejectReason, SniperStage};

const OWNED_TASK_ABORT_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

struct OwnedTask {
    handle: Option<tokio::task::JoinHandle<()>>,
    label: &'static str,
}

impl OwnedTask {
    fn new(handle: tokio::task::JoinHandle<()>, label: &'static str) -> Self {
        Self {
            handle: Some(handle),
            label,
        }
    }

    async fn join_with_timeout(mut self, timeout: std::time::Duration) {
        let result = {
            let Some(task) = self.handle.as_mut() else {
                return;
            };
            tokio::time::timeout(timeout, task).await
        };
        match result {
            Ok(Ok(())) => {
                self.handle.take();
            }
            Ok(Err(error)) => {
                if !error.is_cancelled() {
                    warn!(task = self.label, error = %error, "owned task failed");
                }
                self.handle.take();
            }
            Err(_) => {
                warn!(task = self.label, "task did not stop before deadline; aborting it");
                if let Some(task) = &self.handle {
                    task.abort();
                }
                let aborted = {
                    let Some(task) = self.handle.as_mut() else {
                        return;
                    };
                    tokio::time::timeout(OWNED_TASK_ABORT_GRACE, task).await
                };
                match aborted {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if !error.is_cancelled() => {
                        warn!(task = self.label, error = %error, "aborted task failed while joining");
                    }
                    Ok(Err(_)) => {}
                    Err(_) => warn!(task = self.label, "task did not finish during abort join grace"),
                }
                self.handle.take();
            }
        }
    }
}

impl Drop for OwnedTask {
    fn drop(&mut self) {
        if let Some(task) = &self.handle {
            task.abort();
        }
    }
}

/// The sniper. One instance owns the detection feeds, the execution path and
/// the exit sweeper for Module 1.
pub struct Sniper {
    state: Shared,
    rpc: Rpc,
    wallet: Arc<Wallet>,
    executor: Executor,
    risk: RiskEngine,
    layouts: Arc<RwLock<LayoutStore>>,
    /// Signer registry for transactions that need signers beyond the wallet.
    /// `None` keeps wallet-only behaviour; required-but-missing signers then
    /// fail the build explicitly (see `solana_kit::tx`).
    signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
    /// Write-ahead intent journal (§I crash point C), injected by the server
    /// when `[recovery] intent_journal` is on. `None` = legacy behaviour.
    intents: Option<Arc<dyn bot_core::recovery::IntentSink>>,
    /// Distributed execution ownership (Prompt 3 §B/§F), injected by the
    /// server. When present, every money path claims its logical execution
    /// identity before broadcasting; `None` = single-instance/legacy.
    ownership: Option<Arc<bot_core::ownership::OwnershipRegistry>>,
    /// Tenant execution context (PROMPT 4/10). When present, every
    /// money-moving request is stamped with tenant metadata and the
    /// executor carries the final tenant broadcast guard — an unscoped or
    /// cross-tenant broadcast is refused fail-closed. `None` = the
    /// deployment-global operator mode, byte-for-byte unchanged.
    tenant: Option<TenantSigningContext>,
    /// The guard attached to the internal executor. Kept alongside the
    /// context so the exit sweeper's SEPARATE executor carries the SAME
    /// fail-closed gate — exits move money too and must never run
    /// unguarded in tenant mode.
    tenant_guard: Option<Arc<TenantBroadcastGuard>>,
    /// Sweeper-local exit bookkeeping (mark failures, retry backoff).
    exits: exit::ExitTracker,
    /// Advanced exit policy (GAP-MAP v2 P2): laddered take-profit,
    /// break-even stop, dev-sell trigger. Built from
    /// `sniper.advanced_exit`; inert (no-op) when that block is absent, so
    /// classic behaviour is unchanged unless the operator opts in.
    exit_policy: crate::exit_policy::ExitPolicyEngine,
    /// Dev-sell signal feed from the risk_intel layer (mint + observed
    /// time). The sweeper drains this into `exit_policy` each pass. `None`
    /// = no dev-sell detection wired.
    dev_sell_rx: Option<tokio::sync::broadcast::Receiver<crate::exit_policy::DevSellSignal>>,
    /// The dev-sell bus sender kept so the exit-sweeper clone can subscribe
    /// (`None` until `with_dev_sell_bus` attaches one).
    dev_sell_tx: Option<crate::exit_policy::DevSellBus>,
}

impl Sniper {
    /// Build a sniper from the shared state and a loaded wallet.
    ///
    /// The execution policy is derived from the live config now and refreshed
    /// before every trade, so an operator toggling `execution.mode` over
    /// Telegram or REST takes effect on the next snipe without a restart.
    pub async fn new(
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
    ) -> BotResult<Self> {
        let cfg = state.config_snapshot().await;
        let policy = exec_policy(&cfg);
        let mut executor = Executor::new(rpc.clone(), wallet.clone(), policy)
            .with_fee_policy(solana_kit::execute::fee_policy_from_config(&cfg));
        // Dynamic Jito tip (GAP-MAP P1): priced from the tip floor when
        // `execution.jito_dynamic_tip_percentile > 0`.
        if let Some(tip) = solana_kit::execute::dynamic_tip_from_config(&cfg) {
            executor = executor.with_dynamic_tip(tip);
        }
        if let Some(reg) = &signers {
            executor = executor.with_signer_registry(Arc::clone(reg));
        }
        let risk = RiskEngine::new(state.clone());

        // Load (or start) the account-layout store so a learned template
        // survives restarts. `load` already swallows missing/corrupt files.
        let path = cfg.sniper.pump_layout_file.clone();
        let layouts = if path.trim().is_empty() {
            info!("sniper layout store disabled (empty pump_layout_file)");
            Arc::new(RwLock::new(LayoutStore::default()))
        } else {
            let store = LayoutStore::load(&path).await;
            info!(path = %path, count = store.layouts.len(), "loaded pump account-layout store");
            Arc::new(RwLock::new(store))
        };

        Ok(Sniper {
            state,
            rpc,
            wallet,
            executor,
            risk,
            layouts,
            signers,
            intents: None,
            ownership: None,
            tenant: None,
            tenant_guard: None,
            exits: exit::ExitTracker::default(),
            exit_policy: crate::exit_policy::engine_from_config(cfg.sniper.advanced_exit.as_ref()),
            dev_sell_rx: None,
            dev_sell_tx: None,
        })
    }

    /// Attach the distributed execution-ownership registry (Prompt 3
    /// §B/§F). Entry and exit money paths then claim their logical
    /// execution id before any broadcast: losers skip deterministically
    /// (§G), stale owners are fenced (§E), and ownership-store failures
    /// fail closed (§K).
    #[must_use]
    pub fn with_ownership(mut self, reg: Arc<bot_core::ownership::OwnershipRegistry>) -> Self {
        self.ownership = Some(reg);
        self
    }

    /// Attach the write-ahead intent journal (§I crash point C). Every
    /// money-moving broadcast is journaled BEFORE it leaves the process and
    /// linked to its signature (or abandoned) immediately after; orphans are
    /// reconciled at startup and gate their symbol — never resubmitted.
    #[must_use]
    pub fn with_intent_sink(mut self, sink: Arc<dyn bot_core::recovery::IntentSink>) -> Self {
        self.intents = Some(sink);
        self
    }

    /// Attach the dev-sell signal bus (GAP-MAP v2 P2). The risk_intel layer
    /// publishes `(mint, observed_at)` here; the exit sweeper subscribes and
    /// feeds the advanced exit policy's dev-sell trigger. Attaching also
    /// (re)subscribes THIS sniper's own receiver.
    #[must_use]
    pub fn with_dev_sell_bus(mut self, tx: crate::exit_policy::DevSellBus) -> Self {
        self.dev_sell_rx = Some(tx.subscribe());
        self.dev_sell_tx = Some(tx);
        self
    }

    /// The dev-sell bus sender, when attached — risk_intel uses this to
    /// publish creator/dev-sell observations.
    pub fn dev_sell_sender(&self) -> Option<crate::exit_policy::DevSellBus> {
        self.dev_sell_tx.clone()
    }

    /// Bind this sniper to ONE tenant (PROMPT 4/10 §B).
    ///
    /// Attaches the final tenant broadcast guard to the internal executor
    /// (wrong organization / runtime / generation / module / wallet is
    /// refused before signing or broadcast) and remembers the tenant
    /// signing context so every entry and exit request this sniper builds
    /// carries its tenant metadata. The guard's funding wallet must be
    /// this sniper's wallet — a mismatch fails at attach time.
    ///
    /// Without this call the sniper keeps its deployment-global operator
    /// behaviour unchanged.
    pub fn with_tenant_context(mut self, guard: Arc<TenantBroadcastGuard>) -> BotResult<Self> {
        self.executor = self.executor.with_tenant_guard(Arc::clone(&guard))?;
        // Bind the risk engine to the same tenant, so the cluster-wide
        // capacity and daily-loss reads are restricted to THIS tenant.
        // Without this the shared oracle would let another tenant's open
        // positions consume this tenant's capacity.
        self.risk = self.risk.clone().with_tenant(guard.organization_id());
        self.tenant = Some(guard.context().clone());
        self.tenant_guard = Some(guard);
        Ok(self)
    }

    /// The bound tenant context, when this sniper is tenant-scoped.
    pub fn tenant_context(&self) -> Option<&TenantSigningContext> {
        self.tenant.as_ref()
    }

    /// Stamp tenant metadata onto an outgoing request. No-op in operator
    /// mode; in tenant mode the request carries the tenant identity the
    /// guard will verify immediately before signing/broadcast.
    pub(crate) fn tenant_stamp(&self, req: solana_kit::tx::TxRequest) -> solana_kit::tx::TxRequest {
        // An explicitly attached meta is NEVER overwritten: if a caller
        // pinned (foreign) tenant metadata, the guard must judge it and
        // deny — silently rewriting it to our own identity would mask
        // exactly the cross-tenant mistake the guard exists to catch.
        if req.tenant.is_some() {
            return req;
        }
        match &self.tenant {
            Some(ctx) => {
                let meta = TenantTransactionMeta::from_context(
                    ctx.execution_context(),
                    &req.module,
                    req.intent_id.as_deref(),
                );
                req.tenant(meta)
            }
            None => req,
        }
    }

    /// Stamp tenant metadata onto a prebuilt (Jupiter-signed) transaction.
    pub(crate) fn tenant_stamp_built(
        &self,
        built: solana_kit::tx::BuiltTx,
    ) -> solana_kit::tx::BuiltTx {
        // Same rule as `tenant_stamp`: explicit metadata is judged, never
        // rewritten.
        if built.tenant.is_some() {
            return built;
        }
        match &self.tenant {
            Some(ctx) => {
                let meta = TenantTransactionMeta::from_context(
                    ctx.execution_context(),
                    &built.module,
                    Some(built.intent_id.as_str()),
                );
                built.with_tenant(meta)
            }
            None => built,
        }
    }

    /// Fresh intent record for one money-moving broadcast.
    pub(crate) fn intent_rec(
        &self,
        symbol: &str,
        side: &str,
        qty: &str,
    ) -> bot_core::db::repo::IntentRecord {
        bot_core::db::repo::IntentRecord {
            intent_id: self.state.next_id("intent"),
            module: "sniper".into(),
            symbol: symbol.to_string(),
            wallet: self.wallet.pubkey.to_string(),
            side: side.into(),
            qty: qty.into(),
            status: "pending".into(),
            signature: None,
            created_at: chrono::Utc::now(),
        }
    }

    /// Run an already-built request through this sniper's executor
    /// (tenant-stamped first when a tenant context is bound). Used by the
    /// tenant executor for scoped submissions and by tests that need to
    /// exercise the guarded money path directly.
    pub async fn execute_request(
        &self,
        req: solana_kit::tx::TxRequest,
    ) -> BotResult<solana_kit::execute::ExecutionResult> {
        let req = self.tenant_stamp(req);
        self.executor.run(req).await
    }

    pub fn state(&self) -> &Shared {
        &self.state
    }

    pub fn rpc(&self) -> &Rpc {
        &self.rpc
    }

    pub fn wallet(&self) -> &Wallet {
        &self.wallet
    }

    pub fn risk(&self) -> &RiskEngine {
        &self.risk
    }

    pub fn layouts(&self) -> &Arc<RwLock<LayoutStore>> {
        &self.layouts
    }

    /// Refresh the executor policy from the live config. Called before each
    /// trade so runtime toggles are honoured.
    async fn refresh_policy(&mut self) {
        let cfg = self.state.config_snapshot().await;
        self.executor.set_policy(exec_policy(&cfg));
        let fees = solana_kit::execute::fee_policy_from_config(&cfg);
        if *self.executor.fee_policy() != fees {
            self.executor.set_fee_policy(fees);
        }
    }

    /// Run the module until the process is shut down.
    ///
    /// This is the task the server spawns for Module 1. It never returns under
    /// normal operation; it idles when the module is disabled and resumes when
    /// it is re-enabled, so an operator can toggle it live.
    pub async fn run(mut self) {
        info!("module 1 (sniper) task started");
        self.state.set_detail(BotModule::Sniper, "starting").await;

        // The exit sweeper runs independently of launch detection: even if the
        // feeds drop, open positions must still be managed.
        let sweeper = {
            let sweeper_cfg = self.state.config_snapshot().await;
            let mut sweeper_executor = Executor::new(
                self.rpc.clone(),
                self.wallet.clone(),
                exec_policy(&sweeper_cfg),
            )
            .with_fee_policy(solana_kit::execute::fee_policy_from_config(&sweeper_cfg));
            if let Some(tip) = solana_kit::execute::dynamic_tip_from_config(&sweeper_cfg) {
                sweeper_executor = sweeper_executor.with_dynamic_tip(tip);
            }
            if let Some(reg) = &self.signers {
                sweeper_executor = sweeper_executor.with_signer_registry(Arc::clone(reg));
            }
            // PROMPT 4/10 §B: in tenant mode the sweeper's executor must
            // carry the SAME final tenant broadcast guard as the entry
            // executor — exits move money too. A guard that cannot attach
            // is a fail-closed stop, never an unguarded sweeper.
            if let Some(guard) = &self.tenant_guard {
                match sweeper_executor.with_tenant_guard(Arc::clone(guard)) {
                    Ok(executor_with_guard) => sweeper_executor = executor_with_guard,
                    Err(e) => {
                        error!(error = %e, "tenant guard could not attach to the exit sweeper; sniper stopping");
                        self.state
                            .record_error(BotModule::Sniper, &format!("sweeper tenant guard: {e}"))
                            .await;
                        self.state.set_running(BotModule::Sniper, false, true).await;
                        return;
                    }
                }
            }
            // The sweeper gets its OWN policy-engine instance (per-position
            // ladder cursors belong to the loop that evaluates them) and its
            // own subscription to the dev-sell bus when one is attached.
            let sweeper_exit_policy = crate::exit_policy::engine_from_config(
                sweeper_cfg.sniper.advanced_exit.as_ref(),
            );
            let sweeper_dev_sell_rx = self.dev_sell_tx.as_ref().map(|tx| tx.subscribe());
            let mut this = Sniper {
                state: self.state.clone(),
                rpc: self.rpc.clone(),
                wallet: self.wallet.clone(),
                executor: sweeper_executor,
                risk: self.risk.clone(),
                layouts: self.layouts.clone(),
                signers: self.signers.clone(),
                intents: self.intents.clone(),
                ownership: self.ownership.clone(),
                tenant: self.tenant.clone(),
                tenant_guard: self.tenant_guard.clone(),
                exits: exit::ExitTracker::default(),
                exit_policy: sweeper_exit_policy,
                dev_sell_rx: sweeper_dev_sell_rx,
                dev_sell_tx: self.dev_sell_tx.clone(),
            };
            OwnedTask::new(
                tokio::spawn(async move { this.exit_sweeper().await }),
                "sniper exit sweeper",
            )
        };

        // Launch detection produces a merged stream of normalised events.
        let mut launches =
            match LaunchDetector::spawn(self.state.clone(), self.rpc.clone()).await {
                Ok(rx) => rx,
                Err(e) => {
                    error!(error = %e, "launch detection failed to start; sniper will idle");
                    self.state
                        .record_error(BotModule::Sniper, &format!("detection: {e}"))
                        .await;
                    // Keep the sweeper alive while the service is running,
                    // then stop and join it on process shutdown.
                    self.state.wait_shutdown().await;
                    sweeper
                        .join_with_timeout(std::time::Duration::from_secs(5))
                        .await;
                    return;
                }
            };

        self.state.set_running(BotModule::Sniper, true, true).await;
        self.state.events.publish(AppEvent::Info {
            ts: Utc::now(),
            module: Some(BotModule::Sniper),
            message: "launch detection online".into(),
        });

        // Idle/active bookkeeping: when disabled we drain-and-drop launches so
        // the detector channel never wedges, but we do not trade.
        loop {
            // Shutdown-aware receive: SIGTERM stops the loop instead of
            // killing an in-flight decision.
            let launch = tokio::select! {
                l = launches.recv() => l,
                _ = self.state.wait_shutdown() => None,
            };
            let Some(launch) = launch else {
                info!("module 1 (sniper) stopping (shutdown or feed closed)");
                break;
            };
            // Decision-queue backlog (cheap atomic read of the mpsc length).
            bot_core::obs::metrics::global()
                .gauge(
                    "bot_module_queue_depth",
                    "Pending items in the module's decision queue.",
                    &[("module", "sniper")],
                )
                .set(launches.len() as i64);
            // Heartbeat + connection state on every observed launch.
            self.state.heartbeat(BotModule::Sniper).await;
            self.state.inc_events(BotModule::Sniper, 1).await;

            if !self.state.is_enabled(BotModule::Sniper).await {
                continue; // disabled: observe but do not act
            }
            if self.state.kill_switch() {
                warn!(mint = %launch.mint, "kill switch engaged — skipping launch");
                continue;
            }

            let outcome = self.consider_event(launch).await;
            if outcome.infra_failure() {
                // A single failed snipe is not fatal; record and carry on.
                let detail = outcome
                    .rejection
                    .as_ref()
                    .map(|r| r.detail.clone())
                    .unwrap_or_default();
                self.state
                    .record_error(
                        BotModule::Sniper,
                        &format!("snipe {}: {detail}", outcome.mint),
                    )
                    .await;
                warn!(mint = %outcome.mint, %detail, "snipe attempt failed");
            } else {
                if let Some(r) = &outcome.rejection {
                    debug!(
                        event = %outcome.event_id,
                        reason = %r.reason,
                        stage = %r.stage,
                        "launch not traded"
                    );
                }
                self.state.clear_error(BotModule::Sniper).await;
            }
        }

        // Closing the merged receiver wakes all feed forwarders. Join them
        // before returning so their websocket/PumpPortal owners cannot outlive
        // this module task.
        launches.shutdown().await;
        info!("launch stream ended; stopping sniper");
        self.state
            .set_running(BotModule::Sniper, false, false)
            .await;
        sweeper
            .join_with_timeout(std::time::Duration::from_secs(5))
            .await;
    }

    /// Current execution mode from live config (paper by default).
    pub async fn mode(&self) -> ExecutionMode {
        self.state.execution_mode().await
    }
}

/// Translate the runtime config into an [`ExecPolicy`].
///
/// The hard gate lives in `AppState::may_broadcast`: even `mode = Live` will
/// not broadcast unless `allow_live_trading` is true. We mirror that here by
/// downgrading to `Simulate` when live is requested but not permitted, so the
/// executor never even tries to send.
pub fn exec_policy(cfg: &Config) -> ExecPolicy {
    solana_kit::execute::exec_policy_from_config(cfg)
}

/// Small helper used by entry/exit: the SOL balance available to spend, or an
/// error the caller turns into a risk rejection.
pub async fn available_sol(state: &Shared, wallet: &Wallet, rpc: &Rpc) -> BotResult<f64> {
    match wallet.sol_balance(rpc).await {
        Ok(sol) => {
            // Mirror the balance into shared state for the dashboard.
            state.set_balances(Some(sol), None).await;
            Ok(sol)
        }
        Err(e) => {
            // Only PAPER mode may fall back to the cached demo balance: a
            // missing/unfunded wallet must not block the fill model there.
            // Simulate and live surface the real RPC error, so sizing can
            // never run on a stale or paper-seeded number when the chain read
            // fails (the seed from a paper start would otherwise survive a
            // runtime mode switch).
            let mode = state.execution_mode().await;
            let cached = state.balances().await.sol;
            if let Some(v) = sol_balance_fallback(mode, cached) {
                warn!(error = %e, "balance fetch failed, using cached {v} SOL (paper mode)");
                Ok(v)
            } else {
                Err(BotError::rpc(format!("sol balance: {e}")))
            }
        }
    }
}

/// Pure fallback rule for [`available_sol`]: the cached balance may substitute
/// for a FAILED chain read only in explicit paper mode with a positive cached
/// value. Everything else (simulate/live, empty cache) propagates the error.
pub(crate) fn sol_balance_fallback(mode: ExecutionMode, cached: f64) -> Option<f64> {
    (mode == ExecutionMode::Paper && cached > 0.0).then_some(cached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::config::AppConfig;
    use solana_kit::execute::BroadcastMode;
    use std::time::Duration;

    fn state_with(mode: ExecutionMode, allow_live: bool, jito: bool) -> Shared {
        let mut cfg = AppConfig::from_defaults();
        cfg.raw.execution.mode = mode;
        cfg.raw.execution.allow_live_trading = allow_live;
        cfg.raw.execution.use_jito = jito;
        bot_core::state::AppState::new(cfg)
    }

    #[test]
    fn sol_fallback_is_paper_only() {
        // Paper with a seeded/cached balance may substitute it for a FAILED
        // chain read...
        assert_eq!(sol_balance_fallback(ExecutionMode::Paper, 10.0), Some(10.0));
        // ...but never with an empty/zero cache.
        assert_eq!(sol_balance_fallback(ExecutionMode::Paper, 0.0), None);
        assert_eq!(sol_balance_fallback(ExecutionMode::Paper, -1.0), None);
        // Simulate and live MUST surface the RPC error instead of sizing on a
        // stale or paper-seeded number (regression: mode switch after a paper
        // start left a 10 SOL seed in shared state).
        assert_eq!(sol_balance_fallback(ExecutionMode::Simulate, 10.0), None);
        assert_eq!(sol_balance_fallback(ExecutionMode::Live, 10.0), None);
    }

    #[test]
    fn paper_mode_maps_to_paper_policy() {
        let cfg = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.mode = ExecutionMode::Paper;
            c.raw
        };
        let p = exec_policy(&cfg);
        assert_eq!(p.mode, ExecutionMode::Paper);
        assert!(p.simulate_first);
        assert!(p.abort_on_simulation_failure);
    }

    #[test]
    fn live_without_the_gate_downgrades_to_simulate() {
        let cfg = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.mode = ExecutionMode::Live;
            c.raw.execution.allow_live_trading = false; // gate closed
            c.raw
        };
        let p = exec_policy(&cfg);
        assert_eq!(
            p.mode,
            ExecutionMode::Simulate,
            "live requested but not allowed must simulate, never broadcast"
        );
    }

    #[test]
    fn live_with_the_gate_stays_live() {
        let cfg = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.mode = ExecutionMode::Live;
            c.raw.execution.allow_live_trading = true;
            c.raw
        };
        let p = exec_policy(&cfg);
        assert_eq!(p.mode, ExecutionMode::Live);
    }

    #[test]
    fn jito_flag_selects_the_broadcast_mode() {
        let cfg = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.use_jito = true;
            c.raw.execution.jito_block_engine_url = "https://example.jito".into();
            c.raw
        };
        let p = exec_policy(&cfg);
        assert!(matches!(p.broadcast, BroadcastMode::JitoThenRpc));
        assert_eq!(p.jito_url.as_deref(), Some("https://example.jito"));

        let cfg2 = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.use_jito = false;
            c.raw
        };
        let p2 = exec_policy(&cfg2);
        assert!(matches!(p2.broadcast, BroadcastMode::Rpc));
        assert!(p2.jito_url.is_none());
    }

    #[test]
    fn retries_are_clamped_into_the_valid_range() {
        let cfg = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.send_retries = 999;
            c.raw
        };
        assert_eq!(exec_policy(&cfg).max_attempts, 5);
        let cfg = {
            let mut c = AppConfig::from_defaults();
            c.raw.execution.send_retries = 0;
            c.raw
        };
        assert_eq!(exec_policy(&cfg).max_attempts, 1);
    }

    #[tokio::test]
    async fn confirm_intervals_come_from_config() {
        let state = state_with(ExecutionMode::Paper, false, false);
        state
            .update_config(|c| {
                c.execution.confirm_timeout_ms = 1234;
                c.execution.confirm_poll_ms = 250;
            })
            .await;
        let cfg = state.config_snapshot().await;
        let p = exec_policy(&cfg);
        assert_eq!(p.confirm_timeout, Duration::from_millis(1234));
        assert_eq!(p.confirm_poll_interval, Duration::from_millis(250));
    }

    #[tokio::test]
    async fn a_zero_poll_interval_is_floored() {
        let state = state_with(ExecutionMode::Paper, false, false);
        state
            .update_config(|c| c.execution.confirm_poll_ms = 0)
            .await;
        let p = exec_policy(&state.config_snapshot().await);
        assert_eq!(p.confirm_poll_interval, Duration::from_millis(50));
    }
}
