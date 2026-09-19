//! Launch detection for Module 1.
//!
//! Three independent feeds are merged into one `mpsc<TokenLaunch>`:
//!
//!   * **PumpPortal** (`subscribeNewToken`) — the fastest source, because
//!     PumpPortal runs its own Geyser node and pushes the creation the instant
//!     it is committed.
//!   * **Solana `logsSubscribe`** on the pump program — a self-hosted
//!     cross-check that does not depend on a third party. Every `Create` event
//!     is decoded from the transaction logs with [`solana_kit::events`].
//!   * **Geyser `transactionSubscribe`** (`sniper.use_transaction_subscribe` +
//!     `network.geyser_ws_url`) — processed-commitment push from your own
//!     Geyser/Yellowstone node: the launch arrives with the full transaction
//!     meta before the block is even confirmed, and the `Create` event is
//!     decoded from the pushed logs through the same parser (BUILD PLAN §5).
//!
//! Running several is deliberate: they race, and the first to report a given
//! mint wins (deduplication happens downstream in `AppState::mark_launch_seen`).
//! If one feed lags or drops, the others still catch the launch.

use chrono::Utc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use bot_core::error::BotResult;
use bot_core::maths;
use bot_core::models::{LaunchFeed, TokenLaunch, TokenSocials};
use bot_core::state::Shared;

use solana_kit::decode::parse_transaction_notification;
use solana_kit::events::{self, PumpEvent};
use solana_kit::pumpportal::{
    NewTokenMessage, PumpPortalFeed, PumpPortalMessage, PumpPortalSubscription,
};
use solana_kit::ws::{SolanaWs, TransactionFilter, WsMessage, WsStatus};

/// Capacity of the merged launch channel. A burst of launches should buffer,
/// not block the feed readers.
const LAUNCH_BUFFER: usize = 512;

/// Spawns the configured feeds and returns the merged launch stream.
pub struct LaunchDetector;

impl LaunchDetector {
    /// Start whichever feeds the config enables and return their merged output.
    ///
    /// Returns an error only if *neither* feed could be started; a single feed
    /// failing degrades latency but keeps the module alive.
    pub async fn spawn(
        state: Shared,
        rpc: solana_kit::rpc::Rpc,
    ) -> BotResult<mpsc::Receiver<TokenLaunch>> {
        let cfg = state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let (tx, rx) = mpsc::channel(LAUNCH_BUFFER);

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
            let api_key = sniper.pumpportal_api_key.clone();
            match start_pumpportal(url, api_key, tx_pp, state_pp).await {
                Ok(()) => {
                    started += 1;
                    info!("sniper launch feed: pumpportal enabled");
                }
                Err(e) => warn!(error = %e, "pumpportal feed failed to start"),
            }
        }

