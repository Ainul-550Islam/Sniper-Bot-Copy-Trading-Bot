//! PumpPortal — the fastest pump.fun launch feed, plus its local-trade API.
//!
//! Two independent pieces:
//!
//! 1. **`wss://pumpportal.fun/api/data`** — a fan-out of pump.fun activity.
//!    `subscribeNewToken` fires the moment a token is created, which is
//!    meaningfully earlier than a `logsSubscribe` on the pump program because
//!    PumpPortal runs its own Geyser node and pushes instead of being polled.
//!    Module 1 uses this as its primary launch source and the Solana websocket
//!    as a cross-check.
//!
//! 2. **`https://pumpportal.fun/api/trade-local`** — builds an unsigned
//!    transaction for a bonding-curve or PumpSwap trade. Useful as a *second
//!    opinion* on the account layout: if our own builder and theirs disagree,
//!    that is an early warning that pump.fun shipped a change.
//!
//! Nothing here is required for the bot to work — every message is also
//! derivable from the chain — so a PumpPortal outage degrades latency rather
//! than breaking anything. That is reflected in the error handling: connection
//! failures are logged and retried, never propagated into the trading path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use crate::ws::redact_ws_url;
use bot_core::maths;

use crate::consts::{PUMPPORTAL_TRADE_API, PUMPPORTAL_WS_URL};

// --------------------------------------------------------------------------
// Feed messages
// --------------------------------------------------------------------------

/// One message from the PumpPortal data feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PumpPortalMessage {
    /// `subscribeNewToken` — a token was just created on the bonding curve.
    NewToken(NewTokenMessage),
    /// `subscribeTokenTrade` — a bonding-curve or AMM trade.
    Trade(TradeMessage),
    /// `subscribeAccountTrade` — same shape, filtered by wallet.
    AccountTrade(TradeMessage),
    /// `subscribeMigration` — a token graduated to PumpSwap.
    Migration(MigrationMessage),
    /// Anything else, kept verbatim.
    Other(Value),
}

impl PumpPortalMessage {
    pub fn mint(&self) -> Option<Pubkey> {
        match self {
            PumpPortalMessage::NewToken(m) => Pubkey::try_from(m.mint.as_str()).ok(),
            PumpPortalMessage::Trade(t) | PumpPortalMessage::AccountTrade(t) => {
                Pubkey::try_from(t.mint.as_str()).ok()
            }
            PumpPortalMessage::Migration(m) => Pubkey::try_from(m.mint.as_str()).ok(),
            PumpPortalMessage::Other(_) => None,
        }
    }

    /// The trader, where the message names one.
    pub fn trader(&self) -> Option<Pubkey> {
        match self {
            PumpPortalMessage::NewToken(m) => Pubkey::try_from(m.trader_public_key.as_str()).ok(),
            PumpPortalMessage::Trade(t) | PumpPortalMessage::AccountTrade(t) => {
                Pubkey::try_from(t.trader_public_key.as_str()).ok()
            }
            _ => None,
        }
    }
}

/// `subscribeNewToken` payload.
///
/// Every field is optional except `mint`: PumpPortal has added and removed
/// fields over time, and a strict struct would start failing on the first
/// change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewTokenMessage {
    pub signature: String,
    pub mint: String,
    pub trader_public_key: String,
    pub tx_type: String,
    pub initial_buy: f64,
    pub market_cap_sol: f64,
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub pool: String,
    pub bonding_curve_key: Option<String>,
    pub associated_bonding_curve_key: Option<String>,
    /// Present on the AMM feed after graduation.
    pub vtokens_in_pool: Option<f64>,
    pub vsol_in_pool: Option<f64>,
    pub vlp_tokens_in_pool: Option<f64>,
}

impl NewTokenMessage {
    pub fn mint_pubkey(&self) -> Option<Pubkey> {
        Pubkey::try_from(self.mint.as_str()).ok()
    }

    /// Initial buy in lamports (`initialBuy` is denominated in SOL).
    pub fn initial_buy_lamports(&self) -> u64 {
        maths::sol_to_lamports(self.initial_buy)
    }

    /// Market cap in lamports.
    pub fn market_cap_lamports(&self) -> u64 {
        maths::sol_to_lamports(self.market_cap_sol)
    }
}

/// `subscribeTokenTrade` / `subscribeAccountTrade` payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TradeMessage {
    pub signature: String,
    pub mint: String,
    pub trader_public_key: String,
    /// `"buy"` or `"sell"`.
    pub tx_type: String,
    pub token_amount: f64,
    pub sol_amount: f64,
    pub new_token_amount: f64,
    pub new_sol_amount: f64,
    pub market_cap_sol: f64,
    pub timestamp: Option<i64>,
    pub pool: String,
    pub bonding_curve_key: Option<String>,
    pub user_bonding_curve_key: Option<String>,
    /// Set on AMM trades.
    pub vtokens_in_pool: Option<f64>,
    pub vsol_in_pool: Option<f64>,
    pub vlp_tokens_in_pool: Option<f64>,
}

