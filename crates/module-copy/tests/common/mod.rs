//! Shared harness for the copy-engine integration tests (TASK 3).
//!
//! Reuses the sniper's offline harness — the scripted JSON-RPC mock node,
//! the byte-accurate pump.fun account encoders and the config helpers —
//! by including it by path, so the REAL `CopyBot` (pipeline, leader
//! registry, dedup, risk engine, hardened executor, ledger) runs end to end
//! without a network. Everything here is test-only code; nothing in `src/`
//! depends on it.

#![allow(dead_code)]
#![allow(unused_imports)]

#[path = "../../../module-sniper/tests/common/mod.rs"]
pub mod harness;

pub use harness::{
    install_pump_token, mock_rpc, offline_rpc, signature, spawn_mock_node, state_with, CurveSpec,
    MockNode, SendBehaviour,
};

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use solana_sdk::pubkey::Pubkey;

use bot_core::config::{Config, CopyWallet};
use bot_core::events::AppEvent;
use bot_core::models::{ExecutionMode, PositionSide, Venue, WalletTrade};
use bot_core::state::Shared;
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;

use module_copy::event::{CopyOutcome, EventSource, LeaderTradeEvent};
use module_copy::CopyBot;

/// Deterministic leader address used across the tests.
pub const LEADER: &str = "Leader111111111111111111111111111111111111";
/// A second leader.
pub const LEADER_B: &str = "Leader222222222222222222222222222222222222";

/// Wallet rule for `address`: fixed 0.05 SOL per mirror, no minimum, wide
/// staleness, sells mirrored.
pub fn rule(address: &str) -> CopyWallet {
    CopyWallet {
        address: address.to_string(),
        label: Some(format!("test-{}", &address[..6])),
        fixed_sol: Some(0.05),
        fraction_of_their_size: 0.05,
        max_sol: 1.0,
        min_sol: 0.0,
        buys_only: false,
        slippage_pct: Some(15.0),
        max_staleness_secs: 300,
        paused: false,
        max_exposure_sol: 0.0,
        max_open_positions: 0,
    }
}

/// Base config for copy pipeline tests: copy on, paper mode, PumpPortal
/// feed label, one leader, generous copy caps, sniper feeds off, fixed fees.
pub fn copy_config() -> Config {
    let mut cfg = harness::base_config();
    cfg.sniper.enabled = false;
    cfg.copy.enabled = true;
    cfg.copy.feed = "pumpportal".into();
    cfg.copy.wallets = vec![rule(LEADER)];
    cfg.copy.max_event_age_secs = 300;
    cfg.copy.reconcile_interval_secs = 0;
    cfg.risk.copy_cooldown_secs = 0;
    cfg.risk.copy_max_pending_executions = 0;
    cfg.risk.copy_failed_entry_cooldown_secs = 0;
    cfg.risk.max_open_positions = 16;
    cfg.risk.max_position_quote = 1.0;
    cfg.execution.mode = ExecutionMode::Paper;
    cfg.execution.allow_live_trading = false;
    cfg
}

/// [`copy_config`] in live mode (the mock node receives the broadcast).
pub fn live_copy_config() -> Config {
    let mut cfg = copy_config();
    cfg.execution.mode = ExecutionMode::Live;
    cfg.execution.allow_live_trading = true;
    cfg
}

/// A leader buy of `sol` SOL in `mint`, observed now with chain time 1 s ago.
pub fn leader_buy(leader: &str, mint: Pubkey, sig_tag: u8, sol: f64) -> WalletTrade {
    let now = Utc::now();
    WalletTrade {
        wallet: leader.to_string(),
        signature: signature(sig_tag),
        slot: 500 + sig_tag as u64,
        block_time: Some(now - Duration::seconds(1)),
        side: PositionSide::Long,
        mint: mint.to_string(),
        symbol: Some("HRN".into()),
        token_amount: 1_000_000.0,
        sol_amount: sol,
        venue: Venue::PumpFun,
        fee_sol: 0.000_005,
        discriminator: None,
        observed_at: now,
    }
}

/// A leader sell of `tokens` in `mint`.
pub fn leader_sell(leader: &str, mint: Pubkey, sig_tag: u8, tokens: f64) -> WalletTrade {
    let mut t = leader_buy(leader, mint, sig_tag, 0.5);
    t.side = PositionSide::Short;
    t.token_amount = tokens;
    t
}

/// Lift a trade into an event.
pub fn event(trade: &WalletTrade, source: EventSource, seq: u64) -> LeaderTradeEvent {
    LeaderTradeEvent::from_wallet_trade(trade, source, seq)
}