        // ---- Solana logsSubscribe ----------------------------------------
        if sniper.use_log_subscription {
            let ws_url = rpc.ws_url().to_string();
            let tx_logs = tx.clone();
            let state_logs = state.clone();
            match start_log_subscription(ws_url.clone(), tx_logs, state_logs).await {
                Ok(()) => {
                    started += 1;
                    info!(%ws_url, "sniper launch feed: logsSubscribe enabled");
                }
                Err(e) => warn!(error = %e, "logsSubscribe feed failed to start"),
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
                    match start_geyser_subscription(url.to_string(), tx_g, state_g).await {
                        Ok(()) => {
                            started += 1;
                            info!(url, "sniper launch feed: transactionSubscribe (geyser) enabled");
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
        Ok(rx)
    }
}

/// Start the PumpPortal feed and forward `NewToken` messages as launches.
async fn start_pumpportal(
    url: String,
    api_key: Option<String>,
    tx: mpsc::Sender<TokenLaunch>,
    state: Shared,
) -> BotResult<()> {
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
            return Err(bot_core::error::BotError::ws(
                "pumpportal receiver already taken",
            ))
        }
    };

    tokio::spawn(async move {
        // Keep the feed alive for the lifetime of this task.
        let _feed = feed;
        while let Some(message) = rx.recv().await {
            match message {
                PumpPortalMessage::NewToken(m) => {
                    let launch = launch_from_pumpportal(&m);
                    state.heartbeat(bot_core::models::BotModule::Sniper).await;
                    if tx.send(launch).await.is_err() {
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
    });
    Ok(())
}

/// Start a `logsSubscribe` on the pump program and decode `Create` events.
async fn start_log_subscription(
    ws_url: String,
    tx: mpsc::Sender<TokenLaunch>,
    state: Shared,
) -> BotResult<()> {
    let ws = SolanaWs::new(ws_url);
    let _handle = ws.spawn();
    let mut sub = ws.pump_logs_subscribe().await?;
    let mut rx = sub.receiver();

    tokio::spawn(async move {
        // Keep both the connection and the subscription alive.
        let _ws = ws;
        let _sub = &mut sub;
        while let Some(message) = rx.recv().await {
            match message {
                WsMessage::Logs {
                    signature,
                    logs,
                    err,
                    ..
                } => {
                    if err.is_some() {
                        // A failed transaction cannot have created a token.
                        continue;
                    }
                    if let Some(event) = events::find_launch(&logs) {
                        if let Some(launch) = launch_from_event(&event, Some(signature)) {
                            state.heartbeat(bot_core::models::BotModule::Sniper).await;
                            if tx.send(launch).await.is_err() {
                                debug!("sniper launch consumer gone; stopping log forwarder");
                                break;
                            }
                        }
                    }
                }
                WsMessage::Status(WsStatus::Connected) => {
                    info!("logsSubscribe connected");
                    state
                        .set_running(bot_core::models::BotModule::Sniper, true, true)
                        .await;
                }
                WsMessage::Status(s) => {
                    warn!(?s, "logsSubscribe status change");
                }
                WsMessage::Error { message } => {
                    error!(%message, "logsSubscribe error");
                }
                _ => {}
            }
        }
        warn!("logsSubscribe forwarder ended");
    });
    Ok(())
}

/// Start a Geyser `transactionSubscribe` on the pump program and decode
/// `Create` events from the pushed transaction meta (BUILD PLAN §5).
///
/// Compared with `logsSubscribe` this removes the node-side log filtering and
/// delivers the whole meta (logs, balances, slot) at processed commitment —
/// the lowest-latency self-hosted launch source. Requires a Yellowstone /
/// Geyser-enabled websocket endpoint; a plain node rejects the method and the
/// feed simply does not start (the other feeds keep running).
async fn start_geyser_subscription(
    ws_url: String,
    tx: mpsc::Sender<TokenLaunch>,
    state: Shared,
) -> BotResult<()> {
    let ws = SolanaWs::new(ws_url);
    let _handle = ws.spawn();
    let filter = TransactionFilter {
        account_include: vec![solana_kit::consts::PUMP_PROGRAM_ID.to_string()],
        ..Default::default()
    };
    let mut sub = ws.transaction_subscribe(filter).await?;
    let mut rx = sub.receiver();

    tokio::spawn(async move {
        // Keep both the connection and the subscription alive.
        let _ws = ws;
        let _sub = &mut sub;
        while let Some(message) = rx.recv().await {
            match message {
                WsMessage::Transaction { slot, raw, .. } => {
                    let Some(notif) = parse_transaction_notification(&raw) else {
                        debug!("sniper geyser: unparseable notification, skipping");
                        continue;
                    };
                    if !notif.succeeded() {
                        // A failed transaction cannot have created a token.
                        continue;
                    }
                    if let Some(event) = events::find_launch(notif.log_messages()) {
                        if let Some(mut launch) = launch_from_event(&event, Some(notif.signature)) {
                            launch.feed = LaunchFeed::TransactionSubscribe;
                            launch.slot = Some(if notif.slot > 0 { notif.slot } else { slot });
                            state.heartbeat(bot_core::models::BotModule::Sniper).await;
                            if tx.send(launch).await.is_err() {
                                debug!("sniper launch consumer gone; stopping geyser forwarder");
                                break;
                            }
                        }
                    }
                }
                WsMessage::Status(WsStatus::Connected) => {
                    info!("geyser transactionSubscribe connected");
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
    });
    Ok(())
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
    fn create_event_becomes_a_launch_with_a_computed_market_cap() {
        // A canonical fresh curve: 30 virtual SOL, 1.073e9 virtual tokens.
        let event = PumpEvent::Create {
            name: "Cat".into(),
            symbol: "CAT".into(),
            uri: "ipfs://x".into(),
            mint: Pubkey::new_unique(),
            bonding_curve: Pubkey::new_unique(),
            user: Pubkey::new_unique(),
            creator: Pubkey::new_unique(),
            timestamp: 1,
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
        };
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
    fn non_create_events_do_not_become_launches() {
        let event = PumpEvent::Complete {
            user: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            bonding_curve: Pubkey::new_unique(),
            timestamp: 1,
        };
        assert!(launch_from_event(&event, None).is_none());
    }

    #[test]
    fn socials_from_uri_handles_empty() {
        assert!(socials_from_uri(None).is_none());
        assert!(socials_from_uri(Some("")).is_none());
        assert!(socials_from_uri(Some("ipfs://x")).is_some());
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