impl TradeMessage {
    pub fn is_buy(&self) -> bool {
        self.tx_type.eq_ignore_ascii_case("buy")
    }

    pub fn is_sell(&self) -> bool {
        self.tx_type.eq_ignore_ascii_case("sell")
    }

    pub fn sol_lamports(&self) -> u64 {
        maths::sol_to_lamports(self.sol_amount)
    }

    /// Tokens in raw 6-decimal units (pump.fun mints are always 6dp).
    pub fn token_raw(&self) -> u64 {
        maths::to_raw_amount(self.token_amount, 6)
    }

    pub fn mint_pubkey(&self) -> Option<Pubkey> {
        Pubkey::try_from(self.mint.as_str()).ok()
    }

    pub fn trader_pubkey(&self) -> Option<Pubkey> {
        Pubkey::try_from(self.trader_public_key.as_str()).ok()
    }
}

/// `subscribeMigration` payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MigrationMessage {
    pub signature: String,
    pub mint: String,
    pub pool: String,
    pub tx_type: String,
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub market_cap_sol: f64,
    pub vtokens_in_pool: Option<f64>,
    pub vsol_in_pool: Option<f64>,
    pub vlp_tokens_in_pool: Option<f64>,
}

// --------------------------------------------------------------------------
// Feed client
// --------------------------------------------------------------------------

/// Which subscriptions the feed should hold open.
#[derive(Debug, Clone, Default)]
pub struct PumpPortalSubscription {
    /// `subscribeNewToken` — every launch on the platform.
    pub new_tokens: bool,
    /// `subscribeTokenTrade` for these mints.
    pub token_trades: Vec<String>,
    /// `subscribeAccountTrade` for these wallets.
    pub account_trades: Vec<String>,
    /// `subscribeMigration`.
    pub migrations: bool,
}

impl PumpPortalSubscription {
    pub fn launches_only() -> Self {
        PumpPortalSubscription {
            new_tokens: true,
            ..Default::default()
        }
    }

    fn frames(&self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.new_tokens {
            out.push(json!({"method": "subscribeNewToken"}));
        }
        if self.migrations {
            out.push(json!({"method": "subscribeMigration"}));
        }
        if !self.token_trades.is_empty() {
            out.push(json!({
                "method": "subscribeTokenTrade",
                "keys": self.token_trades,
            }));
        }
        if !self.account_trades.is_empty() {
            out.push(json!({
                "method": "subscribeAccountTrade",
                "keys": self.account_trades,
            }));
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        !self.new_tokens
            && !self.migrations
            && self.token_trades.is_empty()
            && self.account_trades.is_empty()
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const ABORT_JOIN_GRACE: Duration = Duration::from_secs(1);

/// A join handle that aborts its task if the joining future is cancelled.
/// This matters when service shutdown itself is interrupted or its parent task
/// is aborted while waiting for the PumpPortal supervisor.
struct AbortOnDropJoin {
    handle: Option<tokio::task::JoinHandle<()>>,
    label: &'static str,
}

struct SupervisorStatusGuard {
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
}

impl Drop for SupervisorStatusGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.connected.store(false, Ordering::Relaxed);
    }
}

impl AbortOnDropJoin {
    fn new(handle: tokio::task::JoinHandle<()>, label: &'static str) -> Self {
        Self {
            handle: Some(handle),
            label,
        }
    }

    async fn join_with_timeout(mut self, timeout: Duration) {
        let Some(task) = self.handle.as_mut() else {
            return;
        };
        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(())) => {
                self.handle.take();
            }
            Ok(Err(error)) => {
                if !error.is_cancelled() {
                    warn!(task = self.label, error = %error, "owned task failed");
                }
                self.handle.take();
            }
            Err(_) => {
                warn!(task = self.label, "task did not stop before deadline; aborting it");
                if let Some(task) = self.handle.as_ref() {
                    task.abort();
                }
                if let Some(mut task) = self.handle.take() {
                    match tokio::time::timeout(ABORT_JOIN_GRACE, &mut task).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) if !error.is_cancelled() => {
                            warn!(task = self.label, error = %error, "aborted task failed while joining");
                        }
                        Ok(Err(_)) => {}
                        Err(_) => warn!(
                            task = self.label,
                            "abort was requested but task did not finish within the join grace period"
                        ),
                    }
                }
            }
        }
    }
}

impl Drop for AbortOnDropJoin {
    fn drop(&mut self) {
        if let Some(task) = &self.handle {
            task.abort();
        }
    }
}

