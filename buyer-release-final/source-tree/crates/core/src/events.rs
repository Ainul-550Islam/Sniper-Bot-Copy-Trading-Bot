use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use crate::models::{BotModule, Position, TokenLaunch, Trade, WalletTrade};

/// Everything that flows through the process.
///
/// One `EventBus` is created at startup and cloned into every module. Modules
/// publish; the Telegram alerter, the WebSocket status feed and the JSONL
/// journal all subscribe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppEvent {
    /// Process started / stopped.
    Lifecycle { ts: DateTime<Utc>, message: String },

    /// A module changed state (enabled/disabled/connected…).
    ModuleStatus {
        ts: DateTime<Utc>,
        module: BotModule,
        enabled: bool,
        running: bool,
        connected: bool,
        detail: Option<String>,
    },

    /// Module 1 saw a new token launch.
    Launch {
        ts: DateTime<Utc>,
        launch: Box<TokenLaunch>,
        /// true when the filters let it through to execution.
        accepted: bool,
        reason: Option<String>,
    },

    /// A strategy wants to trade.
    Signal {
        ts: DateTime<Utc>,
        module: BotModule,
        symbol: String,
        side: String,
        reason: String,
        strength: f64,
    },

    /// The risk engine refused a signal.
    RiskRejected {
        ts: DateTime<Utc>,
        module: BotModule,
        symbol: String,
        reason: String,
    },

    /// An order was submitted (or paper-filled).
    OrderSent {
        ts: DateTime<Utc>,
        module: BotModule,
        symbol: String,
        venue: String,
        mode: String,
        quote_amount: f64,
        signature: Option<String>,
        /// Public identity of the signing wallet (never key material). Lets
        /// the persisted transaction claim carry signer attribution for
        /// reconciliation without trusting anything but local config.
        #[serde(default)]
        signer: Option<String>,
        /// Broadcast attempts that led to this submission (executor retry
        /// count; `None` for venues without a broadcast loop).
        #[serde(default)]
        attempts: Option<u8>,
        latency_ms: Option<u64>,
    },

    /// A fill landed.
    Fill {
        ts: DateTime<Utc>,
        trade: Box<Trade>,
    },

    /// A position was opened or updated.
    PositionUpdate {
        ts: DateTime<Utc>,
        position: Box<Position>,
    },

    /// A position was closed.
    PositionClosed {
        ts: DateTime<Utc>,
        position: Box<Position>,
        pnl: f64,
        pnl_pct: f64,
        reason: String,
    },

    /// Module 2 decoded a whale trade.
    WalletTrade {
        ts: DateTime<Utc>,
        trade: Box<WalletTrade>,
    },

    /// Module 3 market snapshot / orderbook tick worth surfacing.
    Polymarket {
        ts: DateTime<Utc>,
        message: String,
        market: Option<String>,
        price: Option<f64>,
    },

    /// Telegram command received.
    Command {
        ts: DateTime<Utc>,
        chat_id: i64,
        user_id: Option<i64>,
        text: String,
        accepted: bool,
        response: Option<String>,
    },

    Error {
        ts: DateTime<Utc>,
        module: Option<BotModule>,
        message: String,
        fatal: bool,
    },

    Info {
        ts: DateTime<Utc>,
        module: Option<BotModule>,
        message: String,
    },

    /// A control-plane / system action for the audit trail (kill switch,
    /// mode change, key rotation, config reload, recovery verdicts…).
    Audit {
        ts: DateTime<Utc>,
        actor: String,
        action: String,
        target: Option<String>,
        outcome: String,
    },
}

