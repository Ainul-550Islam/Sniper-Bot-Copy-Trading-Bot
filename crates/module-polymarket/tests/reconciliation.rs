//! Local-vs-CLOB reconciliation (TASK 4): orphans on the venue (reported or
//! cancelled by policy), local orders the venue lost (ambiguous → failed,
//! accepted → unknown), matched-size drift (booked once from venue truth),
//! positions without an order, stale resting orders — every finding
//! journaled, metered and audited.

mod common;

use std::sync::Arc;

use bot_core::models::{BotModule, ExecutionMode, Position, TradeSource, Venue};
use bot_core::oms::OrderStatus;
use common::*;
use module_polymarket::orders::{LocalOrderState, PolyStage};
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::ReconKind;
use serde_json::json;

#[tokio::test]
async fn orphan_venue_orders_are_reported_or_cancelled_by_policy() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let mut cfg = poly_cfg(&state).await;

    // The venue holds an order for our API key that we never tracked.
    venue.set_open_orders(vec![venue_order("0xorphan", "live", 0.0, 12.0)]);

    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, ReconKind::OrphanVenueOrder);
    assert_eq!(findings[0].venue_order_id.as_deref(), Some("0xorphan"));
    assert_eq!(findings[0].action, "reported");
    assert_eq!(
        venue.count("DELETE", "/clob/order"),
        0,
        "report-only by default"
    );

    // Opt-in: cancel orphans.
    cfg.reconcile_cancel_orphans = true;
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].action, "cancelled");
    assert_eq!(venue.count("DELETE", "/clob/order"), 1);
    assert_eq!(
        venue.last_body("DELETE", "/clob/order").unwrap()["orderID"],
        "0xorphan"
    );

    // Gone from the venue now → clean pass.
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert!(findings.is_empty(), "{findings:?}");

    // Journal + audit.
    let journaled = store.findings().await;
    assert_eq!(journaled.len(), 2);
    assert!(journaled.iter().all(|f| f.kind == "orphan_venue_order"));
    assert_eq!(journaled[1].action, "cancelled");
    let actions = audit_actions(&state).await;
    assert!(
        actions.iter().any(|a| a == "poly.recon.orphan_venue_order"),
        "{actions:?}"
    );
}

#[tokio::test]
async fn local_orders_the_venue_lost_resolve_by_prior_state() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    // A: ambiguous submit (HTTP 500) that never reached the venue.
    venue.set_post(PostBehaviour::ServerError);
    let a = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(a.stage, PolyStage::Ambiguous);
    let a_id = a.venue_order_id.clone().unwrap();

    // B: accepted (resting), later forgotten by the venue.
    venue.set_post(PostBehaviour::Accept {
        status: "live".into(),
    });
    let b = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(b.stage, PolyStage::Resting);
    let b_id = b.venue_order_id.clone().unwrap();

    // Venue: open list empty, both ids 404.
    venue.set_open_orders(vec![]);
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    let fa = findings
        .iter()
        .find(|f| f.venue_order_id.as_deref() == Some(a_id.as_str()))
        .expect("finding for A");
    let fb = findings
        .iter()
        .find(|f| f.venue_order_id.as_deref() == Some(b_id.as_str()))
        .expect("finding for B");
    assert_eq!(fa.kind, ReconKind::LocalOrderMissingOnVenue);
    assert_eq!(fa.action, "marked_failed");
    assert_eq!(fb.kind, ReconKind::LocalOrderMissingOnVenue);
    assert_eq!(fb.action, "marked_unknown");

    let tracked = bot.tracked_orders().await;
    let ta = tracked.iter().find(|t| t.venue_order_id == a_id).unwrap();
    let tb = tracked.iter().find(|t| t.venue_order_id == b_id).unwrap();
    assert_eq!(ta.state, LocalOrderState::Failed);
    assert_eq!(tb.state, LocalOrderState::Unknown);
    assert_eq!(
        oms.get(a.order_id.as_deref().unwrap())
            .await
            .unwrap()
            .status,
        OrderStatus::Failed
    );
    assert_eq!(
        oms.get(b.order_id.as_deref().unwrap())
            .await
            .unwrap()
            .status,
        OrderStatus::Unknown
    );
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
    assert!(state.find_open(BotModule::Polymarket, NO).await.is_none());
    assert_eq!(store.findings().await.len(), 2);
}

