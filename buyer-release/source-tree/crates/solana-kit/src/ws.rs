//! Reconnecting Solana WebSocket client.
//!
//! Solana's RPC websocket is a plain JSON-RPC endpoint: you send
//! `{"jsonrpc":"2.0","id":N,"method":"logsSubscribe","params":[…]}`, get back
//! `{"id":N,"result":<server subscription id>}`, and then receive notifications
//! shaped like
//! `{"method":"logsNotification","params":{"subscription":<server id>,"result":…}}`.
//!
//! This client owns the connection lifecycle: exponential-backoff reconnect,
//! re-issuing every registered subscription after a reconnect, and routing
//! notifications to per-subscription channels. Callers never deal with ids.
//!
//! ## Id bookkeeping
//!
//! Three distinct ids are in play and conflating them is the classic source of
//! "the bot silently stopped seeing launches after a reconnect" bugs:
//!
//! * `request_id` — our JSON-RPC `id`, unique per *subscribe call*
//! * `server_id`  — the `result` of a subscribe call, unique per *connection*
//! * `local_id`   — stable identity of a subscription across reconnects
//!
//! `subs` is keyed by `local_id`; `by_request` maps `request_id -> local_id`
//! and `by_server` maps `server_id -> local_id`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};

/// Everything a subscriber can receive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WsMessage {
    /// `logsSubscribe` notification.
    Logs {
        subscription: u64,
        signature: String,
        err: Option<Value>,
        logs: Vec<String>,
    },
    /// `accountSubscribe` notification (base64 account data, already decoded).
    Account {
        subscription: u64,
        pubkey: String,
        lamports: u64,
        owner: String,
        data: Vec<u8>,
        slot: u64,
    },
    /// `transactionSubscribe` notification (Geyser/Yellowstone only).
    Transaction {
        subscription: u64,
        signature: String,
        slot: u64,
        raw: Value,
    },
    /// A notification we did not recognise. Kept rather than dropped so a new
    /// server-side subscription type stays observable.
    Raw {
        subscription: Option<u64>,
        value: Value,
    },
    /// Connection lifecycle event, surfaced so modules can report health.
    Status(WsStatus),
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsStatus {
    Connected,
    Disconnected,
    Reconnecting,
}

/// Parameters for `logsSubscribe`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LogsFilter {
    /// Only transactions that mention one of these accounts.
    pub mentions: Vec<String>,
    /// Only transactions that mention *all* of these accounts.
    pub mention_all: Vec<String>,
    pub commitment: Option<String>,
    pub include_votes: bool,
}

impl LogsFilter {
    pub fn mentions(mut self, accounts: impl IntoIterator<Item = String>) -> Self {
        self.mentions.extend(accounts);
        self
    }

    pub fn to_params(&self) -> Value {
        let filter = if !self.mentions.is_empty() {
            json!({ "mentions": self.mentions })
        } else if !self.mention_all.is_empty() {
            json!({ "mentionsAll": self.mention_all })
        } else {
            json!("all")
        };
        json!([filter, self.config()])
    }

    fn config(&self) -> Value {
        json!({
            "commitment": self.commitment.clone().unwrap_or_else(|| "processed".into()),
            "includeVotes": self.include_votes,
        })
    }
}

/// Parameters for `accountSubscribe`.
#[derive(Debug, Clone, Serialize)]
pub struct AccountFilter {
    pub pubkey: String,
    pub commitment: Option<String>,
}

impl AccountFilter {
    pub fn to_params(&self) -> Value {
        json!([
            self.pubkey,
            {
                "encoding": "base64",
                "commitment": self.commitment.clone().unwrap_or_else(|| "processed".into()),
            }
        ])
    }
}

/// Parameters for `transactionSubscribe` (requires a Geyser-enabled node).
#[derive(Debug, Clone, Serialize)]
pub struct TransactionFilter {
    pub account_include: Vec<String>,
    pub account_exclude: Vec<String>,
    pub account_required: Vec<String>,
    pub commitment: Option<String>,
    pub include_votes: bool,
    /// `full` sends the whole encoded transaction; `signatures` only the id.
    pub transaction_details: String,
    pub max_supported_transaction_version: u8,
}