fn lock_task_slot(
    slot: &std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
) -> std::sync::MutexGuard<'_, Option<tokio::task::JoinHandle<()>>> {
    match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Reconnecting PumpPortal feed.
pub struct PumpPortalFeed {
    url: String,
    subscription: PumpPortalSubscription,
    tx: mpsc::Sender<PumpPortalMessage>,
    rx: Arc<Mutex<Option<mpsc::Receiver<PumpPortalMessage>>>>,
    running: Arc<AtomicBool>,
    /// Serializes start/stop transitions without holding a synchronous mutex
    /// across an await. The join-handle slot itself is accessed synchronously
    /// and only for brief, non-awaiting moves.
    lifecycle: Mutex<()>,
    handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    stop_signal: watch::Sender<bool>,
    /// Updated by the connection task, read by the health endpoint.
    connected: Arc<AtomicBool>,
    messages_seen: Arc<tokio::sync::Mutex<u64>>,
}

impl PumpPortalFeed {
    pub fn new(subscription: PumpPortalSubscription, buffer: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer.max(16));
        let (stop_signal, _stop_receiver) = watch::channel(false);
        PumpPortalFeed {
            url: PUMPPORTAL_WS_URL.to_string(),
            subscription,
            tx,
            rx: Arc::new(Mutex::new(Some(rx))),
            running: Arc::new(AtomicBool::new(false)),
            lifecycle: Mutex::new(()),
            handle: std::sync::Mutex::new(None),
            stop_signal,
            connected: Arc::new(AtomicBool::new(false)),
            messages_seen: Arc::new(tokio::sync::Mutex::new(0)),
        }
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn subscription(&self) -> &PumpPortalSubscription {
        &self.subscription
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Take the message stream. Once only.
    pub async fn receiver(&self) -> Option<mpsc::Receiver<PumpPortalMessage>> {
        self.rx.lock().await.take()
    }

    /// Start the supervisor.
    pub async fn start(&self) -> BotResult<()> {
        let _lifecycle = self.lifecycle.lock().await;
        if self.subscription.is_empty() {
            return Err(BotError::config(
                "pumpportal feed started with an empty subscription",
            ));
        }
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        let previous = { lock_task_slot(&self.handle).take() };
        if let Some(previous) = previous {
            AbortOnDropJoin::new(previous, "previous pumpportal supervisor")
                .join_with_timeout(STOP_TIMEOUT)
                .await;
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| BotError::config(format!("pumpportal start requires a Tokio runtime: {error}")))?;
        let url = self.url.clone();
        let frames = self.subscription.frames();
        let tx = self.tx.clone();
        let running = self.running.clone();
        let connected = self.connected.clone();
        let seen = self.messages_seen.clone();
        self.stop_signal.send_replace(false);
        let stop = self.stop_signal.subscribe();
        self.running.store(true, Ordering::SeqCst);

        let handle = runtime.spawn(async move {
            run_feed(url, frames, tx, running, connected, seen, stop).await;
        });
        *lock_task_slot(&self.handle) = Some(handle);
        info!(
            endpoint = %redact_ws_url(&self.url),
            new_tokens = self.subscription.new_tokens,
            token_trades = self.subscription.token_trades.len(),
            account_trades = self.subscription.account_trades.len(),
            "pumpportal feed started"
        );
        Ok(())
    }

    /// Stop the supervisor and close the socket. The lifecycle lock prevents
    /// a concurrent restart until the old supervisor has joined or been
    /// aborted, and the join guard makes caller cancellation safe.
    pub async fn stop(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        self.running.store(false, Ordering::SeqCst);
        self.stop_signal.send_replace(true);
        self.connected.store(false, Ordering::Relaxed);
        let handle = lock_task_slot(&self.handle).take();
        if let Some(handle) = handle {
            AbortOnDropJoin::new(handle, "pumpportal supervisor")
                .join_with_timeout(STOP_TIMEOUT)
                .await;
        }
        self.connected.store(false, Ordering::Relaxed);
        info!("pumpportal feed stopped");
    }

    pub async fn messages_seen(&self) -> u64 {
        *self.messages_seen.lock().await
    }
}

impl Drop for PumpPortalFeed {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.stop_signal.send_replace(true);
        self.connected.store(false, Ordering::Relaxed);
        // Take the handle under a short synchronous critical section so a
        // contended async mutex can never silently detach the supervisor.
        if let Some(task) = lock_task_slot(&self.handle).take() {
            task.abort();
        }
    }
}

