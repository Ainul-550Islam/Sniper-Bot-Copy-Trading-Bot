//! A minimal Telegram Bot API client (long polling + sendMessage).
//!
//! We deliberately avoid a heavyweight framework: the control bot only needs
//! `getUpdates`, `sendMessage`, `setMyCommands` and `deleteWebhook`. Everything
//! is plain HTTPS JSON against `https://api.telegram.org/bot<token>/<method>`.

use serde::{Deserialize, Serialize};

use bot_core::error::{BotError, BotResult};

/// Default Telegram Bot API base.
pub const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Envelope every Bot API method returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TgResponse<T> {
    /// Call succeeded; `result` is populated.
    pub ok: bool,
    /// Method payload when `ok`.
    #[serde(default)]
    pub result: Option<T>,
    /// Human-readable error when not `ok`.
    #[serde(default)]
    pub description: Option<String>,
    /// HTTP-style error code when not `ok` (e.g. 401, 429).
    #[serde(default)]
    pub error_code: Option<u16>,
}

/// A chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    /// Unique chat id (negative for groups/channels).
    pub id: i64,
    /// Chat type: `private`, `group`, `supergroup`, `channel`.
    #[serde(default)]
    pub r#type: Option<String>,
    /// Group/channel title, when applicable.
    #[serde(default)]
    pub title: Option<String>,
    /// Public @username, when set.
    #[serde(default)]
    pub username: Option<String>,
}

/// A user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Unique user id — the value matched against the allow/role lists.
    pub id: i64,
    /// True when the account is itself a bot.
    #[serde(default)]
    pub is_bot: Option<bool>,
    /// Display first name.
    #[serde(default)]
    pub first_name: Option<String>,
    /// Public @username, when set.
    #[serde(default)]
    pub username: Option<String>,
}

/// An incoming message (only the fields we use).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Per-chat unique message id.
    pub message_id: i64,
    /// Sender (absent for channel posts).
    #[serde(default)]
    pub from: Option<User>,
    /// Chat the message arrived in.
    pub chat: Chat,
    /// Send time (unix seconds).
    #[serde(default)]
    pub date: Option<i64>,
    /// Text body for plain text messages (absent for media etc.).
    #[serde(default)]
    pub text: Option<String>,
}

/// An update from `getUpdates`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Update {
    /// Monotonic update id — used as the `getUpdates` long-poll offset.
    pub update_id: i64,
    /// Present only for plain message updates (the only kind we handle).
    #[serde(default)]
    pub message: Option<Message>,
}

/// A bot command for the `/`-menu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotCommand {
    /// Command name without the leading `/`.
    pub command: String,
    /// Help text shown in the Telegram command menu.
    pub description: String,
}

/// The Bot API client.
#[derive(Clone)]
pub struct TelegramApi {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl TelegramApi {
    /// Create a client for a bot token.
    pub fn new(token: impl Into<String>) -> BotResult<Self> {
        let token = token.into();
        if token.trim().is_empty() {
            return Err(BotError::config("telegram bot token is empty"));
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(75))
            .build()
            .map_err(|e| BotError::http(format!("telegram http client: {e}")))?;
        Ok(TelegramApi {
            base_url: TELEGRAM_API_BASE.to_string(),
            token,
            http,
        })
    }

    /// Override the API base (useful for a local Bot API server).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Build the method URL. NOTE: the Bot API embeds the token in the URL
    /// path, so this string is SECRET material. Every `reqwest::Error`
    /// mapping in this file calls `.without_url()` — reqwest's `Display`
    /// includes the full URL on send errors, which would leak the token into
    /// logs and alert text. Regression test:
    /// `error_strings_never_contain_the_bot_token`.
    fn method_url(&self, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.base_url.trim_end_matches('/'),
            self.token,
            method
        )
    }

