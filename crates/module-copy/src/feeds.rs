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
//! All feeds push into one channel. Polling and Geyser deliberately do not
//! mark a signature as processed before the pipeline: an early shared mark
//! could suppress another wallet, replica, or retry after a failed fetch.
//! This may repeat RPC/decode work and hand duplicate trades to the channel.
//! `CopyBot::process_event` applies its separate event-level dedup key
//! (`copy:{signature}:{leader}:{mint}:{side}`); its retention and fallback
//! limitations still apply. A channel send is not a durable processing ack.

use std::collections::{HashMap, HashSet};
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
    // was polled before, subject to provider history and handoff limits.
    // Only when no durable position
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
            let until = cursors.get(wstr).copied().flatten();
            // `before` pages toward older history. A saved newest signature
            // is an exclusive `until` boundary when polling for newer trades.
            let sigs = match poll_signature_batch(&rpc, wkey, sig_limit, until).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(wallet = %wstr, error = %e, "copy poll batch incomplete; cursor unchanged");
                    continue;
                }
            };
            // The provider supplies newest first; process older entries first
            // so strict event ordering does not reject the rest of the batch.
            let mut retry_batch = false;
            for info in sigs.iter().rev() {
                match emit_signature(&rpc, &state, &out, wkey, info).await {
                    Emit::ConsumerGone => {
                        debug!("copy consumer gone; stopping poll forwarder");
                        return;
                    }
                    Emit::Retry => {
                        warn!(wallet = %wstr, sig = %info.signature, "copy poll could not process signature; retaining cursor for retry");
                        retry_batch = true;
                        break;
                    }
                    Emit::Emitted | Emit::Skipped => {}
                }
            }
            if retry_batch {
                continue;
            }
            if let Some(newest) = sigs.first() {
                cursors.insert(wstr.clone(), Some(newest.signature));
                // TASK 6 — persist the position so a restart resumes here
                // instead of at "now"; the offer also suppresses a repeated
                // head and feeds the cursor-lag gauge. The advance is written
                // PENDING and only acknowledged once this batch has been
                // processed, so a crash mid-batch replays it instead of
                // skipping it.
                let advance = state
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
                if advance.should_process() {
                    let _ = state
                        .ha()
                        .acknowledge(
                            FeedId::CopyLogs,
                            wstr,
                            None,
                            Some(&newest.signature.to_string()),
                        )
                        .await;
                }
            }
        }
    }
}

// Bound memory and RPC work during a large outage. An incomplete scan must
// not advance the saved cursor: doing so would silently skip the older pages.
const MAX_POLL_SIGNATURE_PAGES: usize = 100;

/// Gather a complete bounded window before processing it. The RPC boundary is
/// exclusive. A full page requires another page even when it is the last one.
/// A broken provider that repeats page tokens fails closed, as does the page
/// cap; neither condition advances the caller's cursor.
async fn poll_signature_batch(
    rpc: &Rpc,
    wallet: &Pubkey,
    limit: usize,
    until: Option<Signature>,
) -> BotResult<Vec<SignatureInfo>> {
    let limit = limit.clamp(1, 1000);
    let mut before = None;
    let mut seen_page_ends = HashSet::new();
    let mut all = Vec::new();
    for _ in 0..MAX_POLL_SIGNATURE_PAGES {
        let page = rpc
            .signatures_for_address_range(wallet, limit, before, until)
            .await?;
        let complete = page.len() < limit;
        let next = page.last().map(|info| info.signature);
        all.extend(page);
        if complete {
            return Ok(all);
        }
        let next = next.expect("a full page is nonempty");
        if !seen_page_ends.insert(next) {
            return Err(BotError::rpc("copy poll provider repeated a page cursor"));
        }
        before = Some(next);
    }
    Err(BotError::rpc(
        "copy poll signature page cap reached; cursor retained",
    ))
}

/// Outcome of [`emit_signature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emit {
    /// A decoded swap was handed to the consumer.
    Emitted,
    /// Failed on-chain transaction or decoded non-swap — nothing sent.
    Skipped,
    /// Fetch, metadata or decode could not be resolved; retry the batch.
    Retry,
    /// The consumer hung up.
    ConsumerGone,
}

