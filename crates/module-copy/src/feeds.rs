//! Copy-trading feeds: turn tracked-wallet activity into [`WalletTrade`]s.
//!
//! Three sources are supported, selected by `copy.feed`:
//! * `pumpportal` — `subscribeAccountTrade` over the PumpPortal websocket. Fast
//!   and cheap, but only sees trades PumpPortal indexes (pump.fun / PumpSwap /
//!   Raydium) and carries no slot.
//! * `transaction_subscribe` — Geyser/Yellowstone push feed over
//!   `network.geyser_ws_url`: the transaction arrives the moment it is
//!   processed, with the full base64 payload, and goes through the same
//!   [`decode_swap`] pipeline as polling. Lowest-latency source; degrades to
//!   `logs_poll` when the endpoint is missing or rejects the subscription.
//! * `logs_poll` — `getSignaturesForAddress` + `getTransaction` +
//!   [`decode_swap`]. Works on any RPC and yields exact amounts, at the cost
//!   of a poll interval.
//!
//! All feeds push into one channel. Two different marks exist and they must
//! never be confused (TASK 3 §03):
//!
//! * **Fetch suppression, here.** The polling and Geyser paths call
//!   `AppState::mark_signature_seen` (namespace `sig`) once per transaction
//!   they *decode*, so a signature that both the poll loop and the
//!   post-reconnect backfill (or two feeds) discover is fetched and decoded
//!   only once. This is an RPC-saving guard, not a trading decision: it says
//!   nothing about whether the trade was mirrored.
//! * **The one authoritative dedup, in the pipeline.** Every event a feed
//!   hands over is marked seen exactly once — by `event_dedup::claim` inside
//!   `CopyBot::process_event` — through `AppState::mark_copy_event_seen`
//!   (the durable dedup facade, namespace `copy_event`, key
//!   `copy:{signature}:{leader}:{mint}:{side}`). That mark is the decision
//!   record; a redelivery of the same leader trade is refused as
//!   `DUPLICATE_EVENT` there and nowhere else.
//!
//! The consumer (`lib.rs` run loop) therefore never re-marks the `sig`
//! namespace: before TASK 3 it did, so every polled / Geyser event — already
//! marked by its feed — was dropped as a duplicate before it could be
//! mirrored (the PumpPortal feed, which does not pre-mark, was unaffected,
//! which is why the regression went unnoticed). The PumpPortal path does not
//! mark `sig` at all: PumpPortal delivers each trade once and its messages
//! are not re-fetched.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::ha::FeedId;
use bot_core::maths;
use bot_core::models::{BotModule, PositionSide, Venue, WalletTrade};
use bot_core::state::Shared;

use solana_kit::consts::{PUMPPORTAL_WS_URL, WSOL_MINT};
use solana_kit::decode::{
    decode_swap, parse_transaction_notification, DecodedSwap, Side, SwapVenue,
};
use solana_kit::pumpportal::{
    PumpPortalFeed, PumpPortalMessage, PumpPortalSubscription, TradeMessage,
};
use solana_kit::rpc::{Rpc, SignatureInfo};
use solana_kit::ws::{redact_ws_url, SolanaWs, TransactionFilter, WsMessage, WsPolicy};

/// Buffer for the merged output channel.
const OUT_BUFFER: usize = 512;
/// Buffer for the PumpPortal websocket reader.
const PP_BUFFER: usize = 512;

const FEED_TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const FEED_TASK_ABORT_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct FeedTasks {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl FeedTasks {
    fn push(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.handles.push(handle);
    }

    async fn join_all(&mut self) {
        let join_deadline = tokio::time::Instant::now() + FEED_TASK_JOIN_TIMEOUT;
        let abort_deadline = join_deadline + FEED_TASK_ABORT_GRACE;
        let mut aborting = false;

        // Keep handles in the owner while awaiting. If this shutdown future is
        // cancelled, Drop still has access to every worker and can abort them.
        for task in &mut self.handles {
            if aborting {
                abort_copy_task(task, abort_deadline).await;
                continue;
            }
            let remaining = join_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                aborting = true;
                abort_copy_task(task, abort_deadline).await;
                continue;
            }
            match tokio::time::timeout(remaining, &mut *task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(error = %error, "copy feed task failed while joining");
                }
                Err(_) => {
                    warn!("copy feed task did not stop before deadline; aborting it");
                    aborting = true;
                    abort_copy_task(task, abort_deadline).await;
                }
            }
        }
        self.handles.clear();
    }
}

