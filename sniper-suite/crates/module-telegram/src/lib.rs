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
pub mod commands;

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
        }))
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
                let role = telegram_role(chat_id, user_id, &tg);
                let authorized = role.is_some();
                self.state
                    .events
                    .publish(bot_core::events::AppEvent::Command {
                        ts: chrono::Utc::now(),
                        chat_id,
                        user_id,
                        text: text.clone(),
                        accepted: authorized,
                        response: None,
                    });

                if !authorized {
                    warn!(chat_id, user_id = ?user_id, "unauthorized telegram command attempt");
                    let _ = self
                        .api
                        .send_message(
                            chat_id,
                            "⛔ You are not authorized to control this bot.",
                            Some(&tg.parse_mode),
                        )
                        .await;
                    continue;
                }

                let command = parse_command(&text, &tg.prefix);
                let reply = handle(&self.state, command, role.expect("checked above")).await;
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
        for expected in ["status", "on", "off", "kill", "resume", "mode", "help"] {
            assert!(names.contains(&expected), "menu missing {expected}");
        }
    }
}
