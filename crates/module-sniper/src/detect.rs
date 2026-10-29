//! Launch detection for Module 1 (TASK 2 §C/§D/§E).
//!
//! Three independent feeds are merged into one `mpsc<LaunchEvent>`:
//!
//!   * **PumpPortal** (`subscribeNewToken`) — the fastest pump.fun source,
//!     because PumpPortal runs its own Geyser node and pushes the creation the
//!     instant it is committed.
//!   * **Solana `logsSubscribe`** — a self-hosted cross-check that does not
//!     depend on a third party. One subscription per enabled protocol:
//!     the pump program (`Create` events), the PumpSwap program
//!     (`CreatePoolEvent`, when `sniper.trade_pumpswap`) and the Raydium AMM
//!     v4 program (`initialize2` log line → the creating transaction is
//!     fetched and decoded, when `sniper.trade_raydium`).
//!   * **Geyser `transactionSubscribe`** (`sniper.use_transaction_subscribe` +
//!     `network.geyser_ws_url`) — processed-commitment push from your own
//!     Geyser/Yellowstone node with the full transaction meta, so all three
//!     protocols are decoded straight from the pushed transaction.
//!
//! Every observation is normalised into a [`LaunchEvent`] here: protocol,
//! source, slot, signature, per-source sequence number, raw payload hash and
//! whatever the source knows about liquidity and price. Feeds race and the
//! first to report a launch wins — deduplication happens downstream in the
//! pipeline through `AppState::mark_launch_seen` (the one authoritative
//! dedup). If one feed lags or drops, the others still catch the launch.
//!
//! Reconnects, gaps and reordering are observable rather than silent:
//! `sniper_feed_reconnects_total{source}`, `sniper_feed_gaps_total{source}`
//! and `sniper_feed_out_of_order_total{source}`; a slot regression after a
//! reconnect is logged with the sequence it was observed under.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use solana_sdk::pubkey::Pubkey;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

use bot_core::error::BotResult;
use bot_core::maths;
use bot_core::models::{LaunchFeed, TokenLaunch, TokenSocials};
use bot_core::state::Shared;

use solana_kit::consts::{
    PUMPSWAP_PROGRAM_ID, PUMP_PROGRAM_ID, RAYDIUM_AMM_V4, RAYDIUM_CPMM, RAYDIUM_LAUNCHLAB,
    RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_WITH_TOKEN_2022, WSOL_MINT,
};
use solana_kit::decode::{decode_instructions, parse_transaction_notification, TxNotification};
use solana_kit::events::{self, PumpEvent};
use solana_kit::pumpportal::{
    NewTokenMessage, PumpPortalFeed, PumpPortalMessage, PumpPortalSubscription,
};
use solana_kit::raydium::{self, PoolInitEvent};
use solana_kit::rpc::Rpc;
use solana_kit::venues::raydium_cpmm::decode::CpmmPoolInitEvent;
use solana_kit::venues::raydium_launchlab::decode::LaunchLabPoolInitEvent;
use solana_kit::ws::{
    redact_ws_url, LogsFilter, SolanaWs, TransactionFilter, WsMessage, WsPolicy, WsStatus,
};

use crate::event::{raw_hash_of, LaunchEvent, LaunchProtocol, SequenceTracker};

/// Capacity of the merged launch channel. A burst of launches should buffer,
/// not block the feed readers.
const LAUNCH_BUFFER: usize = 512;
const FEED_TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const FEED_TASK_ABORT_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct DetectorTasks {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl DetectorTasks {
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
                abort_launch_task(task, abort_deadline).await;
                continue;
            }
            let remaining = join_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                aborting = true;
                abort_launch_task(task, abort_deadline).await;
                continue;
            }
            match tokio::time::timeout(remaining, &mut *task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(error = %error, "sniper launch-feed task failed while joining");
                }
                Err(_) => {
                    warn!("sniper launch-feed task did not stop before deadline; aborting it");
                    aborting = true;
                    abort_launch_task(task, abort_deadline).await;
                }
            }
        }
        self.handles.clear();
    }
}

async fn abort_launch_task(task: &mut tokio::task::JoinHandle<()>, deadline: tokio::time::Instant) {
    task.abort();
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        warn!("sniper launch-feed abort join grace elapsed; cancellation was requested");
        return;
    }
    match tokio::time::timeout(remaining, &mut *task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if !error.is_cancelled() => {
            warn!(error = %error, "aborted sniper launch-feed task failed while joining");
        }
        Ok(Err(_)) => {}
        Err(_) => warn!("sniper launch-feed task did not finish during abort join grace"),
    }
}

impl Drop for DetectorTasks {
    fn drop(&mut self) {
        for task in &self.handles {
            task.abort();
        }
    }
}

/// Owns the merged launch stream and every task that feeds it.
#[derive(Debug)]
pub struct LaunchFeedHandle {
    receiver: mpsc::Receiver<LaunchEvent>,
    tasks: DetectorTasks,
}

impl LaunchFeedHandle {
    pub async fn recv(&mut self) -> Option<LaunchEvent> {
        self.receiver.recv().await
    }

    /// Number of launch events currently queued in the channel.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    /// True when no launch event is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Close the stream and join all feed tasks within one bounded deadline.
    pub async fn shutdown(&mut self) {
        self.receiver.close();
        self.tasks.join_all().await;
    }
}

/// Source labels for metrics (low cardinality, fixed set).
const SRC_PUMPPORTAL: &str = "pumpportal";
const SRC_LOGS_PUMP: &str = "logs_pump";
const SRC_LOGS_PUMPSWAP: &str = "logs_pumpswap";
const SRC_LOGS_RAYDIUM: &str = "logs_raydium";
const SRC_LOGS_RAYDIUM_CPMM: &str = "logs_raydium_cpmm";
const SRC_LOGS_RAYDIUM_LAUNCHLAB: &str = "logs_raydium_launchlab";
const SRC_GEYSER: &str = "geyser";

fn feed_counter(name: &str, help: &str, source: &str) {
    bot_core::obs::metrics::global()
        .counter(name, help, &[("source", source)])
        .inc();
}

fn count_reconnect(source: &str) {
    feed_counter(
        "sniper_feed_reconnects_total",
        "Launch-feed websocket (re)connections by source.",
        source,
    );
}

fn count_gap(source: &str) {
    feed_counter(
        "sniper_feed_gaps_total",
        "Launch-feed gaps reported after reconnects or bounded consumer-queue overflow.",
        source,
    );
}

fn count_out_of_order(source: &str) {
    feed_counter(
        "sniper_feed_out_of_order_total",
        "Launch-feed notifications whose slot regressed (reorder or replay after a reconnect).",
        source,
    );
}

fn count_decoded(source: &str, protocol: LaunchProtocol) {
    bot_core::obs::metrics::global()
        .counter(
            "sniper_feed_events_total",
            "Launch events decoded by the detector, by source and protocol.",
            &[("source", source), ("protocol", protocol.as_str())],
        )
        .inc();
}

/// Spawns the configured feeds and returns the merged launch stream.
pub struct LaunchDetector;