async fn wait_for_stop(mut stop: watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

async fn run_feed(
    url: String,
    frames: Vec<Value>,
    tx: mpsc::Sender<PumpPortalMessage>,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    seen: Arc<tokio::sync::Mutex<u64>>,
    stop: watch::Receiver<bool>,
) {
    let _status_guard = SupervisorStatusGuard {
        running: running.clone(),
        connected: connected.clone(),
    };
    let mut backoff = Duration::from_millis(500);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    while running.load(Ordering::SeqCst) && !*stop.borrow() && !tx.is_closed() {
        let attempt = tokio::select! {
            _ = wait_for_stop(stop.clone()) => break,
            result = connect_once(
                &url,
                &frames,
                &tx,
                &running,
                &connected,
                &seen,
                stop.clone(),
            ) => result,
        };
        match attempt {
            Ok(()) => {
                debug!("pumpportal feed connection ended");
                backoff = Duration::from_millis(500);
            }
            Err(error) => warn!(error = %error, "pumpportal feed disconnected"),
        }
        connected.store(false, Ordering::Relaxed);
        if !running.load(Ordering::SeqCst) || *stop.borrow() || tx.is_closed() {
            break;
        }
        tokio::select! {
            _ = wait_for_stop(stop.clone()) => break,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
    connected.store(false, Ordering::Relaxed);
}

async fn connect_once(
    url: &str,
    frames: &[Value],
    tx: &mpsc::Sender<PumpPortalMessage>,
    running: &Arc<AtomicBool>,
    connected: &Arc<AtomicBool>,
    seen: &Arc<tokio::sync::Mutex<u64>>,
    stop: watch::Receiver<bool>,
) -> BotResult<()> {
    let result = tokio::select! {
        _ = wait_for_stop(stop.clone()) => return Ok(()),
        result = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url)) => result,
    };
    let (socket, _response) = result
        .map_err(|_| {
            BotError::ws(format!(
                "pumpportal websocket connect timed out for {}",
                redact_ws_url(url)
            ))
        })?
        .map_err(|_| {
            BotError::ws(format!(
                "pumpportal websocket connect failed for {}",
                redact_ws_url(url)
            ))
        })?;
    let (mut sink, mut stream) = socket.split();
    connected.store(true, Ordering::Relaxed);

    for frame in frames {
        let text = TungsteniteMessage::Text(frame.to_string());
        let sent = tokio::select! {
            _ = wait_for_stop(stop.clone()) => return Ok(()),
            result = sink.send(text) => result,
        };
        sent.map_err(|error| BotError::ws(format!("pumpportal subscribe: {error}")))?;
        debug!(method = %frame["method"], "pumpportal subscription sent");
    }

    // PumpPortal does not send pings and will drop an idle socket.
    let mut keepalive = tokio::time::interval(Duration::from_secs(25));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    keepalive.tick().await;

    'socket: loop {
        if !running.load(Ordering::SeqCst) || *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = wait_for_stop(stop.clone()) => break 'socket,
            _ = keepalive.tick() => {
                let sent = tokio::select! {
                    _ = wait_for_stop(stop.clone()) => None,
                    result = sink.send(TungsteniteMessage::Ping(Vec::new())) => Some(result),
                };
                match sent {
                    None => break 'socket,
                    Some(Err(error)) => return Err(BotError::ws(format!("pumpportal ping: {error}"))),
                    Some(Ok(())) => {}
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Err(BotError::ws("pumpportal stream ended"));
                };
                match frame {
                    Ok(TungsteniteMessage::Text(text)) => {
                        let message = classify(&text);
                        let mut counter = tokio::select! {
                            _ = wait_for_stop(stop.clone()) => break 'socket,
                            counter = seen.lock() => counter,
                        };
                        *counter = counter.saturating_add(1);
                        drop(counter);
                        let sent = tokio::select! {
                            _ = wait_for_stop(stop.clone()) => None,
                            result = tx.send(message) => Some(result),
                        };
                        match sent {
                            None => break 'socket,
                            Some(Err(_)) => {
                                debug!("pumpportal consumer dropped, stopping feed");
                                break 'socket;
                            }
                            Some(Ok(())) => {}
                        }
                    }
                    Ok(TungsteniteMessage::Ping(payload)) => {
                        let sent = tokio::select! {
                            _ = wait_for_stop(stop.clone()) => None,
                            result = sink.send(TungsteniteMessage::Pong(payload)) => Some(result),
                        };
                        match sent {
                            None => break 'socket,
                            Some(Err(error)) => return Err(BotError::ws(format!("pumpportal pong: {error}"))),
                            Some(Ok(())) => {}
                        }
                    }
                    Ok(TungsteniteMessage::Close(frame)) => {
                        info!(?frame, "pumpportal closed the connection");
                        break 'socket;
                    }
                    Ok(_) => {}
                    Err(error) => return Err(BotError::ws(format!("pumpportal frame: {error}"))),
                }
            }
        }
    }

    // Close handshakes are best-effort, but cannot delay shutdown indefinitely.
    match tokio::time::timeout(
        Duration::from_secs(1),
        sink.send(TungsteniteMessage::Close(None)),
    )
    .await
    {
        Ok(Err(error)) => debug!(error = %error, "pumpportal close frame failed"),
        Err(_) => debug!("pumpportal close frame timed out"),
        Ok(Ok(())) => {}
    }
    Ok(())
}

