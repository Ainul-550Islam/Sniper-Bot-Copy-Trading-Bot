use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The five modules described in the product spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default, PartialOrd, Ord)]
pub enum BotModule {
    /// Module 1 — new-launch sniper (Pump.fun bonding curve + Raydium).
    #[default]
    Sniper,
    /// Module 2 — mirror the trades of tracked wallets.
    Copy,
    /// Module 3 — Polymarket CLOB betting.
    Polymarket,
    /// Module 4 — on-chain staking / token / fee suite.
    Contract,
    /// Module 5 — Telegram control plane.
    Telegram,
}

impl BotModule {
    pub const ALL: [BotModule; 5] = [
        BotModule::Sniper,
        BotModule::Copy,
        BotModule::Polymarket,
        BotModule::Contract,
        BotModule::Telegram,
    ];

    /// The three modules that can place orders and therefore can be killed.
    pub const TRADING: [BotModule; 3] = [BotModule::Sniper, BotModule::Copy, BotModule::Polymarket];

    pub fn as_str(&self) -> &'static str {
        match self {
            BotModule::Sniper => "sniper",
            BotModule::Copy => "copy",
            BotModule::Polymarket => "polymarket",
            BotModule::Contract => "contract",
            BotModule::Telegram => "telegram",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            BotModule::Sniper => "🎯",
            BotModule::Copy => "🧬",
            BotModule::Polymarket => "🎲",
            BotModule::Contract => "📜",
            BotModule::Telegram => "📡",
        }
    }
}

impl fmt::Display for BotModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BotModule {
    type Err = crate::BotError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sniper" | "1" | "module1" | "pump" | "pumpfun" => Ok(BotModule::Sniper),
            "copy" | "2" | "module2" | "copytrade" => Ok(BotModule::Copy),
            "polymarket" | "poly" | "3" | "module3" => Ok(BotModule::Polymarket),
            "contract" | "4" | "module4" | "staking" => Ok(BotModule::Contract),
            "telegram" | "tg" | "5" | "module5" => Ok(BotModule::Telegram),
            other => Err(crate::BotError::invalid(format!(
                "unknown module '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Solana,
    Polygon,
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Chain::Solana => f.write_str("solana"),
            Chain::Polygon => f.write_str("polygon"),
        }
    }
}

/// Where an order actually goes. `Paper` is the default and the safe mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default, PartialOrd, Ord)]
pub enum ExecutionMode {
    /// Build and log the order, fill it against a local model. No network writes.
    #[default]
    Paper,
    /// Send the transaction/order for simulation only (`simulateTransaction` /
    /// dry-run) and record the result, but never broadcast it.
    Simulate,
    /// Really send it. Requires `allow_live_trading = true` in the config.
    Live,
}

impl ExecutionMode {
    pub fn is_live(&self) -> bool {
        matches!(self, ExecutionMode::Live)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionMode::Paper => "paper",
            ExecutionMode::Simulate => "simulate",
            ExecutionMode::Live => "LIVE",
        }
    }
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExecutionMode {
    type Err = crate::BotError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "paper" | "dry" | "dryrun" => Ok(ExecutionMode::Paper),
            "simulate" | "sim" => Ok(ExecutionMode::Simulate),
            "live" | "real" => Ok(ExecutionMode::Live),
            other => Err(crate::BotError::invalid(format!(
                "unknown execution mode '{other}'"
            ))),
        }
    }
}

/// Execution venue for a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Venue {
    /// Pump.fun bonding curve (pre-graduation).
    PumpFun,
    /// PumpSwap AMM (post-graduation pump.fun pools).
    PumpSwap,
    /// Raydium AMM v4 (OpenBook-backed constant product).
    RaydiumAmmV4,
    /// Raydium CLMM / CPMM.
    RaydiumClmm,
    /// Jupiter aggregator (routing fallback).
    Jupiter,
    /// Polymarket central limit order book.
    PolymarketClob,
    /// Local paper-trading fill model.
    Paper,
}

impl Venue {
    pub fn chain(&self) -> Chain {
        match self {
            Venue::PolymarketClob => Chain::Polygon,
            _ => Chain::Solana,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Venue::PumpFun => "pump.fun",
            Venue::PumpSwap => "pumpswap",
            Venue::RaydiumAmmV4 => "raydium-amm-v4",
            Venue::RaydiumClmm => "raydium-clmm",
            Venue::Jupiter => "jupiter",
            Venue::PolymarketClob => "polymarket",
            Venue::Paper => "paper",
        }
    }