async fn abort_copy_task(task: &mut tokio::task::JoinHandle<()>, deadline: tokio::time::Instant) {
    task.abort();
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        warn!("copy feed abort join grace elapsed; cancellation was requested");
        return;
    }
    match tokio::time::timeout(remaining, &mut *task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if !error.is_cancelled() => {
            warn!(error = %error, "aborted copy feed task failed while joining");
        }
        Ok(Err(_)) => {}
        Err(_) => warn!("copy feed task did not finish during abort join grace"),
    }
}

impl Drop for FeedTasks {
    fn drop(&mut self) {
        // Dropping the owning feed handle (e.g. parent future cancellation)
        // must not detach a network/RPC task.
        for task in &self.handles {
            task.abort();
        }
    }
}

/// Point-in-time health snapshot of the copy feed.
///
/// `reconnects` is only reported for the `transaction_subscribe` feed: it
/// counts websocket outage gaps that were recovered after a reconnect (this
/// also includes queue-overflow gaps). The PumpPortal client reconnects
/// internally and exposes no counter, so there it is `None`. `lag_ms` is the
/// delay between the chain `block_time` and the consumer receiving the most
/// recent trade. Unmeasured values are `None`, never fabricated.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FeedHealth {
    /// Configured feed kind: `idle`, `pumpportal`, `transaction_subscribe`, or `logs_poll`.
    pub source: &'static str,
    /// Wall-clock time the consumer last received a leader trade via `recv()`.
    pub last_event_at: Option<DateTime<Utc>>,
    /// Trades currently queued and not yet consumed.
    pub queued: usize,
    /// True when feed tasks were started but all have finished (feed is down).
    pub degraded: bool,
    /// Recovered websocket gaps (reconnect recoveries); `transaction_subscribe` only.
    pub reconnects: Option<u64>,
    /// Chain-to-consumer delay of the most recent trade, in milliseconds.
    pub lag_ms: Option<u64>,
}

/// Owns the merged copy stream and every task feeding it.
#[derive(Debug)]
pub struct CopyFeedHandle {
    receiver: mpsc::Receiver<WalletTrade>,
    tasks: FeedTasks,
    /// Keeps an empty-wallet feed idle without a detached keepalive task.
    keepalive: Option<mpsc::Sender<WalletTrade>>,
    source: &'static str,
    last_event_at: Option<DateTime<Utc>>,
    last_lag_ms: Option<u64>,
    /// Incremented by the geyser task on every recovered websocket gap.
    gap_count: Arc<AtomicU64>,
}

impl CopyFeedHandle {
    /// Receive the next leader trade, or `None` once every feed has closed.
    pub async fn recv(&mut self) -> Option<WalletTrade> {
        let trade = self.receiver.recv().await;
        if let Some(t) = &trade {
            let now = Utc::now();
            self.last_event_at = Some(now);
            if let Some(block_time) = t.block_time {
                self.last_lag_ms = Some((now - block_time).num_milliseconds().max(0) as u64);
            }
        }
        trade
    }

    /// Snapshot of feed health for status endpoints and monitoring.
    pub fn health(&self) -> FeedHealth {
        let degraded = match self.keepalive {
            // Idle feed (no wallets): not degraded, it is intentionally inactive.
            Some(_) => false,
            None => {
                !self.tasks.handles.is_empty() && self.tasks.handles.iter().all(|h| h.is_finished())
            }
        };
        let reconnects = (self.source == "transaction_subscribe")
            .then(|| self.gap_count.load(Ordering::Relaxed));
        FeedHealth {
            source: self.source,
            last_event_at: self.last_event_at,
            queued: self.len(),
            degraded,
            reconnects,
            lag_ms: self.last_lag_ms,
        }
    }

    /// Number of leader trades currently queued in the channel.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    /// True when no leader trade is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Close the stream and join every feed task within one bounded deadline.
    pub async fn shutdown(&mut self) {
        self.receiver.close();
        self.keepalive.take();
        self.tasks.join_all().await;
    }
}

/// Spawns the configured copy feed(s).
pub struct CopyFeed;