impl LaunchDetector {
    /// Start whichever feeds the config enables and return their merged output.
    ///
    /// Returns an error only if *no* feed could be started; a single feed
    /// failing degrades latency but keeps the module alive.
    pub async fn spawn(state: Shared, rpc: Rpc) -> BotResult<LaunchFeedHandle> {
        let cfg = state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let (tx, rx) = mpsc::channel(LAUNCH_BUFFER);
        let mut tasks = DetectorTasks::default();

        let mut started = 0;

        // ---- PumpPortal ---------------------------------------------------
        if sniper.use_pumpportal {
            let url = if sniper.pumpportal_ws_url.trim().is_empty() {
                solana_kit::consts::PUMPPORTAL_WS_URL.to_string()
            } else {
                sniper.pumpportal_ws_url.clone()
            };
            let tx_pp = tx.clone();
            let state_pp = state.clone();
            let rpc_pp = rpc.clone();
            let api_key = sniper.pumpportal_api_key.clone();
            match start_pumpportal(url, api_key, tx_pp, state_pp, rpc_pp).await {
                Ok(task) => {
                    tasks.push(task);
                    started += 1;
                    info!("sniper launch feed: pumpportal enabled");
                }
                Err(e) => warn!(error = %e, "pumpportal feed failed to start"),
            }
        }

        // Which protocols the websocket feeds observe. A protocol is worth
        // a subscription when something can consume its events: the direct
        // route, the Jupiter fallback (CPMM/LaunchLab have no other
        // detection source), or the launch dataset recorder, which records
        // observations before any trading gate runs.
        let observe_pumpswap = sniper.trade_pumpswap || sniper.record_launch_dataset;
        let observe_raydium = sniper.trade_raydium || sniper.record_launch_dataset;
        let observe_cpmm = sniper.trade_raydium_cpmm
            || sniper.use_jupiter_fallback
            || sniper.record_launch_dataset;
        let observe_launchlab = sniper.trade_raydium_launchlab
            || sniper.use_jupiter_fallback
            || sniper.record_launch_dataset;

        // ---- Solana logsSubscribe (one subscription per protocol) ---------
        if sniper.use_log_subscription {
            let ws_url = rpc.ws_url().to_string();
            let mut protocols = vec![LaunchProtocol::PumpFun];
            if observe_pumpswap {
                protocols.push(LaunchProtocol::PumpSwap);
            }
            if observe_raydium {
                protocols.push(LaunchProtocol::RaydiumAmmV4);
            }
            if observe_cpmm {
                protocols.push(LaunchProtocol::RaydiumCpmm);
            }
            if observe_launchlab {
                protocols.push(LaunchProtocol::RaydiumLaunchLab);
            }
            for protocol in protocols {
                match start_log_subscription(
                    ws_url.clone(),
                    protocol,
                    tx.clone(),
                    state.clone(),
                    rpc.clone(),
                )
                .await
                {
                    Ok(task) => {
                        tasks.push(task);
                        started += 1;
                        info!(endpoint = %redact_ws_url(&ws_url), %protocol, "sniper launch feed: logsSubscribe enabled");
                    }
                    Err(e) => {
                        warn!(error = %e, %protocol, "logsSubscribe feed failed to start")
                    }
                }
            }
        }

        // ---- Geyser transactionSubscribe (BUILD PLAN §5) ------------------
        if sniper.use_transaction_subscribe {
            match cfg
                .network
                .geyser_ws_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
            {
                Some(url) => {
                    let tx_g = tx.clone();
                    let state_g = state.clone();
                    let rpc_g = rpc.clone();
                    let mut programs = vec![PUMP_PROGRAM_ID.to_string()];
                    if observe_pumpswap {
                        programs.push(PUMPSWAP_PROGRAM_ID.to_string());
                    }
                    if observe_raydium {
                        programs.push(RAYDIUM_AMM_V4.to_string());
                    }
                    if observe_cpmm {
                        programs.push(RAYDIUM_CPMM.to_string());
                    }
                    if observe_launchlab {
                        programs.push(RAYDIUM_LAUNCHLAB.to_string());
                    }
                    match start_geyser_subscription(
                        url.to_string(),
                        programs,
                        tx_g,
                        state_g,
                        rpc_g,
                    )
                    .await
                    {
                        Ok(task) => {
                            tasks.push(task);
                            started += 1;
                            info!(endpoint = %redact_ws_url(url), "sniper launch feed: transactionSubscribe (geyser) enabled");
                        }
                        Err(e) => warn!(error = %e, "geyser transactionSubscribe feed failed to start"),
                    }
                }
                None => warn!(
                    "sniper.use_transaction_subscribe is set but network.geyser_ws_url is empty — feed skipped"
                ),
            }
        }

        if started == 0 {
            return Err(bot_core::error::BotError::config(
                "no sniper launch feed is enabled or startable — set sniper.use_pumpportal, sniper.use_log_subscription, or sniper.use_transaction_subscribe + network.geyser_ws_url",
            ));
        }

        // Drop our copy of the sender: the feed tasks hold the only others, so
        // the receiver closes when every feed has stopped.
        drop(tx);
        Ok(LaunchFeedHandle {
            receiver: rx,
            tasks,
        })
    }
}

/// Start the PumpPortal feed and forward `NewToken` messages as launches.
async fn start_pumpportal(
    url: String,
    api_key: Option<String>,
    tx: mpsc::Sender<LaunchEvent>,
    state: Shared,
    rpc: Rpc,
) -> BotResult<tokio::task::JoinHandle<()>> {
    // PumpPortal needs no key for the public data feed; a key only raises rate
    // limits. We accept it for completeness but the subscription is identical.
    if api_key.is_some() {
        debug!("pumpportal api key present (rate-limit tier only)");
    }
    let feed =
        PumpPortalFeed::new(PumpPortalSubscription::launches_only(), LAUNCH_BUFFER).with_url(url);
    feed.start().await?;

    let mut rx = match feed.receiver().await {
        Some(rx) => rx,
        None => {
            feed.stop().await;
            return Err(bot_core::error::BotError::ws(
                "pumpportal receiver already taken",
            ));
        }
    };

    let task = tokio::spawn(async move {
        // Keep the feed alive for the lifetime of this task.
        let feed = feed;
        let mut seq = SequenceTracker::default();
        count_reconnect(SRC_PUMPPORTAL);
        loop {
            let message = tokio::select! {
                _ = tx.closed() => break,
                _ = state.wait_shutdown() => break,
                message = rx.recv() => message,
            };
            let Some(message) = message else { break };
            match message {
                PumpPortalMessage::NewToken(m) => {
                    let mut event = event_from_pumpportal(&m, seq.next_seq());
                    // PART 03 K1: a pumpportal event carries no slot, so the
                    // producer resolves the launch transaction first (one
                    // `getTransaction`) and then reads that slot. Nothing here
                    // runs when `sniper.max_bundler_ratio` is 0.
                    let cfg = state.config_snapshot().await;
                    crate::risk_intel::first_slot::attach_if_enabled(&rpc, &cfg.sniper, &mut event)
                        .await;
                    count_decoded(SRC_PUMPPORTAL, event.protocol);
                    state.heartbeat(bot_core::models::BotModule::Sniper).await;
                    if tx.send(event).await.is_err() {
                        debug!("sniper launch consumer gone; stopping pumpportal forwarder");
                        break;
                    }
                }
                PumpPortalMessage::Migration(m) => {
                    debug!(mint = %m.mint, "pumpportal migration (ignored by the launch feed)");
                }
                other => {
                    debug!(?other, "pumpportal message ignored by the sniper");
                }
            }
        }
        warn!("pumpportal forwarder ended");
        feed.stop().await;
    });
    Ok(task)
}

