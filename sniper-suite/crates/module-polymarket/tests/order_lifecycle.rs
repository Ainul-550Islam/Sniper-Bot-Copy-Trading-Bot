//! Live order lifecycle (TASK 4) against the mock CLOB: signed + L2-
//! authenticated POST, resting → partial → filled via status polling,
//! cancel on TTL with the partial fill kept, definite venue rejection,
//! transport-ambiguous submit held as Unknown and resolved by asking the
//! venue, GTD expiry and reprice cancels.

mod common;

use std::sync::Arc;

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::oms::OrderStatus;
use common::*;
use module_polymarket::orders::{LocalOrderState, PolyStage, RejectReason};
use module_polymarket::store::MemoryPolyStore;

#[tokio::test]
async fn live_order_rests_then_fills_through_polling() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let sig = signal(decision(), ExecutionMode::Live);
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let venue_id = out.venue_order_id.clone().expect("derived venue order id");
    assert!(
        venue_id.starts_with("0x") && venue_id.len() == 66,
        "{venue_id}"
    );
    let order_id = out.order_id.clone().unwrap();

    // The POST carried the signed order + L2 headers and the configured
    // order type.
    assert_eq!(venue.posted_orders(), 1);
    let posted = venue.last_body("POST", "/clob/order").unwrap();
    assert_eq!(posted["orderType"], "GTC");
    assert_eq!(posted["order"]["tokenId"], YES);
    assert_eq!(posted["order"]["side"], "BUY");
    assert_eq!(
        posted["order"]["maker"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase(),
        test_address()
    );
    let cap = venue
        .captures()
        .into_iter()
        .find(|c| c.method == "POST" && c.path == "/clob/order")
        .unwrap();
    assert_eq!(
        cap.poly_headers.get("POLY_API_KEY").map(String::as_str),
        Some("mock-key")
    );
    assert!(cap.poly_headers.contains_key("POLY_SIGNATURE"));

    // OMS: Accepted (venue live), external id + signature attached.
    let order = oms.get(&order_id).await.unwrap();
    assert_eq!(order.status, OrderStatus::Accepted, "{order:?}");
    assert_eq!(order.external_id.as_deref(), Some(venue_id.as_str()));
    assert!(order
        .signature
        .as_deref()
        .is_some_and(|s| s.starts_with("0x")));
    assert!(
        state.find_open(BotModule::Polymarket, YES).await.is_none(),
        "no fill yet"
    );

    // Venue reports a partial fill: 10 of 25.
    venue.set_order(&venue_id, "live", 10.0, 25.0);
    let polled = bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(polled, 1);
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert!((t.size_matched - 10.0).abs() < 1e-9);
    let pos = state
        .find_open(BotModule::Polymarket, YES)
        .await
        .expect("partial books a position");
    assert!((pos.qty - 10.0).abs() < 1e-9);
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::PartiallyFilled
    );
    assert_eq!(store.fills().await.len(), 1);

    // Same answer again: nothing is booked twice.
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(store.fills().await.len(), 1);
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

    // Full fill.
    venue.set_order(&venue_id, "matched", 25.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Filled);
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 25.0).abs() < 1e-9);
    assert!((pos.cost_basis - 10.0).abs() < 1e-6, "{pos:?}");
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Filled
    );
    assert_eq!(store.fills().await.len(), 2);
    assert_eq!(fill_events(&state).await, 2);
    let journaled = store.orders().await;
    assert_eq!(journaled[0].state, "filled");
    assert!((journaled[0].size_matched - 25.0).abs() < 1e-9);

    // Terminal orders are not polled again.
    assert_eq!(bot.poll_orders_once(&cfg).await.unwrap(), 0);
}

#[tokio::test]
async fn ttl_cancel_keeps_the_partial_fill_and_releases_the_rest() {
    let venue = mock_venue().await;
    let mut cfg = live_config(&venue);
    cfg.polymarket.order_ttl_secs = 1;
    let (state, oms) = state_with_oms(cfg);
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
    let venue_id = out.venue_order_id.clone().unwrap();
    let order_id = out.order_id.clone().unwrap();
    venue.set_order(&venue_id, "live", 5.0, 25.0);

    // A resting BUY is counted as committed collateral.
    let t = &bot.tracked_orders().await[0];
    assert!((t.resting_quote() - 10.0).abs() < 1e-9);

    // TTL elapses → poll books the partial, then cancels.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(venue.count("DELETE", "/clob/order"), 1);
    let cancel = venue.last_body("DELETE", "/clob/order").unwrap();
    assert_eq!(cancel["orderID"], venue_id);

    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Cancelled);
    assert!((t.size_matched - 5.0).abs() < 1e-9, "partial fill kept");
    assert!(
        (t.resting_quote()).abs() < 1e-12,
        "nothing committed after cancel"
    );
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 5.0).abs() < 1e-9);
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Cancelled
    );
    assert_eq!(store.orders().await[0].state, "cancelled");
    let actions = audit_actions(&state).await;
    assert!(
        actions.iter().any(|a| a == "poly.order.cancelled"),
        "{actions:?}"
    );

    // A late venue "matched" for the same order does not resurrect it.
    venue.set_order(&venue_id, "matched", 25.0, 25.0);
    assert_eq!(bot.poll_orders_once(&cfg).await.unwrap(), 0);
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Cancelled
    );
}