/// Drain `rx` and return every `copy.*` audit record as `(action, outcome)`.
pub fn copy_audits(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<AppEvent>>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::Audit {
            actor,
            action,
            outcome,
            ..
        } = ev.as_ref()
        {
            if actor == "copy" {
                out.push((action.clone(), outcome.clone()));
            }
        }
    }
    out
}

/// Drain `rx` and count events by variant name.
pub fn drain_kinds(rx: &mut tokio::sync::broadcast::Receiver<Arc<AppEvent>>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        let name = match ev.as_ref() {
            AppEvent::WalletTrade { .. } => "wallet_trade",
            AppEvent::Signal { .. } => "signal",
            AppEvent::RiskRejected { .. } => "risk_rejected",
            AppEvent::OrderSent { .. } => "order_sent",
            AppEvent::Fill { .. } => "fill",
            AppEvent::PositionUpdate { .. } => "position_update",
            AppEvent::Audit { .. } => "audit",
            _ => "other",
        };
        out.push(name.to_string());
    }
    out
}

/// A copy bot wired to `rpc` with a fresh wallet.
pub async fn copy_bot(state: Shared, rpc: Rpc) -> (CopyBot, Arc<Wallet>) {
    let wallet = Arc::new(Wallet::generate());
    let bot = CopyBot::new(state, rpc, wallet.clone(), None).await;
    (bot, wallet)
}

/// A fully wired offline world: mock node with a tradable pump token, a copy
/// bot pointed at it, and event builders for that token.
pub struct CopyWorld {
    pub node: Arc<MockNode>,
    pub url: String,
    pub state: Shared,
    pub bot: CopyBot,
    pub wallet: Arc<Wallet>,
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub cfg: Config,
}

impl CopyWorld {
    pub async fn new(cfg: Config) -> Self {
        let node = Arc::new(MockNode::default());
        let url = spawn_mock_node(node.clone()).await;
        let mint = Pubkey::new_unique();
        let creator = Pubkey::new_unique();
        install_pump_token(&node, mint, creator, CurveSpec::fresh());
        let state = state_with(cfg.clone());
        state.set_balances(Some(10.0), Some(1_000.0)).await;
        let (bot, wallet) = copy_bot(state.clone(), mock_rpc(&url)).await;
        CopyWorld {
            node,
            url,
            state,
            bot,
            wallet,
            mint,
            creator,
            cfg,
        }
    }

    /// A second bot sharing this world's state and node (another replica /
    /// concurrent worker).
    pub async fn sibling(&self) -> CopyBot {
        let (bot, _) = copy_bot(self.state.clone(), mock_rpc(&self.url)).await;
        bot
    }

    /// Replace the bot with one that journals into `store` (same state/node).
    pub async fn attach_store(&mut self, store: Arc<dyn module_copy::recovery::CopyStore>) {
        let (bot, wallet) = copy_bot(self.state.clone(), mock_rpc(&self.url)).await;
        self.bot = bot.with_copy_store(store);
        self.wallet = wallet;
    }

    /// Install another tradable pump token on the node.
    pub fn new_token(&self) -> Pubkey {
        let mint = Pubkey::new_unique();
        install_pump_token(&self.node, mint, self.creator, CurveSpec::fresh());
        mint
    }

    /// Leader buy event of 0.5 SOL in this world's mint.
    pub fn buy(&self, sig_tag: u8) -> LeaderTradeEvent {
        event(
            &leader_buy(LEADER, self.mint, sig_tag, 0.5),
            EventSource::PumpPortal,
            sig_tag as u64,
        )
    }

    /// Leader buy event in `mint`.
    pub fn buy_in(&self, mint: Pubkey, sig_tag: u8) -> LeaderTradeEvent {
        event(
            &leader_buy(LEADER, mint, sig_tag, 0.5),
            EventSource::PumpPortal,
            sig_tag as u64,
        )
    }

    /// Leader sell event in this world's mint.
    pub fn sell(&self, sig_tag: u8) -> LeaderTradeEvent {
        event(
            &leader_sell(LEADER, self.mint, sig_tag, 500_000.0),
            EventSource::PumpPortal,
            sig_tag as u64,
        )
    }

    /// Run one event through the pipeline with the current config snapshot.
    pub async fn process(&mut self, event: &LeaderTradeEvent) -> CopyOutcome {
        let cfg = self.state.config_snapshot().await;
        self.bot.process_event(event, &cfg).await
    }

    /// Replace the config (hot reload) and resync leaders.
    pub async fn reload(&mut self, cfg: Config) {
        let next = cfg.clone();
        self.state.update_config(move |c| *c = next).await;
        self.cfg = cfg.clone();
        self.bot.sync_leaders(&cfg).await;
    }
}

/// `now - secs`.
pub fn ago(secs: i64) -> DateTime<Utc> {
    Utc::now() - Duration::seconds(secs)
}