impl Default for TransactionFilter {
    fn default() -> Self {
        TransactionFilter {
            account_include: Vec::new(),
            account_exclude: Vec::new(),
            account_required: Vec::new(),
            commitment: Some("processed".into()),
            include_votes: false,
            transaction_details: "full".into(),
            max_supported_transaction_version: 0,
        }
    }
}

impl TransactionFilter {
    pub fn to_params(&self) -> Value {
        let mut filter = serde_json::Map::new();
        if !self.account_include.is_empty() {
            filter.insert("accountInclude".into(), json!(self.account_include));
        }
        if !self.account_exclude.is_empty() {
            filter.insert("accountExclude".into(), json!(self.account_exclude));
        }
        if !self.account_required.is_empty() {
            filter.insert("accountRequired".into(), json!(self.account_required));
        }
        if self.include_votes {
            filter.insert("vote".into(), json!(true));
        }
        json!([
            Value::Object(filter),
            {
                "commitment": self.commitment.clone().unwrap_or_else(|| "processed".into()),
                "encoding": "base64",
                "transactionDetails": self.transaction_details,
                "showRewards": false,
                "maxSupportedTransactionVersion": self.max_supported_transaction_version,
            }
        ])
    }
}

/// A registered subscription: survives reconnects.
#[derive(Debug, Clone)]
struct Subscription {
    method: String,
    params: Value,
    tx: mpsc::UnboundedSender<WsMessage>,
}

struct Shared {
    /// local_id -> subscription
    subs: RwLock<HashMap<u64, Subscription>>,
    /// request_id -> local_id (an in-flight subscribe call)
    by_request: RwLock<HashMap<u64, u64>>,
    /// server_id -> local_id (a live subscription on the current connection)
    by_server: RwLock<HashMap<u64, u64>>,
    /// local_id -> the caller waiting for its first subscription confirmation
    waiters: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<BotResult<u64>>>>,
    next_local_id: AtomicU64,
    next_request_id: AtomicU64,
    connected: AtomicBool,
    shutdown: AtomicBool,
    /// Frames the supervisor must write to the socket. Using a channel rather
    /// than `Notify` avoids a lost-wakeup race: a subscription registered while
    /// the socket is idle would otherwise sit unsent until the next reconnect.
    outbound: mpsc::UnboundedSender<String>,
    outbound_rx: RwLock<Option<mpsc::UnboundedReceiver<String>>>,
}

impl Shared {
    fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        Arc::new(Shared {
            subs: RwLock::new(HashMap::new()),
            by_request: RwLock::new(HashMap::new()),
            by_server: RwLock::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            next_local_id: AtomicU64::new(1),
            next_request_id: AtomicU64::new(1),
            connected: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            outbound: tx,
            outbound_rx: RwLock::new(Some(rx)),
        })
    }

    fn alloc_local(&self) -> u64 {
        self.next_local_id.fetch_add(1, Ordering::Relaxed)
    }

    fn alloc_request(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    async fn broadcast_status(&self, status: WsStatus) {
        let subs = self.subs.read().await;
        for sub in subs.values() {
            let _ = sub.tx.send(WsMessage::Status(status));
        }
    }

    /// Queue a JSON-RPC frame for the socket. Returns false when the
    /// supervisor is not running, in which case the subscription stays
    /// registered and is flushed on the next (re)connect.
    fn enqueue(&self, frame: String) -> bool {
        self.outbound.send(frame).is_ok()
    }

    /// Take the outbound receiver; only the supervisor may call this, once.
    async fn take_outbound(&self) -> Option<mpsc::UnboundedReceiver<String>> {
        self.outbound_rx.write().await.take()
    }

    /// Drop every server-side mapping (the connection is gone) and tell all
    /// subscribers we are reconnecting.
    async fn on_disconnect(&self) {
        self.connected.store(false, Ordering::Relaxed);
        self.by_server.write().await.clear();
        // In-flight subscribe requests die with the connection: their frames
        // are gone and no response will ever arrive. Clearing the mappings
        // lets `register_outgoing` re-send the surviving subscriptions on the
        // next connect instead of skipping them as "in flight" forever.
        self.by_request.write().await.clear();
        // Any subscribe call that was in flight will never be answered.
        let mut waiters = self.waiters.lock().await;
        for (_, w) in waiters.drain() {
            let _ = w.send(Err(BotError::ws("connection dropped while subscribing")));
        }
        self.broadcast_status(WsStatus::Reconnecting).await;
    }
}