#[tokio::test]
async fn venue_rejection_fails_the_order_and_frees_the_token() {
    let venue = mock_venue().await;
    venue.set_post(PostBehaviour::Reject {
        msg: "invalid signature".into(),
    });
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
    assert_eq!(out.stage, PolyStage::Failed, "{out:?}");
    assert_eq!(out.reject_reason, Some(RejectReason::VenueRejected));
    assert!(out.detail.contains("invalid signature"));
    let order = oms.get(out.order_id.as_deref().unwrap()).await.unwrap();
    assert_eq!(order.status, OrderStatus::Failed);
    assert_eq!(bot.tracked_orders().await[0].state, LocalOrderState::Failed);
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
    assert_eq!(store.signals().await[0].stage, "FAILED");

    // The token is free again: a new (different) intent may try once the
    // venue accepts.
    venue.set_post(PostBehaviour::Accept {
        status: "live".into(),
    });
    let out = bot
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.39, 25.0), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
}

#[tokio::test]
async fn ambiguous_submit_is_held_unknown_until_the_venue_answers() {
    let venue = mock_venue().await;
    venue.set_post(PostBehaviour::ServerError);
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
    assert_eq!(out.stage, PolyStage::Ambiguous, "{out:?}");
    assert_eq!(out.reject_reason, Some(RejectReason::SubmitUnknown));
    let venue_id = out.venue_order_id.clone().unwrap();
    let order_id = out.order_id.clone().unwrap();
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Unknown
    );
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Unknown
    );
    assert_eq!(store.orders().await[0].state, "unknown");

    // While unknown, the token is blocked for new entries (an order may
    // rest on the venue).
    let out2 = bot
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.39, 25.0), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(
        out2.reject_reason,
        Some(RejectReason::OrderAlreadyOpen),
        "{out2:?}"
    );
    assert_eq!(venue.posted_orders(), 1, "no second POST while ambiguous");

    // The venue turns out to hold it, fully matched → booked exactly once.
    venue.set_order(&venue_id, "matched", 25.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Filled);
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 25.0).abs() < 1e-9);
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Filled
    );
    assert_eq!(store.fills().await.len(), 1);
}

#[tokio::test]
async fn ambiguous_submit_that_never_reached_the_venue_fails_cleanly() {
    let venue = mock_venue().await;
    venue.set_post(PostBehaviour::ServerError);
    let (state, oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let mut cfg = poly_cfg(&state).await;
    cfg.order_poll_interval_secs = 0;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Ambiguous);
    let order_id = out.order_id.clone().unwrap();

    // 404 from the venue for an order that was never accepted → Failed.
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(bot.tracked_orders().await[0].state, LocalOrderState::Failed);
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Failed
    );
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
}

#[tokio::test]
async fn accepted_order_that_vanishes_is_marked_unknown_not_filled() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let venue_id = out.venue_order_id.clone().unwrap();
    venue.set_order(&venue_id, "live", 0.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting
    );

    // The venue forgets the order: we do not invent a fill or a cancel.
    venue.remove_order(&venue_id);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Unknown
    );
    assert_eq!(
        oms.get(out.order_id.as_deref().unwrap())
            .await
            .unwrap()
            .status,
        OrderStatus::Unknown
    );
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
}

