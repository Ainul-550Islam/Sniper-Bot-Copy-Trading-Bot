//! CLOB websockets — live order-book snapshots for tracked tokens (market
//! channel) and authenticated order / trade events for our own orders (user
//! channel).
//!
//! **Market channel.** Connects to `…/market`, subscribes to a set of CTF
//! token ids, and folds `book` events into a shared [`QuoteMap`] the strategy
//! reads. The socket is kept alive with periodic `PING`s and reconnected with
//! backoff; a dropped feed degrades to the last known quotes rather than
//! stopping the bot.
//!
//! **User channel (TASK 4).** Connects to `…/user` with the L2 API
//! credentials, subscribes to the markets we trade and forwards every
//! `order` / `trade` event as a typed [`UserEvent`] over an mpsc channel. The
//! engine applies those events to its order lifecycle exactly like a status
//! poll (same [`crate::orders::TrackedOrder::apply_venue`] path), so a fill is
//! never booked twice whichever source reports it first. Parsing is pure and
//! unit-tested; the socket loop is thin.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use bot_core::models::BotModule;
use bot_core::state::Shared;

use crate::auth::ApiKey;
use crate::clob::{parse_venue_timestamp, BookLevel, OrderBook};
use crate::error::PolyResult;
use crate::orders::VenueOrderState;
use crate::strategy::Quote;

/// Shared map of token id -> latest quote.
pub type QuoteMap = Arc<RwLock<HashMap<String, Quote>>>;

