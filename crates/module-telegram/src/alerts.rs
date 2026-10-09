//! Forwards notable [`AppEvent`]s to the Telegram alert chat.
//!
//! Respects the operator's `alert_on_*` flags and applies two layers of
//! rate-limiting: a per-kind cooldown and a global per-minute cap, so a noisy
//! market cannot flood the chat (or trip Telegram's limits).
//!
//! ## Notification preferences (GAP-MAP v2, P2)
//! Delivery decisions are made through the [`AlertGate`] trait instead of
//! reading config flags inline:
//! * operator mode wires [`ConfigAlertGate`] (the classic `alert_on_*`
//!   flags, behaviour unchanged);
//! * the SaaS server wires a gate over the tenant's
//!   `notification_preferences` row — `telegram_instant_fills` maps to
//!   fill + exit alerts, `telegram_circuit_breaker` maps to risk/breach
//!   alerts — so each tenant's preferences are honoured per message.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tracing::{debug, warn};

use bot_core::events::AppEvent;
use bot_core::models::PositionSide;
use bot_core::state::Shared;

use crate::api::TelegramApi;

/// Alert categories used by the preference gate. One category per
/// DELIVERY decision (not per event variant) so both the operator flags
/// and the SaaS `notification_preferences` columns map cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    /// An order filled.
    Fill,
    /// A position closed (the "exit" alert).
    Exit,
    /// The risk engine rejected an order.
    RiskRejected,
    /// A module lost its feed connection.
    Disconnect,
    /// A fatal error surfaced.
    FatalError,
    /// The daily loss limit tripped.
    LossLimit,
    /// The hourly summary.
    HourlySummary,
}

/// Per-recipient delivery gate. The server implements this over the SaaS
/// `notification_preferences` table; operator mode uses [`ConfigAlertGate`].
pub trait AlertGate: Send + Sync {
    /// True when this category may be delivered right now.
    fn allow(&self, kind: AlertKind) -> bool;
}

/// Operator-mode gate: the classic `alert_on_*` config flags. Exit alerts
/// ride the `alert_on_fill` flag (their historical behaviour); fatal
/// errors are ALWAYS delivered — a dead bot must not be silenable by
/// preference.
pub struct ConfigAlertGate<'a>(pub &'a bot_core::config::TelegramConfig);

impl AlertGate for ConfigAlertGate<'_> {
    fn allow(&self, kind: AlertKind) -> bool {
        match kind {
            AlertKind::Fill | AlertKind::Exit => self.0.alert_on_fill,
            AlertKind::RiskRejected => self.0.alert_on_risk_reject,
            AlertKind::Disconnect => self.0.alert_on_disconnect,
            AlertKind::FatalError => true,
            AlertKind::LossLimit => self.0.alert_on_loss_limit,
            AlertKind::HourlySummary => self.0.hourly_summary,
        }
    }
}

/// Run the alert forwarder until the event bus closes.
pub async fn run_alert_forwarder(state: Shared, api: TelegramApi, chat_id: i64) {
    let mut rx = state.events.subscribe();
    let mut last_by_kind: HashMap<AlertKind, Instant> = HashMap::new();
    let mut minute_start = Instant::now();
    let mut minute_count: u32 = 0;
    let mut loss_alerted = false;

    let mut check_tick = tokio::time::interval(Duration::from_secs(15));
    check_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut hour_tick = tokio::time::interval(Duration::from_secs(3600));
    hour_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick fires immediately; skip it.
    hour_tick.tick().await;

    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(event) => {
                        let cfg = state.config_snapshot().await;
                        let tg = &cfg.telegram;
                        if !tg.enabled {
                            continue;
                        }
                        // Global per-minute cap.
                        if minute_start.elapsed() >= Duration::from_secs(60) {
                            minute_start = Instant::now();
                            minute_count = 0;
                        }
                        if tg.max_alerts_per_minute > 0 && minute_count >= tg.max_alerts_per_minute {
                            continue;
                        }
                        let Some((kind, text)) = classify(&event, tg) else { continue };
                        // Notification-preference gate (operator mode: the
                        // alert_on_* flags; SaaS mode: the server's gate).
                        if !ConfigAlertGate(tg).allow(kind) {
                            continue;
                        }
                        // Per-kind cooldown.
                        let cooldown = Duration::from_secs(tg.alert_cooldown_secs.max(0) as u64);
                        if let Some(prev) = last_by_kind.get(&kind) {
                            if prev.elapsed() < cooldown {
                                continue;
                            }
                        }
                        last_by_kind.insert(kind, Instant::now());
                        minute_count += 1;
                        let prefix = if tg.prefix.trim().is_empty() { String::new() } else { format!("{} ", tg.prefix.trim()) };
                        if let Err(e) = api.send_message(chat_id, &format!("{prefix}{text}"), Some(&tg.parse_mode)).await {
                            debug!(error = %e, "telegram alert send failed");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(skipped = n, "telegram alert forwarder lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = check_tick.tick() => {
                let cfg = state.config_snapshot().await;
                if !cfg.telegram.enabled
                    || !ConfigAlertGate(&cfg.telegram).allow(AlertKind::LossLimit)
                {
                    continue;
                }
                let tripped = state.daily_stats().await.loss_limit_tripped;
                if tripped && !loss_alerted {
                    loss_alerted = true;
                    let _ = api.send_message(chat_id, "⚠️ Daily loss limit TRIPPED — trading halted until reset.", Some(&cfg.telegram.parse_mode)).await;
                } else if !tripped {
                    loss_alerted = false;
                }
            }
            _ = hour_tick.tick() => {
                let cfg = state.config_snapshot().await;
                if !cfg.telegram.enabled
                    || !ConfigAlertGate(&cfg.telegram).allow(AlertKind::HourlySummary)
                {
                    continue;
                }
                let s = state.summary().await;
                let text = format!(
                    "⏱️ Hourly summary\nmode {} | open {} | realized {:.4} | unrealized {:.4}\ntoday: W{} L{}, buys {} sells {}",
                    s.execution_mode.as_str(),
                    s.open_positions,
                    s.realized_pnl,
                    s.unrealized_pnl,
                    s.daily.wins,
                    s.daily.losses,
                    s.daily.buys,
                    s.daily.sells,
                );
                if let Err(e) = api.send_message(chat_id, &text, Some(&cfg.telegram.parse_mode)).await {
                    warn!(error = %e, "hourly summary send failed");
                }
            }
        }
    }
}

