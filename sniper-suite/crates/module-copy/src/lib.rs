//! Module 2 — copy-trading bot.
//!
//! Watches a set of tracked wallets ("whales") and mirrors their pump.fun /
//! PumpSwap / Raydium trades. A whale's confirmed swap is decoded into a
//! [`WalletTrade`], sized according to that wallet's rules, gated by the shared
//! risk engine, and executed on the same venue. If the whale exits and
//! `mirror_exits` is on, we close our mirrored position too.
//!
//! Like every module it is **paper-trading by default**: trades are constructed
//! and simulated, never broadcast, until `execution.mode = "live"` *and*
//! `execution.allow_live_trading = true`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod exit;
pub mod feeds;
pub mod mirror;

use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use bot_core::config::Config;
use bot_core::config::CopyWallet;
use bot_core::error::BotResult;
use bot_core::models::{BotModule, ExecutionMode, PositionSide, WalletTrade};
use bot_core::risk::RiskEngine;
use bot_core::state::Shared;

use solana_kit::execute::{ExecPolicy, Executor};
use solana_kit::layout::LayoutStore;
use solana_kit::rpc::Rpc;
use solana_kit::signer::SignerRegistry;
use solana_kit::tokens::Wallet;

use feeds::CopyFeed;

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
}

/// Fresh write-ahead intent record for one copy-trade broadcast (§I).
pub(crate) fn copy_intent(
    state: &Shared,
    wallet: &Arc<Wallet>,
    symbol: &str,
    side: &str,
    qty: &str,
) -> bot_core::db::repo::IntentRecord {
    bot_core::db::repo::IntentRecord {
        intent_id: state.next_id("intent"),
        module: "copy".into(),
        symbol: symbol.to_string(),
        wallet: wallet.pubkey.to_string(),
        side: side.into(),
        qty: qty.into(),
        status: "pending".into(),
        signature: None,
        created_at: chrono::Utc::now(),
    }
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
        );
        if let Some(reg) = &signers {
            executor = executor.with_signer_registry(Arc::clone(reg));
        }
        let layouts = Arc::new(RwLock::new(
            LayoutStore::load(&cfg.sniper.pump_layout_file).await,
        ));
        let risk = RiskEngine::new(state.clone());
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

    /// Run the copy bot until the task is aborted.
    pub async fn run(&mut self, mut feeds: mpsc::Receiver<WalletTrade>) -> BotResult<()> {
        self.state.set_running(BotModule::Copy, true, true).await;
        self.state.set_detail(BotModule::Copy, "starting").await;
        self.state.heartbeat(BotModule::Copy).await;

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

        let cfg = self.state.config_snapshot().await;
        info!(wallets = cfg.copy.wallets.len(), "copy-trading bot running");

        loop {
            // Shutdown-aware receive (mirrors module 1).
            let trade = tokio::select! {
                t = feeds.recv() => t,
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
            // De-duplicate: the same signature can arrive from two feeds.
            if !trade.signature.is_empty()
                && !self.state.mark_signature_seen(&trade.signature).await
            {
                continue;
            }
            if let Err(e) = self.mirror_trade(&trade, &cfg).await {
                warn!(
                    wallet = %trade.wallet,
                    mint = %trade.mint,
                    error = %e,
                    "copy mirror failed"
                );
                self.state
                    .record_error(BotModule::Copy, &format!("{}: {e}", trade.mint))
                    .await;
            }
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
