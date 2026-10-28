//! Shared test support for the `module-telegram` integration tests.
//!
//! Two things live here:
//!
//! * [`FakeTelegram`] — a real, tiny HTTP/1.1 server that speaks just enough of
//!   the Bot API (`getUpdates`, `sendMessage`, `setMyCommands`,
//!   `deleteWebhook`, `answerCallbackQuery`) for the **actual** polling
//!   dispatcher loop to run against it. No test ever contacts Telegram: the
//!   client is pointed at `127.0.0.1`.
//! * small helpers for driving the bot and reading its replies.
//!
//! This module is compiled into each integration test binary that declares
//! `mod support;`, which is why every item is `pub` and the module is not part
//! of the crate.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use module_telegram::api::{Chat, Message, Update, User};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One message the bot sent, as captured by the fake server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMessage {
    /// Destination chat.
    pub chat_id: i64,
    /// Message body exactly as received on the wire (MarkdownV2 escaped).
    pub text: String,
    /// `parse_mode` form field, when the client sent one.
    pub parse_mode: Option<String>,
}

impl SentMessage {
    /// `true` when the body contains `needle` after unescaping MarkdownV2
    /// backslashes (so tests can assert on readable text).
    pub fn contains(&self, needle: &str) -> bool {
        self.text.contains(needle) || unescape(&self.text).contains(needle)
    }
}

/// Drop MarkdownV2 escape backslashes so assertions read naturally.
pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Default)]
struct Inner {
    updates: Vec<serde_json::Value>,
    sent: Vec<SentMessage>,
    methods: Vec<String>,
}

