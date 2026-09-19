//! CLOB market websocket — live order-book snapshots for tracked tokens.
//!
//! Connects to the `market` channel, subscribes to a set of CTF token ids, and
//! folds `book` events into a shared [`QuoteMap`] the strategy reads. The socket
//! is kept alive with periodic `PING`s and reconnected with backoff; a dropped
//! feed degrades to the last known quotes rather than stopping the bot.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use bot_core::models::BotModule;
use bot_core::state::Shared;

use crate::clob::{BookLevel, OrderBook};
use crate::error::PolyResult;
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
/// `price_change` deltas are handled by the next snapshot.
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
        timestamp: None,
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
}