    /// `getUpdates` with long polling.
    pub async fn get_updates(&self, offset: i64, timeout_secs: u64) -> BotResult<Vec<Update>> {
        let url = self.method_url("getUpdates");
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("offset", offset.to_string()),
                ("timeout", timeout_secs.to_string()),
                ("allowed_updates", "[\"message\"]".to_string()),
            ])
            .send()
            .await
            .map_err(|e| BotError::http(format!("getUpdates: {}", e.without_url())))?;
        let status = resp.status();
        let body: TgResponse<Vec<Update>> = resp
            .json()
            .await
            .map_err(|e| BotError::encoding(format!("getUpdates json: {}", e.without_url())))?;
        if !body.ok {
            return Err(BotError::http(format!(
                "getUpdates not ok ({}): {}",
                status,
                body.description.unwrap_or_default()
            )));
        }
        Ok(body.result.unwrap_or_default())
    }

    /// `sendMessage`. Long text is split into <=4096-char chunks.
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
    ) -> BotResult<()> {
        for chunk in split_message(text, 4096) {
            let url = self.method_url("sendMessage");
            let mut form = vec![
                ("chat_id", chat_id.to_string()),
                ("text", chunk.clone()),
                ("disable_web_page_preview", "true".to_string()),
            ];
            let pm;
            if let Some(mode) = parse_mode {
                if !mode.trim().is_empty() && mode.trim() != "none" {
                    pm = mode.to_string();
                    form.push(("parse_mode", pm.clone()));
                }
            }
            let resp = self
                .http
                .post(&url)
                .form(&form)
                .send()
                .await
                .map_err(|e| BotError::http(format!("sendMessage: {}", e.without_url())))?;
            let body: TgResponse<serde_json::Value> = resp.json().await.map_err(|e| {
                BotError::encoding(format!("sendMessage json: {}", e.without_url()))
            })?;
            if !body.ok {
                // A parse_mode failure is common (unescaped HTML); retry plain.
                if parse_mode.is_some() {
                    let retry = self
                        .http
                        .post(&url)
                        .form(&[
                            ("chat_id", chat_id.to_string()),
                            ("text", chunk.clone()),
                            ("disable_web_page_preview", "true".to_string()),
                        ])
                        .send()
                        .await
                        .map_err(|e| {
                            BotError::http(format!("sendMessage retry: {}", e.without_url()))
                        })?;
                    let rbody: TgResponse<serde_json::Value> = retry.json().await.map_err(|e| {
                        BotError::encoding(format!("sendMessage retry json: {}", e.without_url()))
                    })?;
                    if !rbody.ok {
                        return Err(BotError::http(format!(
                            "sendMessage failed: {}",
                            rbody.description.unwrap_or_default()
                        )));
                    }
                } else {
                    return Err(BotError::http(format!(
                        "sendMessage failed: {}",
                        body.description.unwrap_or_default()
                    )));
                }
            }
        }
        Ok(())
    }

    /// `setMyCommands` — populate the `/`-menu.
    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> BotResult<()> {
        let url = self.method_url("setMyCommands");
        let json = serde_json::to_string(commands)
            .map_err(|e| BotError::encoding(format!("commands json: {e}")))?;
        let resp = self
            .http
            .post(&url)
            .form(&[("commands", json)])
            .send()
            .await
            .map_err(|e| BotError::http(format!("setMyCommands: {}", e.without_url())))?;
        let body: TgResponse<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| BotError::encoding(format!("setMyCommands json: {}", e.without_url())))?;
        if !body.ok {
            return Err(BotError::http(format!(
                "setMyCommands failed: {}",
                body.description.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// `deleteWebhook` — ensure long polling receives updates.
    pub async fn delete_webhook(&self) -> BotResult<()> {
        let url = self.method_url("deleteWebhook");
        let resp = self
            .http
            .post(&url)
            .form(&[("drop_pending_updates", "false".to_string())])
            .send()
            .await
            .map_err(|e| BotError::http(format!("deleteWebhook: {}", e.without_url())))?;
        let body: TgResponse<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| BotError::encoding(format!("deleteWebhook json: {}", e.without_url())))?;
        if !body.ok {
            return Err(BotError::http(format!(
                "deleteWebhook failed: {}",
                body.description.unwrap_or_default()
            )));
        }
        Ok(())
    }
}

/// Split a message into chunks no larger than `max`, preferring line breaks and
/// falling back to char boundaries so we never split a UTF-8 sequence.
pub fn split_message(text: &str, max: usize) -> Vec<String> {
    if text.chars().count() <= max {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if current.chars().count() + line.chars().count() > max {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            // A single line longer than max must itself be split by chars.
            if line.chars().count() > max {
                let mut buf = String::new();
                for ch in line.chars() {
                    buf.push(ch);
                    if buf.chars().count() >= max {
                        out.push(std::mem::take(&mut buf));
                    }
                }
                current = buf;
                continue;
            }
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_message_is_one_chunk() {
        assert_eq!(split_message("hello", 4096), vec!["hello".to_string()]);
    }

    #[test]
    fn splits_on_line_boundaries() {
        let text = "aaaa\nbbbb\ncccc";
        let chunks = split_message(text, 9);
        assert!(chunks.iter().all(|c| c.chars().count() <= 9));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn splits_a_long_single_line_by_chars() {
        let text = "x".repeat(10);
        let chunks = split_message(&text, 4);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 4));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn never_splits_a_multibyte_char() {
        let text = "😀".repeat(5); // each is 1 char, 4 bytes
        let chunks = split_message(&text, 2);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|c| c.chars().count() <= 2));
    }

    #[test]
    fn empty_token_is_rejected() {
        assert!(TelegramApi::new("").is_err());
        assert!(TelegramApi::new("   ").is_err());
    }

    #[test]
    fn method_url_is_wellformed() {
        let api = TelegramApi::new("TOKEN").unwrap();
        assert_eq!(
            api.method_url("getUpdates"),
            "https://api.telegram.org/botTOKEN/getUpdates"
        );
    }

    /// SECRET-LEAK REGRESSION (engineering-freeze audit): the bot token is
    /// embedded in every request URL, and `reqwest::Error`'s `Display`
    /// includes the full URL for send errors. Without `.without_url()` in
    /// every error mapping the token ends up in tracing logs, audit detail
    /// strings and Telegram alert text. Port 1 on loopback is closed, so
    /// every call below fails fast and deterministically with no network.
    #[tokio::test]
    async fn error_strings_never_contain_the_bot_token() {
        let token = "123456:FREEZE-AUDIT-TOKEN-MUST-NEVER-APPEAR";
        let api = TelegramApi::new(token)
            .unwrap()
            .with_base_url("http://127.0.0.1:1");

        let err = api.delete_webhook().await.expect_err("closed port");
        assert!(
            !err.to_string().contains(token),
            "deleteWebhook error leaked the token: {err}"
        );
        let err = api.get_updates(0, 0).await.expect_err("closed port");
        assert!(
            !err.to_string().contains(token),
            "getUpdates error leaked the token: {err}"
        );
        let err = api
            .send_message(-1, "x", None)
            .await
            .expect_err("closed port");
        assert!(
            !err.to_string().contains(token),
            "sendMessage error leaked the token: {err}"
        );
        let err = api.set_my_commands(&[]).await.expect_err("closed port");
        assert!(
            !err.to_string().contains(token),
            "setMyCommands error leaked the token: {err}"
        );
    }
}