    /// Inverse of [`Venue::as_str`] (persistence + API parsing).
    pub fn parse(s: &str) -> Option<Venue> {
        Some(match s {
            "pump.fun" => Venue::PumpFun,
            "pumpswap" => Venue::PumpSwap,
            "raydium-amm-v4" => Venue::RaydiumAmmV4,
            "raydium-clmm" => Venue::RaydiumClmm,
            "jupiter" => Venue::Jupiter,
            "polymarket" => Venue::PolymarketClob,
            "paper" => Venue::Paper,
            _ => return None,
        })
    }
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PositionSide {
    #[default]
    Long,
    Short,
}

impl PositionSide {
    pub fn as_str(&self) -> &'static str {
        match self {
            PositionSide::Long => "long",
            PositionSide::Short => "short",
        }
    }

    pub fn parse(s: &str) -> Option<PositionSide> {
        Some(match s {
            "long" | "buy" => PositionSide::Long,
            "short" | "sell" => PositionSide::Short,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionStatus {
    Open,
    Closing,
    Closed,
    /// Closed by the risk engine, not by a strategy signal.
    StoppedOut,
    Failed,
}

impl PositionStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            PositionStatus::Closed | PositionStatus::StoppedOut | PositionStatus::Failed
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PositionStatus::Open => "open",
            PositionStatus::Closing => "closing",
            PositionStatus::Closed => "closed",
            PositionStatus::StoppedOut => "stopped_out",
            PositionStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<PositionStatus> {
        Some(match s {
            "open" => PositionStatus::Open,
            "closing" => PositionStatus::Closing,
            "closed" => PositionStatus::Closed,
            "stopped_out" => PositionStatus::StoppedOut,
            "failed" => PositionStatus::Failed,
            _ => return None,
        })
    }
}

/// Which module opened the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeSource {
    Sniper,
    Copy,
    Polymarket,
    Manual,
    Risk,
}

impl fmt::Display for TradeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TradeSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeSource::Sniper => "sniper",
            TradeSource::Copy => "copy",
            TradeSource::Polymarket => "polymarket",
            TradeSource::Manual => "manual",
            TradeSource::Risk => "risk",
        }
    }

    pub fn parse(s: &str) -> Option<TradeSource> {
        Some(match s {
            "sniper" => TradeSource::Sniper,
            "copy" => TradeSource::Copy,
            "polymarket" => TradeSource::Polymarket,
            "manual" => TradeSource::Manual,
            "risk" => TradeSource::Risk,
            _ => return None,
        })
    }
}

/// A single executed (or paper-executed) fill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub source: TradeSource,
    pub venue: Venue,
    pub mode: ExecutionMode,
    pub side: PositionSide,
    /// Token mint (Solana) or CLOB token id (Polymarket).
    pub symbol: String,
    pub symbol_display: String,
    pub amount_in: f64,
    pub amount_out: f64,
    pub quote_symbol: String,
    pub price: f64,
    pub fee: f64,
    pub slippage_bps: u64,
    pub signature: Option<String>,
    pub position_id: Option<String>,
    pub note: Option<String>,
    pub latency_ms: Option<u64>,
}

impl Trade {
    pub fn is_buy(&self) -> bool {
        matches!(self.side, PositionSide::Long)
    }

    /// Signed PnL contribution in quote units (positive for sells of a long).
    pub fn signed_quote(&self) -> f64 {
        match self.side {
            PositionSide::Long => -self.amount_in,
            PositionSide::Short => self.amount_out,
        }
    }
}

/// An open or historical position, tracked per symbol per module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub id: String,
    pub opened_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub source: TradeSource,
    pub venue: Venue,
    pub mode: ExecutionMode,
    pub status: PositionStatus,
    /// Solana mint address or Polymarket token id.
    pub symbol: String,
    pub symbol_display: String,
    pub quote_symbol: String,
    /// Amount of the base asset held.
    pub qty: f64,
    /// Volume-weighted average entry price in quote units.
    pub avg_entry: f64,
    /// Total quote spent on entry (incl. fees).
    pub cost_basis: f64,
    /// Quote received on exits (incl. fees deducted).
    pub realized_quote: f64,
    /// Last observed mark price in quote units.
    pub last_mark: f64,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    /// Trailing-stop distance as a *fraction* (0.25 == 25% below the high-water
    /// mark). Distinct from `stop_loss`/`take_profit`, which are price levels.
    pub trailing_stop: Option<f64>,
    /// Highest mark observed since entry.
    pub trailing_high_water: Option<f64>,
    pub max_hold_secs: Option<i64>,
    pub entry_signature: Option<String>,
    pub exit_signature: Option<String>,
    pub entry_latency_ms: Option<u64>,
    /// For copy trading: the wallet we mirrored.
    pub copied_wallet: Option<String>,
    /// For Polymarket: the market / condition id.
    pub market_id: Option<String>,
    pub outcome: Option<String>,
    pub reason_closed: Option<String>,
}