impl CopyFeed {
    /// Start the feed tasks and return their joined receiver owner.
    pub async fn spawn(state: Shared, rpc: Rpc) -> BotResult<CopyFeedHandle> {
        let cfg = state.config_snapshot().await;
        let wallets: Vec<String> = cfg
            .copy
            .wallets
            .iter()
            .map(|w| w.address.trim().to_string())
            .filter(|w| !w.is_empty())
            .collect();

        let (tx, rx) = mpsc::channel(OUT_BUFFER);
        let gap_count = Arc::new(AtomicU64::new(0));

        if wallets.is_empty() {
            // Nothing tracked yet. Keep the channel open (so `run` stays alive
            // and picks up wallets added via hot reload) using the returned
            // handle itself rather than a detached keepalive task.
            warn!("copy feed: no wallets configured — idling");
            return Ok(CopyFeedHandle {
                receiver: rx,
                tasks: FeedTasks::default(),
                keepalive: Some(tx),
                source: "idle",
                last_event_at: None,
                last_lag_ms: None,
                gap_count,
            });
        }

        let mut tasks = FeedTasks::default();
        let feed = cfg.copy.feed.trim().to_ascii_lowercase();
        let poll_ms = cfg.copy.poll_interval_ms.clamp(500, 60_000);
        let sig_limit = cfg.copy.poll_signature_limit.clamp(1, 100);
        let url = if cfg.sniper.pumpportal_ws_url.trim().is_empty() {
            PUMPPORTAL_WS_URL.to_string()
        } else {
            cfg.sniper.pumpportal_ws_url.clone()
        };

        let wallet_count = wallets.len();
        let mut started = 0;
        let mut source: &'static str = "logs_poll";
        match feed.as_str() {
            "pumpportal" => {
                let tx = tx.clone();
                let state = state.clone();
                let wallets = wallets.clone();
                let url = url.clone();
                tasks.push(tokio::spawn(async move {
                    run_pumpportal(url, wallets, tx, state).await
                }));
                started += 1;
                source = "pumpportal";
                info!(
                    wallets = wallet_count,
                    "copy feed: pumpportal account-trades"
                );
            }
            "transaction_subscribe" => {
                // Geyser/Yellowstone push feed (BUILD PLAN §5). Requires a
                // geyser WS endpoint; without one (or if the endpoint does
                // not speak transactionSubscribe) we degrade to polling,
                // which is correct on any RPC.
                let geyser = cfg
                    .network
                    .geyser_ws_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .map(str::to_string);
                let tx = tx.clone();
                let state = state.clone();
                let wallets = wallets.clone();
                let rpc = rpc.clone();
                let gaps = gap_count.clone();
                match geyser {
                    Some(url) => {
                        tasks.push(tokio::spawn(async move {
                            run_transaction_subscribe(
                                rpc,
                                url,
                                wallets,
                                tx,
                                state,
                                PollTuning { poll_ms, sig_limit },
                                gaps,
                            )
                            .await
                        }));
                        source = "transaction_subscribe";
                        info!(
                            wallets = wallet_count,
                            "copy feed: transaction_subscribe (geyser push)"
                        );
                    }
                    None => {
                        warn!("copy feed: transaction_subscribe requires network.geyser_ws_url — using logs_poll");
                        tasks.push(tokio::spawn(async move {
                            run_poll(rpc, wallets, tx, state, poll_ms, sig_limit).await
                        }));
                    }
                }
                started += 1;
            }
            _ => {
                // "logs_poll" and anything else.
                let tx = tx.clone();
                let state = state.clone();
                let wallets = wallets.clone();
                let rpc = rpc.clone();
                tasks.push(tokio::spawn(async move {
                    run_poll(rpc, wallets, tx, state, poll_ms, sig_limit).await
                }));
                started += 1;
                info!(wallets = wallet_count, poll_ms, "copy feed: logs_poll");
            }
        }

        if started == 0 {
            return Err(BotError::config("no copy feed could be started"));
        }

        drop(tx);
        Ok(CopyFeedHandle {
            receiver: rx,
            tasks,
            keepalive: None,
            source,
            last_event_at: None,
            last_lag_ms: None,
            gap_count,
        })
    }
}

/// PumpPortal `subscribeAccountTrade` forwarder.
async fn run_pumpportal(
    url: String,
    wallets: Vec<String>,
    out: mpsc::Sender<WalletTrade>,
    state: Shared,
) {
    let sub = PumpPortalSubscription {
        account_trades: wallets,
        ..Default::default()
    };
    let feed = PumpPortalFeed::new(sub, PP_BUFFER).with_url(url);
    if let Err(e) = feed.start().await {
        warn!(error = %e, "copy pumpportal feed failed to start");
        return;
    }
    let mut rx = match feed.receiver().await {
        Some(rx) => rx,
        None => {
            warn!("copy pumpportal receiver already taken");
            feed.stop().await;
            return;
        }
    };
    // Keep the feed alive for the lifetime of this task.
    let feed = feed;
    loop {
        let message = tokio::select! {
            _ = out.closed() => {
                debug!("copy consumer gone; stopping pumpportal forwarder");
                break;
            }
            _ = state.wait_shutdown() => break,
            message = rx.recv() => message,
        };
        let Some(message) = message else { break };
        let trade = match message {
            PumpPortalMessage::AccountTrade(t) | PumpPortalMessage::Trade(t) => {
                wallet_trade_from_pumpportal(&t)
            }
            _ => continue,
        };
        state.heartbeat(BotModule::Copy).await;
        if out.send(trade).await.is_err() {
            debug!("copy consumer gone; stopping pumpportal forwarder");
            break;
        }
    }
    feed.stop().await;
}

