//! Module 5 — Telegram control plane.
//!
//! Long-polls `getUpdates`, authorizes each command against the configured
//! allowlist, dispatches it against shared state (enable/disable modules,
//! kill switch, mode switch, status queries), and replies. A separate task
//! forwards notable events (fills, risk rejections, disconnects, loss-limit)
//! to the alert chat.
//!
//! The bot token is read from the environment variable named by
//! `telegram.bot_token_env` (default `TELEGRAM_BOT_TOKEN`). With no token the
//! module idles harmlessly.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod alerts;
pub mod api;
pub mod callbacks;
pub mod commands;
pub mod trade_session;

use std::sync::Arc;

use tracing::{info, warn};

use bot_core::error::BotResult;
use bot_core::state::Shared;

use api::{BotCommand, TelegramApi};
use commands::{handle, parse_command, telegram_role};

/// The Telegram control bot.
pub struct TelegramBot {
    state: Shared,
    api: TelegramApi,
    alert_chat_id: Option<i64>,
    /// Optional trade desk (GAP-MAP v2 P2): when attached, /buy and /sell
    /// become available behind inline-keyboard confirmations; when absent
    /// those commands reply with a "trading not configured" notice and the
    /// bot stays read-only exactly like before.
    trade_desk: Option<Arc<commands::TradeDesk>>,
}

impl TelegramBot {
    /// Build the bot. Reads the token from the configured environment variable.
    /// Returns `Ok(None)` when no token is configured (module disabled).
    pub async fn new(state: Shared) -> BotResult<Option<Self>> {
        let cfg = state.config_snapshot().await;
        let tg = cfg.telegram.clone();
        if !tg.enabled {
            info!("telegram module disabled in config");
            return Ok(None);
        }
        let token = std::env::var(&tg.bot_token_env).unwrap_or_default();
        let token = token.trim();
        if token.is_empty() {
            warn!(
                env = %tg.bot_token_env,
                "no telegram bot token in environment; control bot disabled"
            );
            return Ok(None);
        }
        let api = TelegramApi::new(token)?;
        let alert_chat_id = tg
            .alert_chat_id
            .or_else(|| tg.allowed_chat_ids.first().copied());
        Ok(Some(TelegramBot {
            state,
            api,
            alert_chat_id,
            trade_desk: None,
        }))
    }

    /// Attach the trade desk (server wiring supplies the executor and the
    /// tenant chat bindings).
    #[must_use]
    pub fn with_trade_desk(mut self, desk: Arc<commands::TradeDesk>) -> Self {
        self.trade_desk = Some(desk);
        self
    }