#[tokio::test]
async fn ambiguous_submit_found_on_the_venue_is_resolved_and_booked_once() {
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
    assert_eq!(out.stage, PolyStage::Ambiguous);
    let id = out.venue_order_id.clone().unwrap();

    // The venue actually has it, partially matched and still open.
    venue.set_open_orders(vec![venue_order(&id, "live", 10.0, 25.0)]);
    venue.set_order(&id, "live", 10.0, 25.0);
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.kind == ReconKind::AmbiguousSubmitResolved
                || f.kind == ReconKind::MatchedSizeMismatch),
        "{findings:?}"
    );
    let t = &bot.tracked_orders().await[0];
    assert_eq!(
        t.state,
        LocalOrderState::PartiallyFilled,
        "{findings:?} {t:?}"
    );
    assert!((t.size_matched - 10.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1);
    assert_eq!(store.fills().await[0].source, "recon");
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 10.0).abs() < 1e-9);
    // The OMS state machine only lets `Unknown` leave through a terminal
    // state, so the OMS record stays Unknown (still counted as open) while
    // the engine's own lifecycle carries the partially-filled truth.
    assert_eq!(
        oms.get(out.order_id.as_deref().unwrap())
            .await
            .unwrap()
            .status,
        OrderStatus::Unknown
    );

    // A second pass with the same venue truth changes nothing.
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!(store.fills().await.len(), 1);

    // Venue now fully matched and off the open list → resolved filled.
    venue.set_open_orders(vec![]);
    venue.set_order(&id, "matched", 25.0, 25.0);
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, ReconKind::LocalOrderMissingOnVenue);
    assert_eq!(findings[0].action, "resolved_filled");
    assert_eq!(bot.tracked_orders().await[0].state, LocalOrderState::Filled);
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
    assert_eq!(store.fills().await.len(), 2);
    assert_eq!(
        oms.get(out.order_id.as_deref().unwrap())
            .await
            .unwrap()
            .status,
        OrderStatus::Filled
    );
}

#[tokio::test]
async fn matched_size_drift_is_booked_from_venue_truth() {
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
    // Poll sees a resting order, no fills.
    venue.set_order(&id, "live", 0.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting
    );

    // The open list says 8 matched (a missed user-ws event).
    venue.set_open_orders(vec![venue_order(&id, "live", 8.0, 25.0)]);
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, ReconKind::MatchedSizeMismatch);
    assert_eq!(findings[0].action, "resolved_booked");
    assert!(
        findings[0]
            .detail
            .contains("venue matched 8.0000 vs local 0.0000"),
        "{}",
        findings[0].detail
    );
    assert!((bot.tracked_orders().await[0].size_matched - 8.0).abs() < 1e-9);
    assert!(
        (state
            .find_open(BotModule::Polymarket, YES)
            .await
            .unwrap()
            .qty
            - 8.0)
            .abs()
            < 1e-9
    );
    assert_eq!(store.fills().await.len(), 1);
}

#[tokio::test]
async fn positions_without_orders_and_stale_orders_are_reported() {
    let venue = mock_venue().await;
    let mut cfg = live_config(&venue);
    cfg.polymarket.order_ttl_secs = 1;
    let (state, _oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    // A restored position from before this engine (no order behind it).
    let mut legacy = Position::new(
        "p_legacy".into(),
        TradeSource::Polymarket,
        Venue::PolymarketClob,
        ExecutionMode::Live,
        "777".into(),
        "Legacy Yes".into(),
        "USDC".into(),
    );
    legacy.apply_buy(10.0, 0.5, 5.0);
    legacy.market_id = Some("0xlegacy".into());
    state.upsert_position(legacy).await;

    // A resting order that outlives its TTL while the venue refuses cancels.
    venue.set_cancel(CancelBehaviour::Refuse {
        reason: "maintenance".into(),
    });
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    let id = out.venue_order_id.clone().unwrap();
    venue.set_order(&id, "live", 0.0, 25.0);
    venue.set_open_orders(vec![venue_order(&id, "live", 0.0, 25.0)]);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    bot.poll_orders_once(&cfg).await.unwrap();
    assert_eq!(
        bot.tracked_orders().await[0].state,
        LocalOrderState::Resting,
        "cancel refused"
    );

    let findings = bot.reconcile_once(&cfg).await.unwrap();
    let kinds: Vec<ReconKind> = findings.iter().map(|f| f.kind).collect();
    assert!(
        kinds.contains(&ReconKind::PositionWithoutOrder),
        "{findings:?}"
    );
    assert!(kinds.contains(&ReconKind::StaleOrder), "{findings:?}");
    let pwo = findings
        .iter()
        .find(|f| f.kind == ReconKind::PositionWithoutOrder)
        .unwrap();
    assert_eq!(pwo.token_id.as_deref(), Some("777"));
    assert_eq!(pwo.action, "reported");
    assert!(
        state.position("p_legacy").await.is_some(),
        "reporting never closes a position"
    );

    let journaled = store.findings().await;
    assert!(journaled.iter().any(|f| f.kind == "position_without_order"));
    assert!(journaled.iter().any(|f| f.kind == "stale_order"));
    assert!(journaled.iter().all(|f| !f.replica_id.is_empty()));
}

#[tokio::test]
async fn paper_mode_reconciliation_never_touches_the_venue() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled);
    // Venue would report an orphan — paper mode does not ask.
    venue.set_open_orders(vec![
        json!({"id": "0xorphan", "status": "live", "asset_id": YES}),
    ]);
    let findings = bot.reconcile_once(&cfg).await.unwrap();
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!(venue.count("GET", "/clob/data/orders"), 0);
    assert!(store.findings().await.is_empty());
}