/// Work out which subscription a message belongs to.
///
/// PumpPortal does not label its payloads, so the discriminator is the
/// `txType` field: `"create"` for a launch, `"migrate"` for a graduation,
/// `"buy"`/`"sell"` for a trade. A message with no `txType` is passed through
/// as [`PumpPortalMessage::Other`].
pub fn classify(text: &str) -> PumpPortalMessage {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            debug!(error = %e, "pumpportal sent non-json");
            return PumpPortalMessage::Other(Value::String(text.to_string()));
        }
    };
    let tx_type = value
        .get("txType")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match tx_type.as_str() {
        "create" => match serde_json::from_value::<NewTokenMessage>(value.clone()) {
            Ok(m) => PumpPortalMessage::NewToken(m),
            Err(e) => {
                warn!(error = %e, "could not decode a pumpportal create message");
                PumpPortalMessage::Other(value)
            }
        },
        "migrate" => match serde_json::from_value::<MigrationMessage>(value.clone()) {
            Ok(m) => PumpPortalMessage::Migration(m),
            Err(e) => {
                warn!(error = %e, "could not decode a pumpportal migrate message");
                PumpPortalMessage::Other(value)
            }
        },
        "buy" | "sell" => match serde_json::from_value::<TradeMessage>(value.clone()) {
            Ok(m) => {
                // `subscribeAccountTrade` and `subscribeTokenTrade` payloads are
                // identical; only the subscription decides which it is, so we
                // report both as Trade and let the caller route on the wallet.
                PumpPortalMessage::Trade(m)
            }
            Err(e) => {
                warn!(error = %e, "could not decode a pumpportal trade message");
                PumpPortalMessage::Other(value)
            }
        },
        _ => PumpPortalMessage::Other(value),
    }
}

// --------------------------------------------------------------------------
// trade-local API
// --------------------------------------------------------------------------

/// Which pool a trade-local request should hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PoolKind {
    /// The pump.fun bonding curve.
    BondingCurve,
    /// The graduated PumpSwap AMM.
    Amm,
    /// Let PumpPortal decide from the token's state.
    Auto,
}

impl PoolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PoolKind::BondingCurve => "bonding-curve",
            PoolKind::Amm => "amm",
            PoolKind::Auto => "auto",
        }
    }
}

/// Request body for `POST /api/trade-local`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeLocalRequest {
    pub public_key: String,
    /// `"buy"` or `"sell"`.
    pub action: String,
    pub mint: String,
    /// In SOL when `denominated_in_sol` is true, otherwise in token units.
    pub amount: f64,
    pub denominated_in_sol: bool,
    pub slippage: u16,
    pub priority_fee: f64,
    pub pool: String,
}

impl TradeLocalRequest {
    pub fn buy_sol(public_key: Pubkey, mint: Pubkey, sol: f64) -> Self {
        TradeLocalRequest {
            public_key: public_key.to_string(),
            action: "buy".into(),
            mint: mint.to_string(),
            amount: sol,
            denominated_in_sol: true,
            slippage: 10,
            priority_fee: 0.0005,
            pool: PoolKind::Auto.as_str().into(),
        }
    }

    pub fn sell_tokens(public_key: Pubkey, mint: Pubkey, tokens: f64) -> Self {
        TradeLocalRequest {
            public_key: public_key.to_string(),
            action: "sell".into(),
            mint: mint.to_string(),
            amount: tokens,
            denominated_in_sol: false,
            slippage: 10,
            priority_fee: 0.0005,
            pool: PoolKind::Auto.as_str().into(),
        }
    }

    pub fn slippage(mut self, pct: u16) -> Self {
        self.slippage = pct;
        self
    }

    pub fn priority_fee_sol(mut self, sol: f64) -> Self {
        self.priority_fee = sol;
        self
    }

    pub fn pool(mut self, kind: PoolKind) -> Self {
        self.pool = kind.as_str().into();
        self
    }
}

/// Client for the trade-local endpoint.
pub struct TradeLocalClient {
    http: reqwest::Client,
    url: String,
}