impl AppEvent {
    pub fn ts(&self) -> DateTime<Utc> {
        match self {
            AppEvent::Lifecycle { ts, .. }
            | AppEvent::ModuleStatus { ts, .. }
            | AppEvent::Launch { ts, .. }
            | AppEvent::Signal { ts, .. }
            | AppEvent::RiskRejected { ts, .. }
            | AppEvent::OrderSent { ts, .. }
            | AppEvent::Fill { ts, .. }
            | AppEvent::PositionUpdate { ts, .. }
            | AppEvent::PositionClosed { ts, .. }
            | AppEvent::WalletTrade { ts, .. }
            | AppEvent::Polymarket { ts, .. }
            | AppEvent::Command { ts, .. }
            | AppEvent::Error { ts, .. }
            | AppEvent::Info { ts, .. }
            | AppEvent::Audit { ts, .. } => *ts,
        }
    }

    pub fn module(&self) -> Option<BotModule> {
        match self {
            AppEvent::ModuleStatus { module, .. }
            | AppEvent::Signal { module, .. }
            | AppEvent::RiskRejected { module, .. }
            | AppEvent::OrderSent { module, .. } => Some(*module),
            AppEvent::Launch { .. }
            | AppEvent::Fill { .. }
            | AppEvent::PositionUpdate { .. }
            | AppEvent::PositionClosed { .. } => Some(BotModule::Sniper),
            AppEvent::WalletTrade { .. } => Some(BotModule::Copy),
            AppEvent::Polymarket { .. } => Some(BotModule::Polymarket),
            AppEvent::Command { .. } => Some(BotModule::Telegram),
            AppEvent::Error { module, .. } | AppEvent::Info { module, .. } => *module,
            AppEvent::Lifecycle { .. } | AppEvent::Audit { .. } => None,
        }
    }

    /// Short human-readable line, used by the dashboard feed and logs.
    pub fn summary(&self) -> String {
        match self {
            AppEvent::Lifecycle { message, .. } => format!("lifecycle: {message}"),
            AppEvent::ModuleStatus {
                module,
                running,
                connected,
                ..
            } => {
                format!("module {module}: running={running} connected={connected}")
            }
            AppEvent::Launch {
                launch,
                accepted,
                reason,
                ..
            } => {
                let r = reason.clone().unwrap_or_default();
                format!(
                    "launch {} ({}) accepted={accepted} {r}",
                    launch.symbol, launch.mint
                )
            }
            AppEvent::Signal {
                module,
                symbol,
                side,
                reason,
                ..
            } => {
                format!("signal {module} {side} {symbol}: {reason}")
            }
            AppEvent::RiskRejected {
                module,
                symbol,
                reason,
                ..
            } => {
                format!("risk rejected {module} {symbol}: {reason}")
            }
            AppEvent::OrderSent {
                module,
                symbol,
                venue,
                mode,
                quote_amount,
                signature,
                latency_ms,
                ..
            } => {
                let sig = signature.clone().unwrap_or_else(|| "n/a".into());
                let lat = latency_ms.map(|l| format!(" in {l}ms")).unwrap_or_default();
                format!(
                    "order {module} {symbol} {quote_amount} via {venue} [{mode}]{lat} sig={sig}"
                )
            }
            AppEvent::Fill { trade, .. } => format!(
                "fill {} {} {} in={} out={} price={}",
                trade.source,
                trade.side_str(),
                trade.symbol_display,
                trade.amount_in,
                trade.amount_out,
                trade.price
            ),
            AppEvent::PositionUpdate { position, .. } => format!(
                "position {} qty={} entry={} mark={} upnl={:.4}",
                position.symbol_display,
                position.qty,
                position.avg_entry,
                position.last_mark,
                position.unrealised()
            ),
            AppEvent::PositionClosed {
                position,
                pnl,
                pnl_pct,
                reason,
                ..
            } => format!(
                "closed {} pnl={pnl:.4} ({pnl_pct:.1}%) reason={reason}",
                position.symbol_display
            ),
            AppEvent::WalletTrade { trade, .. } => format!(
                "whale {} {} {} sol={} mint={}",
                trade.wallet,
                trade.side_str(),
                trade.symbol.clone().unwrap_or_default(),
                trade.sol_amount,
                trade.mint
            ),
            AppEvent::Polymarket {
                message,
                market,
                price,
                ..
            } => {
                let m = market.clone().unwrap_or_default();
                let p = price.map(|p| format!(" price={p}")).unwrap_or_default();
                format!("polymarket {m}{p}: {message}")
            }
            AppEvent::Command {
                chat_id,
                text,
                accepted,
                ..
            } => {
                format!("telegram chat={chat_id} accepted={accepted} cmd={text}")
            }
            AppEvent::Error {
                module,
                message,
                fatal,
                ..
            } => {
                let m = module
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "core".into());
                format!("ERROR [{m}] fatal={fatal}: {message}")
            }
            AppEvent::Info {
                module, message, ..
            } => {
                let m = module
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "core".into());
                format!("info [{m}]: {message}")
            }
            AppEvent::Audit {
                actor,
                action,
                target,
                outcome,
                ..
            } => {
                let t = target.as_deref().unwrap_or("-");
                format!("audit [{outcome}] {actor} {action} {t}")
            }
        }
    }

    pub fn is_alert(&self) -> bool {
        matches!(
            self,
            AppEvent::Fill { .. }
                | AppEvent::PositionClosed { .. }
                | AppEvent::RiskRejected { .. }
                | AppEvent::Error { .. }
        )
    }
}

