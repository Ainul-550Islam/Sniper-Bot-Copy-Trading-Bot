//! Authenticated user channel (TASK 4): order/trade events applied to
//! tracked orders (cumulative order updates, incremental trade deltas,
//! replay + FAILED + untracked handling) and the real websocket feed task
//! against a mock server (auth frame, parsing, delivery, clean stop).

mod common;

use std::sync::Arc;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::oms::OrderStatus;
use chrono::Utc;
use common::*;
use module_polymarket::orders::{LocalOrderState, PolyStage, VenueOrderState};
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::ws::{parse_user_message, run_user_feed, MakerFill, UserEvent};
use serde_json::{json, Value};
use tokio::sync::mpsc;

fn order_event(order_id: &str, status: &str, size_matched: f64) -> UserEvent {
    UserEvent::Order {
        order_id: order_id.to_string(),
        asset_id: YES.into(),
        market: CONDITION.into(),
        state: VenueOrderState::parse(status),
        raw_status: status.to_string(),
        size_matched: Some(size_matched),
        original_size: Some(25.0),
        price: Some(0.40),
        update_type: "UPDATE".into(),
        ts: Some(Utc::now()),
        associate_trades: Vec::new(),
    }
}

fn trade_event(
    trade_id: &str,
    taker: Option<&str>,
    makers: Vec<MakerFill>,
    size: f64,
    status: &str,
) -> UserEvent {
    UserEvent::Trade {
        trade_id: trade_id.to_string(),
        taker_order_id: taker.map(String::from),
        maker_fills: makers,
        asset_id: YES.into(),
        market: CONDITION.into(),
        side: "BUY".into(),
        size,
        price: 0.40,
        status: status.to_string(),
        ts: Some(Utc::now()),
    }
}

#[tokio::test]
async fn order_events_apply_cumulative_matched_size() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting);
    let id = out.venue_order_id.clone().unwrap();
    let order_id = out.order_id.clone().unwrap();

    // PLACEMENT then a partial UPDATE then MATCHED.
    bot.apply_user_event(&order_event(&id, "LIVE", 0.0))
        .await
        .unwrap();
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting
    );
    bot.apply_user_event(&order_event(&id, "LIVE", 10.0))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert!((t.size_matched - 10.0).abs() < 1e-9);
    assert!(
        (state
            .find_open(BotModule::Polymarket, YES)
            .await
            .unwrap()
            .qty
            - 10.0)
            .abs()
            < 1e-9
    );
    // A stale/duplicate update with a lower cumulative never un-books.
    bot.apply_user_event(&order_event(&id, "LIVE", 4.0))
        .await
        .unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 10.0).abs() < 1e-9);
    bot.apply_user_event(&order_event(&id, "MATCHED", 25.0))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Filled);
    assert!(
        (state
            .find_open(BotModule::Polymarket, YES)
            .await
            .unwrap()
            .qty
            - 25.0)
            .abs()
            < 1e-9
    );
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Filled
    );
    assert_eq!(store.fills().await.len(), 2);
    assert!(store.fills().await.iter().all(|f| f.source == "user_ws"));

    // An over-fill from the venue is a lifecycle error, not a fill.
    let err = bot
        .apply_user_event(&order_event(&id, "MATCHED", 30.0))
        .await;
    assert!(err.is_err() || bot.tracked_orders().await[0].size_matched <= 25.0);
    assert!(
        (state
            .find_open(BotModule::Polymarket, YES)
            .await
            .unwrap()
            .qty
            - 25.0)
            .abs()
            < 1e-9
    );

    // Cancellation events for an already-terminal order change nothing.
    bot.apply_user_event(&order_event(&id, "CANCELLED", 25.0))
        .await
        .unwrap();
    assert_eq!(bot.tracked_orders().await[0].state, LocalOrderState::Filled);
}