/// A handle to one subscription.
pub struct SubscriptionHandle {
    local_id: u64,
    method: String,
    rx: Option<mpsc::UnboundedReceiver<WsMessage>>,
    shared: Arc<Shared>,
}

impl SubscriptionHandle {
    /// Our stable subscription id (not the server's).
    pub fn id(&self) -> u64 {
        self.local_id
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    /// Take the receiver. Panics if called twice — there is one consumer per
    /// subscription by design.
    pub fn receiver(&mut self) -> mpsc::UnboundedReceiver<WsMessage> {
        self.rx
            .take()
            .expect("SubscriptionHandle::receiver called twice")
    }

    /// Stop the subscription locally and ask the server to unsubscribe.
    pub async fn cancel(self) {
        self.shared.subs.write().await.remove(&self.local_id);
        let mut by_server = self.shared.by_server.write().await;
        by_server.retain(|_, local| *local != self.local_id);
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        // Dropping the handle stops delivery: nobody can read the channel any
        // more, so remove the registration instead of leaking it.
        let shared = self.shared.clone();
        let local_id = self.local_id;
        tokio::spawn(async move {
            shared.subs.write().await.remove(&local_id);
            shared
                .by_server
                .write()
                .await
                .retain(|_, local| *local != local_id);
        });
    }
}

/// The client. Clone it freely.
#[derive(Clone)]
pub struct SolanaWs {
    url: String,
    shared: Arc<Shared>,
}

impl SolanaWs {
    pub fn new(url: impl Into<String>) -> Self {
        SolanaWs {
            url: url.into(),
            shared: Shared::new(),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn is_connected(&self) -> bool {
        self.shared.is_connected()
    }

    /// Spawn the connection supervisor.
    pub fn spawn(&self) -> WsHandle {
        let shared = self.shared.clone();
        let url = self.url.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(supervise(url, shared, shutdown_rx));
        WsHandle {
            task,
            shutdown_tx,
            shared: self.shared.clone(),
        }
    }

    /// Register a subscription and wait for the server to confirm it.
    pub async fn subscribe(&self, method: &str, params: Value) -> BotResult<SubscriptionHandle> {
        let (tx, rx) = mpsc::unbounded_channel();
        let local_id = self.shared.alloc_local();
        let request_id = self.shared.alloc_request();

        self.shared.subs.write().await.insert(
            local_id,
            Subscription {
                method: method.to_string(),
                params: params.clone(),
                tx,
            },
        );

        let (waiter_tx, waiter_rx) = tokio::sync::oneshot::channel();
        self.shared.waiters.lock().await.insert(local_id, waiter_tx);

        // If we are already connected, push the request out right away;
        // otherwise it stays registered *without* a by_request mapping, and
        // `register_outgoing` flushes it (allocating the request id itself) on
        // the next (re)connect. Mapping a request that was never written would
        // poison the in-flight set and the subscription would never be sent.
        if self.shared.is_connected() {
            // Insert the mapping *before* writing: a fast server could answer
            // before we got to record it otherwise.
            self.shared
                .by_request
                .write()
                .await
                .insert(request_id, local_id);
            let frame = json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            })
            .to_string();
            if !self.shared.enqueue(frame) {
                // The supervisor is gone; nothing will ever send or answer.
                self.shared.by_request.write().await.remove(&request_id);
                self.shared.subs.write().await.remove(&local_id);
                self.shared.waiters.lock().await.remove(&local_id);
                return Err(BotError::ws(format!(
                    "{method}: websocket supervisor is not running"
                )));
            }
        }

        let result = tokio::time::timeout(Duration::from_secs(45), waiter_rx).await;
        match result {
            Err(_) => {
                self.shared.waiters.lock().await.remove(&local_id);
                Err(BotError::ws(format!(
                    "{method}: timed out waiting for the websocket to subscribe"
                )))
            }
            Ok(Err(_)) => {
                self.shared.waiters.lock().await.remove(&local_id);
                Err(BotError::ws(format!(
                    "{method}: subscription channel closed before confirmation"
                )))
            }
            Ok(Ok(inner)) => match inner {
                Ok(server_id) => {
                    info!(method, local_id, server_id, "websocket subscription active");
                    Ok(SubscriptionHandle {
                        local_id,
                        method: method.to_string(),
                        rx: Some(rx),
                        shared: self.shared.clone(),
                    })
                }
                Err(e) => {
                    self.shared.subs.write().await.remove(&local_id);
                    Err(e)
                }
            },
        }
    }