/// Start a `logsSubscribe` on the protocol's program and decode its launch
/// signal from the notification logs.
async fn start_log_subscription(
    ws_url: String,
    protocol: LaunchProtocol,
    tx: mpsc::Sender<LaunchEvent>,
    state: Shared,
    rpc: Rpc,
) -> BotResult<tokio::task::JoinHandle<()>> {
    let ws = SolanaWs::with_policy(
        ws_url,
        WsPolicy::from_network(&state.config_snapshot().await.network),
    );
    let connection = ws.spawn();
    let (program, source) = match protocol {
        LaunchProtocol::PumpFun => (PUMP_PROGRAM_ID.to_string(), SRC_LOGS_PUMP),
        LaunchProtocol::PumpSwap => (PUMPSWAP_PROGRAM_ID.to_string(), SRC_LOGS_PUMPSWAP),
        LaunchProtocol::RaydiumAmmV4 => (RAYDIUM_AMM_V4.to_string(), SRC_LOGS_RAYDIUM),
        LaunchProtocol::RaydiumCpmm => (RAYDIUM_CPMM.to_string(), SRC_LOGS_RAYDIUM_CPMM),
        LaunchProtocol::RaydiumLaunchLab => {
            (RAYDIUM_LAUNCHLAB.to_string(), SRC_LOGS_RAYDIUM_LAUNCHLAB)
        }
    };
    let mut sub = match ws
        .logs_subscribe(LogsFilter::default().mentions([program]))
        .await
    {
        Ok(sub) => sub,
        Err(error) => {
            connection.shutdown().await;
            return Err(error);
        }
    };
    let mut rx = match sub.receiver() {
        Ok(receiver) => receiver,
        Err(error) => {
            sub.cancel().await;
            connection.shutdown().await;
            return Err(error);
        }
    };
    let seq = Arc::new(Mutex::new(SequenceTracker::default()));

    let task = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                _ = tx.closed() => break,
                _ = state.wait_shutdown() => break,
                message = rx.recv() => message,
            };
            let Some(message) = message else { break };
            match message {
                WsMessage::Logs {
                    signature,
                    logs,
                    err,
                    slot,
                    ..
                } => {
                    if err.is_some() {
                        // A failed transaction cannot have created anything.
                        continue;
                    }
                    let (seq_no, regressed) = {
                        let mut s = seq.lock().await;
                        let n = s.next_seq();
                        (n, s.note_slot(Some(slot)))
                    };
                    if regressed {
                        count_out_of_order(source);
                        warn!(source, slot, seq = seq_no, %signature, "logs notification slot regressed (reorder/replay after reconnect)");
                    }
                    let raw = raw_hash_of(logs.join("\n").as_bytes());
                    let event = match protocol {
                        LaunchProtocol::PumpFun => events::find_launch(&logs).and_then(|ev| {
                            event_from_pump_create(
                                &ev,
                                Some(signature.clone()),
                                slot,
                                LaunchFeed::SolanaLogs,
                                seq_no,
                                raw.clone(),
                            )
                        }),
                        LaunchProtocol::PumpSwap => {
                            events::find_pool_creation(&logs).and_then(|ev| {
                                event_from_pumpswap_pool(
                                    &ev,
                                    Some(signature.clone()),
                                    slot,
                                    LaunchFeed::SolanaLogs,
                                    seq_no,
                                    raw.clone(),
                                )
                            })
                        }
                        LaunchProtocol::RaydiumAmmV4 => {
                            match raydium_event_from_logs(
                                &rpc,
                                &signature,
                                slot,
                                &logs,
                                seq_no,
                                raw.clone(),
                            )
                            .await
                            {
                                Ok(ev) => ev,
                                Err(e) => {
                                    debug!(%signature, error = %e, "raydium initialize2 seen but the transaction could not be decoded");
                                    None
                                }
                            }
                        }
                        LaunchProtocol::RaydiumCpmm => {
                            match cpmm_event_from_logs(
                                &rpc,
                                &signature,
                                slot,
                                &logs,
                                seq_no,
                                raw.clone(),
                            )
                            .await
                            {
                                Ok(ev) => ev,
                                Err(e) => {
                                    debug!(%signature, error = %e, "raydium cpmm initialize seen but the transaction could not be decoded");
                                    None
                                }
                            }
                        }
                        LaunchProtocol::RaydiumLaunchLab => {
                            match launchlab_event_from_logs(
                                &rpc,
                                &signature,
                                slot,
                                &logs,
                                seq_no,
                                raw.clone(),
                            )
                            .await
                            {
                                Ok(ev) => ev,
                                Err(e) => {
                                    debug!(%signature, error = %e, "raydium launchlab initialize seen but the transaction could not be decoded");
                                    None
                                }
                            }
                        }
                    };
                    if let Some(mut event) = event {
                        // PART 03 K1: fill `first_slot_buys` BEFORE the event
                        // enters the decision queue, because the bundler gate
                        // reads it. `attach_if_enabled` does nothing at all
                        // when `sniper.max_bundler_ratio` is 0 (the gate is
                        // off), and a block it cannot read leaves the list
                        // empty — which the gate already treats as missing
                        // evidence. See `risk_intel::first_slot`.
                        let cfg = state.config_snapshot().await;
                        crate::risk_intel::first_slot::attach_if_enabled(
                            &rpc,
                            &cfg.sniper,
                            &mut event,
                        )
                        .await;
                        count_decoded(source, event.protocol);
                        state.heartbeat(bot_core::models::BotModule::Sniper).await;
                        if tx.send(event).await.is_err() {
                            debug!("sniper launch consumer gone; stopping log forwarder");
                            break;
                        }
                    }
                }
                WsMessage::Status(WsStatus::Connected) => {
                    count_reconnect(source);
                    info!(source, "logsSubscribe connected");
                    state
                        .set_running(bot_core::models::BotModule::Sniper, true, true)
                        .await;
                }
                // Launches that happened during the outage are not
                // snipeable any more (the edge is gone within seconds), so
                // there is nothing to backfill: record the gap for the
                // operator and carry on with live events.
                WsMessage::Gap {
                    last_slot,
                    outage_ms,
                    dropped_messages,
                    ..
                } => {
                    count_gap(source);
                    let detail = if dropped_messages > 0 {
                        warn!(
                            source,
                            ?last_slot,
                            outage_ms,
                            dropped_messages,
                            "logsSubscribe consumer queue overflow; launches may have been missed"
                        );
                        format!(
                            "launch feed consumer overflow ({source}): dropped {dropped_messages} notifications over about {outage_ms} ms"
                        )
                    } else {
                        warn!(
                            source,
                            ?last_slot,
                            outage_ms,
                            "logsSubscribe restored after an outage — launches in the gap were not observed"
                        );
                        format!("launch feed gap ({source}): websocket down for {outage_ms} ms")
                    };
                    state
                        .record_error(bot_core::models::BotModule::Sniper, &detail)
                        .await;
                }
                WsMessage::Status(s) => {
                    warn!(source, ?s, "logsSubscribe status change");
                }
                WsMessage::Error { message } => {
                    error!(source, %message, "logsSubscribe error");
                }
                _ => {}
            }
        }
        warn!(source, "logsSubscribe forwarder ended");
        sub.cancel().await;
        connection.shutdown().await;
    });
    Ok(task)
}

/// Start a Geyser `transactionSubscribe` on the enabled programs and decode
/// every protocol's launch signal from the pushed transaction (BUILD PLAN §5).
///
/// Compared with `logsSubscribe` this removes the node-side log filtering and
/// delivers the whole meta (logs, balances, slot) at processed commitment —
/// the lowest-latency self-hosted launch source. Requires a Yellowstone /
/// Geyser-enabled websocket endpoint; a plain node rejects the method and the
/// feed simply does not start (the other feeds keep running).
async fn start_geyser_subscription(
    ws_url: String,
    programs: Vec<String>,
    tx: mpsc::Sender<LaunchEvent>,
    state: Shared,
    rpc: Rpc,
) -> BotResult<tokio::task::JoinHandle<()>> {
    let ws = SolanaWs::with_policy(
        ws_url,
        WsPolicy::from_network(&state.config_snapshot().await.network),
    );
    let connection = ws.spawn();
    let filter = TransactionFilter {
        account_include: programs,
        ..Default::default()
    };
    let mut sub = match ws.transaction_subscribe(filter).await {
        Ok(sub) => sub,
        Err(error) => {
            connection.shutdown().await;
            return Err(error);
        }
    };
    let mut rx = match sub.receiver() {
        Ok(receiver) => receiver,
        Err(error) => {
            sub.cancel().await;
            connection.shutdown().await;
            return Err(error);
        }
    };

    let task = tokio::spawn(async move {
        let mut seq = SequenceTracker::default();
        loop {
            let message = tokio::select! {
                _ = tx.closed() => break,
                _ = state.wait_shutdown() => break,
                message = rx.recv() => message,
            };
            let Some(message) = message else { break };
            match message {
                WsMessage::Transaction { slot, raw, .. } => {
                    let Some(notif) = parse_transaction_notification(&raw) else {
                        debug!("sniper geyser: unparseable notification, skipping");
                        continue;
                    };
                    if !notif.succeeded() {
                        // A failed transaction cannot have created anything.
                        continue;
                    }
                    let slot = if notif.slot > 0 { notif.slot } else { slot };
                    let seq_no = seq.next_seq();
                    if seq.note_slot(Some(slot)) {
                        count_out_of_order(SRC_GEYSER);
                        warn!(slot, seq = seq_no, signature = %notif.signature, "geyser notification slot regressed (reorder/replay after reconnect)");
                    }
                    let raw_hash = raw_hash_of(raw.to_string().as_bytes());
                    if let Some(mut event) = event_from_notification(&notif, slot, seq_no, raw_hash)
                    {
                        // PART 03 K1: same enrichment as the logsSubscribe
                        // feed — see `risk_intel::first_slot`. A geyser
                        // notification carries the slot, so the launch slot's
                        // block is read directly.
                        let cfg = state.config_snapshot().await;
                        crate::risk_intel::first_slot::attach_if_enabled(
                            &rpc,
                            &cfg.sniper,
                            &mut event,
                        )
                        .await;
                        count_decoded(SRC_GEYSER, event.protocol);
                        state.heartbeat(bot_core::models::BotModule::Sniper).await;
                        if tx.send(event).await.is_err() {
                            debug!("sniper launch consumer gone; stopping geyser forwarder");
                            break;
                        }
                    }
                }
                WsMessage::Status(WsStatus::Connected) => {
                    count_reconnect(SRC_GEYSER);
                    info!("geyser transactionSubscribe connected");
                }
                WsMessage::Gap {
                    last_slot,
                    outage_ms,
                    dropped_messages,
                    ..
                } => {
                    count_gap(SRC_GEYSER);
                    let detail = if dropped_messages > 0 {
                        warn!(
                            ?last_slot,
                            outage_ms,
                            dropped_messages,
                            "geyser consumer queue overflow; launches may have been missed"
                        );
                        format!(
                            "launch feed consumer overflow (geyser): dropped {dropped_messages} notifications over about {outage_ms} ms"
                        )
                    } else {
                        warn!(
                            ?last_slot,
                            outage_ms,
                            "geyser transactionSubscribe restored after an outage — launches in the gap were not observed"
                        );
                        format!("launch feed gap: geyser websocket down for {outage_ms} ms")
                    };
                    state
                        .record_error(bot_core::models::BotModule::Sniper, &detail)
                        .await;
                }
                WsMessage::Status(s) => {
                    warn!(?s, "geyser transactionSubscribe status change");
                }
                WsMessage::Error { message } => {
                    error!(%message, "geyser transactionSubscribe error");
                }
                _ => {}
            }
        }
        warn!("geyser transactionSubscribe forwarder ended");
        sub.cancel().await;
        connection.shutdown().await;
    });
    Ok(task)
}

