//! Module 1 — the new-launch sniper.
//!
//! ## What it does
//! Watches pump.fun for brand-new tokens and buys within roughly a second of
//! launch, then manages the exit (take-profit / stop-loss / trailing / time).
//!
//! ## How it is wired
//! ```text
//!  PumpPortal ─┐
//!              ├─► detect::LaunchDetector ─► mpsc<TokenLaunch> ─► Sniper::run
//!  logsSubscribe┘                                                    │
//!                                                                    ▼
//!                              risk.check_launch_with_lists  ──►  entry::consider_launch
//!                                                                    │  (buy)
//!                                                                    ▼
//!                                              state.upsert_position + EventBus
//!                                                                    │
//!                              exit::sweep (mark price → risk.check_exit) ─► sell
//! ```
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
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

use bot_core::config::Config;
use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::models::{BotModule, ExecutionMode, TokenLaunch};
use bot_core::risk::RiskEngine;
use bot_core::state::Shared;

use solana_kit::execute::{ExecPolicy, Executor};
use solana_kit::layout::LayoutStore;
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;

pub mod detect;
pub mod entry;
pub mod exit;

pub use detect::LaunchDetector;

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
        let mut executor = Executor::new(rpc.clone(), wallet.clone(), policy);
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
            let mut sweeper_executor = Executor::new(
                self.rpc.clone(),
                self.wallet.clone(),
                exec_policy(&self.state.config_snapshot().await),
            );
            if let Some(reg) = &self.signers {
                sweeper_executor = sweeper_executor.with_signer_registry(Arc::clone(reg));
            }
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
            };
            tokio::spawn(async move { this.exit_sweeper().await })
        };

        // Launch detection produces a merged stream of TokenLaunch.
        let mut launches: mpsc::Receiver<TokenLaunch> =
            match LaunchDetector::spawn(self.state.clone(), self.rpc.clone()).await {
                Ok(rx) => rx,
                Err(e) => {
                    error!(error = %e, "launch detection failed to start; sniper will idle");
                    self.state
                        .record_error(BotModule::Sniper, &format!("detection: {e}"))
                        .await;
                    // Keep the sweeper alive but stop this task.
                    let _ = sweeper.await;
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

            if let Err(e) = self.consider_launch(launch).await {
                // A single failed snipe is not fatal; record and carry on.
                self.state
                    .record_error(BotModule::Sniper, &e.to_string())
                    .await;
                warn!(error = %e, "snipe attempt failed");
            } else {
                self.state.clear_error(BotModule::Sniper).await;
            }
        }

        // The detector closed (shutdown). Wind down.
        info!("launch stream ended; stopping sniper");
        self.state
            .set_running(BotModule::Sniper, false, false)
            .await;
        sweeper.abort();
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