/// A stand-in for `https://api.telegram.org`.
pub struct FakeTelegram {
    base_url: String,
    inner: Arc<Mutex<Inner>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeTelegram {
    /// Bind an ephemeral port and start serving.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake tg");
        let addr = listener.local_addr().expect("local addr");
        let inner = Arc::new(Mutex::new(Inner::default()));
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
        let shared = Arc::clone(&inner);
        tokio::spawn(async move {
            loop {
                let accept = tokio::select! {
                    biased;
                    _ = &mut rx => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((stream, _peer)) = accept else {
                    break;
                };
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    let _ = serve_one(stream, shared).await;
                });
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            inner,
            shutdown: Some(tx),
        }
    }

    /// The base URL to hand to `TelegramApi::with_base_url`.
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// Queue one message update for the next `getUpdates` call.
    pub fn push_message(&self, update_id: i64, chat_id: i64, user_id: i64, text: &str) {
        let update = Update {
            update_id,
            message: Some(Message {
                message_id: update_id,
                from: Some(User {
                    id: user_id,
                    is_bot: Some(false),
                    first_name: Some("tester".into()),
                    username: None,
                }),
                chat: Chat {
                    id: chat_id,
                    r#type: Some("private".into()),
                    title: None,
                    username: None,
                },
                date: Some(1_700_000_000),
                text: Some(text.to_string()),
            }),
            callback_query: None,
        };
        self.push_update(update);
    }

    /// Queue a pre-built update.
    pub fn push_update(&self, update: Update) {
        let mut inner = self.inner.lock().expect("fake tg lock");
        inner
            .updates
            .push(serde_json::to_value(update).expect("serialize update"));
    }

    /// Every message sent so far, in order.
    pub fn sent(&self) -> Vec<SentMessage> {
        self.inner.lock().expect("fake tg lock").sent.clone()
    }

    /// The plain text of every message sent so far.
    pub fn texts(&self) -> Vec<String> {
        self.sent().into_iter().map(|m| unescape(&m.text)).collect()
    }

    /// Every method name the client called (diagnostics).
    pub fn methods(&self) -> Vec<String> {
        self.inner.lock().expect("fake tg lock").methods.clone()
    }

    /// Wait until the bot has sent at least `count` messages.
    pub async fn wait_for_count(&self, count: usize, timeout: std::time::Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let texts = self.texts();
            if texts.len() >= count {
                return texts;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for {count} messages; got {texts:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Wait until at least one message contains `needle`, returning all texts.
    pub async fn wait_for_text(&self, needle: &str, timeout: std::time::Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let texts = self.texts();
            if texts.iter().any(|t| t.contains(needle)) {
                return texts;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for text {needle:?}; got {texts:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

impl Drop for FakeTelegram {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn serve_one(
    mut stream: tokio::net::TcpStream,
    inner: Arc<Mutex<Inner>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(headers_end) = find_subslice(&buffer, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..headers_end]).to_string();
            let content_length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            let body_start = headers_end + 4;
            while buffer.len() < body_start + content_length {
                let read = stream.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            let body = String::from_utf8_lossy(&buffer[body_start..]).to_string();
            // Behave like long polling: hold an empty getUpdates for up to
            // 40ms so the bot's loop does not spin.
            if head.contains("/getUpdates") {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(40);
                loop {
                    if !inner.lock().expect("fake tg lock").updates.is_empty() {
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }
            let response = respond(&head, &body, &inner);
            let http = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            stream.write_all(http.as_bytes()).await?;
            stream.flush().await?;
            break;
        }
    }
    let _ = stream.shutdown().await;
    Ok(())
}

fn respond(head: &str, body: &str, inner: &Arc<Mutex<Inner>>) -> String {
    let request_line = head.lines().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    // Strip any query string: `getUpdates?offset=…` must still match.
    let method_path = path.split('?').next().unwrap_or(path);
    let method = method_path.rsplit('/').next().unwrap_or("").to_string();
    let mut guard = inner.lock().expect("fake tg lock");
    guard.methods.push(method.clone());
    match method.as_str() {
        "getUpdates" => {
            let updates: Vec<serde_json::Value> = std::mem::take(&mut guard.updates);
            serde_json::json!({ "ok": true, "result": updates }).to_string()
        }
        "sendMessage" => {
            let fields = parse_form(body);
            let chat_id = fields
                .iter()
                .find(|(k, _)| k == "chat_id")
                .and_then(|(_, v)| v.parse::<i64>().ok())
                .unwrap_or_default();
            let text = fields
                .iter()
                .find(|(k, _)| k == "text")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let parse_mode = fields
                .iter()
                .find(|(k, _)| k == "parse_mode")
                .map(|(_, v)| v.clone());
            guard.sent.push(SentMessage {
                chat_id,
                text,
                parse_mode,
            });
            serde_json::json!({ "ok": true, "result": { "message_id": 1 } }).to_string()
        }
        _ => serde_json::json!({ "ok": true, "result": true }).to_string(),
    }
}

fn parse_form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &input[index + 1..index + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Extract the confirmation nonce from a `/confirm <nonce>` reply.
pub fn extract_nonce(text: &str) -> Option<String> {
    unescape(text)
        .lines()
        .find_map(|line| line.trim().strip_prefix("/confirm ").map(str::trim))
        .filter(|nonce| !nonce.is_empty())
        .map(str::to_string)
}
// ---------------------------------------------------------------------------
// A running bot wired to the fake server
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicI64, Ordering};

use bot_core::config::AppConfig;
use bot_core::state::Shared;
use module_telegram::{
    ChatLink, FakeTradePort, TelegramBot, TradeConsole, TradeConsoleConfig, TradeStores,
};

/// Telegram user id every test links its chats to.
pub const LINKED_USER: i64 = 111;
/// Default chat used by the happy-path tests.
pub const CHAT: i64 = 100;
/// Second chat (used for per-chat isolation).
pub const CHAT_B: i64 = 101;
/// A chat that is allowed by configuration but never linked to a tenant.
pub const UNLINKED_CHAT: i64 = 999;
/// A second Telegram user (used for the "wrong user" case).
pub const OTHER_USER: i64 = 222;
/// The tenant the test chats are linked to.
pub const ORG: &str = "org-1";

/// Build the shared state every test bot runs on.
pub fn test_state() -> Shared {
    bot_core::state::AppState::new(AppConfig::from_defaults())
}

/// Put the telegram allowlist in force (no owner list ⇒ allowlisted chats keep
/// full owner rights, exactly like a single-operator deployment).
pub async fn configure_telegram(state: &Shared) {
    state
        .update_config(|cfg| {
            cfg.telegram.enabled = true;
            cfg.telegram.allowed_chat_ids = vec![CHAT, CHAT_B, UNLINKED_CHAT];
            cfg.telegram.allowed_user_ids = Vec::new();
            cfg.telegram.owner_user_ids = Vec::new();
            cfg.telegram.parse_mode = "MarkdownV2".into();
        })
        .await;
}

/// Build a console over `stores` with the given chat links.
pub async fn console_with_stores(
    state: &Shared,
    port: std::sync::Arc<FakeTradePort>,
    stores: TradeStores,
    links: &[(i64, ChatLink)],
) -> std::sync::Arc<TradeConsole> {
    let console = TradeConsole::new(state.clone(), port, stores, TradeConsoleConfig::default())
        .expect("console builds with the default limits");
    for (chat_id, link) in links {
        console.link_chat(*chat_id, link.clone()).await;
    }
    std::sync::Arc::new(console)
}

/// Build a console over fresh in-memory stores.
pub async fn console_with(
    state: &Shared,
    port: std::sync::Arc<FakeTradePort>,
    links: &[(i64, ChatLink)],
) -> std::sync::Arc<TradeConsole> {
    console_with_stores(state, port, TradeStores::in_memory(), links).await
}

/// A bot process under test: the real `TelegramBot::run` polling loop, pointed
/// at [`FakeTelegram`] instead of api.telegram.org.
pub struct Bot {
    /// The stand-in Bot API server.
    pub fake: FakeTelegram,
    /// Shared bot state.
    pub state: Shared,
    /// The port the console talks to.
    pub port: std::sync::Arc<FakeTradePort>,
    /// The console, when one is attached.
    pub console: Option<std::sync::Arc<TradeConsole>>,
    /// The stores behind the console (empty bundle when no console is wired).
    pub stores: TradeStores,
    update: AtomicI64,
    shutdown: std::sync::Arc<bot_core::lifecycle::Shutdown>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Bot {
    /// Start a bot. `with_console` attaches the BATCH 04 trade console.
    pub async fn start(with_console: bool) -> Self {
        let fake = FakeTelegram::start().await;
        let state = test_state();
        configure_telegram(&state).await;
        let port = std::sync::Arc::new(FakeTradePort::new());
        let stores = TradeStores::in_memory();
        let console = if with_console {
            Some(
                console_with_stores(
                    &state,
                    std::sync::Arc::clone(&port),
                    stores.clone(),
                    &[
                        (CHAT, ChatLink::trading(ORG, LINKED_USER)),
                        (CHAT_B, ChatLink::trading(ORG, LINKED_USER)),
                    ],
                )
                .await,
            )
        } else {
            None
        };

        let shutdown = bot_core::lifecycle::Shutdown::new();
        state.attach_shutdown(std::sync::Arc::clone(&shutdown));

        let api = module_telegram::api::TelegramApi::new("test-token-not-real")
            .expect("api client")
            .with_base_url(fake.base_url());
        let mut bot = TelegramBot::from_api(state.clone(), api, None);
        if let Some(console) = &console {
            bot = bot.with_trade_console(std::sync::Arc::clone(console));
        }
        let task = tokio::spawn(async move {
            let _ = bot.run().await;
        });

        Self {
            fake,
            state,
            port,
            console,
            stores,
            update: AtomicI64::new(1),
            shutdown,
            task: Some(task),
        }
    }

    /// Queue one command message without waiting for a reply.
    pub fn send(&self, chat_id: i64, user_id: i64, text: &str) {
        let update_id = self.update.fetch_add(1, Ordering::SeqCst);
        self.fake.push_message(update_id, chat_id, user_id, text);
    }

    /// Queue one command message and return the bot's reply text.
    pub async fn say(&self, chat_id: i64, user_id: i64, text: &str) -> String {
        let before = self.fake.sent().len();
        self.send(chat_id, user_id, text);
        let texts = self
            .fake
            .wait_for_count(before + 1, std::time::Duration::from_secs(10))
            .await;
        texts.last().cloned().unwrap_or_default()
    }

    /// Queue one command and require the reply to contain `needle`.
    pub async fn say_expect(&self, chat_id: i64, user_id: i64, text: &str, needle: &str) -> String {
        let reply = self.say(chat_id, user_id, text).await;
        assert!(
            reply.contains(needle),
            "reply to {text:?} did not contain {needle:?}: {reply:?}"
        );
        reply
    }

    /// Send the consent-granting command and return the reply.
    pub async fn grant_consent(&self) -> String {
        self.say_expect(
            CHAT,
            LINKED_USER,
            "/consent grant 0.05 0.2 24 I CONSENT TO TELEGRAM TRADING",
            "Consent granted",
        )
        .await
    }

    /// Request a buy and return the pending-command nonce.
    pub async fn request_buy(&self, amount_sol: &str) -> String {
        let reply = self
            .say(
                CHAT,
                LINKED_USER,
                &format!("/buy So11111111111111111111111111111111111111112 {amount_sol} 100"),
            )
            .await;
        extract_nonce(&reply).unwrap_or_else(|| panic!("no nonce in reply: {reply:?}"))
    }

    /// Stop the polling loop and wait for the task to finish.
    pub async fn stop(mut self) {
        self.shutdown.signal("test finished");
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
    }
}