/// Decode whichever launch signal a pushed transaction carries: a pump.fun
/// `Create`, a PumpSwap `CreatePoolEvent`, or a Raydium `initialize2`.
pub fn event_from_notification(
    notif: &TxNotification,
    slot: u64,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    let logs = notif.log_messages();
    let sig = Some(notif.signature.clone());
    let block_time = notif.block_time;
    if let Some(ev) = events::find_launch(logs) {
        return event_from_pump_create(
            &ev,
            sig,
            slot,
            LaunchFeed::TransactionSubscribe,
            seq,
            raw_hash,
        );
    }
    if let Some(ev) = events::find_pool_creation(logs) {
        return event_from_pumpswap_pool(
            &ev,
            sig,
            slot,
            LaunchFeed::TransactionSubscribe,
            seq,
            raw_hash,
        );
    }
    if raydium::find_initialize2_log(logs).is_some() {
        let ixs = decode_instructions(&notif.transaction, &notif.meta).ok()?;
        let init = PoolInitEvent::find(&ixs)?;
        return event_from_raydium_init(
            &init,
            sig,
            slot,
            block_time,
            LaunchFeed::TransactionSubscribe,
            seq,
            raw_hash,
        );
    }
    if has_initialize_log(logs, CPMM_INITIALIZE_LOG_NAMES)
        || has_initialize_log(logs, LAUNCHLAB_INITIALIZE_LOG_NAMES)
    {
        let ixs = decode_instructions(&notif.transaction, &notif.meta).ok()?;
        for ix in &ixs {
            if let Ok(Some(init)) =
                CpmmPoolInitEvent::from_instruction(&ix.program_id, &ix.accounts, &ix.data)
            {
                return event_from_cpmm_init(
                    &init,
                    sig,
                    slot,
                    block_time,
                    LaunchFeed::TransactionSubscribe,
                    seq,
                    raw_hash,
                );
            }
        }
        for ix in &ixs {
            if let Some(ev) = launchlab_event_from_instruction(
                ix,
                sig.clone(),
                slot,
                block_time,
                LaunchFeed::TransactionSubscribe,
                seq,
                raw_hash.clone(),
            ) {
                return Some(ev);
            }
        }
    }
    None
}

/// `logsSubscribe` only carries the `initialize2` log line; fetch the
/// creating transaction to read the mints and the initial deposit.
async fn raydium_event_from_logs(
    rpc: &Rpc,
    signature: &str,
    slot: u64,
    logs: &[String],
    seq: u64,
    raw_hash: String,
) -> BotResult<Option<LaunchEvent>> {
    if raydium::find_initialize2_log(logs).is_none() {
        return Ok(None);
    }
    let sig = solana_sdk::signature::Signature::from_str(signature)
        .map_err(|e| bot_core::error::BotError::encoding(format!("signature {signature}: {e}")))?;
    let Some(confirmed) = rpc.get_transaction(&sig).await? else {
        return Ok(None);
    };
    let meta = confirmed
        .transaction
        .meta
        .as_ref()
        .ok_or_else(|| bot_core::error::BotError::solana("confirmed tx has no meta"))?;
    let ixs = decode_instructions(&confirmed.transaction.transaction, meta)?;
    let Some(init) = PoolInitEvent::find(&ixs) else {
        return Ok(None);
    };
    let slot = if confirmed.slot > 0 {
        confirmed.slot
    } else {
        slot
    };
    Ok(event_from_raydium_init(
        &init,
        Some(signature.to_string()),
        slot,
        confirmed.block_time,
        LaunchFeed::SolanaLogs,
        seq,
        raw_hash,
    ))
}

/// Anchor `Instruction: <Name>` log lines that mark a CPMM pool creation.
const CPMM_INITIALIZE_LOG_NAMES: &[&str] = &["Initialize", "InitializeWithPermission"];
/// Anchor `Instruction: <Name>` log lines that mark a LaunchLab pool creation.
const LAUNCHLAB_INITIALIZE_LOG_NAMES: &[&str] =
    &["Initialize", "InitializeV2", "InitializeWithToken2022"];

/// True when `logs` carry an Anchor instruction line for one of `names`.
/// Used as a cheap pre-filter: the creating transaction is only fetched
/// when its logs already say a pool was initialised.
fn has_initialize_log(logs: &[String], names: &[&str]) -> bool {
    logs.iter().any(|line| {
        let line = line.trim();
        names
            .iter()
            .any(|name| line.ends_with(&format!("Instruction: {name}")))
    })
}

/// Fetch a confirmed transaction and resolve its top-level instructions.
async fn decoded_tx_instructions(
    rpc: &Rpc,
    signature: &str,
) -> BotResult<
    Option<(
        Vec<solana_kit::decode::DecodedInstruction>,
        u64,
        Option<i64>,
    )>,
> {
    let sig = solana_sdk::signature::Signature::from_str(signature)
        .map_err(|e| bot_core::error::BotError::encoding(format!("signature {signature}: {e}")))?;
    let Some(confirmed) = rpc.get_transaction(&sig).await? else {
        return Ok(None);
    };
    let meta = confirmed
        .transaction
        .meta
        .as_ref()
        .ok_or_else(|| bot_core::error::BotError::solana("confirmed tx has no meta"))?;
    let ixs = decode_instructions(&confirmed.transaction.transaction, meta)?;
    Ok(Some((ixs, confirmed.slot, confirmed.block_time)))
}

/// `logsSubscribe` only carries the Anchor instruction line; fetch the
/// creating transaction to read the CPMM pool's mints and deposits.
async fn cpmm_event_from_logs(
    rpc: &Rpc,
    signature: &str,
    slot: u64,
    logs: &[String],
    seq: u64,
    raw_hash: String,
) -> BotResult<Option<LaunchEvent>> {
    if !has_initialize_log(logs, CPMM_INITIALIZE_LOG_NAMES) {
        return Ok(None);
    }
    let Some((ixs, tx_slot, block_time)) = decoded_tx_instructions(rpc, signature).await? else {
        return Ok(None);
    };
    for ix in &ixs {
        if let Some(init) =
            CpmmPoolInitEvent::from_instruction(&ix.program_id, &ix.accounts, &ix.data)?
        {
            return Ok(event_from_cpmm_init(
                &init,
                Some(signature.to_string()),
                if tx_slot > 0 { tx_slot } else { slot },
                block_time,
                LaunchFeed::SolanaLogs,
                seq,
                raw_hash,
            ));
        }
    }
    Ok(None)
}

/// Same shape as [`cpmm_event_from_logs`] for LaunchLab pool creations.
async fn launchlab_event_from_logs(
    rpc: &Rpc,
    signature: &str,
    slot: u64,
    logs: &[String],
    seq: u64,
    raw_hash: String,
) -> BotResult<Option<LaunchEvent>> {
    if !has_initialize_log(logs, LAUNCHLAB_INITIALIZE_LOG_NAMES) {
        return Ok(None);
    }
    let Some((ixs, tx_slot, block_time)) = decoded_tx_instructions(rpc, signature).await? else {
        return Ok(None);
    };
    let slot = if tx_slot > 0 { tx_slot } else { slot };
    for ix in &ixs {
        if let Some(ev) = launchlab_event_from_instruction(
            ix,
            Some(signature.to_string()),
            slot,
            block_time,
            LaunchFeed::SolanaLogs,
            seq,
            raw_hash.clone(),
        ) {
            return Ok(Some(ev));
        }
    }
    Ok(None)
}

