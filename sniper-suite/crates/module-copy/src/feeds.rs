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
//! All feeds push into one channel; the consumer de-duplicates by signature
//! (and so does each feed, via `mark_signature_seen`), so it is safe to run
//! them together.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
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
use solana_kit::rpc::Rpc;
use solana_kit::ws::{SolanaWs, TransactionFilter, WsMessage};

/// Buffer for the merged output channel.
const OUT_BUFFER: usize = 512;
/// Buffer for the PumpPortal websocket reader.
const PP_BUFFER: usize = 512;

/// Spawns the configured copy feed(s).
pub struct CopyFeed;

impl CopyFeed {
    /// Start the feed tasks and return the merged receiver.
    pub async fn spawn(state: Shared, rpc: Rpc) -> BotResult<mpsc::Receiver<WalletTrade>> {
        let cfg = state.config_snapshot().await;
        let wallets: Vec<String> = cfg
            .copy
            .wallets
            .iter()
            .map(|w| w.address.trim().to_string())
            .filter(|w| !w.is_empty())
            .collect();

        let (tx, rx) = mpsc::channel(OUT_BUFFER);

        if wallets.is_empty() {
            // Nothing tracked yet. Keep the channel open (so `run` stays alive
            // and picks up wallets added via hot reload) but emit nothing.
            warn!("copy feed: no wallets configured — idling");
            tokio::spawn(async move {
                let _keep = tx;
                loop {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            });
            return Ok(rx);
        }

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
        match feed.as_str() {
            "pumpportal" => {
                let tx = tx.clone();
                let state = state.clone();
                let wallets = wallets.clone();
                let url = url.clone();
                tokio::spawn(async move { run_pumpportal(url, wallets, tx, state).await });
                started += 1;
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
                match geyser {
                    Some(url) => {
                        tokio::spawn(async move {
                            run_transaction_subscribe(
                                rpc, url, wallets, tx, state, poll_ms, sig_limit,
                            )
                            .await
                        });
                        info!(
                            wallets = wallet_count,
                            "copy feed: transaction_subscribe (geyser push)"
                        );
                    }
                    None => {
                        warn!("copy feed: transaction_subscribe requires network.geyser_ws_url — using logs_poll");
                        tokio::spawn(async move {
                            run_poll(rpc, wallets, tx, state, poll_ms, sig_limit).await
                        });
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
                tokio::spawn(
                    async move { run_poll(rpc, wallets, tx, state, poll_ms, sig_limit).await },
                );
                started += 1;
                info!(wallets = wallet_count, poll_ms, "copy feed: logs_poll");
            }
        }

        if started == 0 {
            return Err(BotError::config("no copy feed could be started"));
        }

        drop(tx);
        Ok(rx)
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
            return;
        }
    };
    // Keep the feed alive for the lifetime of this task.
    let _feed = feed;
    while let Some(message) = rx.recv().await {
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

    // Warm-up: seed each cursor with the current newest signature so the first
    // live poll only emits trades that happen *after* start-up. Without this we
    // would replay recent history and mirror stale whale trades.
    let mut cursors: HashMap<String, Option<Signature>> = HashMap::new();
    for (wstr, wkey) in &tracked {
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
        ticker.tick().await;
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
                if info.err.is_some() {
                    continue; // failed transaction
                }
                if !state.mark_signature_seen(&info.signature.to_string()).await {
                    // `false` = already emitted (possibly by another feed);
                    // still advance the cursor below.
                    continue;
                }
                let confirmed = match rpc.get_transaction(&info.signature).await {
                    Ok(Some(c)) => c,
                    Ok(None) => continue,
                    Err(e) => {
                        debug!(sig = %info.signature, error = %e, "copy poll getTransaction failed");
                        continue;
                    }
                };
                let Some(meta) = confirmed.transaction.meta.clone() else {
                    continue;
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
                            debug!("copy consumer gone; stopping poll forwarder");
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => debug!(sig = %info.signature, error = %e, "copy decode failed"),
                }
            }
            if let Some(newest) = sigs.first() {
                cursors.insert(wstr.clone(), Some(newest.signature));
            }
        }
    }
}

/// Geyser `transactionSubscribe` forwarder (BUILD PLAN §5): whale
/// transactions are *pushed* by a Yellowstone-compatible websocket the moment
/// they are processed, instead of being discovered by the poll loop one
/// `poll_interval_ms` later. Notifications carry the full base64 transaction
/// plus meta — the exact inputs [`decode_swap`] already consumes — so the
/// decode path is shared with polling, including signature dedup (a trade
/// seen by another feed is never emitted twice).
///
/// Failure policy: an endpoint that does not speak `transactionSubscribe`,
/// or a stream that ends, degrades to [`run_poll`] — the copy feed must
/// never silently die.
async fn run_transaction_subscribe(
    rpc: Rpc,
    url: String,
    wallets: Vec<String>,
    out: mpsc::Sender<WalletTrade>,
    state: Shared,
    poll_ms: u64,
    sig_limit: usize,
) {
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

    let ws = SolanaWs::new(url.clone());
    let conn = ws.spawn();
    let filter = TransactionFilter {
        account_include: tracked.iter().map(|(_, k)| k.to_string()).collect(),
        ..Default::default()
    };
    let mut sub = match ws.transaction_subscribe(filter).await {
        Ok(sub) => sub,
        Err(e) => {
            warn!(
                error = %e, url = %url,
                "copy geyser: transactionSubscribe rejected (not a Geyser endpoint?) — falling back to logs_poll"
            );
            conn.shutdown().await;
            run_poll(rpc, wallets, out, state, poll_ms, sig_limit).await;
            return;
        }
    };

    let mut rx = sub.receiver();
    let mut consumer_gone = false;
    while let Some(msg) = rx.recv().await {
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
                    continue; // `false` = already emitted by another feed
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