/// Convenience accessors used by `summary()`.
pub trait SideStr {
    fn side_str(&self) -> String;
}

impl SideStr for Trade {
    fn side_str(&self) -> String {
        match self.side {
            crate::models::PositionSide::Long => "BUY".into(),
            crate::models::PositionSide::Short => "SELL".into(),
        }
    }
}

impl SideStr for WalletTrade {
    fn side_str(&self) -> String {
        match self.side {
            crate::models::PositionSide::Long => "BUY".into(),
            crate::models::PositionSide::Short => "SELL".into(),
        }
    }
}

/// A clonable fan-out channel plus a bounded ring buffer of recent events.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<AppEvent>>,
    recent: Arc<Mutex<Vec<Arc<AppEvent>>>>,
    capacity: usize,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        // The channel capacity is deliberately larger than the ring buffer:
        // slow subscribers should lag, not wedge the publishers.
        let (tx, _rx) = broadcast::channel(capacity.max(64) * 4);
        EventBus {
            tx,
            recent: Arc::new(Mutex::new(Vec::with_capacity(capacity.max(64)))),
            capacity: capacity.max(64),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<AppEvent>> {
        self.tx.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Publish an event. Returns the number of receivers it reached.
    /// A publish never fails: if nobody is listening the event is simply
    /// recorded in the ring buffer.
    pub fn publish(&self, event: AppEvent) -> usize {
        let arc = Arc::new(event);
        let n = self.tx.send(arc.clone()).unwrap_or(0);
        let recent = self.recent.clone();
        let capacity = self.capacity;
        // `block_on` would deadlock inside the runtime, so push from a task.
        tokio::spawn(async move {
            let mut buf = recent.lock().await;
            buf.push(arc);
            if buf.len() > capacity {
                let overflow = buf.len() - capacity;
                buf.drain(0..overflow);
            }
        });
        n
    }

    /// Synchronous publish for use inside async contexts that must not spawn.
    pub fn publish_now(&self, event: AppEvent) -> usize {
        let arc = Arc::new(event);
        self.tx.send(arc).unwrap_or(0)
    }

    /// Most recent events, newest last. `limit` caps the returned slice.
    pub async fn recent(&self, limit: usize) -> Vec<AppEvent> {
        let buf = self.recent.lock().await;
        let start = buf.len().saturating_sub(limit.max(1));
        buf[start..].iter().map(|e| (**e).clone()).collect()
    }

    pub async fn clear_recent(&self) {
        self.recent.lock().await.clear();
    }
}

impl Default for EventBus {
    fn default() -> Self {
        EventBus::new(2_000)
    }
}