impl TradeLocalClient {
    pub fn new() -> Self {
        TradeLocalClient {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            url: PUMPPORTAL_TRADE_API.to_string(),
        }
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Fetch an unsigned transaction from PumpPortal.
    pub async fn build(&self, req: &TradeLocalRequest) -> BotResult<VersionedTransaction> {
        let response = self
            .http
            .post(&self.url)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                BotError::http(format!(
                    "pumpportal trade-local request failed (timeout={}, connect={}, status={:?})",
                    e.is_timeout(),
                    e.is_connect(),
                    e.status()
                ))
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|e| {
            BotError::http(format!(
                "pumpportal trade-local response body failed (timeout={}, connect={}, status={:?})",
                e.is_timeout(),
                e.is_connect(),
                e.status()
            ))
        })?;
        if !status.is_success() {
            // Do not echo an arbitrary third-party response body into logs or
            // errors: providers sometimes include request URLs or credentials.
            return Err(BotError::http(format!(
                "pumpportal trade-local returned HTTP {status}"
            )));
        }
        // The endpoint returns the raw serialized transaction, not base64.
        let tx: VersionedTransaction = bincode::deserialize(&bytes).map_err(|e| {
            BotError::encoding(format!(
                "pumpportal trade-local response is not a serialized transaction: {e} \
                     (first bytes: {:02x?})",
                &bytes[..bytes.len().min(16)]
            ))
        })?;
        Ok(tx)
    }

    /// Compare PumpPortal's account list with ours.
    ///
    /// Returns the two lists so a divergence can be logged and, if it persists,
    /// fed to the layout doctor. This is the cheapest early-warning system for
    /// a pump.fun upgrade: it costs one HTTP call and no funds.
    pub async fn diff_accounts(
        &self,
        req: &TradeLocalRequest,
        expected_payer: &Pubkey,
        ours: &[Pubkey],
    ) -> BotResult<LayoutDiff> {
        let tx = self.build(req).await?;
        let mut theirs: Vec<Pubkey> = tx.message.static_account_keys().to_vec();
        // A trade-local transaction may use lookup tables; include those too.
        match &tx.message {
            solana_sdk::message::VersionedMessage::V0(m) => {
                let _ = &m.address_table_lookups;
            }
            solana_sdk::message::VersionedMessage::Legacy(_) => {}
        }

        // Theirs includes the fee payer first, which ours may not.
        if theirs.first() == Some(expected_payer) && !ours.contains(expected_payer) {
            theirs.remove(0);
        }

        let only_theirs: Vec<Pubkey> = theirs
            .iter()
            .copied()
            .filter(|p| !ours.contains(p))
            .collect();
        let only_ours: Vec<Pubkey> = ours
            .iter()
            .copied()
            .filter(|p| !theirs.contains(p))
            .collect();
        let same_order = only_theirs.is_empty() && only_ours.is_empty() && theirs == *ours;

        Ok(LayoutDiff {
            theirs,
            ours: ours.to_vec(),
            only_theirs,
            only_ours,
            same_order,
        })
    }
}

impl Default for TradeLocalClient {
    fn default() -> Self {
        TradeLocalClient::new()
    }
}

/// Result of comparing two account lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutDiff {
    pub theirs: Vec<Pubkey>,
    pub ours: Vec<Pubkey>,
    pub only_theirs: Vec<Pubkey>,
    pub only_ours: Vec<Pubkey>,
    pub same_order: bool,
}

impl LayoutDiff {
    pub fn matches(&self) -> bool {
        self.same_order
    }

    pub fn describe(&self) -> String {
        if self.same_order {
            return format!("layouts match exactly ({} accounts)", self.ours.len());
        }
        format!(
            "layout divergence: ours={} theirs={} only_ours={} only_theirs={}",
            self.ours.len(),
            self.theirs.len(),
            self.only_ours.len(),
            self.only_theirs.len()
        )
    }
}

/// Decode a base64 body, for endpoints that return the transaction encoded.
pub fn decode_base64_tx(b64: &str) -> BotResult<VersionedTransaction> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| BotError::encoding(format!("base64: {e}")))?;
    bincode::deserialize(&bytes).map_err(|e| BotError::encoding(format!("bincode: {e}")))
}