/// Fetch, decode and emit one confirmed signature for this wallet. Do not
/// pre-mark a signature: a failed fetch or competing wallet must not prevent
/// another attempt. Duplicate deliveries are filtered by the event pipeline.
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
    let confirmed = match rpc.get_transaction(&info.signature).await {
        Ok(Some(c)) => c,
        Ok(None) => return Emit::Retry,
        Err(e) => {
            debug!(sig = %info.signature, error = %e, "copy poll getTransaction failed");
            return Emit::Retry;
        }
    };
    let Some(meta) = confirmed.transaction.meta.clone() else {
        return Emit::Retry;
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
            Emit::Retry
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
/// signatures can be emitted again and the event pipeline performs dedup.
/// Fetch/decode failures are logged but this one-shot backfill has no durable
/// retry queue. Returns `false` when the consumer is gone.
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
                Emit::Retry => {
                    warn!(wallet = %wstr, sig = %info.signature, "copy geyser backfill could not process signature; no durable retry queued")
                }
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
/// decode path is shared with polling. Duplicate notifications may be
/// decoded again; the pipeline's `copy_event` mark filters repeats within
/// its retention and backend guarantees.
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
        SwapVenue::RaydiumAmmV4 => Venue::RaydiumAmmV4,
        SwapVenue::RaydiumCpmm => Venue::RaydiumCpmm,
        SwapVenue::RaydiumLaunchLab => Venue::RaydiumLaunchLab,
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

    // A transient null must not mark this signature, suppress a retry, or
    // prevent another tracked wallet from attempting its own perspective.
    #[tokio::test]
    async fn null_fetch_allows_retry_and_other_wallet() {
        use bot_core::config::{AppConfig, Config};
        use bot_core::state::AppState;
        use solana_sdk::signature::Signature;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let server_requests = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let count = server_requests.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buf = [0_u8; 4096];
                    loop {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..i]);
                            let len = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                            if bytes.len() >= i + 4 + len {
                                let request: serde_json::Value =
                                    serde_json::from_slice(&bytes[i + 4..i + 4 + len]).unwrap();
                                assert_eq!(request["method"], "getTransaction");
                                count.fetch_add(1, AtomicOrdering::SeqCst);
                                let body = serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":null}).to_string();
                                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                                stream.write_all(response.as_bytes()).await.unwrap();
                                return;
                            }
                        }
                    }
                });
            }
        });
        let mut cfg = Config::default();
        cfg.network.rpc_url = format!("http://{addr}");
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let state = AppState::new(AppConfig {
            raw: cfg,
            source_path: None,
            warnings: vec![],
        });
        let (tx, mut rx) = mpsc::channel(4);
        let info = SignatureInfo {
            signature: Signature::new_unique(),
            slot: 1,
            err: None,
            block_time: None,
            memo: None,
        };
        let wallet_a = Pubkey::new_unique();
        let wallet_b = Pubkey::new_unique();
        assert_eq!(
            emit_signature(&rpc, &state, &tx, &wallet_a, &info).await,
            Emit::Retry
        );
        assert_eq!(
            requests.load(AtomicOrdering::SeqCst),
            1,
            "first fetch got null"
        );
        assert_eq!(
            emit_signature(&rpc, &state, &tx, &wallet_a, &info).await,
            Emit::Retry
        );
        assert_eq!(
            emit_signature(&rpc, &state, &tx, &wallet_b, &info).await,
            Emit::Retry
        );
        assert_eq!(
            requests.load(AtomicOrdering::SeqCst),
            3,
            "both retries must fetch the transaction"
        );
        assert!(rx.try_recv().is_err(), "nothing reached the event pipeline");
        server.abort();
    }

    #[tokio::test]
    async fn poll_requests_newer_transactions_using_until_cursor() {
        use bot_core::config::{AppConfig, Config};
        use bot_core::state::AppState;
        use solana_sdk::signature::Signature;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let old = Signature::new_unique();
        let old_text = old.to_string();
        let expected = old_text.clone();
        let (seen_tx, seen_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut seen_tx = Some(seen_tx);
            for round in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0_u8; 4096];
                let request = loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0, "request ended before body");
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..i]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if bytes.len() >= i + 4 + len {
                            break serde_json::from_slice::<serde_json::Value>(
                                &bytes[i + 4..i + 4 + len],
                            )
                            .unwrap();
                        }
                    }
                };
                assert_eq!(request["method"], "getSignaturesForAddress");
                let result = if round == 0 {
                    serde_json::json!([{ "signature": old_text, "slot": 10, "err": null,
                        "memo": null, "blockTime": null, "confirmationStatus": "confirmed" }])
                } else {
                    seen_tx
                        .take()
                        .unwrap()
                        .send(request["params"][1].clone())
                        .unwrap();
                    serde_json::json!([])
                };
                let body =
                    serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
                        .to_string();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let mut cfg = Config::default();
        cfg.copy.enabled = true;
        cfg.network.rpc_url = format!("http://{addr}");
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let state = AppState::new(AppConfig {
            raw: cfg,
            source_path: None,
            warnings: vec![],
        });
        let (tx, _rx) = mpsc::channel(4);
        let poll = tokio::spawn(run_poll(
            rpc,
            vec![Pubkey::new_unique().to_string()],
            tx,
            state,
            60_000,
            10,
        ));
        let params = tokio::time::timeout(Duration::from_secs(15), seen_rx)
            .await
            .expect("poll did not issue request")
            .expect("mock server failed");
        assert_eq!(params["until"], expected);
        assert!(
            params.get("before").is_none_or(|v| v.is_null()),
            "must not search older history: {params}"
        );
        poll.abort();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn poll_retries_null_fetch_without_advancing_cursor() {
        use bot_core::config::{AppConfig, Config};
        use bot_core::state::AppState;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let saved = Signature::new_unique();
        let new = Signature::new_unique();
        let (done_tx, done_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut seen_options = Vec::new();
            for step in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0_u8; 4096];
                let request = loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..i]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if bytes.len() >= i + 4 + len {
                            break serde_json::from_slice::<serde_json::Value>(
                                &bytes[i + 4..i + 4 + len],
                            )
                            .unwrap();
                        }
                    }
                };
                let result = if matches!(step, 0 | 1 | 3) {
                    assert_eq!(request["method"], "getSignaturesForAddress");
                    if step > 0 {
                        seen_options.push(request["params"][1].clone());
                    }
                    let sig = if step == 0 { saved } else { new };
                    serde_json::json!([{ "signature":sig.to_string(), "slot":11,
                        "err":null, "memo":null, "blockTime":null, "confirmationStatus":"confirmed" }])
                } else {
                    assert_eq!(request["method"], "getTransaction");
                    serde_json::Value::Null
                };
                let body =
                    serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
                        .to_string();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            done_tx.send(seen_options).unwrap();
        });
        let mut cfg = Config::default();
        cfg.copy.enabled = true;
        cfg.network.rpc_url = format!("http://{address}");
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let state = AppState::new(AppConfig {
            raw: cfg,
            source_path: None,
            warnings: vec![],
        });
        let (tx, mut rx) = mpsc::channel(4);
        let poll = tokio::spawn(run_poll(
            rpc,
            vec![Pubkey::new_unique().to_string()],
            tx,
            state,
            1,
            10,
        ));
        let opts = tokio::time::timeout(Duration::from_secs(15), done_rx)
            .await
            .expect("poll did not retry")
            .expect("mock server failed");
        assert_eq!(opts.len(), 2);
        for options in opts {
            assert_eq!(
                options["until"],
                saved.to_string(),
                "failed fetch must not advance cursor"
            );
        }
        assert!(rx.try_recv().is_err());
        poll.abort();
        server.await.unwrap();
    }

    async fn signature_page_server(
        pages: Vec<Vec<String>>,
    ) -> (String, tokio::task::JoinHandle<Vec<serde_json::Value>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut options = Vec::new();
            for page in pages {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0_u8; 4096];
                let request = loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..i]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if bytes.len() >= i + 4 + len {
                            break serde_json::from_slice::<serde_json::Value>(
                                &bytes[i + 4..i + 4 + len],
                            )
                            .unwrap();
                        }
                    }
                };
                assert_eq!(request["method"], "getSignaturesForAddress");
                options.push(request["params"][1].clone());
                let rows: Vec<_> = page
                    .iter()
                    .enumerate()
                    .map(|(i, sig)| {
                        serde_json::json!({
                            "signature":sig.to_string(), "slot":100-i, "err":null, "memo":null,
                            "blockTime":null, "confirmationStatus":"confirmed"
                        })
                    })
                    .collect();
                let body = serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":rows})
                    .to_string();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            options
        });
        (address, task)
    }

    #[tokio::test]
    async fn poll_batch_paginates_to_saved_cursor_without_skipping_full_page() {
        let signatures: Vec<Signature> = (0..4).map(|_| Signature::new_unique()).collect();
        let until = Signature::new_unique();
        let (url, server) = signature_page_server(vec![
            signatures[..2].iter().map(ToString::to_string).collect(),
            signatures[2..].iter().map(ToString::to_string).collect(),
            vec![],
        ])
        .await;
        let mut cfg = bot_core::config::Config::default();
        cfg.network.rpc_url = url;
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let result = poll_signature_batch(&rpc, &Pubkey::new_unique(), 2, Some(until))
            .await
            .unwrap();
        assert_eq!(
            result.iter().map(|i| i.signature).collect::<Vec<_>>(),
            signatures
        );
        let opts = server.await.unwrap();
        assert_eq!(
            opts.len(),
            3,
            "a full last page still needs an empty terminator"
        );
        for opt in &opts {
            assert_eq!(opt["until"], until.to_string());
        }
        assert!(opts[0].get("before").is_none_or(|v| v.is_null()));
        assert_eq!(opts[1]["before"], signatures[1].to_string());
        assert_eq!(opts[2]["before"], signatures[3].to_string());
    }

    #[tokio::test]
    async fn poll_batch_repeated_provider_page_is_rejected() {
        let sig = Signature::new_unique();
        let (url, server) =
            signature_page_server(vec![vec![sig.to_string()], vec![sig.to_string()]]).await;
        let mut cfg = bot_core::config::Config::default();
        cfg.network.rpc_url = url;
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let error = poll_signature_batch(&rpc, &Pubkey::new_unique(), 1, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("repeated a page cursor"));
        assert_eq!(server.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn poll_batch_rejects_malformed_signature_in_full_page() {
        let valid = Signature::new_unique().to_string();
        let (url, server) =
            signature_page_server(vec![vec![valid, "invalid-signature".into()]]).await;
        let mut cfg = bot_core::config::Config::default();
        cfg.network.rpc_url = url;
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let error = poll_signature_batch(&rpc, &Pubkey::new_unique(), 2, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("malformed signature"));
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn poll_batch_page_cap_reports_incomplete_without_returning_a_partial_batch() {
        let pages = (0..MAX_POLL_SIGNATURE_PAGES)
            .map(|_| vec![Signature::new_unique().to_string()])
            .collect();
        let (url, server) = signature_page_server(pages).await;
        let mut cfg = bot_core::config::Config::default();
        cfg.network.rpc_url = url;
        cfg.network.rpc_url_fallbacks.clear();
        cfg.network.max_retries = 0;
        cfg.network.request_timeout_ms = 5_000;
        let rpc = Rpc::new(&cfg.network).unwrap();
        let error = poll_signature_batch(&rpc, &Pubkey::new_unique(), 1, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("page cap reached"));
        assert_eq!(server.await.unwrap().len(), MAX_POLL_SIGNATURE_PAGES);
    }

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

    #[test]
    fn decoded_raydium_cpmm_and_launchlab_keep_their_precise_venues() {
        let mut swap = DecodedSwap {
            signature: "sig".into(),
            slot: 9,
            block_time: None,
            wallet: "w".into(),
            side: Side::Buy,
            venue: SwapVenue::RaydiumLaunchLab,
            base_mint: "base".into(),
            quote_mint: WSOL_MINT.to_string(),
            base_amount: 3_000_000,
            quote_amount: 2_000_000_000,
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
        assert_eq!(trade.venue, Venue::RaydiumLaunchLab);
        assert_eq!(trade.side, PositionSide::Long);
        assert!((trade.sol_amount - 2.0).abs() < 1e-9);
        swap.venue = SwapVenue::RaydiumCpmm;
        let trade = wallet_trade_from_decoded(&swap);
        assert_eq!(trade.venue, Venue::RaydiumCpmm);
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