impl Position {
    pub fn new(
        id: String,
        source: TradeSource,
        venue: Venue,
        mode: ExecutionMode,
        symbol: String,
        symbol_display: String,
        quote_symbol: String,
    ) -> Self {
        let now = Utc::now();
        Position {
            id,
            opened_at: now,
            updated_at: now,
            closed_at: None,
            source,
            venue,
            mode,
            status: PositionStatus::Open,
            symbol,
            symbol_display,
            quote_symbol,
            qty: 0.0,
            avg_entry: 0.0,
            cost_basis: 0.0,
            realized_quote: 0.0,
            last_mark: 0.0,
            stop_loss: None,
            take_profit: None,
            trailing_stop: None,
            trailing_high_water: None,
            max_hold_secs: None,
            entry_signature: None,
            exit_signature: None,
            entry_latency_ms: None,
            copied_wallet: None,
            market_id: None,
            outcome: None,
            reason_closed: None,
        }
    }

    /// Unrealised PnL in quote units at the current mark.
    pub fn unrealised(&self) -> f64 {
        if self.qty <= 0.0 {
            return 0.0;
        }
        (self.last_mark - self.avg_entry) * self.qty
    }

    /// Realised PnL in quote units.
    pub fn realised(&self) -> f64 {
        self.realized_quote - self.cost_basis
    }

    pub fn total_pnl(&self) -> f64 {
        self.realised() + self.unrealised()
    }

    pub fn total_pnl_pct(&self) -> f64 {
        if self.cost_basis <= 0.0 {
            return 0.0;
        }
        self.total_pnl() / self.cost_basis * 100.0
    }

    pub fn notional(&self) -> f64 {
        self.qty * self.last_mark
    }

    pub fn age_secs(&self) -> i64 {
        Utc::now()
            .signed_duration_since(self.opened_at)
            .num_seconds()
    }

    /// Apply a fill to the position, maintaining a volume-weighted entry.
    pub fn apply_buy(&mut self, qty: f64, price: f64, quote_spent: f64) {
        let new_qty = self.qty + qty;
        if new_qty > 0.0 {
            self.avg_entry = (self.avg_entry * self.qty + price * qty) / new_qty;
        }
        self.qty = new_qty;
        self.cost_basis += quote_spent;
        self.last_mark = price;
        self.updated_at = Utc::now();
        self.trailing_high_water = Some(self.trailing_high_water.map_or(price, |h| h.max(price)));
    }

    /// Apply a partial or full exit. Returns the realised quote for this slice.
    pub fn apply_sell(&mut self, qty: f64, price: f64, quote_received: f64) -> f64 {
        let qty = qty.min(self.qty);
        if qty <= 0.0 {
            return 0.0;
        }
        let cost_of_slice = self.avg_entry * qty;
        let realised = quote_received - cost_of_slice;
        self.qty -= qty;
        self.realized_quote += quote_received;
        // Keep cost_basis proportional so `realised()` stays correct.
        if self.qty <= f64::EPSILON {
            self.qty = 0.0;
        } else {
            self.cost_basis -= cost_of_slice;
        }
        self.last_mark = price;
        self.updated_at = Utc::now();
        realised
    }

    pub fn close(&mut self, status: PositionStatus, reason: impl Into<String>) {
        self.status = status;
        self.closed_at = Some(Utc::now());
        self.updated_at = Utc::now();
        self.reason_closed = Some(reason.into());
    }
}

/// Runtime state of one module, as exposed over REST/WS/Telegram.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleState {
    pub module: BotModule,
    /// Set by the operator (Telegram / REST). If false the task loop idles.
    pub enabled: bool,
    /// True while the task loop is actually running and connected.
    pub running: bool,
    /// True when the module is unhealthy (disconnected WS, repeated errors).
    pub degraded: bool,
    pub connected: bool,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub last_event: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub events_seen: u64,
    pub signals_generated: u64,
    pub orders_sent: u64,
    pub orders_filled: u64,
    pub orders_failed: u64,
    pub orders_rejected_by_risk: u64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub consecutive_errors: u32,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