/// One decoded instruction -> LaunchLab launch event. Handles the
/// IDL-decoded `initialize` / `initialize_v2` here, plus an account-only
/// conversion for `initialize_with_token_2022`, whose argument layout
/// differs but whose account prefix (payer, creator, global_config,
/// platform_config, authority, pool_state, base_mint, quote_mint, vaults)
/// is identical in the IDL; the market loader reads the decimals and the
/// curve state from chain, so nothing is fabricated.
fn launchlab_event_from_instruction(
    ix: &solana_kit::decode::DecodedInstruction,
    signature: Option<String>,
    slot: u64,
    block_time: Option<i64>,
    feed: LaunchFeed,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    if let Ok(Some(init)) =
        LaunchLabPoolInitEvent::from_instruction(&ix.program_id, &ix.accounts, &ix.data)
    {
        return event_from_launchlab_init(&init, signature, slot, block_time, feed, seq, raw_hash);
    }
    if ix.program_id == *RAYDIUM_LAUNCHLAB
        && ix.data.len() >= 8
        && ix.data[..8] == RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_WITH_TOKEN_2022
        && ix.accounts.len() >= 10
    {
        let init = LaunchLabPoolInitEvent {
            pool_state: ix.accounts[5],
            creator: ix.accounts[1],
            global_config: ix.accounts[2],
            platform_config: ix.accounts[3],
            base_mint: ix.accounts[6],
            quote_mint: ix.accounts[7],
            base_vault: ix.accounts[8],
            quote_vault: ix.accounts[9],
            curve_type: 0,
            supply: 0,
            total_base_sell: 0,
            total_quote_fund_raising: 0,
            migrate_type: 0,
            decimals: 0,
        };
        let mut ev =
            event_from_launchlab_init(&init, signature, slot, block_time, feed, seq, raw_hash)?;
        // The zeroed fields above mean "not decoded from this layout":
        // the decimals in particular must come from the mint read, not
        // from this event.
        ev.base_decimals = None;
        ev.event_id = ev.compute_event_id();
        return Some(ev);
    }
    None
}

/// Feed timestamp of a chain `block_time`, shared by the block reader and the
/// dev-sell watcher so both place an observation on the same clock. A missing
/// or non-positive block time is `None` — never a fabricated date.
pub(crate) fn unix_to_utc(secs: i64) -> Option<DateTime<Utc>> {
    if secs <= 0 {
        return None;
    }
    Utc.timestamp_opt(secs, 0).single()
}

/// Convert a PumpPortal `NewToken` payload into a [`LaunchEvent`].
pub fn event_from_pumpportal(m: &NewTokenMessage, seq: u64) -> LaunchEvent {
    let launch = launch_from_pumpportal(m);
    let raw = serde_json::to_vec(m)
        .map(|b| raw_hash_of(&b))
        .unwrap_or_else(|_| raw_hash_of(m.signature.as_bytes()));
    let mut ev = LaunchEvent::from_token_launch(launch, seq, raw);
    if let Some(curve) = m
        .bonding_curve_key
        .as_deref()
        .filter(|k| Pubkey::from_str(k).is_ok())
    {
        ev.pool = Some(curve.to_string());
    }
    // The creator's opening buy is the only real SOL on a fresh curve.
    if m.initial_buy > 0.0 && m.initial_buy.is_finite() {
        ev.liquidity_quote_lamports = Some(maths::sol_to_lamports(m.initial_buy));
    }
    if let (Some(vsol), Some(vtok)) = (m.vsol_in_pool, m.vtokens_in_pool) {
        if vsol.is_finite() && vtok.is_finite() && vtok > 0.0 {
            ev.initial_price_sol = Some(vsol / vtok);
        }
    }
    ev.event_id = ev.compute_event_id();
    ev
}

/// Convert a decoded pump `Create` event into a [`LaunchEvent`].
pub fn event_from_pump_create(
    event: &PumpEvent,
    signature: Option<String>,
    slot: u64,
    feed: LaunchFeed,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    let mut launch = launch_from_event(event, signature)?;
    launch.feed = feed;
    launch.slot = (slot > 0).then_some(slot);
    let PumpEvent::Create {
        timestamp,
        virtual_sol_reserves,
        virtual_token_reserves,
        bonding_curve,
        ..
    } = event
    else {
        return None;
    };
    let mut ev = LaunchEvent::from_token_launch(launch, seq, raw_hash);
    ev.pool = Some(bonding_curve.to_string());
    ev.event_ts = unix_to_utc(*timestamp);
    ev.liquidity_base_raw = None;
    ev.initial_price_sol = Some(maths::pump_spot_price_sol(
        *virtual_sol_reserves,
        *virtual_token_reserves,
    ))
    .filter(|p| p.is_finite() && *p > 0.0);
    ev.event_id = ev.compute_event_id();
    Some(ev)
}

/// Convert a PumpSwap `CreatePoolEvent` into a [`LaunchEvent`]. Only SOL
/// quoted pools are launches the sniper can act on; others are dropped here
/// (they would fail the route check anyway) to keep the feed quiet.
pub fn event_from_pumpswap_pool(
    event: &PumpEvent,
    signature: Option<String>,
    slot: u64,
    feed: LaunchFeed,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    let PumpEvent::CreatePool {
        timestamp,
        creator,
        base_mint,
        quote_mint,
        base_mint_decimals,
        quote_mint_decimals,
        pool_base_amount,
        pool_quote_amount,
        pool,
        ..
    } = event
    else {
        return None;
    };
    if *quote_mint != *WSOL_MINT {
        debug!(%pool, %quote_mint, "pumpswap pool is not SOL-quoted; ignored");
        return None;
    }
    let base_h = maths::from_raw_amount(*pool_base_amount, *base_mint_decimals);
    let quote_h = maths::from_raw_amount(*pool_quote_amount, *quote_mint_decimals);
    let price = if base_h > 0.0 { quote_h / base_h } else { 0.0 };
    let launch = TokenLaunch {
        mint: base_mint.to_string(),
        name: format!("pumpswap pool {}", short(&pool.to_string())),
        symbol: short(&base_mint.to_string()),
        uri: None,
        creator: creator.to_string(),
        pool: pool.to_string(),
        initial_buy_sol: 0.0,
        market_cap_sol: 0.0,
        market_cap_usd: None,
        total_supply: None,
        slot: (slot > 0).then_some(slot),
        signature,
        tx_type: Some("create_pool".to_string()),
        observed_at: Utc::now(),
        feed,
        socials: None,
    };
    let mut ev = LaunchEvent::from_token_launch(launch, seq, raw_hash);
    ev.protocol = LaunchProtocol::PumpSwap;
    ev.pool = Some(pool.to_string());
    ev.liquidity_quote_lamports = Some(*pool_quote_amount);
    ev.liquidity_base_raw = Some(*pool_base_amount);
    ev.initial_price_sol = Some(price).filter(|p| p.is_finite() && *p > 0.0);
    ev.base_decimals = Some(*base_mint_decimals);
    ev.event_ts = unix_to_utc(*timestamp);
    ev.event_id = ev.compute_event_id();
    Some(ev)
}

/// Convert a decoded Raydium `initialize2` into a [`LaunchEvent`]. Only SOL
/// pairs are launches the sniper can act on.
pub fn event_from_raydium_init(
    init: &PoolInitEvent,
    signature: Option<String>,
    slot: u64,
    block_time: Option<i64>,
    feed: LaunchFeed,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    let base_mint = init.base_mint()?;
    let sol = init.initial_sol_lamports()?;
    let base_raw = init.initial_base_raw()?;
    let launch = TokenLaunch {
        mint: base_mint.to_string(),
        name: format!("raydium pool {}", short(&init.amm_id.to_string())),
        symbol: short(&base_mint.to_string()),
        uri: None,
        creator: init.creator.to_string(),
        pool: init.amm_id.to_string(),
        initial_buy_sol: 0.0,
        market_cap_sol: 0.0,
        market_cap_usd: None,
        total_supply: None,
        slot: (slot > 0).then_some(slot),
        signature,
        tx_type: Some("initialize2".to_string()),
        observed_at: Utc::now(),
        feed,
        socials: None,
    };
    let mut ev = LaunchEvent::from_token_launch(launch, seq, raw_hash);
    ev.protocol = LaunchProtocol::RaydiumAmmV4;
    ev.pool = Some(init.amm_id.to_string());
    ev.liquidity_quote_lamports = Some(sol);
    ev.liquidity_base_raw = Some(base_raw);
    // Decimals are unknown until the mint is read; the price needs them.
    ev.base_decimals = None;
    ev.initial_price_sol = None;
    ev.event_ts = block_time.and_then(unix_to_utc);
    ev.event_id = ev.compute_event_id();
    Some(ev)
}