/// RPC polling forwarder: signatures → transactions → decoded swaps.
async fn run_poll(
    rpc: Rpc,
    wallets: Vec<String>,
    out: mpsc::Sender<WalletTrade>,
    state: Shared,
    poll_ms: u64,
    sig_limit: usize,
) {
    let tracked: Vec<(String, Pubkey)> = wallets
        .into_iter()
        .filter_map(|w| match Pubkey::from_str(&w) {
            Ok(pk) => Some((w, pk)),
            Err(e) => {
                warn!(wallet = %w, error = %e, "copy poll: invalid wallet address, skipping");
                None
            }
        })
        .collect();
    if tracked.is_empty() {
        return;
    }

    // Warm-up (TASK 6 §4): resume from the DURABLE cursor when this wallet
    // was polled before — a restart must not silently skip the trades that
    // happened while the process was down. Only when no durable position
    // exists do we seed with the current newest signature, so a brand-new
    // wallet does not replay its whole history.
    let mut cursors: HashMap<String, Option<Signature>> = HashMap::new();
    for (wstr, wkey) in &tracked {
        let durable = state
            .ha()
            .cursor(FeedId::CopyLogs, wstr)
            .await
            .token
            .and_then(|t| Signature::from_str(&t).ok());
        if let Some(sig) = durable {
            info!(wallet = %wstr, cursor = %sig, "copy poll resuming from the durable cursor");
            cursors.insert(wstr.clone(), Some(sig));
            continue;
        }
        match rpc.signatures_for_address(wkey, 1, None).await {
            Ok(sigs) => {
                cursors.insert(wstr.clone(), sigs.first().map(|s| s.signature));
            }
            Err(e) => {
                warn!(wallet = %wstr, error = %e, "copy poll warm-up failed");
                cursors.insert(wstr.clone(), None);
            }
        }
    }

    let mut ticker = tokio::time::interval(Duration::from_millis(poll_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {},
            _ = out.closed() => return,
            _ = state.wait_shutdown() => return,
        }
        let cfg = state.config_snapshot().await;
        if !cfg.copy.enabled {
            continue;
        }
        for (wstr, wkey) in &tracked {
            let before = cursors.get(wstr).copied().flatten();
            let sigs = match rpc.signatures_for_address(wkey, sig_limit, before).await {
                Ok(s) => s,
                Err(e) => {
                    debug!(wallet = %wstr, error = %e, "copy poll signatures failed");
                    continue;
                }
            };
            // Newest-first. Walk them, emitting decoded swaps.
            for info in &sigs {
                if emit_signature(&rpc, &state, &out, wkey, info).await == Emit::ConsumerGone {
                    debug!("copy consumer gone; stopping poll forwarder");
                    return;
                }
            }
            if let Some(newest) = sigs.first() {
                cursors.insert(wstr.clone(), Some(newest.signature));
                // TASK 6 — persist the position so a restart resumes here
                // instead of at "now"; the offer also suppresses a repeated
                // head and feeds the cursor-lag gauge.
                state
                    .ha()
                    .offer_token(
                        FeedId::CopyLogs,
                        wstr,
                        &newest.signature.to_string(),
                        newest
                            .block_time
                            .and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
                    )
                    .await;
            }
        }
    }
}

/// Outcome of [`emit_signature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emit {
    /// A decoded swap was handed to the consumer.
    Emitted,
    /// Failed tx, already seen, not a swap, or unreadable — nothing sent.
    Skipped,
    /// The consumer hung up.
    ConsumerGone,
}