    /// Run the control bot: register commands, start alerting, then poll.
    pub async fn run(&self) -> BotResult<()> {
        self.state
            .set_running(bot_core::models::BotModule::Telegram, true, true)
            .await;

        // Ensure long polling receives updates (no webhook set).
        if let Err(e) = self.api.delete_webhook().await {
            warn!(error = %e, "deleteWebhook failed (continuing)");
        }
        if let Err(e) = self.api.set_my_commands(&menu()).await {
            warn!(error = %e, "setMyCommands failed (continuing)");
        }

        // Alert forwarder.
        if let Some(chat_id) = self.alert_chat_id {
            let state = self.state.clone();
            let api = self.api.clone();
            tokio::spawn(async move {
                alerts::run_alert_forwarder(state, api, chat_id).await;
            });
            info!(chat_id, "telegram alerts forwarding");
        } else {
            warn!("no alert chat id configured; alerts disabled");
        }

        let cfg = self.state.config_snapshot().await;
        let poll_timeout = cfg.telegram.poll_interval_secs.clamp(5, 60);
        info!("telegram control bot polling for commands");

        let mut offset: i64 = 0;
        loop {
            // Shutdown-aware long poll: SIGTERM does not wait out the poll.
            let result = tokio::select! {
                r = self.api.get_updates(offset, poll_timeout) => r,
                _ = self.state.wait_shutdown() => {
                    info!("module 5 (telegram) stopping (shutdown)");
                    break Ok(());
                }
            };
            let updates = match result {
                Ok(u) => u,
                Err(e) => {
                    warn!(error = %e, "getUpdates failed; backing off");
                    self.state
                        .record_error(bot_core::models::BotModule::Telegram, &e.to_string())
                        .await;
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    continue;
                }
            };

            for update in updates {
                offset = update.update_id + 1;
                // Inline-keyboard taps (trade confirmations) first — they
                // carry no message text and must never fall through.
                if let Some(query) = update.callback_query.clone() {
                    self.handle_callback_query(&query).await;
                    continue;
                }
                let Some(message) = update.message else {
                    continue;
                };
                let Some(text) = message.text.clone() else {
                    continue;
                };
                let chat_id = message.chat.id;
                let user_id = message.from.as_ref().map(|u| u.id);

                let cfg = self.state.config_snapshot().await;
                let tg = cfg.telegram.clone();
                if !tg.enabled {
                    continue;
                }

                // Echo the command into the event log.
                let Some(role) = telegram_role(chat_id, user_id, &tg) else {
                    warn!(chat_id, user_id = ?user_id, "unauthorized telegram command attempt");
                    let _ = self
                        .api
                        .send_message(
                            chat_id,
                            "⛔ You are not authorized to control this bot.",
                            Some(&tg.parse_mode),
                        )
                        .await;
                    self.state
                        .events
                        .publish(bot_core::events::AppEvent::Command {
                            ts: chrono::Utc::now(),
                            chat_id,
                            user_id,
                            text: text.clone(),
                            accepted: false,
                            response: None,
                        });
                    continue;
                };
                self.state
                    .events
                    .publish(bot_core::events::AppEvent::Command {
                        ts: chrono::Utc::now(),
                        chat_id,
                        user_id,
                        text: text.clone(),
                        accepted: true,
                        response: None,
                    });

                let command = parse_command(&text, &tg.prefix);
                // Trade commands go through the desk when one is attached
                // (they need the session + confirmation keyboard).
                if matches!(
                    command,
                    commands::Command::Buy { .. }
                        | commands::Command::Sell { .. }
                        | commands::Command::Limit
                ) {
                    match &self.trade_desk {
                        Some(desk) => {
                            let now = chrono::Utc::now();
                            desk.prune(now).await;
                            let reply = commands::handle_trade_command(
                                desk,
                                &self.state,
                                command,
                                role,
                                chat_id,
                                user_id.unwrap_or(0),
                                now,
                            )
                            .await;
                            if let Some(tr) = reply {
                                self.send_trade_reply(chat_id, &tr, &tg.parse_mode).await;
                            }
                            continue;
                        }
                        None => {
                            let _ = self
                                .api
                                .send_message(
                                    chat_id,
                                    "⚠️ Telegram trading is not configured on this deployment.",
                                    Some(&tg.parse_mode),
                                )
                                .await;
                            continue;
                        }
                    }
                }
                let reply = handle(&self.state, command, role).await;
                if reply.is_empty() {
                    continue;
                }
                if let Err(e) = self
                    .api
                    .send_message(chat_id, &reply, Some(&tg.parse_mode))
                    .await
                {
                    warn!(error = %e, "failed to send command reply");
                }
            }
        }
    }
}