#[tokio::test]
async fn trade_events_book_deltas_once_and_ignore_failed_or_unknown() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let id = out.venue_order_id.clone().unwrap();

    // We are the taker of a 6-token trade.
    bot.apply_user_event(&trade_event("t-1", Some(&id), vec![], 6.0, "MATCHED"))
        .await
        .unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 6.0).abs() < 1e-9);
    // The venue re-emits the same trade as it moves MATCHED → MINED →
    // CONFIRMED: never booked twice.
    bot.apply_user_event(&trade_event("t-1", Some(&id), vec![], 6.0, "MINED"))
        .await
        .unwrap();
    bot.apply_user_event(&trade_event("t-1", Some(&id), vec![], 6.0, "CONFIRMED"))
        .await
        .unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 6.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1);

    // A FAILED trade is not a fill.
    bot.apply_user_event(&trade_event("t-2", Some(&id), vec![], 5.0, "FAILED"))
        .await
        .unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 6.0).abs() < 1e-9);

    // We are one of the makers: only our matched amount counts.
    let makers = vec![
        MakerFill {
            order_id: "0xsomeoneelse".into(),
            matched: 3.0,
            price: 0.40,
        },
        MakerFill {
            order_id: id.clone(),
            matched: 4.0,
            price: 0.40,
        },
    ];
    bot.apply_user_event(&trade_event("t-3", Some("0xtaker"), makers, 7.0, "MATCHED"))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 10.0).abs() < 1e-9, "{t:?}");
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert_eq!(store.fills().await.len(), 2);
    assert_eq!(
        store
            .fills()
            .await
            .iter()
            .filter(|f| f.fill_id == "trade:t-3")
            .count(),
        1
    );
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 10.0).abs() < 1e-9);

    // Events for orders we do not track are ignored (another process on
    // the same key); reconciliation reports them as orphans instead.
    bot.apply_user_event(&order_event("0xnotours", "LIVE", 1.0))
        .await
        .unwrap();
    bot.apply_user_event(&trade_event(
        "t-4",
        Some("0xnotours"),
        vec![],
        1.0,
        "MATCHED",
    ))
    .await
    .unwrap();
    assert_eq!(bot.tracked_orders().await.len(), 1);
    assert_eq!(store.fills().await.len(), 2);
}

#[tokio::test]
async fn trade_events_a_poll_already_captured_are_not_booked_twice() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting);
    let id = out.venue_order_id.clone().unwrap();
    let order_id = out.order_id.clone().unwrap();

    // The poll sees the fill first (the user channel was reconnecting).
    venue.set_order(&id, "live", 6.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 6.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1);

    // The late trade event for that same fill adds nothing …
    bot.apply_user_event(&trade_event("t-1", Some(&id), vec![], 6.0, "MINED"))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 6.0).abs() < 1e-9, "{t:?}");
    assert!(t.booked_trade_ids.contains(&"t-1".to_string()));
    assert_eq!(store.fills().await.len(), 1);
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 6.0).abs() < 1e-9);

    // … while a genuinely new trade books in full at once.
    bot.apply_user_event(&trade_event("t-2", Some(&id), vec![], 4.0, "MATCHED"))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 10.0).abs() < 1e-9, "{t:?}");
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert_eq!(store.fills().await.len(), 2);
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::PartiallyFilled
    );

    // A poll that names its trades adds nothing and remembers the ids.
    venue.set_order_with_trades(&id, "live", 10.0, 25.0, &["t-1", "t-2"]);
    bot.poll_orders_once(&cfg).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 10.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 2);
    for replay in ["t-1", "t-2"] {
        bot.apply_user_event(&trade_event(replay, Some(&id), vec![], 6.0, "CONFIRMED"))
            .await
            .unwrap();
    }
    assert!((bot.tracked_orders().await[0].size_matched - 10.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 2);

    // A poll that reports a trade the channel never delivered (t-3) books
    // it from venue truth; the trade event arriving afterwards is a no-op.
    venue.set_order_with_trades(&id, "live", 15.0, 25.0, &["t-1", "t-2", "t-3"]);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 15.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 3);
    bot.apply_user_event(&trade_event("t-3", Some(&id), vec![], 5.0, "MATCHED"))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 15.0).abs() < 1e-9, "{t:?}");
    assert_eq!(store.fills().await.len(), 3);
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 15.0).abs() < 1e-9, "{pos:?}");

    // An `order` event that names its trades behaves like the poll.
    let mut ev = order_event(&id, "LIVE", 18.0);
    if let UserEvent::Order {
        associate_trades, ..
    } = &mut ev
    {
        *associate_trades = vec!["t-1".into(), "t-2".into(), "t-3".into(), "t-4".into()];
    }
    bot.apply_user_event(&ev).await.unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 18.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 4);
    bot.apply_user_event(&trade_event("t-4", Some(&id), vec![], 3.0, "MATCHED"))
        .await
        .unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 18.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 4);
}