    pub async fn logs_subscribe(&self, filter: LogsFilter) -> BotResult<SubscriptionHandle> {
        self.subscribe("logsSubscribe", filter.to_params()).await
    }

    pub async fn account_subscribe(&self, filter: AccountFilter) -> BotResult<SubscriptionHandle> {
        self.subscribe("accountSubscribe", filter.to_params()).await
    }

    /// Requires a Geyser/Yellowstone-enabled endpoint; a plain node answers
    /// with `-32601 Method not found`, which surfaces as an error here.
    pub async fn transaction_subscribe(
        &self,
        filter: TransactionFilter,
    ) -> BotResult<SubscriptionHandle> {
        self.subscribe("transactionSubscribe", filter.to_params())
            .await
    }

    /// Every pump.fun log — the launch-detection feed for Module 1.
    pub async fn pump_logs_subscribe(&self) -> BotResult<SubscriptionHandle> {
        self.logs_subscribe(LogsFilter {
            mentions: vec![crate::consts::PUMP_PROGRAM_ID.to_string()],
            ..Default::default()
        })
        .await
    }

    pub async fn subscription_count(&self) -> usize {
        self.shared.subs.read().await.len()
    }

    /// Test-only accessor: `Shared` is private to this module.
    #[cfg(test)]
    fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }
}

/// Owns the supervisor task.
pub struct WsHandle {
    task: tokio::task::JoinHandle<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shared: Arc<Shared>,
}