impl TelegramBot {
    /// Resolve one inline-keyboard tap through the trade desk.
    async fn handle_callback_query(&self, query: &api::CallbackQuery) {
        let Some(data) = query.data.clone() else { return };
        let user_id = query.from.as_ref().map(|u| u.id).unwrap_or(0);
        let Some(desk) = &self.trade_desk else {
            // Keyboard from a desk that no longer exists (restart): ack it
            // so the client spinner stops.
            let _ = self
                .api
                .answer_callback_query(&query.id, "This confirmation is no longer valid.")
                .await;
            return;
        };
        let (toast, follow_up) = desk.resolve_callback(&data, user_id, chrono::Utc::now()).await;
        if let Err(e) = self.api.answer_callback_query(&query.id, &toast).await {
            warn!(error = %e, "answerCallbackQuery failed");
        }
        if let Some(reply) = follow_up {
            let chat_id = query.message.as_ref().map(|m| m.chat.id);
            if let Some(chat_id) = chat_id {
                let cfg = self.state.config_snapshot().await;
                self.send_trade_reply(chat_id, &reply, &cfg.telegram.parse_mode).await;
            }
        }
    }

    /// Send a trade reply, attaching the confirmation keyboard when present.
    async fn send_trade_reply(&self, chat_id: i64, reply: &commands::TradeReply, parse_mode: &str) {
        let result = match &reply.keyboard {
            Some(kb) => {
                self.api
                    .send_message_with_keyboard(chat_id, &reply.text, Some(parse_mode), kb)
                    .await
            }
            None => self.api.send_message(chat_id, &reply.text, Some(parse_mode)).await,
        };
        if let Err(e) = result {
            warn!(error = %e, "failed to send trade reply");
        }
    }
}

/// The `/`-menu shown in Telegram clients.
pub fn menu() -> Vec<BotCommand> {
    vec![
        BotCommand {
            command: "status".into(),
            description: "All modules, PnL, kill switch".into(),
        },
        BotCommand {
            command: "on".into(),
            description: "Enable a module (or all)".into(),
        },
        BotCommand {
            command: "off".into(),
            description: "Disable a module (or all)".into(),
        },
        BotCommand {
            command: "kill".into(),
            description: "Engage the kill switch".into(),
        },
        BotCommand {
            command: "resume".into(),
            description: "Clear the kill switch".into(),
        },
        BotCommand {
            command: "positions".into(),
            description: "Open positions".into(),
        },
        BotCommand {
            command: "trades".into(),
            description: "Recent fills".into(),
        },
        BotCommand {
            command: "pnl".into(),
            description: "Realized/unrealized PnL".into(),
        },
        BotCommand {
            command: "balance".into(),
            description: "Wallet balances".into(),
        },
        BotCommand {
            command: "mode".into(),
            description: "Show/set paper|simulate|live".into(),
        },
        BotCommand {
            command: "config".into(),
            description: "Key configuration".into(),
        },
        BotCommand {
            command: "buy".into(),
            description: "Buy: /buy <mint> <sol> (keyboard confirm)".into(),
        },
        BotCommand {
            command: "sell".into(),
            description: "Sell: /sell <mint> <percent> (keyboard confirm)".into(),
        },
        BotCommand {
            command: "snipe".into(),
            description: "Toggle the sniper module on|off".into(),
        },
        BotCommand {
            command: "wallets".into(),
            description: "Configured wallets + balances".into(),
        },
        BotCommand {
            command: "limit".into(),
            description: "This chat's trade limits and usage".into(),
        },
        BotCommand {
            command: "help".into(),
            description: "Show this help".into(),
        },
    ]
}

/// Convenience for the server binary: build + run if a token is present.
pub async fn spawn(state: Shared) -> BotResult<Option<tokio::task::JoinHandle<()>>> {
    match TelegramBot::new(state.clone()).await? {
        Some(bot) => {
            let bot = Arc::new(bot);
            let handle = tokio::spawn(async move {
                if let Err(e) = bot.run().await {
                    warn!(error = %e, "telegram bot stopped");
                }
            });
            Ok(Some(handle))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_has_the_core_commands() {
        let cmds = menu();
        let names: Vec<&str> = cmds.iter().map(|c| c.command.as_str()).collect();
        for expected in [
            "status", "on", "off", "kill", "resume", "mode", "help", "buy", "sell",
            "snipe", "wallets", "limit",
        ] {
            assert!(names.contains(&expected), "menu missing {expected}");
        }
    }
}