impl ModuleState {
    pub fn new(module: BotModule) -> Self {
        ModuleState {
            module,
            ..Default::default()
        }
    }

    /// Snapshot for the dashboard / Telegram `/status`.
    pub fn to_status(&self) -> ModuleStatus {
        ModuleStatus {
            module: self.module,
            enabled: self.enabled,
            running: self.running,
            healthy: self.running && !self.degraded,
            connected: self.connected,
            events_seen: self.events_seen,
            signals_generated: self.signals_generated,
            orders_sent: self.orders_sent,
            orders_filled: self.orders_filled,
            orders_failed: self.orders_failed,
            orders_rejected_by_risk: self.orders_rejected_by_risk,
            realized_pnl: self.realized_pnl,
            unrealized_pnl: self.unrealized_pnl,
            consecutive_errors: self.consecutive_errors,
            last_error: self.last_error.clone(),
            last_heartbeat: self.last_heartbeat,
            last_event: self.last_event,
            detail: self.detail.clone(),
        }
    }
}

/// Wire-friendly status payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleStatus {
    pub module: BotModule,
    pub enabled: bool,
    pub running: bool,
    pub healthy: bool,
    pub connected: bool,
    pub events_seen: u64,
    pub signals_generated: u64,
    pub orders_sent: u64,
    pub orders_filled: u64,
    pub orders_failed: u64,
    pub orders_rejected_by_risk: u64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub consecutive_errors: u32,
    pub last_error: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub last_event: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

/// A newly launched token, as observed by Module 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLaunch {
    pub mint: String,
    pub name: String,
    pub symbol: String,
    pub uri: Option<String>,
    pub creator: String,
    pub pool: String,
    /// SOL the creator bought in the same transaction as creation.
    pub initial_buy_sol: f64,
    pub market_cap_sol: f64,
    pub market_cap_usd: Option<f64>,
    pub total_supply: Option<f64>,
    pub slot: Option<u64>,
    pub signature: Option<String>,
    pub tx_type: Option<String>,
    /// When our node first saw it.
    pub observed_at: DateTime<Utc>,
    /// Source of the observation (useful for latency accounting).
    pub feed: LaunchFeed,
    pub socials: Option<TokenSocials>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchFeed {
    /// PumpPortal websocket (`subscribeNewToken`).
    PumpPortal,
    /// Native Solana `logsSubscribe` on the pump program.
    SolanaLogs,
    /// `transactionSubscribe` (Geyser / Yellowstone).
    TransactionSubscribe,
    Manual,
}

impl fmt::Display for LaunchFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchFeed::PumpPortal => f.write_str("pumpportal"),
            LaunchFeed::SolanaLogs => f.write_str("logsSubscribe"),
            LaunchFeed::TransactionSubscribe => f.write_str("transactionSubscribe"),
            LaunchFeed::Manual => f.write_str("manual"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenSocials {
    pub website: Option<String>,
    pub twitter: Option<String>,
    pub telegram: Option<String>,
}

impl TokenSocials {
    pub fn count(&self) -> usize {
        [&self.website, &self.twitter, &self.telegram]
            .iter()
            .filter(|s| s.as_ref().is_some_and(|v| !v.trim().is_empty()))
            .count()
    }
}

/// A decoded on-chain trade performed by a *tracked* wallet (Module 2 input).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTrade {
    pub wallet: String,
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<DateTime<Utc>>,
    pub side: PositionSide,
    pub mint: String,
    pub symbol: Option<String>,
    pub token_amount: f64,
    pub sol_amount: f64,
    pub venue: Venue,
    pub fee_sol: f64,
    /// Raw discriminator that was matched, for debugging.
    pub discriminator: Option<String>,
    pub observed_at: DateTime<Utc>,
}

/// A Polymarket market as seen by Module 3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolyMarket {
    pub condition_id: String,
    pub question: String,
    pub slug: String,
    pub neg_risk: bool,
    pub active: bool,
    pub closed: bool,
    pub accepting_orders: bool,
    pub end_date: Option<DateTime<Utc>>,
    pub volume: f64,
    pub liquidity: f64,
    pub outcomes: Vec<PolyOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolyOutcome {
    pub outcome: String,
    pub token_id: String,
    pub price: f64,
    pub winner: Option<bool>,
}

impl PolyMarket {
    pub fn outcome(&self, name: &str) -> Option<&PolyOutcome> {
        self.outcomes
            .iter()
            .find(|o| o.outcome.eq_ignore_ascii_case(name))
    }
}