/// Fetch, decode and emit ONE confirmed signature for `wkey` — shared by the
/// poll loop and the post-reconnect backfill so both produce identical
/// trades and go through the same fetch suppression (`sig` namespace). The
/// emitted trade is NOT yet decided: the pipeline's authoritative dedup
/// (`copy_event` namespace) marks it exactly once when it is processed.
async fn emit_signature(
    rpc: &Rpc,
    state: &Shared,
    out: &mpsc::Sender<WalletTrade>,
    wkey: &Pubkey,
    info: &SignatureInfo,
) -> Emit {
    if info.err.is_some() {
        return Emit::Skipped; // failed transaction
    }
    if !state.mark_signature_seen(&info.signature.to_string()).await {
        // `false` = already fetched/decoded by this process (poll loop,
        // backfill or another feed) — fetch suppression only; the
        // pipeline's `copy_event` mark is untouched by this call.
        return Emit::Skipped;
    }
    let confirmed = match rpc.get_transaction(&info.signature).await {
        Ok(Some(c)) => c,
        Ok(None) => return Emit::Skipped,
        Err(e) => {
            debug!(sig = %info.signature, error = %e, "copy poll getTransaction failed");
            return Emit::Skipped;
        }
    };
    let Some(meta) = confirmed.transaction.meta.clone() else {
        return Emit::Skipped;
    };
    match decode_swap(
        wkey,
        &info.signature.to_string(),
        confirmed.slot,
        confirmed.block_time,
        &confirmed.transaction.transaction,
        &meta,
    ) {
        Ok(Some(swap)) => {
            let trade = wallet_trade_from_decoded(&swap);
            state.heartbeat(BotModule::Copy).await;
            if out.send(trade).await.is_err() {
                return Emit::ConsumerGone;
            }
            Emit::Emitted
        }
        Ok(None) => Emit::Skipped,
        Err(e) => {
            debug!(sig = %info.signature, error = %e, "copy decode failed");
            Emit::Skipped
        }
    }
}

/// Slack added around a websocket outage when deciding which signatures the
/// backfill may replay: covers clock skew and the last notification that was
/// in flight when the socket died.
const BACKFILL_SLACK: Duration = Duration::from_secs(5);

/// Missed-event recovery for the push feed: after a websocket outage, walk
/// every tracked wallet's recent signatures and emit the ones that landed
/// inside the outage window. The window is bounded by `block_time` (never by
/// a bare `limit`) so a long outage can never replay pre-startup history, and
/// everything still goes through `mark_signature_seen` (fetch suppression),
/// so a trade the socket did deliver is not decoded and handed over twice;
/// should one slip through anyway, the pipeline's authoritative dedup
/// refuses the second copy. Returns `false` when the consumer is gone.
async fn backfill_after_gap(
    rpc: &Rpc,
    state: &Shared,
    out: &mpsc::Sender<WalletTrade>,
    tracked: &[(String, Pubkey)],
    outage: Duration,
    sig_limit: usize,
) -> bool {
    let since_unix = (Utc::now()
        - chrono::Duration::from_std(outage + BACKFILL_SLACK)
            .unwrap_or_else(|_| chrono::Duration::seconds(60)))
    .timestamp();
    let mut recovered = 0usize;
    for (wstr, wkey) in tracked {
        let sigs = match rpc.signatures_for_address(wkey, sig_limit, None).await {
            Ok(s) => s,
            Err(e) => {
                debug!(wallet = %wstr, error = %e, "copy geyser backfill: signatures failed");
                continue;
            }
        };
        for info in sigs
            .iter()
            .filter(|i| i.block_time.is_some_and(|t| t >= since_unix))
        {
            match emit_signature(rpc, state, out, wkey, info).await {
                Emit::Emitted => recovered += 1,
                Emit::Skipped => {}
                Emit::ConsumerGone => return false,
            }
        }
    }
    bot_core::obs::metrics::global()
        .counter(
            "bot_ws_backfilled_events_total",
            "Events recovered by the post-reconnect backfill after a websocket outage.",
            &[("feed", "copy_transaction_subscribe")],
        )
        .inc_by(recovered as u64);
    info!(
        outage_ms = outage.as_millis() as u64,
        wallets = tracked.len(),
        recovered,
        "copy geyser: backfill after websocket gap complete"
    );
    true
}

/// Geyser `transactionSubscribe` forwarder (BUILD PLAN §5): whale
/// transactions are *pushed* by a Yellowstone-compatible websocket the moment
/// they are processed, instead of being discovered by the poll loop one
/// `poll_interval_ms` later. Notifications carry the full base64 transaction
/// plus meta — the exact inputs [`decode_swap`] already consumes — so the
/// decode path is shared with polling, including fetch suppression (a
/// transaction already decoded by another feed is not decoded again). The
/// once-only trading decision is the pipeline's `copy_event` mark.
///
/// Failure policy: an endpoint that does not speak `transactionSubscribe`,
/// or a stream that ends, degrades to [`run_poll`] — the copy feed must
/// never silently die.
/// Polling knobs shared by the poll loop and the geyser fallback.
#[derive(Debug, Clone, Copy)]
struct PollTuning {
    poll_ms: u64,
    sig_limit: usize,
}