#[tokio::test]
async fn reprice_and_expiry_cancel_resting_orders() {
    let venue = mock_venue().await;
    let mut cfg = live_config(&venue);
    cfg.polymarket.reprice_threshold = 0.05;
    let (state, _oms) = state_with_oms(cfg);
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let cfg = poly_cfg(&state).await;

    // Resting order at 0.40; the book (REST fallback) sits at 0.39/0.40, so
    // no reprice yet.
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let venue_id = out.venue_order_id.clone().unwrap();
    venue.set_order(&venue_id, "live", 0.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(venue.count("DELETE", "/clob/order"), 0);
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting
    );

    // The market runs away (best ask 0.50 > limit + threshold) → cancel.
    bot.ingest_quote(
        YES,
        module_polymarket::strategy::Quote::observed(0.49, 0.50, chrono::Utc::now()),
    )
    .await;
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(venue.count("DELETE", "/clob/order"), 1);
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Cancelled
    );

    // GTD: an order whose expiry has passed is cancelled locally too.
    let mut expired = signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Live);
    expired.expiration = 1_700_000_000; // long past
    let out = bot
        .process_signal(&expired, &market(), &quotes(), &cfg)
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let venue_id = out.venue_order_id.clone().unwrap();
    venue.set_order(&venue_id, "live", 0.0, 20.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(venue.count("DELETE", "/clob/order"), 2);
    let t = bot
        .tracked_orders()
        .await
        .into_iter()
        .find(|t| t.venue_order_id == venue_id)
        .unwrap();
    assert_eq!(t.state, LocalOrderState::Cancelled);
}

#[tokio::test]
async fn refused_cancel_keeps_the_order_open() {
    let venue = mock_venue().await;
    venue.set_cancel(CancelBehaviour::Refuse {
        reason: "order already matched".into(),
    });
    let (state, _oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let venue_id = out.venue_order_id.clone().unwrap();
    venue.set_order(&venue_id, "live", 0.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();

    let confirmed = bot
        .cancel_tracked_order(&venue_id, "operator")
        .await
        .unwrap();
    assert!(!confirmed);
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting
    );
    assert_eq!(bot.cancel_all_tracked("shutdown").await, 0);

    // Unknown ids are a lifecycle error, paper orders are never cancellable.
    assert!(bot.cancel_tracked_order("0xnope", "x").await.is_err());
}

#[tokio::test]
async fn unrecognised_cancel_answer_is_never_a_confirmation() {
    let venue = mock_venue().await;
    venue.set_cancel(CancelBehaviour::Unrecognised);
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
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let venue_id = out.venue_order_id.clone().unwrap();
    let order_id = out.order_id.clone().unwrap();
    venue.set_order(&venue_id, "live", 0.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();

    // `{}` from DELETE /order while GET /data/order still says `live`: not
    // confirmed — the order stays open locally and in the OMS.
    let confirmed = bot
        .cancel_tracked_order(&venue_id, "operator")
        .await
        .unwrap();
    assert!(!confirmed);
    assert_eq!(venue.count("DELETE", "/clob/order"), 1);
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting
    );
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Accepted
    );

    // Same unrecognised answer, but the venue now reports the order
    // partially filled then cancelled: the follow-up status read applies
    // venue truth — the 5 matched are booked, the cancel is confirmed.
    venue.set_order(&venue_id, "cancelled", 5.0, 25.0);
    let confirmed = bot
        .cancel_tracked_order(&venue_id, "operator")
        .await
        .unwrap();
    assert!(confirmed);
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Cancelled);
    assert!((t.size_matched - 5.0).abs() < 1e-9, "partial fill kept");
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Cancelled
    );
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 5.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1);

    // A venue that no longer reports the order after `{}` is not a
    // confirmation either: poll / reconciliation own that outcome.
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let gone = out.venue_order_id.clone().unwrap();
    venue.set_order(&gone, "live", 0.0, 20.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    venue.remove_order(&gone);
    let confirmed = bot.cancel_tracked_order(&gone, "operator").await.unwrap();
    assert!(!confirmed);
    let t = bot
        .tracked_orders()
        .await
        .into_iter()
        .find(|t| t.venue_order_id == gone)
        .unwrap();
    assert_eq!(t.state, LocalOrderState::Resting);
}

