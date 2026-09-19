//! Forwards notable [`AppEvent`]s to the Telegram alert chat.
//!
//! Respects the operator's `alert_on_*` flags and applies two layers of
//! rate-limiting: a per-kind cooldown and a global per-minute cap, so a noisy
//! market cannot flood the chat (or trip Telegram's limits).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tracing::{debug, warn};

use bot_core::events::AppEvent;
use bot_core::models::PositionSide;
use bot_core::state::Shared;

use crate::api::TelegramApi;

/// Run the alert forwarder until the event bus closes.
pub async fn run_alert_forwarder(state: Shared, api: TelegramApi, chat_id: i64) {
    let mut rx = state.events.subscribe();
    let mut last_by_kind: HashMap<&'static str, Instant> = HashMap::new();
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
                        // Per-kind cooldown.
                        let cooldown = Duration::from_secs(tg.alert_cooldown_secs.max(0) as u64);
                        if let Some(prev) = last_by_kind.get(kind) {
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
                if !cfg.telegram.enabled || !cfg.telegram.alert_on_loss_limit {
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
                if !cfg.telegram.enabled || !cfg.telegram.hourly_summary {
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

/// Map an event to `(rate-limit-kind, message)` when it should alert.
fn classify(
    event: &Arc<AppEvent>,
    tg: &bot_core::config::TelegramConfig,
) -> Option<(&'static str, String)> {
    match event.as_ref() {
        AppEvent::Fill { trade, .. } if tg.alert_on_fill => {
            let side = match trade.side {
                PositionSide::Long => "BUY",
                PositionSide::Short => "SELL",
            };
            Some((
                "fill",
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
        } if tg.alert_on_fill => Some((
            "close",
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
        } if tg.alert_on_risk_reject => Some((
            "risk",
            format!("🚫 {} rejected {} — {}", module.as_str(), symbol, reason),
        )),
        AppEvent::ModuleStatus {
            module,
            connected,
            running,
            ..
        } if tg.alert_on_disconnect && *running && !*connected => Some((
            "disconnect",
            format!("📡 {} lost its feed connection", module.as_str()),
        )),
        AppEvent::Error {
            module,
            message,
            fatal,
            ..
        } if *fatal => Some((
            "error",
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