/// A new, empty quote map.
pub fn new_quote_map() -> QuoteMap {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Parse a CLOB `book` websocket event into `(asset_id, Quote)`.
///
/// Returns `None` for any other event type (price_change, pong, …) so the
/// caller can ignore it. Only full `book` snapshots are folded in; incremental
/// `price_change` deltas are handled by the next snapshot. The venue
/// `timestamp` (ms) is carried into [`Quote::observed_at`] so the staleness
/// gate can judge the snapshot's age.
pub fn parse_book_message(text: &str) -> Option<(String, Quote)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let event_type = v.get("event_type")?.as_str()?;
    if event_type != "book" {
        return None;
    }
    let asset_id = v.get("asset_id")?.as_str()?.to_string();
    let bids: Vec<BookLevel> =
        serde_json::from_value(v.get("bids").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    let asks: Vec<BookLevel> =
        serde_json::from_value(v.get("asks").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    let book = OrderBook {
        market: None,
        asset_id: Some(asset_id.clone()),
        bids,
        asks,
        hash: None,
        timestamp: value_as_string(v.get("timestamp")),
    };
    Some((asset_id, book.to_quote()))
}

/// Run the market feed, updating `quotes`, until the task is aborted.
pub async fn run_market_feed(
    url: String,
    asset_ids: Vec<String>,
    quotes: QuoteMap,
    state: Shared,
) -> PolyResult<()> {
    if asset_ids.is_empty() {
        debug!("polymarket ws: no assets to subscribe");
        return Ok(());
    }
    let subscribe = serde_json::json!({
        "type": "market",
        "assets_ids": asset_ids,
    })
    .to_string();

    let mut backoff = Duration::from_secs(1);
    loop {
        match connect_async(&url).await {
            Ok((mut socket, _resp)) => {
                backoff = Duration::from_secs(1);
                state.set_running(BotModule::Polymarket, true, true).await;
                info!(assets = asset_ids.len(), "polymarket market ws connected");
                if let Err(e) = socket.send(Message::Text(subscribe.clone())).await {
                    warn!(error = %e, "polymarket ws subscribe send failed");
                }

                let mut ping = tokio::time::interval(Duration::from_secs(10));
                ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                loop {
                    tokio::select! {
                        _ = ping.tick() => {
                            if socket.send(Message::Text("PING".to_string())).await.is_err() {
                                warn!("polymarket ws ping failed; reconnecting");
                                break;
                            }
                        }
                        msg = socket.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    state.heartbeat(BotModule::Polymarket).await;
                                    if let Some((asset_id, quote)) = parse_book_message(&text) {
                                        let mut map = quotes.write().await;
                                        map.insert(asset_id, quote);
                                    } else {
                                        debug!(len = text.len(), "polymarket ws: non-book message");
                                    }
                                }
                                Some(Ok(Message::Ping(p))) => {
                                    let _ = socket.send(Message::Pong(p)).await;
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    warn!("polymarket ws closed; reconnecting");
                                    break;
                                }
                                Some(Err(e)) => {
                                    warn!(error = %e, "polymarket ws error; reconnecting");
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                state.set_running(BotModule::Polymarket, true, false).await;
            }
            Err(e) => {
                warn!(error = %e, "polymarket ws connect failed");
                state.set_running(BotModule::Polymarket, true, false).await;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

// ---------------------------------------------------------------------------
// User channel
// ---------------------------------------------------------------------------

/// One of our maker orders touched by a trade event.
#[derive(Debug, Clone, PartialEq)]
pub struct MakerFill {
    /// Our order id.
    pub order_id: String,
    /// Size matched against that order in this trade.
    pub matched: f64,
    /// Fill price for that maker order.
    pub price: f64,
}

/// A typed event from the authenticated `user` channel.
#[derive(Debug, Clone, PartialEq)]
pub enum UserEvent {
    /// Status update for one of our orders (`PLACEMENT` / `UPDATE` /
    /// `CANCELLATION`). `size_matched` is CUMULATIVE.
    Order {
        /// Venue order id.
        order_id: String,
        /// Outcome token id.
        asset_id: String,
        /// Condition id.
        market: String,
        /// Normalised status.
        state: VenueOrderState,
        /// Raw status string.
        raw_status: String,
        /// Cumulative matched size, when present.
        size_matched: Option<f64>,
        /// Original size, when present.
        original_size: Option<f64>,
        /// Limit price, when present.
        price: Option<f64>,
        /// `PLACEMENT` | `UPDATE` | `CANCELLATION` (free-form).
        update_type: String,
        /// Venue timestamp.
        ts: Option<DateTime<Utc>>,
        /// Venue trade ids behind `size_matched` (`associate_trades`),
        /// empty when the frame does not carry them.
        associate_trades: Vec<String>,
    },
    /// A trade that involved at least one of our orders. Sizes are DELTAS.
    Trade {
        /// Venue trade id.
        trade_id: String,
        /// Our taker order id, when we were the taker.
        taker_order_id: Option<String>,
        /// Our maker orders filled by this trade.
        maker_fills: Vec<MakerFill>,
        /// Outcome token id.
        asset_id: String,
        /// Condition id.
        market: String,
        /// `BUY` | `SELL`.
        side: String,
        /// Total trade size.
        size: f64,
        /// Trade price.
        price: f64,
        /// `MATCHED` | `MINED` | `CONFIRMED` | `RETRYING` | `FAILED`.
        status: String,
        /// Venue timestamp.
        ts: Option<DateTime<Utc>>,
    },
}

impl UserEvent {
    /// Whether a trade event must NOT be booked (settlement failed).
    pub fn is_failed_trade(&self) -> bool {
        matches!(self, UserEvent::Trade { status, .. } if status.eq_ignore_ascii_case("FAILED"))
    }

    /// Stable label for metrics.
    pub fn kind(&self) -> &'static str {
        match self {
            UserEvent::Order { .. } => "order",
            UserEvent::Trade { .. } => "trade",
        }
    }
}

/// Parse one user-channel frame into zero or more events. The venue sends
/// single objects and (on subscribe) arrays; anything unrecognised is
/// skipped, never guessed.
pub fn parse_user_message(text: &str) -> Vec<UserEvent> {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    match v {
        Value::Array(items) => items.iter().filter_map(parse_user_event).collect(),
        other => parse_user_event(&other).into_iter().collect(),
    }
}

fn parse_user_event(v: &Value) -> Option<UserEvent> {
    let event_type = v.get("event_type")?.as_str()?.to_ascii_lowercase();
    match event_type.as_str() {
        "order" => {
            let raw_status = value_as_string(v.get("status")).unwrap_or_default();
            Some(UserEvent::Order {
                order_id: value_as_string(v.get("id"))?,
                asset_id: value_as_string(v.get("asset_id")).unwrap_or_default(),
                market: value_as_string(v.get("market")).unwrap_or_default(),
                state: VenueOrderState::parse(&raw_status),
                raw_status,
                size_matched: value_as_f64(v.get("size_matched")),
                original_size: value_as_f64(v.get("original_size")),
                price: value_as_f64(v.get("price")),
                update_type: value_as_string(v.get("type")).unwrap_or_default(),
                ts: parse_venue_timestamp(value_as_string(v.get("timestamp")).as_deref()),
                associate_trades: v
                    .get("associate_trades")
                    .and_then(|t| t.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str())
                            .filter(|x| !x.trim().is_empty())
                            .map(|x| x.trim().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        }
        "trade" => {
            let maker_fills = v
                .get("maker_orders")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some(MakerFill {
                                order_id: value_as_string(m.get("order_id"))?,
                                matched: value_as_f64(m.get("matched_amount")).unwrap_or(0.0),
                                price: value_as_f64(m.get("price")).unwrap_or(0.0),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(UserEvent::Trade {
                trade_id: value_as_string(v.get("id"))?,
                taker_order_id: value_as_string(v.get("taker_order_id")).filter(|s| !s.is_empty()),
                maker_fills,
                asset_id: value_as_string(v.get("asset_id")).unwrap_or_default(),
                market: value_as_string(v.get("market")).unwrap_or_default(),
                side: value_as_string(v.get("side")).unwrap_or_default(),
                size: value_as_f64(v.get("size")).unwrap_or(0.0),
                price: value_as_f64(v.get("price")).unwrap_or(0.0),
                status: value_as_string(v.get("status")).unwrap_or_default(),
                ts: parse_venue_timestamp(value_as_string(v.get("timestamp")).as_deref()),
            })
        }
        _ => None,
    }
}

/// The subscribe frame for the user channel: L2 credentials + markets.
pub fn user_subscribe_message(api_key: &ApiKey, markets: &[String]) -> String {
    serde_json::json!({
        "auth": api_key.user_ws_auth(),
        "type": "user",
        "markets": markets,
    })
    .to_string()
}

/// Run the user feed, forwarding events over `tx`, until the task is
/// aborted or the receiver is dropped. Reconnects with backoff like the
/// market feed; credentials are never logged.
pub async fn run_user_feed(
    url: String,
    api_key: ApiKey,
    markets: Vec<String>,
    tx: mpsc::Sender<UserEvent>,
    state: Shared,
) -> PolyResult<()> {
    let subscribe = user_subscribe_message(&api_key, &markets);
    let mut backoff = Duration::from_secs(1);
    // TASK 6 — the venue does not number user-channel deliveries, so the
    // durable cursor counts them locally, continuing from the position a
    // previous life reached. That makes "how far did this worker get" and
    // "did a reconnect drop deliveries" answerable after a restart.
    let mut delivery_seq = state
        .ha()
        .cursor(bot_core::ha::FeedId::PolymarketUser, "")
        .await
        .position
        .unwrap_or(0);
    loop {
        if tx.is_closed() {
            debug!("polymarket user ws: receiver dropped; stopping");
            return Ok(());
        }
        match connect_async(&url).await {
            Ok((mut socket, _resp)) => {
                backoff = Duration::from_secs(1);
                info!(markets = markets.len(), "polymarket user ws connected");
                if let Err(e) = socket.send(Message::Text(subscribe.clone())).await {
                    warn!(error = %e, "polymarket user ws subscribe send failed");
                }
                let mut ping = tokio::time::interval(Duration::from_secs(10));
                ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        _ = ping.tick() => {
                            if socket.send(Message::Text("PING".to_string())).await.is_err() {
                                warn!("polymarket user ws ping failed; reconnecting");
                                break;
                            }
                        }
                        _ = tx.closed() => {
                            let _ = socket.close(None).await;
                            return Ok(());
                        }
                        msg = socket.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    state.heartbeat(BotModule::Polymarket).await;
                                    for ev in parse_user_message(&text) {
                                        bot_core::obs::metrics::global()
                                            .counter(
                                                "poly_user_ws_events_total",
                                                "Polymarket user-channel events received.",
                                                &[("type", ev.kind())],
                                            )
                                            .inc();
                                        // TASK 6 §4 — the user channel is the
                                        // authenticated fill feed: every event
                                        // advances a DURABLE cursor keyed by the
                                        // local delivery sequence. A replayed
                                        // delivery after a reconnect is
                                        // suppressed here (the fill journal
                                        // remains the money-level authority) and
                                        // a skipped sequence is recorded as a
                                        // gap instead of vanishing.
                                        delivery_seq += 1;
                                        let advance = state
                                            .ha()
                                            .offer(
                                                bot_core::ha::FeedId::PolymarketUser,
                                                "",
                                                delivery_seq,
                                                Some(chrono::Utc::now()),
                                            )
                                            .await;
                                        if !advance.should_process() {
                                            continue;
                                        }
                                        if tx.send(ev).await.is_err() {
                                            return Ok(());
                                        }
                                    }
                                }
                                Some(Ok(Message::Ping(p))) => {
                                    let _ = socket.send(Message::Pong(p)).await;
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    warn!("polymarket user ws closed; reconnecting");
                                    break;
                                }
                                Some(Err(e)) => {
                                    warn!(error = %e, "polymarket user ws error; reconnecting");
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "polymarket user ws connect failed");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn value_as_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn value_as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_book_event() {
        let text = r#"{
            "event_type": "book",
            "asset_id": "tok1",
            "bids": [{"price": "0.40", "size": "100"}, {"price": "0.42", "size": "50"}],
            "asks": [{"price": "0.45", "size": "80"}, {"price": "0.47", "size": "20"}],
            "hash": "abc",
            "timestamp": "1700000000"
        }"#;
        let (asset, quote) = parse_book_message(text).unwrap();
        assert_eq!(asset, "tok1");
        assert!((quote.best_bid - 0.42).abs() < 1e-9);
        assert!((quote.best_ask - 0.45).abs() < 1e-9);
        assert!((quote.midpoint - 0.435).abs() < 1e-9);
        assert_eq!(quote.observed_at.unwrap().timestamp(), 1_700_000_000);
    }

    #[test]
    fn ignores_non_book_events() {
        let text = r#"{"event_type": "price_change", "asset_id": "tok1", "changes": []}"#;
        assert!(parse_book_message(text).is_none());
    }

    #[test]
    fn ignores_pong_and_garbage() {
        assert!(parse_book_message("PONG").is_none());
        assert!(parse_book_message("not json").is_none());
    }

    #[tokio::test]
    async fn quote_map_insert_and_read() {
        let map = new_quote_map();
        {
            let mut m = map.write().await;
            m.insert("tok".into(), Quote::new(0.4, 0.5));
        }
        let m = map.read().await;
        assert!((m["tok"].midpoint - 0.45).abs() < 1e-9);
    }

    #[test]
    fn book_events_without_timestamp_have_unknown_age() {
        let text = r#"{"event_type":"book","asset_id":"tok1","bids":[],"asks":[{"price":"0.5","size":"1"}]}"#;
        let (_, q) = parse_book_message(text).unwrap();
        assert!(q.observed_at.is_none());
        let text = r#"{"event_type":"book","asset_id":"tok1","bids":[],"asks":[],"timestamp":1700000000123}"#;
        let (_, q) = parse_book_message(text).unwrap();
        assert_eq!(q.observed_at.unwrap().timestamp_millis(), 1_700_000_000_123);
    }

    #[test]
    fn parses_user_order_events() {
        let text = r#"{
            "event_type": "order", "id": "0xabc", "asset_id": "111", "market": "0xcond",
            "original_size": "10", "size_matched": "4", "price": "0.42", "side": "BUY",
            "status": "LIVE", "type": "UPDATE", "timestamp": "1700000000123",
            "associate_trades": ["t-1", "", "t-2"]
        }"#;
        let evs = parse_user_message(text);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            UserEvent::Order {
                order_id,
                asset_id,
                state,
                size_matched,
                original_size,
                price,
                update_type,
                ts,
                associate_trades,
                ..
            } => {
                assert_eq!(order_id, "0xabc");
                assert_eq!(asset_id, "111");
                assert_eq!(*state, VenueOrderState::Live);
                assert_eq!(*size_matched, Some(4.0));
                assert_eq!(*original_size, Some(10.0));
                assert_eq!(*price, Some(0.42));
                assert_eq!(update_type, "UPDATE");
                assert_eq!(ts.unwrap().timestamp_millis(), 1_700_000_000_123);
                assert_eq!(associate_trades, &["t-1".to_string(), "t-2".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(evs[0].kind(), "order");
        // Cancellation uses the venue's US spelling; no trade list → empty.
        let evs = parse_user_message(
            r#"{"event_type":"order","id":"0x1","status":"CANCELED","type":"CANCELLATION"}"#,
        );
        assert!(matches!(
            &evs[0],
            UserEvent::Order {
                state: VenueOrderState::Cancelled,
                associate_trades,
                ..
            } if associate_trades.is_empty()
        ));
    }

    #[test]
    fn parses_user_trade_events_including_maker_fills() {
        let text = r#"[{
            "event_type": "trade", "id": "trade-9", "asset_id": "111", "market": "0xcond",
            "side": "BUY", "size": "6", "price": "0.41", "status": "MATCHED",
            "taker_order_id": "0xtaker",
            "maker_orders": [
                {"order_id": "0xmaker1", "matched_amount": "2.5", "price": "0.40", "asset_id": "111"},
                {"order_id": "0xmaker2", "matched_amount": "3.5", "price": "0.41", "asset_id": "111"}
            ],
            "timestamp": "1700000000"
        }, {"event_type": "unknown_thing"}]"#;
        let evs = parse_user_message(text);
        assert_eq!(evs.len(), 1, "unknown events are skipped");
        match &evs[0] {
            UserEvent::Trade {
                trade_id,
                taker_order_id,
                maker_fills,
                size,
                price,
                status,
                ..
            } => {
                assert_eq!(trade_id, "trade-9");
                assert_eq!(taker_order_id.as_deref(), Some("0xtaker"));
                assert_eq!(maker_fills.len(), 2);
                assert_eq!(maker_fills[0].order_id, "0xmaker1");
                assert!((maker_fills[0].matched - 2.5).abs() < 1e-12);
                assert!((maker_fills[1].price - 0.41).abs() < 1e-12);
                assert!((*size - 6.0).abs() < 1e-12);
                assert!((*price - 0.41).abs() < 1e-12);
                assert_eq!(status, "MATCHED");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!evs[0].is_failed_trade());
        let failed = parse_user_message(
            r#"{"event_type":"trade","id":"t","status":"FAILED","taker_order_id":""}"#,
        );
        assert!(failed[0].is_failed_trade());
        assert!(matches!(
            &failed[0],
            UserEvent::Trade {
                taker_order_id: None,
                ..
            }
        ));
        assert!(parse_user_message("PONG").is_empty());
        assert!(
            parse_user_message(r#"{"event_type":"order"}"#).is_empty(),
            "no id → skipped"
        );
    }

    #[test]
    fn user_subscribe_frame_carries_credentials_and_markets() {
        let key = ApiKey {
            key: "k".into(),
            secret: "s".into(),
            passphrase: "p".into(),
        };
        let frame = user_subscribe_message(&key, &["0xa".to_string(), "0xb".to_string()]);
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["auth"]["apiKey"], "k");
        assert_eq!(v["auth"]["secret"], "s");
        assert_eq!(v["auth"]["passphrase"], "p");
        assert_eq!(v["markets"].as_array().unwrap().len(), 2);
    }
}