#[tokio::test]
async fn user_feed_authenticates_parses_and_delivers_over_a_real_socket() {
    // Mock user channel: records the subscribe frame, replies with one
    // order event and one trade event (array framing), then idles.
    let (frames_tx, mut frames_rx) = mpsc::channel::<String>(8);
    async fn handler(
        ws: WebSocketUpgrade,
        axum::extract::State(tx): axum::extract::State<mpsc::Sender<String>>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |mut socket: WebSocket| async move {
            if let Some(Ok(Message::Text(sub))) = socket.recv().await {
                let _ = tx.send(sub).await;
            }
            let order = json!({
                "event_type": "order", "id": "0xabc", "asset_id": YES, "market": CONDITION,
                "status": "LIVE", "size_matched": "5", "original_size": "25", "price": "0.4",
                "type": "UPDATE", "timestamp": "1700000000"
            });
            let trade = json!([{
                "event_type": "trade", "id": "trade-1", "taker_order_id": "0xabc",
                "asset_id": YES, "market": CONDITION, "side": "BUY", "size": "5",
                "price": "0.4", "status": "MATCHED", "maker_orders": [], "timestamp": "1700000001"
            }]);
            let _ = socket.send(Message::Text(order.to_string())).await;
            let _ = socket.send(Message::Text(trade.to_string())).await;
            // Keep the socket open until the client goes away.
            while let Some(Ok(msg)) = socket.recv().await {
                if matches!(msg, Message::Close(_)) {
                    break;
                }
            }
        })
    }
    let app = Router::new()
        .route("/ws/user", get(handler))
        .with_state(frames_tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(live_config(&venue));
    let (tx, mut rx) = mpsc::channel::<UserEvent>(16);
    let feed = tokio::spawn(run_user_feed(
        format!("ws://{addr}/ws/user"),
        test_api_key(),
        vec![CONDITION.to_string()],
        tx,
        state.clone(),
    ));

    // The subscribe frame carries the credentials and the markets.
    let sub = tokio::time::timeout(std::time::Duration::from_secs(5), frames_rx.recv())
        .await
        .expect("subscribe frame in time")
        .unwrap();
    let sub: Value = serde_json::from_str(&sub).unwrap();
    assert_eq!(sub["type"], "user");
    assert_eq!(sub["auth"]["apiKey"], "mock-key");
    assert_eq!(sub["auth"]["passphrase"], "mock-pass");
    assert_eq!(sub["markets"][0], CONDITION);

    // Both events arrive parsed, in order.
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("order event in time")
        .unwrap();
    match &first {
        UserEvent::Order {
            order_id,
            state,
            size_matched,
            ..
        } => {
            assert_eq!(order_id, "0xabc");
            assert_eq!(*state, VenueOrderState::Live);
            assert_eq!(*size_matched, Some(5.0));
        }
        other => panic!("expected order event, got {other:?}"),
    }
    let second = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("trade event in time")
        .unwrap();
    match &second {
        UserEvent::Trade {
            trade_id,
            taker_order_id,
            size,
            ..
        } => {
            assert_eq!(trade_id, "trade-1");
            assert_eq!(taker_order_id.as_deref(), Some("0xabc"));
            assert!((*size - 5.0).abs() < 1e-12);
        }
        other => panic!("expected trade event, got {other:?}"),
    }
    // Direct parser check on the same framing the server used.
    assert_eq!(
        parse_user_message(r#"{"event_type":"order"}"#).len(),
        0,
        "id-less events are skipped"
    );

    // Dropping the receiver stops the feed task cleanly (no reconnect loop).
    drop(rx);
    let done = tokio::time::timeout(std::time::Duration::from_secs(15), feed).await;
    assert!(done.is_ok(), "feed task stops once the consumer is gone");
    assert!(done.unwrap().unwrap().is_ok());
}

#[tokio::test]
async fn bot_channel_sender_feeds_apply_user_event_end_to_end() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let id = out.venue_order_id.clone().unwrap();

    // The bot's own channel accepts events (the run loop drains it); here
    // we prove the sender is live and the same event applied directly has
    // the documented effect.
    let tx = bot.user_event_sender();
    assert!(tx.try_send(order_event(&id, "LIVE", 0.0)).is_ok());
    bot.apply_user_event(&order_event(&id, "MATCHED", 25.0))
        .await
        .unwrap();
    assert_eq!(bot.tracked_orders().await[0].state, LocalOrderState::Filled);
    assert_eq!(store.fills().await.len(), 1);
}