/// Convert a decoded Raydium CPMM `initialize` into a [`LaunchEvent`].
/// Only SOL pairs are launches the sniper can act on.
pub fn event_from_cpmm_init(
    init: &CpmmPoolInitEvent,
    signature: Option<String>,
    slot: u64,
    block_time: Option<i64>,
    feed: LaunchFeed,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    let (base_mint, sol, base_raw) = if init.token_1_mint == *WSOL_MINT {
        (init.token_0_mint, init.init_amount_1, init.init_amount_0)
    } else if init.token_0_mint == *WSOL_MINT {
        (init.token_1_mint, init.init_amount_0, init.init_amount_1)
    } else {
        debug!(pool = %init.pool_state, "raydium cpmm pool is not SOL-quoted; ignored");
        return None;
    };
    let launch = TokenLaunch {
        mint: base_mint.to_string(),
        name: format!("raydium cpmm pool {}", short(&init.pool_state.to_string())),
        symbol: short(&base_mint.to_string()),
        uri: None,
        creator: init.creator.to_string(),
        pool: init.pool_state.to_string(),
        initial_buy_sol: 0.0,
        market_cap_sol: 0.0,
        market_cap_usd: None,
        total_supply: None,
        slot: (slot > 0).then_some(slot),
        signature,
        tx_type: Some("initialize".to_string()),
        observed_at: Utc::now(),
        feed,
        socials: None,
    };
    let mut ev = LaunchEvent::from_token_launch(launch, seq, raw_hash);
    ev.protocol = LaunchProtocol::RaydiumCpmm;
    ev.pool = Some(init.pool_state.to_string());
    ev.liquidity_quote_lamports = Some(sol);
    ev.liquidity_base_raw = Some(base_raw);
    ev.base_decimals = None;
    ev.initial_price_sol = None;
    ev.event_ts = block_time.and_then(unix_to_utc);
    ev.event_id = ev.compute_event_id();
    Some(ev)
}

/// Convert a decoded Raydium LaunchLab `initialize` into a [`LaunchEvent`].
/// Only SOL-quoted pools are launches the sniper can act on. A fresh curve
/// holds no real quote yet, so the event carries no liquidity figure —
/// the fundraising target is not liquidity and is never reported as one.
pub fn event_from_launchlab_init(
    init: &LaunchLabPoolInitEvent,
    signature: Option<String>,
    slot: u64,
    block_time: Option<i64>,
    feed: LaunchFeed,
    seq: u64,
    raw_hash: String,
) -> Option<LaunchEvent> {
    if init.quote_mint != *WSOL_MINT {
        debug!(pool = %init.pool_state, quote = %init.quote_mint, "raydium launchlab pool is not SOL-quoted; ignored");
        return None;
    }
    let launch = TokenLaunch {
        mint: init.base_mint.to_string(),
        name: format!(
            "raydium launchlab pool {}",
            short(&init.pool_state.to_string())
        ),
        symbol: short(&init.base_mint.to_string()),
        uri: None,
        creator: init.creator.to_string(),
        pool: init.pool_state.to_string(),
        initial_buy_sol: 0.0,
        market_cap_sol: 0.0,
        market_cap_usd: None,
        total_supply: None,
        slot: (slot > 0).then_some(slot),
        signature,
        tx_type: Some("initialize".to_string()),
        observed_at: Utc::now(),
        feed,
        socials: None,
    };
    let mut ev = LaunchEvent::from_token_launch(launch, seq, raw_hash);
    ev.protocol = LaunchProtocol::RaydiumLaunchLab;
    ev.pool = Some(init.pool_state.to_string());
    ev.liquidity_quote_lamports = None;
    ev.liquidity_base_raw = (init.total_base_sell > 0).then_some(init.total_base_sell);
    ev.base_decimals = Some(init.decimals);
    ev.initial_price_sol = None;
    ev.event_ts = block_time.and_then(unix_to_utc);
    ev.event_id = ev.compute_event_id();
    Some(ev)
}

/// First and last four characters of a base58 key — a readable symbol for
/// protocols that carry no token metadata.
fn short(key: &str) -> String {
    if key.len() <= 8 {
        return key.to_string();
    }
    format!("{}..{}", &key[..4], &key[key.len() - 4..])
}

/// Convert a PumpPortal `NewToken` payload into a [`TokenLaunch`].
pub fn launch_from_pumpportal(m: &NewTokenMessage) -> TokenLaunch {
    // PumpPortal reports `marketCapSol` and `initialBuy` already in SOL.
    let market_cap_sol = m.market_cap_sol;
    let initial_buy_sol = m.initial_buy;

    // The metadata URI sometimes carries socials; PumpPortal does not parse
    // them, so we leave them unset here. `entry` can enrich asynchronously if
    // the operator enables socials-based screening.
    TokenLaunch {
        mint: m.mint.clone(),
        name: m.name.clone(),
        symbol: m.symbol.clone(),
        uri: Some(m.uri.clone()).filter(|u| !u.is_empty()),
        creator: m.trader_public_key.clone(),
        pool: if m.pool.is_empty() {
            "bonding-curve".to_string()
        } else {
            m.pool.clone()
        },
        initial_buy_sol,
        market_cap_sol,
        market_cap_usd: None,
        total_supply: None,
        slot: None,
        signature: Some(m.signature.clone()).filter(|s| !s.is_empty()),
        tx_type: Some(m.tx_type.clone()).filter(|t| !t.is_empty()),
        observed_at: Utc::now(),
        feed: LaunchFeed::PumpPortal,
        socials: None,
    }
}

/// Convert a decoded pump `Create` event into a [`TokenLaunch`].
///
/// Returns `None` for any event that is not a creation.
pub fn launch_from_event(event: &PumpEvent, signature: Option<String>) -> Option<TokenLaunch> {
    match event {
        PumpEvent::Create {
            name,
            symbol,
            uri,
            mint,
            creator,
            virtual_sol_reserves,
            virtual_token_reserves,
            real_token_reserves,
            token_total_supply,
            ..
        } => {
            // The bonding curve's market cap in SOL is the standard pump.fun
            // metric: virtual SOL over virtual tokens, scaled to the real
            // circulating supply.
            let market_cap_sol =
                maths::pump_market_cap_sol(*virtual_sol_reserves, *virtual_token_reserves);
            let total_supply = if *token_total_supply > 0 {
                Some(maths::from_raw_amount(*token_total_supply, 6))
            } else {
                None
            };
            let _ = real_token_reserves;

            Some(TokenLaunch {
                mint: mint.to_string(),
                name: name.clone(),
                symbol: symbol.clone(),
                uri: Some(uri.clone()).filter(|u| !u.is_empty()),
                creator: creator.to_string(),
                pool: "bonding-curve".to_string(),
                // The Create event does not carry the creator's opening buy;
                // the bonding-curve reserves already reflect it. Screening on
                // `initial_buy_sol` therefore only bites for the PumpPortal
                // feed, which reports it directly.
                initial_buy_sol: 0.0,
                market_cap_sol,
                market_cap_usd: None,
                total_supply,
                slot: None,
                signature,
                tx_type: Some("create".to_string()),
                observed_at: Utc::now(),
                feed: LaunchFeed::SolanaLogs,
                socials: None,
            })
        }
        _ => None,
    }
}