impl WsHandle {
    pub async fn shutdown(self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = self.shutdown_tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

/// Reconnect loop: exponential backoff, resubscribe everything on reconnect.
async fn supervise(
    url: String,
    shared: Arc<Shared>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_millis(250);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    let reg = bot_core::obs::metrics::global();
    let mut attempt = 0u64;

    loop {
        if *shutdown.borrow_and_update() || shared.shutdown.load(Ordering::SeqCst) {
            info!(%url, "websocket supervisor shutting down");
            shared.on_disconnect().await;
            return;
        }

        if attempt > 0 {
            reg.counter(
                "bot_ws_reconnects_total",
                "Websocket reconnects performed by the supervisor.",
                &[],
            )
            .inc();
        }
        attempt += 1;

        info!(%url, "connecting to solana websocket");
        let outcome = run_connection(&url, &shared, &mut shutdown).await;
        match &outcome {
            Ok(()) => debug!(%url, "websocket closed cleanly"),
            Err(e) => {
                // No URL label: provider URLs may embed credentials.
                reg.counter(
                    "bot_ws_connection_failures_total",
                    "Websocket connection attempts that failed.",
                    &[],
                )
                .inc();
                warn!(%url, error = %e, "websocket connection failed");
            }
        }
        shared.on_disconnect().await;

        // Reset the backoff after a connection that actually stayed up.
        if outcome.is_ok() {
            backoff = Duration::from_millis(250);
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => continue,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// One connection attempt.
async fn run_connection(
    url: &str,
    shared: &Arc<Shared>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> BotResult<()> {
    let (socket, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| BotError::ws(format!("connect {url}: {e}")))?;
    let (mut sink, mut stream) = socket.split();

    shared.connected.store(true, Ordering::Relaxed);
    shared.broadcast_status(WsStatus::Connected).await;

    // Take the outbound queue. It survives across reconnects: on the first
    // connection we take it, on later ones it is already taken so we fall back
    // to flushing registered-but-unmapped subscriptions only.
    let mut outbound = shared.take_outbound().await;

    // Send every registered subscription (new ones and ones to restore).
    for (request_id, method, params) in register_outgoing(shared).await {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });
        sink.send(TungsteniteMessage::Text(msg.to_string()))
            .await
            .map_err(|e| BotError::ws(format!("send {method}: {e}")))?;
        debug!(request_id, %method, "sent subscription request");
    }

    // Solana nodes drop idle websockets after roughly a minute.
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await; // the first tick fires immediately; skip it

    loop {
        tokio::select! {
            _ = ping.tick() => {
                sink.send(TungsteniteMessage::Ping(Vec::new()))
                    .await
                    .map_err(|e| BotError::ws(format!("ping: {e}")))?;
            }
            Some(frame) = async {
                match outbound.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending::<Option<String>>().await,
                }
            } => {
                sink.send(TungsteniteMessage::Text(frame))
                    .await
                    .map_err(|e| BotError::ws(format!("send queued frame: {e}")))?;
                debug!("sent queued subscription frame");
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = sink.send(TungsteniteMessage::Close(None)).await;
                    return Ok(());
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Err(BotError::ws("stream ended"));
                };
                match frame {
                    Ok(TungsteniteMessage::Text(text)) => handle_text(shared, &text).await,
                    Ok(TungsteniteMessage::Binary(bytes)) => {
                        if let Ok(text) = String::from_utf8(bytes) {
                            handle_text(shared, &text).await;
                        }
                    }
                    Ok(TungsteniteMessage::Ping(payload)) => {
                        let _ = sink.send(TungsteniteMessage::Pong(payload)).await;
                    }
                    Ok(TungsteniteMessage::Close(frame)) => {
                        info!(?frame, "websocket closed by server");
                        return Ok(());
                    }
                    Ok(_) => {}
                    Err(e) => return Err(BotError::ws(format!("frame: {e}"))),
                }
            }
        }
    }
}

/// Take every subscription that has no server id yet and allocate a request id
/// for it. Returns the frames to send.
async fn register_outgoing(shared: &Arc<Shared>) -> Vec<(u64, String, Value)> {
    let subs = shared.subs.read().await;
    if subs.is_empty() {
        return Vec::new();
    }
    let by_server = shared.by_server.read().await;
    let already_sent: std::collections::HashSet<u64> = by_server.values().copied().collect();
    let by_request = shared.by_request.read().await;
    let in_flight: std::collections::HashSet<u64> = by_request.values().copied().collect();
    drop(by_request);
    drop(by_server);

    let mut out = Vec::new();
    for (local_id, sub) in subs.iter() {
        if already_sent.contains(local_id) || in_flight.contains(local_id) {
            continue;
        }
        let request_id = shared.alloc_request();
        out.push((request_id, sub.method.clone(), sub.params.clone()));
        // Record the mapping so the response can find us. We insert into
        // by_request below, after releasing the subs read lock.
        shared
            .by_request
            .write()
            .await
            .insert(request_id, *local_id);
    }
    out
}

/// Handle one inbound text frame: either a subscribe response or a notification.
async fn handle_text(shared: &Arc<Shared>, text: &str) {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "non-json websocket frame");
            return;
        }
    };

    // ---- subscribe response: {"id":N,"result":<server id>} | {"id":N,"error":…}
    if let Some(request_id) = value.get("id").and_then(|v| v.as_u64()) {
        let local_id = shared.by_request.write().await.remove(&request_id);
        let Some(local_id) = local_id else {
            debug!(request_id, "response for an unknown request id");
            return;
        };

        if let Some(error) = value.get("error") {
            warn!(request_id, local_id, %error, "subscription rejected");
            if let Some(waiter) = shared.waiters.lock().await.remove(&local_id) {
                let _ = waiter.send(Err(BotError::ws(error.to_string())));
            } else if let Some(sub) = shared.subs.read().await.get(&local_id) {
                let _ = sub.tx.send(WsMessage::Error {
                    message: error.to_string(),
                });
            }
            shared.subs.write().await.remove(&local_id);
            return;
        }

        let Some(server_id) = value.get("result").and_then(|v| v.as_u64()) else {
            warn!(request_id, local_id, "subscribe response had no result");
            return;
        };
        shared.by_server.write().await.insert(server_id, local_id);
        debug!(request_id, local_id, server_id, "subscribed");

        if let Some(waiter) = shared.waiters.lock().await.remove(&local_id) {
            let _ = waiter.send(Ok(server_id));
        } else if let Some(sub) = shared.subs.read().await.get(&local_id) {
            // Re-established after a reconnect: tell the consumer.
            let _ = sub.tx.send(WsMessage::Status(WsStatus::Connected));
        }
        return;
    }

    // ---- notification: {"method":"…Notification","params":{"subscription":N,…}}
    let Some(method) = value.get("method").and_then(|m| m.as_str()) else {
        return;
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let server_id = params.get("subscription").and_then(|s| s.as_u64());
    let result = params.get("result").cloned().unwrap_or(Value::Null);

    let message = match method {
        "logsNotification" => parse_logs(server_id, &result),
        "accountNotification" => parse_account(server_id, &result),
        "transactionNotification" => WsMessage::Transaction {
            subscription: server_id.unwrap_or(0),
            signature: result
                .get("signature")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string(),
            slot: result.get("slot").and_then(|s| s.as_u64()).unwrap_or(0),
            raw: result,
        },
        other => {
            debug!(method = other, "unhandled websocket notification");
            WsMessage::Raw {
                subscription: server_id,
                value: result,
            }
        }
    };

    let local_id = match server_id {
        Some(id) => shared.by_server.read().await.get(&id).copied(),
        None => None,
    };
    let subs = shared.subs.read().await;
    match local_id.and_then(|l| subs.get(&l)) {
        Some(sub) => {
            let _ = sub.tx.send(message);
        }
        None => {
            // Unmapped notification (e.g. it arrived before the subscribe
            // response was processed). Fan out rather than silently drop it:
            // losing a launch event costs far more than a duplicate.
            if matches!(
                message,
                WsMessage::Logs { .. } | WsMessage::Transaction { .. }
            ) {
                for sub in subs.values() {
                    let _ = sub.tx.send(message.clone());
                }
            }
        }
    }
}