/// Map an event to `(rate-limit-key, message)` when it is ALERTABLE.
/// Delivery preferences are applied by the caller through [`AlertGate`] —
/// classification itself is preference-free.
fn classify(
    event: &Arc<AppEvent>,
    _tg: &bot_core::config::TelegramConfig,
) -> Option<(AlertKind, String)> {
    match event.as_ref() {
        AppEvent::Fill { trade, .. } => {
            let side = match trade.side {
                PositionSide::Long => "BUY",
                PositionSide::Short => "SELL",
            };
            Some((
                AlertKind::Fill,
                format!(
                    "💱 {} {} {}\n{:.4} → {:.4} @ {:.6} {}\n{}",
                    side,
                    trade.source,
                    trade.symbol_display,
                    trade.amount_in,
                    trade.amount_out,
                    trade.price,
                    trade.quote_symbol,
                    trade.mode.as_str(),
                ),
            ))
        }
        AppEvent::PositionClosed {
            position,
            pnl,
            pnl_pct,
            reason,
            ..
        } => Some((
            AlertKind::Exit,
            format!(
                "🏁 closed {} {}\nPnL {:.4} ({:.1}%)\n{}",
                position.source,
                position.symbol_display,
                pnl,
                pnl_pct * 100.0,
                reason
            ),
        )),
        AppEvent::RiskRejected {
            module,
            symbol,
            reason,
            ..
        } => Some((
            AlertKind::RiskRejected,
            format!("🚫 {} rejected {} — {}", module.as_str(), symbol, reason),
        )),
        AppEvent::ModuleStatus {
            module,
            connected,
            running,
            ..
        } if *running && !*connected => Some((
            AlertKind::Disconnect,
            format!("📡 {} lost its feed connection", module.as_str()),
        )),
        AppEvent::Error {
            module,
            message,
            fatal,
            ..
        } if *fatal => Some((
            AlertKind::FatalError,
            format!(
                "❌ fatal error{}: {}",
                module
                    .map(|m| format!(" in {}", m.as_str()))
                    .unwrap_or_default(),
                message
            ),
        )),
        _ => None,
    }
}


#[cfg(test)]
mod gate_tests {
    use super::*;
    use bot_core::config::TelegramConfig;

    #[test]
    fn config_gate_maps_flags_to_categories() {
        let mut tg = TelegramConfig::default();
        tg.alert_on_fill = true;
        tg.alert_on_risk_reject = false;
        tg.alert_on_disconnect = true;
        tg.alert_on_loss_limit = false;
        tg.hourly_summary = false;
        let gate = ConfigAlertGate(&tg);
        assert!(gate.allow(AlertKind::Fill));
        assert!(gate.allow(AlertKind::Exit), "exit rides the fill flag (historical)");
        assert!(!gate.allow(AlertKind::RiskRejected));
        assert!(gate.allow(AlertKind::Disconnect));
        assert!(!gate.allow(AlertKind::LossLimit));
        assert!(!gate.allow(AlertKind::HourlySummary));
        assert!(gate.allow(AlertKind::FatalError), "fatal errors are never silenable");
    }

    #[test]
    fn everything_off_still_delivers_fatal_errors() {
        let mut tg = TelegramConfig::default();
        tg.alert_on_fill = false;
        tg.alert_on_risk_reject = false;
        tg.alert_on_disconnect = false;
        tg.alert_on_loss_limit = false;
        tg.hourly_summary = false;
        let gate = ConfigAlertGate(&tg);
        assert!(gate.allow(AlertKind::FatalError));
        assert!(!gate.allow(AlertKind::Fill));
    }
}