async fn run_transaction_subscribe(
    rpc: Rpc,
    url: String,
    wallets: Vec<String>,
    out: mpsc::Sender<WalletTrade>,
    state: Shared,
    tuning: PollTuning,
    gaps: Arc<AtomicU64>,
) {
    let PollTuning { poll_ms, sig_limit } = tuning;
    let tracked: Vec<(String, Pubkey)> = wallets
        .iter()
        .filter_map(|w| match Pubkey::from_str(w) {
            Ok(pk) => Some((w.clone(), pk)),
            Err(e) => {
                warn!(wallet = %w, error = %e, "copy geyser: invalid wallet address, skipping");
                None
            }
        })
        .collect();
    if tracked.is_empty() {
        return;
    }

    let ws = SolanaWs::with_policy(
        url.clone(),
        WsPolicy::from_network(&state.config_snapshot().await.network),
    );
    let conn = ws.spawn();
    let filter = TransactionFilter {
        account_include: tracked.iter().map(|(_, k)| k.to_string()).collect(),
        ..Default::default()
    };
    let mut sub = match ws.transaction_subscribe(filter).await {
        Ok(sub) => sub,
        Err(e) => {
            warn!(
                error = %e,
                endpoint = %redact_ws_url(&url),
                "copy geyser: transactionSubscribe rejected (not a Geyser endpoint?) — falling back to logs_poll"
            );
            conn.shutdown().await;
            run_poll(rpc, wallets, out, state, poll_ms, sig_limit).await;
            return;
        }
    };

    let mut rx = match sub.receiver() {
        Ok(receiver) => receiver,
        Err(e) => {
            warn!(error = %e, "copy geyser: subscription receiver unavailable");
            sub.cancel().await;
            conn.shutdown().await;
            run_poll(rpc, wallets, out, state, poll_ms, sig_limit).await;
            return;
        }
    };
    let mut consumer_gone = false;
    loop {
        let msg = tokio::select! {
            _ = out.closed() => {
                consumer_gone = true;
                break;
            }
            _ = state.wait_shutdown() => {
                consumer_gone = true;
                break;
            }
            msg = rx.recv() => msg,
        };
        let Some(msg) = msg else { break };
        match msg {
            WsMessage::Transaction { signature, raw, .. } => {
                let cfg = state.config_snapshot().await;
                if !cfg.copy.enabled {
                    continue;
                }
                let Some(notif) = parse_transaction_notification(&raw) else {
                    debug!(sig = %signature, "copy geyser: unparseable notification, skipping");
                    continue;
                };
                if !notif.succeeded() {
                    continue; // failed on chain — nothing to copy
                }
                if !state.mark_signature_seen(&notif.signature).await {
                    // `false` = already decoded by another feed / the
                    // backfill — fetch suppression only (see module docs).
                    continue;
                }
                // The filter may match several tracked wallets (e.g. a
                // whale-to-whale transfer); decode against each and emit
                // every perspective that is an actual swap.
                for (wstr, wkey) in &tracked {
                    match decode_swap(
                        wkey,
                        &notif.signature,
                        notif.slot,
                        notif.block_time,
                        &notif.transaction,
                        &notif.meta,
                    ) {
                        Ok(Some(swap)) => {
                            let trade = wallet_trade_from_decoded(&swap);
                            state.heartbeat(BotModule::Copy).await;
                            if out.send(trade).await.is_err() {
                                debug!("copy consumer gone; stopping geyser forwarder");
                                consumer_gone = true;
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            debug!(sig = %notif.signature, wallet = %wstr, error = %e, "copy geyser decode failed")
                        }
                    }
                }
                if consumer_gone {
                    break;
                }
            }
            // Missed-event recovery: the socket was down for `outage_ms`;
            // pull what the tracked wallets did meanwhile through the poll
            // pipeline (bounded by the outage window, deduped by signature).
            WsMessage::Gap {
                outage_ms,
                dropped_messages,
                last_slot,
                ..
            } => {
                let cfg = state.config_snapshot().await;
                if !cfg.copy.enabled {
                    continue;
                }
                gaps.fetch_add(1, Ordering::Relaxed);
                if dropped_messages > 0 {
                    warn!(
                        ?last_slot,
                        outage_ms,
                        dropped_messages,
                        "copy Geyser consumer queue overflow; backfilling tracked-wallet signatures"
                    );
                }
                if !backfill_after_gap(
                    &rpc,
                    &state,
                    &out,
                    &tracked,
                    Duration::from_millis(outage_ms),
                    sig_limit,
                )
                .await
                {
                    consumer_gone = true;
                    break;
                }
            }
            WsMessage::Error { message } => {
                warn!(error = %message, "copy geyser: websocket error");
            }
            _ => {}
        }
    }

    sub.cancel().await;
    conn.shutdown().await;
    if consumer_gone {
        return;
    }
    // The stream ended while the consumer still listens (provider restart,
    // reconnect exhaustion): keep copying via polling rather than go dark.
    warn!("copy geyser: subscription stream ended — falling back to logs_poll");
    run_poll(rpc, wallets, out, state, poll_ms, sig_limit).await;
}

/// Convert a PumpPortal trade message into a [`WalletTrade`].
pub fn wallet_trade_from_pumpportal(t: &TradeMessage) -> WalletTrade {
    let side = if t.is_sell() {
        PositionSide::Short
    } else {
        PositionSide::Long
    };
    let venue = match t.pool.to_ascii_lowercase().as_str() {
        "pump-amm" | "pumpamm" | "pumpswap" | "pump-swap" => Venue::PumpSwap,
        p if p.contains("raydium") => Venue::RaydiumAmmV4,
        _ => Venue::PumpFun,
    };
    WalletTrade {
        wallet: t.trader_public_key.clone(),
        signature: t.signature.clone(),
        slot: 0,
        block_time: t.timestamp.and_then(|ts| DateTime::from_timestamp(ts, 0)),
        side,
        mint: t.mint.clone(),
        symbol: None,
        token_amount: t.token_amount,
        sol_amount: t.sol_amount,
        venue,
        fee_sol: 0.0,
        discriminator: None,
        observed_at: Utc::now(),
    }
}

/// Convert a decoded on-chain swap into a [`WalletTrade`].
pub fn wallet_trade_from_decoded(swap: &DecodedSwap) -> WalletTrade {
    let side = match swap.side {
        Side::Buy => PositionSide::Long,
        Side::Sell => PositionSide::Short,
    };
    let venue = match swap.venue {
        SwapVenue::PumpBondingCurve => Venue::PumpFun,
        SwapVenue::PumpSwap => Venue::PumpSwap,
        SwapVenue::RaydiumAmmV4 | SwapVenue::RaydiumCpmm => Venue::RaydiumAmmV4,
        SwapVenue::RaydiumClmm => Venue::RaydiumClmm,
        SwapVenue::Jupiter | SwapVenue::Other | SwapVenue::None => Venue::Jupiter,
    };
    // Only SOL-quoted swaps have a meaningful SOL size for copy sizing.
    let is_sol_quote = swap.quote_mint == WSOL_MINT.to_string();
    let sol_amount = if is_sol_quote {
        maths::lamports_to_sol(swap.quote_amount)
    } else {
        0.0
    };
    WalletTrade {
        wallet: swap.wallet.clone(),
        signature: swap.signature.clone(),
        slot: swap.slot,
        block_time: swap
            .block_time
            .and_then(|ts| DateTime::from_timestamp(ts, 0)),
        side,
        mint: swap.base_mint.clone(),
        symbol: None,
        token_amount: maths::from_raw_amount(swap.base_amount, swap.base_decimals),
        sol_amount,
        venue,
        fee_sol: maths::lamports_to_sol(swap.fee_lamports),
        discriminator: None,
        observed_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pumpportal_buy_maps_to_long_on_the_curve() {
        let t = TradeMessage {
            signature: "sig".into(),
            mint: "mint".into(),
            trader_public_key: "trader".into(),
            tx_type: "buy".into(),
            token_amount: 1000.0,
            sol_amount: 0.5,
            new_token_amount: 0.0,
            new_sol_amount: 0.0,
            market_cap_sol: 0.0,
            timestamp: Some(1_700_000_000),
            pool: "pump".into(),
            bonding_curve_key: None,
            user_bonding_curve_key: None,
            vtokens_in_pool: None,
            vsol_in_pool: None,
            vlp_tokens_in_pool: None,
        };
        let trade = wallet_trade_from_pumpportal(&t);
        assert_eq!(trade.side, PositionSide::Long);
        assert_eq!(trade.venue, Venue::PumpFun);
        assert!((trade.sol_amount - 0.5).abs() < 1e-9);
        assert!(trade.block_time.is_some());
    }

    #[test]
    fn pumpportal_sell_on_amm_maps_to_short_pumpswap() {
        let t = TradeMessage {
            signature: "sig".into(),
            mint: "mint".into(),
            trader_public_key: "trader".into(),
            tx_type: "sell".into(),
            token_amount: 10.0,
            sol_amount: 0.2,
            new_token_amount: 0.0,
            new_sol_amount: 0.0,
            market_cap_sol: 0.0,
            timestamp: None,
            pool: "pump-amm".into(),
            bonding_curve_key: None,
            user_bonding_curve_key: None,
            vtokens_in_pool: None,
            vsol_in_pool: None,
            vlp_tokens_in_pool: None,
        };
        let trade = wallet_trade_from_pumpportal(&t);
        assert_eq!(trade.side, PositionSide::Short);
        assert_eq!(trade.venue, Venue::PumpSwap);
        assert!(trade.block_time.is_none());
    }

    #[test]
    fn decoded_non_sol_quote_has_zero_sol_amount() {
        let swap = DecodedSwap {
            signature: "sig".into(),
            slot: 42,
            block_time: Some(1_700_000_000),
            wallet: "w".into(),
            side: Side::Buy,
            venue: SwapVenue::RaydiumAmmV4,
            base_mint: "base".into(),
            quote_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(), // USDC
            base_amount: 1_000_000,
            quote_amount: 5_000_000,
            base_decimals: 6,
            quote_decimals: 6,
            fee_lamports: 5000,
            price_quote_per_base: 5.0,
            programs: vec![],
            events: vec![],
            logs: vec![],
            succeeded: true,
            error: None,
        };
        let trade = wallet_trade_from_decoded(&swap);
        assert_eq!(trade.sol_amount, 0.0);
        assert_eq!(trade.venue, Venue::RaydiumAmmV4);
        assert!((trade.token_amount - 1.0).abs() < 1e-9);
    }

    #[test]
    fn decoded_sol_quote_converts_lamports() {
        let swap = DecodedSwap {
            signature: "sig".into(),
            slot: 7,
            block_time: None,
            wallet: "w".into(),
            side: Side::Sell,
            venue: SwapVenue::PumpSwap,
            base_mint: "base".into(),
            quote_mint: WSOL_MINT.to_string(),
            base_amount: 2_000_000,
            quote_amount: 1_500_000_000, // 1.5 SOL
            base_decimals: 6,
            quote_decimals: 9,
            fee_lamports: 0,
            price_quote_per_base: 0.0,
            programs: vec![],
            events: vec![],
            logs: vec![],
            succeeded: true,
            error: None,
        };
        let trade = wallet_trade_from_decoded(&swap);
        assert_eq!(trade.side, PositionSide::Short);
        assert!((trade.sol_amount - 1.5).abs() < 1e-9);
        assert_eq!(trade.venue, Venue::PumpSwap);
    }
}

#[cfg(test)]
mod feed_health_tests {
    use super::*;

    #[tokio::test]
    async fn idle_feed_reports_idle_and_not_degraded() {
        let (tx, rx) = mpsc::channel(4);
        let mut handle = CopyFeedHandle {
            receiver: rx,
            tasks: FeedTasks::default(),
            keepalive: Some(tx),
            source: "idle",
            last_event_at: None,
            last_lag_ms: None,
            gap_count: Arc::new(AtomicU64::new(0)),
        };
        let h = handle.health();
        assert_eq!(h.source, "idle");
        assert!(h.last_event_at.is_none());
        assert_eq!(h.queued, 0);
        assert!(!h.degraded);
        assert!(h.reconnects.is_none() && h.lag_ms.is_none());
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn finished_tasks_mark_feed_degraded_and_recv_sets_last_event() {
        let (tx, rx) = mpsc::channel(4);
        let mut tasks = FeedTasks::default();
        tasks.push(tokio::spawn(async {}));
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut handle = CopyFeedHandle {
            receiver: rx,
            tasks,
            keepalive: None,
            source: "logs_poll",
            last_event_at: None,
            last_lag_ms: None,
            gap_count: Arc::new(AtomicU64::new(0)),
        };
        assert!(handle.health().degraded);
        assert!(handle.health().last_event_at.is_none());
        drop(tx);
        assert!(handle.recv().await.is_none());
        assert!(handle.health().last_event_at.is_none());
    }

    #[tokio::test]
    async fn recv_records_event_time_and_chain_lag() {
        let (tx, rx) = mpsc::channel(4);
        let mut handle = CopyFeedHandle {
            receiver: rx,
            tasks: FeedTasks::default(),
            keepalive: None,
            source: "transaction_subscribe",
            last_event_at: None,
            last_lag_ms: None,
            gap_count: Arc::new(AtomicU64::new(3)),
        };
        let now = Utc::now();
        tx.send(WalletTrade {
            wallet: "w".into(),
            signature: "s".into(),
            slot: 1,
            block_time: Some(now - chrono::Duration::seconds(2)),
            side: PositionSide::Long,
            mint: "m".into(),
            symbol: None,
            token_amount: 1.0,
            sol_amount: 1.0,
            venue: Venue::PumpFun,
            fee_sol: 0.0,
            discriminator: None,
            observed_at: now,
        })
        .await
        .unwrap();
        assert!(handle.recv().await.is_some());
        let h = handle.health();
        assert!(h.last_event_at.is_some());
        let lag = h.lag_ms.expect("lag measured from block_time");
        assert!((2_000..5_000).contains(&lag), "lag was {lag}");
        assert_eq!(h.reconnects, Some(3));
        assert_eq!(h.source, "transaction_subscribe");
    }
}