fn parse_logs(server_id: Option<u64>, result: &Value) -> WsMessage {
    let value = result.get("value").unwrap_or(result);
    WsMessage::Logs {
        subscription: server_id.unwrap_or(0),
        signature: value
            .get("signature")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        err: value.get("err").filter(|e| !e.is_null()).cloned(),
        logs: value
            .get("logs")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn parse_account(server_id: Option<u64>, result: &Value) -> WsMessage {
    let value = result.get("value").unwrap_or(result);
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.as_str())
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .unwrap_or_default();
    WsMessage::Account {
        subscription: server_id.unwrap_or(0),
        pubkey: value
            .get("pubkey")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        lamports: value.get("lamports").and_then(|l| l.as_u64()).unwrap_or(0),
        owner: value
            .get("owner")
            .and_then(|o| o.as_str())
            .unwrap_or_default()
            .to_string(),
        data,
        slot: result.get("slot").and_then(|s| s.as_u64()).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::PUMP_PROGRAM_ID;

    #[test]
    fn logs_filter_serialises_to_the_rpc_shape() {
        let f = LogsFilter {
            mentions: vec![PUMP_PROGRAM_ID.to_string()],
            ..Default::default()
        };
        let p = f.to_params();
        assert_eq!(p[0]["mentions"][0], PUMP_PROGRAM_ID.to_string());
        assert_eq!(p[1]["commitment"], "processed");
        assert_eq!(p[1]["includeVotes"], false);

        assert_eq!(LogsFilter::default().to_params()[0], json!("all"));

        let mention_all = LogsFilter {
            mention_all: vec!["Program log: Instruction: Buy".into()],
            ..Default::default()
        }
        .to_params();
        assert!(mention_all[0]["mentionsAll"].is_array());
    }

    #[test]
    fn transaction_filter_omits_empty_lists() {
        let p = TransactionFilter {
            account_include: vec!["abc".into()],
            ..Default::default()
        }
        .to_params();
        assert_eq!(p[0]["accountInclude"][0], "abc");
        assert!(p[0].get("accountExclude").is_none());
        assert_eq!(p[1]["transactionDetails"], "full");
        assert_eq!(p[1]["maxSupportedTransactionVersion"], 0);
    }

    #[test]
    fn account_filter_requests_base64() {
        let p = AccountFilter {
            pubkey: "abc".into(),
            commitment: None,
        }
        .to_params();
        assert_eq!(p[0], json!("abc"));
        assert_eq!(p[1]["encoding"], "base64");
    }

    #[test]
    fn parses_a_logs_notification() {
        let result = json!({
            "context": {"slot": 1234},
            "value": {
                "signature": "5xYz...",
                "err": null,
                "logs": ["Program 6EF8rrec invoke [1]", "Program log: Instruction: Create"]
            }
        });
        match parse_logs(Some(42), &result) {
            WsMessage::Logs {
                subscription,
                signature,
                err,
                logs,
            } => {
                assert_eq!(subscription, 42);
                assert_eq!(signature, "5xYz...");
                assert!(err.is_none(), "a null err must not become Some");
                assert_eq!(logs.len(), 2);
                assert!(logs[1].contains("Create"));
            }
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[test]
    fn failed_logs_notification_keeps_the_error() {
        let result = json!({
            "value": { "signature": "abc", "err": {"InstructionError":[0,"Custom"]}, "logs": [] }
        });
        match parse_logs(Some(1), &result) {
            WsMessage::Logs { err, .. } => assert!(err.is_some()),
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[test]
    fn parses_an_account_notification_with_base64_data() {
        let payload = vec![1u8, 2, 3, 4];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
        let result = json!({
            "slot": 99,
            "value": {
                "lamports": 1234,
                "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
                "data": [b64, "base64"],
                "pubkey": "mintxyz"
            }
        });
        match parse_account(Some(7), &result) {
            WsMessage::Account {
                subscription,
                pubkey,
                lamports,
                data,
                slot,
                ..
            } => {
                assert_eq!(subscription, 7);
                assert_eq!(pubkey, "mintxyz");
                assert_eq!(lamports, 1234);
                assert_eq!(slot, 99);
                assert_eq!(data, payload);
            }
            other => panic!("expected Account, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_response_maps_request_to_server_id() {
        let shared = Shared::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (wtx, wrx) = tokio::sync::oneshot::channel();

        shared.subs.write().await.insert(
            1,
            Subscription {
                method: "logsSubscribe".into(),
                params: json!([]),
                tx,
            },
        );
        shared.by_request.write().await.insert(11, 1);
        shared.waiters.lock().await.insert(1, wtx);

        handle_text(&shared, r#"{"jsonrpc":"2.0","id":11,"result":999}"#).await;

        assert_eq!(wrx.await.unwrap().unwrap(), 999);
        assert_eq!(shared.by_server.read().await.get(&999).copied(), Some(1));
        assert!(shared.by_request.read().await.is_empty());
        assert!(shared.waiters.lock().await.is_empty());

        // A notification for server id 999 must reach the subscriber.
        let notif = json!({
            "method": "logsNotification",
            "params": {"subscription": 999, "result": {"value": {"signature": "sig1", "err": null, "logs": ["l"]}}}
        });
        handle_text(&shared, &notif.to_string()).await;
        match rx.recv().await.unwrap() {
            WsMessage::Logs { signature, .. } => assert_eq!(signature, "sig1"),
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejected_subscription_reports_the_error_and_deregisters() {
        let shared = Shared::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (wtx, wrx) = tokio::sync::oneshot::channel();
        shared.subs.write().await.insert(
            5,
            Subscription {
                method: "transactionSubscribe".into(),
                params: json!([]),
                tx,
            },
        );
        shared.by_request.write().await.insert(50, 5);
        shared.waiters.lock().await.insert(5, wtx);

        handle_text(
            &shared,
            r#"{"jsonrpc":"2.0","id":50,"error":{"code":-32601,"message":"Method not found"}}"#,
        )
        .await;

        assert!(wrx.await.unwrap().is_err());
        assert!(
            shared.subs.read().await.is_empty(),
            "a rejected subscription must be removed"
        );
        // No waiter remains, so a second rejection notifies the channel.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn disconnect_clears_server_ids_but_keeps_subscriptions() {
        let shared = Shared::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        shared.subs.write().await.insert(
            3,
            Subscription {
                method: "logsSubscribe".into(),
                params: json!([]),
                tx,
            },
        );
        shared.by_server.write().await.insert(777, 3);
        shared.connected.store(true, Ordering::SeqCst);

        shared.on_disconnect().await;

        assert!(!shared.is_connected());
        assert!(shared.by_server.read().await.is_empty());
        // The registration survives so the supervisor can resubscribe it.
        assert_eq!(shared.subs.read().await.len(), 1);
        match rx.recv().await.unwrap() {
            WsMessage::Status(WsStatus::Reconnecting) => {}
            other => panic!("expected Reconnecting, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_outgoing_skips_subscriptions_already_sent() {
        let shared = Shared::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        for id in 1..=3u64 {
            shared.subs.write().await.insert(
                id,
                Subscription {
                    method: "logsSubscribe".into(),
                    params: json!([]),
                    tx: tx.clone(),
                },
            );
        }
        // id 2 is already live on the server, id 3 is in flight.
        shared.by_server.write().await.insert(88, 2);
        shared.by_request.write().await.insert(99, 3);

        let out = register_outgoing(&shared).await;
        assert_eq!(out.len(), 1, "only subscription 1 needs sending");
        assert_eq!(out[0].1, "logsSubscribe");
        assert_eq!(
            shared.by_request.read().await.get(&out[0].0).copied(),
            Some(1)
        );

        // Calling again must not re-queue it.
        assert!(register_outgoing(&shared).await.is_empty());
    }

    #[tokio::test]
    async fn unmapped_logs_notifications_fan_out_instead_of_vanishing() {
        let shared = Shared::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        shared.subs.write().await.insert(
            1,
            Subscription {
                method: "logsSubscribe".into(),
                params: json!([]),
                tx,
            },
        );
        // No by_server entry: the notification arrives before the response.
        let notif = json!({
            "method": "logsNotification",
            "params": {"subscription": 12345, "result": {"value": {"signature": "early", "err": null, "logs": []}}}
        });
        handle_text(&shared, &notif.to_string()).await;
        match rx.recv().await.unwrap() {
            WsMessage::Logs { signature, .. } => assert_eq!(signature, "early"),
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_removes_the_subscription() {
        let ws = SolanaWs::new("wss://example.invalid");
        let (tx, _rx) = mpsc::unbounded_channel();
        ws.shared().subs.write().await.insert(
            9,
            Subscription {
                method: "logsSubscribe".into(),
                params: json!([]),
                tx,
            },
        );
        ws.shared().by_server.write().await.insert(1, 9);

        let mut handle = SubscriptionHandle {
            local_id: 9,
            method: "logsSubscribe".into(),
            rx: Some(mpsc::unbounded_channel().1),
            shared: ws.shared(),
        };
        let _ = handle.receiver();
        assert_eq!(ws.subscription_count().await, 1);
        handle.cancel().await;
        assert_eq!(ws.subscription_count().await, 0);
    }
}