#[tokio::test]
async fn fak_matched_books_only_the_venue_reported_quantity() {
    let venue = mock_venue().await;
    // The POST answers `matched` — status only, no quantity (FAK may have
    // matched any part of the order and killed the rest).
    venue.set_post(PostBehaviour::Accept {
        status: "matched".into(),
    });
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    state
        .update_config(|c| c.polymarket.order_type = "FAK".into())
        .await;
    let cfg = poly_cfg(&state).await;

    let sig = bot.build_signal(decision(), "value", &market(), &cfg).await;
    assert_eq!(sig.order_type, "FAK");
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
    // Accepted, but nothing is booked from a status-only `matched`: the
    // venue could not yet be read for the quantity (GET → 404).
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let venue_id = out.venue_order_id.clone().unwrap();
    let order_id = out.order_id.clone().unwrap();
    assert_eq!(
        venue.count("GET", "/clob/data/order"),
        1,
        "one immediate quantity lookup after the FAK matched"
    );
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Submitted);
    assert_eq!(t.venue_status, "matched");
    assert!(t.venue_acknowledged());
    assert_eq!(t.size_matched, 0.0);
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
    assert!(store.fills().await.is_empty());

    // A poll while the venue still cannot answer: the order was
    // acknowledged (`matched`), so it is held as unknown — never failed,
    // never filled by assumption.
    let mut fast = cfg.clone();
    fast.order_poll_interval_secs = 0;
    bot.poll_orders_once(&fast).await.unwrap();
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Unknown
    );
    assert!(store.fills().await.is_empty());

    // The venue then reports the real quantity: 7 of 25 matched, the rest
    // killed. Exactly 7 are booked and the order is terminal.
    venue.set_order(&venue_id, "matched", 7.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.state, LocalOrderState::Filled);
    assert!((t.size_matched - 7.0).abs() < 1e-9, "{t:?}");
    assert_eq!(t.resting_quote(), 0.0);
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 7.0).abs() < 1e-9, "{pos:?}");
    let fills = store.fills().await;
    assert_eq!(fills.len(), 1);
    assert!((fills[0].size_tokens - 7.0).abs() < 1e-9);
    assert_eq!(
        oms.get(&order_id).await.unwrap().status,
        OrderStatus::Filled
    );

    // Late status-only `matched` observations change nothing.
    bot.poll_orders_once(&cfg).await.unwrap();
    assert!((bot.tracked_orders().await[0].size_matched - 7.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1);

    // When the venue CAN answer right after the POST, the quantity is
    // booked from that answer in the same pipeline call. The other outcome:
    // its venue id is deterministic, so script the answer first.
    let sig = bot
        .build_signal(decision_for(NO, "No", 0.55, 20.0), "value", &market(), &cfg)
        .await;
    let (bundle, _) = sign_for(&sig, 20.0, &cfg);
    let no_id = bundle.derived_order_id().unwrap();
    venue.set_order(&no_id, "matched", 12.0, 20.0);
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert_eq!(out.venue_order_id.as_deref(), Some(no_id.as_str()));
    let t = bot
        .tracked_orders()
        .await
        .into_iter()
        .find(|t| t.token_id == NO)
        .unwrap();
    assert_eq!(t.state, LocalOrderState::Filled);
    assert!((t.size_matched - 12.0).abs() < 1e-9, "{t:?}");
    let pos = state.find_open(BotModule::Polymarket, NO).await.unwrap();
    assert!((pos.qty - 12.0).abs() < 1e-9, "{pos:?}");
    assert_eq!(store.fills().await.len(), 2);
}

#[tokio::test]
async fn cancel_all_reflects_the_venue_wipe_locally() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let cfg = poly_cfg(&state).await;

    let a = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let b = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(a.stage, PolyStage::Resting);
    assert_eq!(b.stage, PolyStage::Resting);
    let a_id = a.venue_order_id.clone().unwrap();
    let b_id = b.venue_order_id.clone().unwrap();

    // The venue's answer names what it actually cancelled: only `a` is on
    // its book. `b` is NOT assumed cancelled — it stays open locally until
    // venue truth (poll / reconciliation) says otherwise.
    venue.set_open_orders(vec![venue_order(&a_id, "live", 0.0, 25.0)]);
    bot.cancel_all().await.unwrap();
    assert_eq!(venue.count("DELETE", "/clob/cancel-all"), 1);
    let state_of = |id: &str, tracked: &[module_polymarket::orders::TrackedOrder]| {
        tracked
            .iter()
            .find(|t| t.venue_order_id.eq_ignore_ascii_case(id))
            .map(|t| t.state)
            .unwrap()
    };
    let tracked = bot.tracked_orders().await;
    assert_eq!(state_of(&a_id, &tracked), LocalOrderState::Cancelled);
    assert_eq!(
        state_of(&b_id, &tracked),
        LocalOrderState::Resting,
        "an order the venue did not confirm cancelled is never closed by assumption"
    );
    assert_eq!(
        oms.get(&a.order_id.clone().unwrap()).await.unwrap().status,
        OrderStatus::Cancelled
    );
    assert_eq!(
        oms.get(&b.order_id.clone().unwrap()).await.unwrap().status,
        OrderStatus::Accepted
    );

    // The venue then reports `b` cancelled: the poll closes it from truth.
    venue.set_order(&b_id, "cancelled", 0.0, 20.0);
    let cfg = poly_cfg(&state).await;
    bot.poll_orders_once(&cfg).await.unwrap();
    for t in bot.tracked_orders().await {
        assert_eq!(t.state, LocalOrderState::Cancelled, "{t:?}");
    }
    for o in oms.list(10).await {
        assert_eq!(o.status, OrderStatus::Cancelled);
    }
}