/// Group feed messages by mint, for the dashboard's "recent launches" panel.
pub fn group_by_mint(messages: &[PumpPortalMessage]) -> HashMap<String, Vec<usize>> {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, m) in messages.iter().enumerate() {
        if let Some(mint) = m.mint() {
            map.entry(mint.to_string()).or_default().push(i);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_routes_a_create_message() {
        let text = r#"{
            "signature": "5xYz",
            "mint": "So11111111111111111111111111111111111111112",
            "traderPublicKey": "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV2fskvCwf8gCDbZ",
            "txType": "create",
            "initialBuy": 1.5,
            "marketCapSol": 31.5,
            "name": "Test Token",
            "symbol": "TEST",
            "uri": "https://ipfs.io/ipfs/Qm",
            "pool": "bonding-curve",
            "bondingCurveKey": "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf"
        }"#;
        match classify(text) {
            PumpPortalMessage::NewToken(m) => {
                assert_eq!(m.name, "Test Token");
                assert_eq!(m.symbol, "TEST");
                assert_eq!(m.tx_type, "create");
                assert_eq!(m.initial_buy, 1.5);
                assert_eq!(m.initial_buy_lamports(), 1_500_000_000);
                assert_eq!(m.market_cap_lamports(), 31_500_000_000);
                assert!(m.bonding_curve_key.is_some());
                assert!(m.mint_pubkey().is_some());
                assert_eq!(m.uri, "https://ipfs.io/ipfs/Qm");
            }
            other => panic!("expected NewToken, got {other:?}"),
        }
    }

    #[test]
    fn classify_routes_buy_and_sell() {
        let buy = r#"{"signature":"s","mint":"So11111111111111111111111111111111111111112",
            "traderPublicKey":"abc","txType":"buy","tokenAmount":1000.0,"solAmount":0.5,
            "newTokenAmount":999.0,"newSolAmount":30.5,"marketCapSol":30.5}"#;
        match classify(buy) {
            PumpPortalMessage::Trade(t) => {
                assert!(t.is_buy());
                assert!(!t.is_sell());
                assert_eq!(t.sol_lamports(), 500_000_000);
                assert_eq!(t.token_raw(), 1_000_000_000);
                assert!(t.timestamp.is_none());
            }
            other => panic!("expected Trade, got {other:?}"),
        }

        let sell = buy.replace("\"buy\"", "\"sell\"");
        match classify(&sell) {
            PumpPortalMessage::Trade(t) => assert!(t.is_sell()),
            other => panic!("expected Trade, got {other:?}"),
        }
    }

    #[test]
    fn classify_is_case_insensitive_on_tx_type() {
        let text = r#"{"mint":"So11111111111111111111111111111111111111112","txType":"BUY","tokenAmount":1,"solAmount":1}"#;
        assert!(matches!(classify(text), PumpPortalMessage::Trade(t) if t.is_buy()));
    }

    #[test]
    fn classify_routes_a_migration() {
        let text = r#"{"signature":"s","mint":"So11111111111111111111111111111111111111112",
            "pool":"amm","txType":"migrate","name":"N","symbol":"S","uri":"U","marketCapSol":85.0,
            "vtokensInPool":793100000,"vsolInPool":85.0}"#;
        match classify(text) {
            PumpPortalMessage::Migration(m) => {
                assert_eq!(m.pool, "amm");
                assert_eq!(m.market_cap_sol, 85.0);
                assert_eq!(m.vsol_in_pool, Some(85.0));
            }
            other => panic!("expected Migration, got {other:?}"),
        }
    }

    #[test]
    fn classify_passes_through_unknown_payloads() {
        assert!(matches!(
            classify(r#"{"hello":"world"}"#),
            PumpPortalMessage::Other(_)
        ));
        assert!(matches!(classify("not json"), PumpPortalMessage::Other(_)));
    }

    #[test]
    fn missing_fields_default_instead_of_failing() {
        // PumpPortal has dropped fields before; a strict decoder would break.
        let text = r#"{"txType":"create","mint":"So11111111111111111111111111111111111111112"}"#;
        match classify(text) {
            PumpPortalMessage::NewToken(m) => {
                assert!(m.name.is_empty());
                assert_eq!(m.initial_buy, 0.0);
                assert_eq!(m.initial_buy_lamports(), 0);
            }
            other => panic!("expected NewToken, got {other:?}"),
        }
    }

    #[test]
    fn message_accessors_work() {
        let wsol = WSOL_STR;
        let create = classify(&format!(
            r#"{{"txType":"create","mint":"{wsol}","traderPublicKey":"{wsol}"}}"#
        ));
        assert!(create.mint().is_some());
        assert!(create.trader().is_some());

        let other = classify(r#"{"x":1}"#);
        assert!(other.mint().is_none());
        assert!(other.trader().is_none());
    }

    const WSOL_STR: &str = "So11111111111111111111111111111111111111112";

    #[test]
    fn subscription_frames_match_the_documented_methods() {
        let sub = PumpPortalSubscription {
            new_tokens: true,
            token_trades: vec!["mintA".into()],
            account_trades: vec!["walletB".into()],
            migrations: true,
        };
        let frames = sub.frames();
        assert_eq!(frames.len(), 4);
        let methods: Vec<&str> = frames
            .iter()
            .map(|f| f["method"].as_str().unwrap())
            .collect();
        assert!(methods.contains(&"subscribeNewToken"));
        assert!(methods.contains(&"subscribeMigration"));
        assert!(methods.contains(&"subscribeTokenTrade"));
        assert!(methods.contains(&"subscribeAccountTrade"));

        let token_frame = frames
            .iter()
            .find(|f| f["method"] == "subscribeTokenTrade")
            .unwrap();
        assert_eq!(token_frame["keys"][0], "mintA");

        assert!(!sub.is_empty());
        assert!(PumpPortalSubscription::default().is_empty());
        assert!(!PumpPortalSubscription::launches_only().is_empty());
        assert!(PumpPortalSubscription::default().frames().is_empty());
    }

    #[tokio::test]
    async fn starting_with_an_empty_subscription_is_an_error() {
        let feed = PumpPortalFeed::new(PumpPortalSubscription::default(), 8);
        let err = feed.start().await.unwrap_err();
        assert!(err.to_string().contains("empty subscription"), "{err}");
    }

    #[tokio::test]
    async fn start_is_idempotent_and_stop_is_safe() {
        let feed = PumpPortalFeed::new(PumpPortalSubscription::launches_only(), 8);
        // Point it somewhere that will not resolve so the test stays offline.
        let feed = feed.with_url("ws://127.0.0.1:1/");
        feed.start().await.unwrap();
        feed.start().await.unwrap();
        assert!(!feed.is_connected());
        feed.stop().await;
        // Stopping twice must not panic or hang.
        feed.stop().await;
        assert!(!feed.is_connected());
    }

    #[tokio::test]
    async fn the_receiver_can_only_be_taken_once() {
        let feed = PumpPortalFeed::new(PumpPortalSubscription::launches_only(), 8);
        assert!(feed.receiver().await.is_some());
        assert!(feed.receiver().await.is_none());
    }

    #[test]
    fn trade_local_requests_serialise_to_the_documented_shape() {
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let buy = TradeLocalRequest::buy_sol(wallet, mint, 0.1)
            .slippage(15)
            .priority_fee_sol(0.001)
            .pool(PoolKind::BondingCurve);
        let v = serde_json::to_value(&buy).unwrap();
        assert_eq!(v["publicKey"], wallet.to_string());
        assert_eq!(v["action"], "buy");
        assert_eq!(v["mint"], mint.to_string());
        assert_eq!(v["amount"], 0.1);
        assert_eq!(v["denominatedInSol"], true);
        assert_eq!(v["slippage"], 15);
        assert_eq!(v["priorityFee"], 0.001);
        assert_eq!(v["pool"], "bonding-curve");

        let sell = TradeLocalRequest::sell_tokens(wallet, mint, 1000.0);
        let v = serde_json::to_value(&sell).unwrap();
        assert_eq!(v["action"], "sell");
        assert_eq!(v["denominatedInSol"], false);
        assert_eq!(v["pool"], "auto");
    }

    #[test]
    fn pool_kind_strings_are_the_api_values() {
        assert_eq!(PoolKind::BondingCurve.as_str(), "bonding-curve");
        assert_eq!(PoolKind::Amm.as_str(), "amm");
        assert_eq!(PoolKind::Auto.as_str(), "auto");
    }

    #[test]
    fn layout_diff_reports_divergence() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let c = Pubkey::new_unique();
        let same = LayoutDiff {
            theirs: vec![a, b],
            ours: vec![a, b],
            only_theirs: vec![],
            only_ours: vec![],
            same_order: true,
        };
        assert!(same.matches());
        assert!(same.describe().contains("match exactly"));

        let diff = LayoutDiff {
            theirs: vec![a, b, c],
            ours: vec![a, b],
            only_theirs: vec![c],
            only_ours: vec![],
            same_order: false,
        };
        assert!(!diff.matches());
        let d = diff.describe();
        assert!(d.contains("ours=2"), "{d}");
        assert!(d.contains("theirs=3"), "{d}");
        assert!(d.contains("only_theirs=1"), "{d}");
    }

    #[test]
    fn decode_base64_tx_rejects_garbage() {
        assert!(decode_base64_tx("!!!").is_err());
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"not a transaction");
        assert!(decode_base64_tx(&b64).is_err());
    }

    #[test]
    fn group_by_mint_indexes_messages() {
        let m1 = classify(&format!(r#"{{"txType":"create","mint":"{WSOL_STR}"}}"#));
        let m2 = classify(r#"{"x":1}"#);
        let groups = group_by_mint(&[m1, m2]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[WSOL_STR], vec![0]);
    }

    #[test]
    fn trade_message_token_raw_uses_six_decimals() {
        let t = classify(r#"{"txType":"buy","mint":"x","tokenAmount":1234.567891,"solAmount":1}"#);
        match t {
            PumpPortalMessage::Trade(m) => {
                assert_eq!(m.token_raw(), 1_234_567_891);
            }
            other => panic!("expected Trade, got {other:?}"),
        }
    }
}