/// Best-effort socials parse from a token metadata URI.
///
/// Kept separate and synchronous-friendly so the entry path can call it only
/// when `risk.min_socials > 0`. Returns `None` when the URI is empty.
pub fn socials_from_uri(uri: Option<&str>) -> Option<TokenSocials> {
    // PumpPortal/pump.fun metadata URIs point at an IPFS JSON blob; fetching it
    // is the caller's job (it is a network read). Here we only normalise a
    // already-fetched JSON object.
    let uri = uri?;
    if uri.trim().is_empty() {
        return None;
    }
    // Without a fetch we cannot know the socials; return an empty set so the
    // count is 0 rather than pretending the field is absent.
    Some(TokenSocials::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_kit::consts::WSOL_MINT;
    use solana_sdk::pubkey::Pubkey;

    fn sig64() -> String {
        let mut s = "1".repeat(63);
        s.push('2');
        s
    }

    #[test]
    fn pumpportal_new_token_becomes_a_launch() {
        let m = NewTokenMessage {
            signature: "sig123".into(),
            mint: WSOL_MINT.to_string(),
            trader_public_key: "creator111".into(),
            tx_type: "create".into(),
            initial_buy: 1.25,
            market_cap_sol: 31.0,
            name: "Dog".into(),
            symbol: "DOG".into(),
            uri: "https://ipfs.io/ipfs/Qm".into(),
            pool: "bonding-curve".into(),
            bonding_curve_key: None,
            associated_bonding_curve_key: None,
            vtokens_in_pool: None,
            vsol_in_pool: None,
            vlp_tokens_in_pool: None,
        };
        let launch = launch_from_pumpportal(&m);
        assert_eq!(launch.mint, WSOL_MINT.to_string());
        assert_eq!(launch.name, "Dog");
        assert_eq!(launch.symbol, "DOG");
        assert_eq!(launch.creator, "creator111");
        assert_eq!(launch.initial_buy_sol, 1.25);
        assert_eq!(launch.market_cap_sol, 31.0);
        assert_eq!(launch.feed, LaunchFeed::PumpPortal);
        assert_eq!(launch.signature.as_deref(), Some("sig123"));
        assert_eq!(launch.uri.as_deref(), Some("https://ipfs.io/ipfs/Qm"));
    }

    #[test]
    fn empty_pool_defaults_to_bonding_curve() {
        let m = NewTokenMessage {
            mint: WSOL_MINT.to_string(),
            pool: "".into(),
            ..Default::default()
        };
        assert_eq!(launch_from_pumpportal(&m).pool, "bonding-curve");
    }

    #[test]
    fn empty_signature_and_uri_become_none() {
        let m = NewTokenMessage {
            mint: WSOL_MINT.to_string(),
            signature: "".into(),
            uri: "".into(),
            tx_type: "".into(),
            ..Default::default()
        };
        let launch = launch_from_pumpportal(&m);
        assert!(launch.signature.is_none());
        assert!(launch.uri.is_none());
        assert!(launch.tx_type.is_none());
    }

    #[test]
    fn pumpportal_event_is_normalised_with_liquidity_price_and_pool() {
        let curve = Pubkey::new_unique().to_string();
        let m = NewTokenMessage {
            signature: sig64(),
            mint: Pubkey::new_unique().to_string(),
            trader_public_key: Pubkey::new_unique().to_string(),
            tx_type: "create".into(),
            initial_buy: 0.5,
            market_cap_sol: 30.5,
            name: "Dog".into(),
            symbol: "DOG".into(),
            uri: "".into(),
            pool: "pump".into(),
            bonding_curve_key: Some(curve.clone()),
            associated_bonding_curve_key: None,
            vtokens_in_pool: Some(1_060_000_000.0),
            vsol_in_pool: Some(30.5),
            vlp_tokens_in_pool: None,
        };
        let ev = event_from_pumpportal(&m, 7);
        assert_eq!(ev.protocol, LaunchProtocol::PumpFun);
        assert_eq!(ev.source, LaunchFeed::PumpPortal);
        assert_eq!(ev.pool.as_deref(), Some(curve.as_str()));
        assert_eq!(ev.liquidity_quote_lamports, Some(500_000_000));
        assert!(ev.initial_price_sol.unwrap() > 0.0);
        assert_eq!(ev.source_seq, 7);
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
        // Same payload twice → same id and raw hash (deterministic).
        let again = event_from_pumpportal(&m, 8);
        assert_eq!(again.event_id, ev.event_id);
        assert_eq!(again.raw_hash, ev.raw_hash);
    }

    fn create_event(mint: Pubkey) -> PumpEvent {
        PumpEvent::Create {
            name: "Cat".into(),
            symbol: "CAT".into(),
            uri: "ipfs://x".into(),
            mint,
            bonding_curve: Pubkey::new_unique(),
            user: Pubkey::new_unique(),
            creator: Pubkey::new_unique(),
            timestamp: 1_700_000_000,
            virtual_token_reserves: 1_073_000_000 * 10u64.pow(6),
            virtual_sol_reserves: 30 * 1_000_000_000,
            real_token_reserves: 793_100_000 * 10u64.pow(6),
            token_total_supply: 1_000_000_000 * 10u64.pow(6),
            token_program: Pubkey::new_unique(),
            is_mayhem_mode: false,
            is_cashback_enabled: false,
            quote_mint: None,
            virtual_quote_reserves: None,
            creator_fee_bps: None,
            is_holder_reward: None,
        }
    }

    #[test]
    fn create_event_becomes_a_launch_with_a_computed_market_cap() {
        // A canonical fresh curve: 30 virtual SOL, 1.073e9 virtual tokens.
        let event = create_event(Pubkey::new_unique());
        let launch = launch_from_event(&event, Some("sigX".into())).expect("create → launch");
        assert_eq!(launch.symbol, "CAT");
        assert_eq!(launch.feed, LaunchFeed::SolanaLogs);
        assert_eq!(launch.signature.as_deref(), Some("sigX"));
        assert!(
            launch.market_cap_sol > 0.0,
            "cap = {}",
            launch.market_cap_sol
        );
        assert_eq!(launch.total_supply, Some(1_000_000_000.0));
        assert_eq!(
            launch.initial_buy_sol, 0.0,
            "the event carries no creator buy"
        );
    }

    #[test]
    fn create_event_is_normalised_with_slot_chain_time_and_curve() {
        let event = create_event(Pubkey::new_unique());
        let ev = event_from_pump_create(
            &event,
            Some(sig64()),
            4_242,
            LaunchFeed::TransactionSubscribe,
            3,
            "raw".into(),
        )
        .expect("create → event");
        assert_eq!(ev.protocol, LaunchProtocol::PumpFun);
        assert_eq!(ev.source, LaunchFeed::TransactionSubscribe);
        assert_eq!(ev.slot, Some(4_242));
        assert_eq!(ev.launch.slot, Some(4_242));
        assert_eq!(ev.event_ts.unwrap().timestamp(), 1_700_000_000);
        assert!(ev.initial_price_sol.unwrap() > 0.0);
        assert!(ev.pool.is_some());
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
        // Slot 0 (unknown) is not recorded as a slot.
        let ev0 = event_from_pump_create(
            &event,
            Some(sig64()),
            0,
            LaunchFeed::SolanaLogs,
            1,
            "raw".into(),
        )
        .unwrap();
        assert_eq!(ev0.slot, None);
        assert_eq!(
            ev0.event_id, ev.event_id,
            "same creation signature and curve → same id whichever feed saw it"
        );
    }

    #[test]
    fn non_create_events_do_not_become_launches() {
        let event = PumpEvent::Complete {
            user: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            bonding_curve: Pubkey::new_unique(),
            timestamp: 1,
        };
        assert!(launch_from_event(&event, None).is_none());
        assert!(
            event_from_pump_create(&event, None, 1, LaunchFeed::SolanaLogs, 1, "r".into())
                .is_none()
        );
        assert!(
            event_from_pumpswap_pool(&event, None, 1, LaunchFeed::SolanaLogs, 1, "r".into())
                .is_none()
        );
    }

    fn pool_event(quote_mint: Pubkey) -> PumpEvent {
        PumpEvent::CreatePool {
            timestamp: 1_700_000_100,
            index: 0,
            creator: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            quote_mint,
            base_mint_decimals: 6,
            quote_mint_decimals: 9,
            base_amount_in: 200_000_000_000_000,
            quote_amount_in: 85_000_000_000,
            pool_base_amount: 200_000_000_000_000,
            pool_quote_amount: 85_000_000_000,
            pool_bump: 254,
            pool: Pubkey::new_unique(),
            lp_mint: Pubkey::new_unique(),
            coin_creator: Pubkey::new_unique(),
        }
    }

    #[test]
    fn pumpswap_pool_creation_becomes_a_pump_swap_event() {
        let event = pool_event(*WSOL_MINT);
        let ev = event_from_pumpswap_pool(
            &event,
            Some(sig64()),
            10,
            LaunchFeed::SolanaLogs,
            1,
            "r".into(),
        )
        .expect("pool → event");
        assert_eq!(ev.protocol, LaunchProtocol::PumpSwap);
        assert_eq!(ev.liquidity_quote_lamports, Some(85_000_000_000));
        assert_eq!(ev.liquidity_base_raw, Some(200_000_000_000_000));
        assert_eq!(ev.base_decimals, Some(6));
        // 85 SOL / 200M tokens = 4.25e-7 SOL per token.
        let price = ev.initial_price_sol.unwrap();
        assert!((price - 4.25e-7).abs() < 1e-12, "{price}");
        assert!(ev.pool.is_some());
        assert_eq!(ev.launch.tx_type.as_deref(), Some("create_pool"));
        assert!(ev.launch.symbol.contains(".."));
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
        assert!(ev.dedup_key().starts_with("pump_swap:"));
        // Non-SOL quote is dropped.
        let other = pool_event(Pubkey::new_unique());
        assert!(
            event_from_pumpswap_pool(&other, None, 10, LaunchFeed::SolanaLogs, 1, "r".into())
                .is_none()
        );
    }

    #[test]
    fn raydium_initialize2_becomes_a_raydium_event() {
        let base = Pubkey::new_unique();
        let init = PoolInitEvent {
            amm_id: Pubkey::new_unique(),
            lp_mint: Pubkey::new_unique(),
            coin_mint: base,
            pc_mint: *WSOL_MINT,
            coin_vault: Pubkey::new_unique(),
            pc_vault: Pubkey::new_unique(),
            market: Pubkey::new_unique(),
            creator: Pubkey::new_unique(),
            nonce: 254,
            open_time: 0,
            init_pc_amount: 5_000_000_000,
            init_coin_amount: 1_000_000_000_000,
        };
        let ev = event_from_raydium_init(
            &init,
            Some(sig64()),
            77,
            Some(1_700_000_200),
            LaunchFeed::SolanaLogs,
            2,
            "r".into(),
        )
        .expect("init → event");
        assert_eq!(ev.protocol, LaunchProtocol::RaydiumAmmV4);
        assert_eq!(ev.mint, base.to_string());
        assert_eq!(ev.pool.as_deref(), Some(init.amm_id.to_string().as_str()));
        assert_eq!(ev.creator, init.creator.to_string());
        assert_eq!(ev.liquidity_quote_lamports, Some(5_000_000_000));
        assert_eq!(ev.liquidity_base_raw, Some(1_000_000_000_000));
        assert_eq!(ev.base_decimals, None, "unknown until the mint is read");
        assert_eq!(ev.event_ts.unwrap().timestamp(), 1_700_000_200);
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
        // A non-SOL pair is not a launch.
        let mut other = init.clone();
        other.pc_mint = Pubkey::new_unique();
        assert!(event_from_raydium_init(
            &other,
            None,
            77,
            None,
            LaunchFeed::SolanaLogs,
            2,
            "r".into()
        )
        .is_none());
    }

    #[test]
    fn cpmm_initialize_becomes_a_cpmm_event_on_either_side() {
        let base = Pubkey::new_unique();
        let init = CpmmPoolInitEvent {
            pool_state: Pubkey::new_unique(),
            amm_config: Pubkey::new_unique(),
            creator: Pubkey::new_unique(),
            token_0_mint: base,
            token_1_mint: *WSOL_MINT,
            lp_mint: Pubkey::new_unique(),
            token_0_vault: Pubkey::new_unique(),
            token_1_vault: Pubkey::new_unique(),
            observation_state: Pubkey::new_unique(),
            init_amount_0: 2_000_000,
            init_amount_1: 3_000_000_000,
            open_time: 0,
        };
        let ev = event_from_cpmm_init(
            &init,
            Some(sig64()),
            88,
            Some(1_700_000_300),
            LaunchFeed::SolanaLogs,
            4,
            "r".into(),
        )
        .expect("init -> event");
        assert_eq!(ev.protocol, LaunchProtocol::RaydiumCpmm);
        assert_eq!(ev.mint, base.to_string());
        assert_eq!(
            ev.pool.as_deref(),
            Some(init.pool_state.to_string().as_str())
        );
        assert_eq!(ev.liquidity_quote_lamports, Some(3_000_000_000));
        assert_eq!(ev.liquidity_base_raw, Some(2_000_000));
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
        // WSOL on the token_0 side flips the orientation.
        let flipped = CpmmPoolInitEvent {
            token_0_mint: *WSOL_MINT,
            token_1_mint: base,
            init_amount_0: 3_000_000_000,
            init_amount_1: 2_000_000,
            ..init.clone()
        };
        let ev = event_from_cpmm_init(
            &flipped,
            Some(sig64()),
            88,
            None,
            LaunchFeed::TransactionSubscribe,
            5,
            "r".into(),
        )
        .expect("flipped init -> event");
        assert_eq!(ev.mint, base.to_string());
        assert_eq!(ev.liquidity_quote_lamports, Some(3_000_000_000));
        assert_eq!(ev.liquidity_base_raw, Some(2_000_000));
        // A non-SOL pair is not a launch.
        let other = CpmmPoolInitEvent {
            token_1_mint: Pubkey::new_unique(),
            ..init
        };
        assert!(event_from_cpmm_init(
            &other,
            None,
            88,
            None,
            LaunchFeed::SolanaLogs,
            6,
            "r".into()
        )
        .is_none());
    }

    #[test]
    fn launchlab_initialize_becomes_a_launchlab_event_without_fake_liquidity() {
        let init = LaunchLabPoolInitEvent {
            pool_state: Pubkey::new_unique(),
            creator: Pubkey::new_unique(),
            global_config: Pubkey::new_unique(),
            platform_config: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            quote_mint: *WSOL_MINT,
            base_vault: Pubkey::new_unique(),
            quote_vault: Pubkey::new_unique(),
            curve_type: 0,
            supply: 1_000_000_000,
            total_base_sell: 800_000_000,
            total_quote_fund_raising: 85_000_000_000,
            migrate_type: 1,
            decimals: 6,
        };
        let ev = event_from_launchlab_init(
            &init,
            Some(sig64()),
            99,
            Some(1_700_000_400),
            LaunchFeed::SolanaLogs,
            7,
            "r".into(),
        )
        .expect("init -> event");
        assert_eq!(ev.protocol, LaunchProtocol::RaydiumLaunchLab);
        assert_eq!(ev.mint, init.base_mint.to_string());
        assert_eq!(
            ev.pool.as_deref(),
            Some(init.pool_state.to_string().as_str())
        );
        assert_eq!(ev.base_decimals, Some(6));
        assert_eq!(ev.liquidity_base_raw, Some(800_000_000));
        assert_eq!(
            ev.liquidity_quote_lamports, None,
            "a fresh curve holds no real quote; the fundraising target is not liquidity"
        );
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
        let other = LaunchLabPoolInitEvent {
            quote_mint: Pubkey::new_unique(),
            ..init
        };
        assert!(event_from_launchlab_init(
            &other,
            None,
            99,
            None,
            LaunchFeed::SolanaLogs,
            8,
            "r".into()
        )
        .is_none());
    }

    #[test]
    fn launchlab_token2022_instruction_converts_from_accounts_only() {
        let accounts: Vec<Pubkey> = (0..15).map(|_| Pubkey::new_unique()).collect();
        let mut accounts = accounts;
        accounts[7] = *WSOL_MINT;
        let ix = solana_kit::decode::DecodedInstruction {
            program_id: *RAYDIUM_LAUNCHLAB,
            accounts: accounts.clone(),
            data: RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_WITH_TOKEN_2022.to_vec(),
        };
        let ev = launchlab_event_from_instruction(
            &ix,
            Some(sig64()),
            100,
            None,
            LaunchFeed::TransactionSubscribe,
            9,
            "r".into(),
        )
        .expect("token2022 init -> event");
        assert_eq!(ev.protocol, LaunchProtocol::RaydiumLaunchLab);
        assert_eq!(ev.mint, accounts[6].to_string());
        assert_eq!(ev.pool.as_deref(), Some(accounts[5].to_string().as_str()));
        assert_eq!(
            ev.base_decimals, None,
            "decimals come from the mint read, never from the undecoded layout"
        );
        assert_eq!(ev.validate_shape(Utc::now()), Ok(()));
    }

    #[test]
    fn initialize_log_matching_is_exact_about_the_instruction_name() {
        let logs = vec![
            "Program LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj invoke [1]".to_string(),
            "Program log: Instruction: InitializeV2".to_string(),
        ];
        assert!(has_initialize_log(&logs, LAUNCHLAB_INITIALIZE_LOG_NAMES));
        assert!(
            !has_initialize_log(&logs, CPMM_INITIALIZE_LOG_NAMES),
            "InitializeV2 must not match the CPMM Initialize pre-filter"
        );
        let swap_logs = vec!["Program log: Instruction: SwapBaseInput".to_string()];
        assert!(!has_initialize_log(&swap_logs, CPMM_INITIALIZE_LOG_NAMES));
        assert!(!has_initialize_log(
            &swap_logs,
            LAUNCHLAB_INITIALIZE_LOG_NAMES
        ));
    }

    #[test]
    fn socials_from_uri_handles_empty() {
        assert!(socials_from_uri(None).is_none());
        assert!(socials_from_uri(Some("")).is_none());
        assert!(socials_from_uri(Some("ipfs://x")).is_some());
        assert_eq!(short("abcdefghijkl"), "abcd..ijkl");
        assert_eq!(short("short"), "short");
        assert!(unix_to_utc(0).is_none());
        assert!(unix_to_utc(-5).is_none());
    }

    #[tokio::test]
    async fn spawn_errors_when_no_feed_is_enabled() {
        let mut cfg = bot_core::config::AppConfig::from_defaults();
        cfg.raw.sniper.use_pumpportal = false;
        cfg.raw.sniper.use_log_subscription = false;
        let state = bot_core::state::AppState::new(cfg);
        let rpc = solana_kit::rpc::Rpc::new(&state.config_snapshot().await.network).unwrap();
        let err = LaunchDetector::spawn(state, rpc).await.unwrap_err();
        assert!(err.to_string().contains("no sniper launch feed"), "{err}");
    }
}
