//! Restart recovery (TASK 4): open venue orders come back from the journal
//! with their booked quantity (no double booking), ambiguous submits are
//! held for reconciliation, OMS-only incomplete orders are adopted as
//! Unknown, stale paper rows are failed, recovery is idempotent, and the
//! re-adopted orders immediately gate new entries.

mod common;

use std::sync::Arc;

use bot_core::models::{BotModule, ExecutionMode, Position, TradeSource, Venue};
use bot_core::oms::{OrderDraft, OrderStatus};
use chrono::{Duration, Utc};
use common::*;
use module_polymarket::orders::{FillSource, LocalOrderState, PolyStage, RejectReason};
use module_polymarket::store::{MemoryPolyStore, PolyFillRecord, PolyOrderRecord, PolyStore};
use module_polymarket::ws::UserEvent;
use module_polymarket::RecoveryAction;

fn journal_row(
    venue_id: &str,
    order_id: &str,
    token: &str,
    state: &str,
    mode: &str,
    matched: f64,
) -> PolyOrderRecord {
    let t0 = Utc::now() - Duration::minutes(10);
    PolyOrderRecord {
        venue_order_id: venue_id.into(),
        order_id: order_id.into(),
        signal_id: format!("psig_{venue_id}"),
        condition_id: CONDITION.into(),
        token_id: token.into(),
        outcome: "Yes".into(),
        side: "buy".into(),
        order_type: "GTC".into(),
        limit_price: 0.40,
        size_tokens: 25.0,
        size_matched: matched,
        mode: mode.into(),
        state: state.into(),
        // What the engine journals for that state: the write-ahead row of a
        // POST in flight carries no venue status yet, an ambiguous submit
        // carries the local `submit_unknown` marker, anything the venue
        // acknowledged carries its last venue status.
        venue_status: match state {
            "submitted" => String::new(),
            "unknown" => "submit_unknown".into(),
            _ => "live".into(),
        },
        expiration: 0,
        position_id: None,
        replica_id: "old-replica".into(),
        submitted_at: t0,
        updated_at: t0,
        closed_at: None,
    }
}

#[tokio::test]
async fn journal_orders_are_readopted_with_their_booked_quantity() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());

    // Before the crash: OMS order accepted, 4 of 25 booked into position p_x.
    let order = oms
        .create(OrderDraft {
            idempotency_key: "intent-a".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: YES.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Live,
            qty: 25.0,
            price: Some(0.40),
            meta: serde_json::json!({"signal_id": "psig_0xaaa", "condition_id": CONDITION}),
        })
        .await
        .unwrap();
    oms.attach_external(&order.id, Some("0xaaa".into()), Some("0xsig".into()))
        .await
        .unwrap();
    oms.transition(&order.id, OrderStatus::Submitted, None)
        .await
        .unwrap();
    oms.transition(&order.id, OrderStatus::PartiallyFilled, None)
        .await
        .unwrap();
    let mut row = journal_row("0xaaa", &order.id, YES, "partially_filled", "live", 4.0);
    row.position_id = Some("p_x".into());
    store.seed_order(row).await;
    let mut pos = Position::new(
        "p_x".into(),
        TradeSource::Polymarket,
        Venue::PolymarketClob,
        ExecutionMode::Live,
        YES.into(),
        "Yes".into(),
        "USDC".into(),
    );
    pos.apply_buy(4.0, 0.40, 1.6);
    pos.market_id = Some(CONDITION.into());
    state.upsert_position(pos).await;

    // The venue still holds it exactly as journaled.
    venue.set_order("0xaaa", "live", 4.0, 25.0);
    venue.set_open_orders(vec![venue_order("0xaaa", "live", 4.0, 25.0)]);

    // Restart.
    let bot = live_bot(&state, store.clone()).await;
    let report = bot.recover_after_restart().await.unwrap();
    assert_eq!(
        report.count(RecoveryAction::AdoptedJournalOrder),
        1,
        "{report:?}"
    );
    assert_eq!(report.count(RecoveryAction::HeldAmbiguous), 0);
    assert!(
        report.findings.is_empty(),
        "venue agrees with the journal: {report:?}"
    );
    let t = &bot.tracked_orders().await[0];
    assert_eq!(t.venue_order_id, "0xaaa");
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert!((t.size_matched - 4.0).abs() < 1e-9);
    assert_eq!(t.position_id.as_deref(), Some("p_x"));
    assert_eq!(
        t.signature.as_deref(),
        Some("0xsig"),
        "signature re-attached from the OMS"
    );

    // The venue now reports 10 matched: only the 6-token delta is booked,
    // onto the existing position.
    venue.set_order("0xaaa", "live", 10.0, 25.0);
    let cfg = poly_cfg(&state).await;
    bot.poll_orders_once(&cfg).await.unwrap();
    let pos = state.position("p_x").await.unwrap();
    assert!((pos.qty - 10.0).abs() < 1e-9, "{pos:?}");
    let fills = store.fills().await;
    assert_eq!(fills.len(), 1);
    assert!((fills[0].size_tokens - 6.0).abs() < 1e-9);
    assert_eq!(fills[0].position_id.as_deref(), Some("p_x"));

    // Re-adopted orders gate new entries on the same token immediately.
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert!(
        matches!(
            out.reject_reason,
            Some(RejectReason::AlreadyInMarket) | Some(RejectReason::OrderAlreadyOpen)
        ),
        "{out:?}"
    );

    // Recovery is idempotent.
    let again = bot.recover_after_restart().await.unwrap();
    assert!(again.actions.is_empty(), "{again:?}");
    let actions = audit_actions(&state).await;
    assert!(
        actions
            .iter()
            .any(|a| a == "poly.recovery.adopted_journal_order"),
        "{actions:?}"
    );
}

#[tokio::test]
async fn ambiguous_and_stale_rows_are_held_or_failed() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());

    // Ambiguous live submit from the previous life (OMS Unknown).
    let amb = oms
        .create(OrderDraft {
            idempotency_key: "intent-amb".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: YES.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Live,
            qty: 25.0,
            price: Some(0.40),
            meta: serde_json::json!({"signal_id": "psig_0xamb"}),
        })
        .await
        .unwrap();
    oms.attach_external(&amb.id, Some("0xamb".into()), None)
        .await
        .unwrap();
    oms.transition(&amb.id, OrderStatus::Unknown, Some("crash"))
        .await
        .unwrap();
    store
        .seed_order(journal_row("0xamb", &amb.id, YES, "submitted", "live", 0.0))
        .await;

    // Paper order that never reached its in-process fill.
    let paper = oms
        .create(OrderDraft {
            idempotency_key: "intent-paper".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: NO.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Paper,
            qty: 20.0,
            price: Some(0.55),
            meta: serde_json::json!({"signal_id": "psig_paper"}),
        })
        .await
        .unwrap();
    oms.attach_external(&paper.id, Some("paper:abc".into()), None)
        .await
        .unwrap();
    oms.transition(&paper.id, OrderStatus::Submitted, None)
        .await
        .unwrap();
    store
        .seed_order(journal_row(
            "paper:abc",
            &paper.id,
            NO,
            "submitted",
            "paper",
            0.0,
        ))
        .await;

    let bot = live_bot(&state, store.clone()).await;
    let report = bot.recover_after_restart().await.unwrap();
    assert_eq!(
        report.count(RecoveryAction::AdoptedJournalOrder),
        1,
        "{report:?}"
    );
    assert_eq!(report.count(RecoveryAction::HeldAmbiguous), 1);
    assert_eq!(report.count(RecoveryAction::FailedStalePaper), 1);

    // Post-recovery reconciliation ran (live + credentials + actions) and,
    // with the venue answering 404, resolved the ambiguous order as failed.
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.venue_order_id.as_deref() == Some("0xamb") && f.action == "marked_failed"),
        "{report:?}"
    );
    let tracked = bot.tracked_orders().await;
    let t_amb = tracked
        .iter()
        .find(|t| t.venue_order_id == "0xamb")
        .unwrap();
    assert_eq!(t_amb.state, LocalOrderState::Failed);
    assert_eq!(oms.get(&amb.id).await.unwrap().status, OrderStatus::Failed);
    assert_eq!(
        oms.get(&paper.id).await.unwrap().status,
        OrderStatus::Failed
    );
    let journaled = store.orders().await;
    let paper_row = journaled
        .iter()
        .find(|o| o.venue_order_id == "paper:abc")
        .unwrap();
    assert_eq!(paper_row.state, "failed");
    assert!(paper_row.closed_at.is_some());
    assert!(
        store.open_orders().await.unwrap().is_empty(),
        "nothing left open in the journal"
    );
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
    assert!(state.find_open(BotModule::Polymarket, NO).await.is_none());
}

#[tokio::test]
async fn oms_only_live_orders_are_adopted_as_unknown() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());

    // An older release created this OMS order (venue id attached) but no
    // journal row exists.
    let legacy = oms
        .create(OrderDraft {
            idempotency_key: "legacy-key".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: YES.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Live,
            qty: 25.0,
            price: Some(0.40),
            meta: serde_json::json!({
                "signal_id": "psig_legacy", "condition_id": CONDITION, "outcome": "Yes",
                "order_type": "GTC", "expiration": 0, "question": "Legacy?"
            }),
        })
        .await
        .unwrap();
    oms.attach_external(&legacy.id, Some("0xlegacy".into()), Some("0xsig".into()))
        .await
        .unwrap();
    oms.transition(&legacy.id, OrderStatus::Submitted, None)
        .await
        .unwrap();
    // A sniper order in the same OMS is not ours.
    oms.create(OrderDraft {
        idempotency_key: "sniper-key".into(),
        module: BotModule::Sniper,
        side: "buy".into(),
        symbol: "MINT".into(),
        venue: "pump.fun".into(),
        mode: ExecutionMode::Live,
        qty: 1.0,
        price: None,
        meta: serde_json::json!({}),
    })
    .await
    .unwrap();

    // The venue still has the order resting with 5 matched.
    venue.set_order("0xlegacy", "live", 5.0, 25.0);
    venue.set_open_orders(vec![venue_order("0xlegacy", "live", 5.0, 25.0)]);

    let bot = live_bot(&state, store.clone()).await;
    let report = bot.recover_after_restart().await.unwrap();
    assert_eq!(
        report.count(RecoveryAction::AdoptedOmsOrder),
        1,
        "{report:?}"
    );
    assert_eq!(report.count(RecoveryAction::HeldAmbiguous), 1);
    let tracked = bot.tracked_orders().await;
    assert_eq!(tracked.len(), 1);
    assert_eq!(tracked[0].venue_order_id, "0xlegacy");
    assert_eq!(tracked[0].condition_id, CONDITION);
    assert_eq!(tracked[0].question, "Legacy?");
    // Reconciliation resolved it against the venue: partially filled, 5
    // booked from venue truth, journal row created.
    assert_eq!(
        tracked[0].state,
        LocalOrderState::PartiallyFilled,
        "{:?}",
        tracked[0]
    );
    assert!((tracked[0].size_matched - 5.0).abs() < 1e-9);
    assert!(
        (state
            .find_open(BotModule::Polymarket, YES)
            .await
            .unwrap()
            .qty
            - 5.0)
            .abs()
            < 1e-9
    );
    assert_eq!(store.orders().await.len(), 1);
    assert_eq!(store.fills().await.len(), 1);
    assert_eq!(
        oms.get(&legacy.id).await.unwrap().status,
        OrderStatus::PartiallyFilled,
        "the OMS record follows venue truth"
    );
}

#[tokio::test]
async fn paper_bots_recover_without_credentials() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let paper = oms
        .create(OrderDraft {
            idempotency_key: "intent-paper".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: NO.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Paper,
            qty: 20.0,
            price: Some(0.55),
            meta: serde_json::json!({"signal_id": "psig_paper"}),
        })
        .await
        .unwrap();
    oms.attach_external(&paper.id, Some("paper:zzz".into()), None)
        .await
        .unwrap();
    store
        .seed_order(journal_row(
            "paper:zzz",
            &paper.id,
            NO,
            "submitted",
            "paper",
            0.0,
        ))
        .await;

    let bot = paper_bot(&state, store.clone()).await;
    let report = bot.recover_after_restart().await.unwrap();
    assert_eq!(report.count(RecoveryAction::FailedStalePaper), 1);
    assert!(
        report.findings.is_empty(),
        "no venue reconciliation in paper mode"
    );
    assert_eq!(venue.count("GET", "/clob/data/orders"), 0);
    assert_eq!(
        oms.get(&paper.id).await.unwrap().status,
        OrderStatus::Failed
    );

    // Business as usual afterwards.
    let cfg = poly_cfg(&state).await;
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
}

#[tokio::test]
async fn never_sent_oms_orders_are_failed_instead_of_held() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());

    // Crash between IDEMPOTENT and SIGNED: an OMS order with no venue id.
    let unsigned = oms
        .create(OrderDraft {
            idempotency_key: signal(decision(), ExecutionMode::Live).intent_key(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: YES.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Live,
            qty: 25.0,
            price: Some(0.40),
            meta: serde_json::json!({"signal_id": "psig_unsigned"}),
        })
        .await
        .unwrap();
    // The server's OMS restore marks every incomplete order Unknown.
    oms.transition(&unsigned.id, OrderStatus::Unknown, Some("restart recovery"))
        .await
        .unwrap();
    // A simulate-mode order with a venue-looking id: fills are in-process,
    // so it can never be resting anywhere either.
    let simulated = oms
        .create(OrderDraft {
            idempotency_key: "intent-sim".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: NO.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Simulate,
            qty: 20.0,
            price: Some(0.55),
            meta: serde_json::json!({"signal_id": "psig_sim"}),
        })
        .await
        .unwrap();
    oms.attach_external(&simulated.id, Some("0xsim".into()), None)
        .await
        .unwrap();
    oms.transition(&simulated.id, OrderStatus::Submitted, None)
        .await
        .unwrap();
    // A sniper order is never touched.
    let sniper = oms
        .create(OrderDraft {
            idempotency_key: "sniper-key".into(),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "MINT".into(),
            venue: "pump.fun".into(),
            mode: ExecutionMode::Live,
            qty: 1.0,
            price: None,
            meta: serde_json::json!({}),
        })
        .await
        .unwrap();

    let bot = live_bot(&state, store.clone()).await;
    let report = bot.recover_after_restart().await.unwrap();
    assert_eq!(report.count(RecoveryAction::FailedUnsent), 2, "{report:?}");
    assert_eq!(report.count(RecoveryAction::HeldAmbiguous), 0);
    assert_eq!(report.count(RecoveryAction::AdoptedOmsOrder), 0);
    assert!(
        report
            .actions
            .iter()
            .any(|(a, id)| *a == RecoveryAction::FailedUnsent && id == &unsigned.id),
        "unsigned orders are reported under their OMS id: {report:?}"
    );
    assert!(
        report
            .actions
            .iter()
            .any(|(a, id)| *a == RecoveryAction::FailedUnsent && id == "0xsim"),
        "{report:?}"
    );
    assert_eq!(
        oms.get(&unsigned.id).await.unwrap().status,
        OrderStatus::Failed
    );
    assert_eq!(
        oms.get(&simulated.id).await.unwrap().status,
        OrderStatus::Failed
    );
    assert_eq!(
        oms.get(&sniper.id).await.unwrap().status,
        OrderStatus::Created
    );
    assert!(
        oms.incomplete()
            .await
            .iter()
            .all(|o| o.module != BotModule::Polymarket),
        "no Polymarket order is left incomplete"
    );
    assert!(bot.tracked_orders().await.is_empty(), "nothing was adopted");
    assert_eq!(
        venue.count("POST", "/clob/order"),
        0,
        "recovery never submits"
    );

    // Idempotent: nothing left to do.
    let again = bot.recover_after_restart().await.unwrap();
    assert!(again.actions.is_empty(), "{again:?}");
    let actions = audit_actions(&state).await;
    assert_eq!(
        actions
            .iter()
            .filter(|a| *a == "poly.recovery.failed_unsent")
            .count(),
        2,
        "{actions:?}"
    );
}

/// A `PolyStore` that records how many venue POSTs had happened when each
/// order row was first journaled — proves the write-ahead ordering.
struct OrderedStore {
    inner: MemoryPolyStore,
    venue: MockVenue,
    first_upsert_posts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

#[async_trait::async_trait]
impl module_polymarket::store::PolyStore for OrderedStore {
    async fn record_signal(&self, rec: module_polymarket::store::PolySignalRecord) -> bool {
        self.inner.record_signal(rec).await
    }
    async fn upsert_order(&self, rec: PolyOrderRecord) -> bool {
        self.first_upsert_posts
            .lock()
            .unwrap()
            .entry(rec.venue_order_id.clone())
            .or_insert_with(|| self.venue.posted_orders());
        self.inner.upsert_order(rec).await
    }
    async fn open_orders(&self) -> Option<Vec<PolyOrderRecord>> {
        self.inner.open_orders().await
    }
    async fn record_fill(&self, rec: module_polymarket::store::PolyFillRecord) -> Option<bool> {
        self.inner.record_fill(rec).await
    }
    async fn append_finding(&self, rec: module_polymarket::store::PolyReconFindingRecord) -> bool {
        self.inner.append_finding(rec).await
    }
}

#[tokio::test]
async fn fills_replayed_after_restart_are_booked_once() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());

    // Before the crash: 10 of 25 matched through user-channel trade `t-1`,
    // booked into position p_r and journaled as fill `trade:t-1`.
    let order = oms
        .create(OrderDraft {
            idempotency_key: "intent-replay".into(),
            module: BotModule::Polymarket,
            side: "buy".into(),
            symbol: YES.into(),
            venue: "polymarket".into(),
            mode: ExecutionMode::Live,
            qty: 25.0,
            price: Some(0.40),
            meta: serde_json::json!({"signal_id": "psig_0xrep", "condition_id": CONDITION}),
        })
        .await
        .unwrap();
    oms.attach_external(&order.id, Some("0xrep".into()), Some("0xsig".into()))
        .await
        .unwrap();
    oms.transition(&order.id, OrderStatus::Submitted, None)
        .await
        .unwrap();
    oms.transition(&order.id, OrderStatus::PartiallyFilled, None)
        .await
        .unwrap();
    let mut row = journal_row("0xrep", &order.id, YES, "partially_filled", "live", 10.0);
    row.position_id = Some("p_r".into());
    store.seed_order(row).await;
    let mut pos = Position::new(
        "p_r".into(),
        TradeSource::Polymarket,
        Venue::PolymarketClob,
        ExecutionMode::Live,
        YES.into(),
        "Yes".into(),
        "USDC".into(),
    );
    pos.apply_buy(10.0, 0.40, 4.0);
    pos.market_id = Some(CONDITION.into());
    state.upsert_position(pos).await;
    assert_eq!(
        store
            .record_fill(PolyFillRecord {
                fill_id: "trade:t-1".into(),
                venue_order_id: "0xrep".into(),
                order_id: order.id.clone(),
                token_id: YES.into(),
                side: "buy".into(),
                price: 0.40,
                size_tokens: 10.0,
                quote_usd: 4.0,
                source: "user_ws".into(),
                position_id: Some("p_r".into()),
                ts: Utc::now() - Duration::minutes(9),
            })
            .await,
        Some(true)
    );
    venue.set_order("0xrep", "live", 10.0, 25.0);
    venue.set_open_orders(vec![venue_order("0xrep", "live", 10.0, 25.0)]);

    // Restart: the tracker is rebuilt from the journal and no longer
    // remembers trade ids in memory.
    let bot = live_bot(&state, store.clone()).await;
    let report = bot.recover_after_restart().await.unwrap();
    assert_eq!(report.count(RecoveryAction::AdoptedJournalOrder), 1);
    assert!(report.findings.is_empty(), "{report:?}");
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 10.0).abs() < 1e-9);
    assert!(t.booked_trade_ids.is_empty());

    // After the restart the per-trade sum starts at zero, so a NEW trade is
    // not booked ahead of the venue's cumulative (safe under-booking until
    // the poll confirms it) — but it is remembered.
    let fresh = |id: &str, size: f64, price: f64| UserEvent::Trade {
        trade_id: id.into(),
        taker_order_id: Some("0xrep".into()),
        maker_fills: Vec::new(),
        asset_id: YES.into(),
        market: CONDITION.into(),
        side: "BUY".into(),
        size,
        price,
        status: "MATCHED".into(),
        ts: Some(Utc::now()),
    };
    bot.apply_user_event(&fresh("t-2", 5.0, 0.41))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 10.0).abs() < 1e-9, "{t:?}");
    assert_eq!(t.booked_trade_ids, vec!["t-2".to_string()]);
    assert_eq!(store.fills().await.len(), 1);

    // The user channel re-emits `t-1` (MATCHED → MINED → CONFIRMED keep the
    // same id). The rebuilt tracker does not know the id, and with `t-2`
    // the per-trade sum (15) now exceeds the cumulative (10) — the durable
    // fill journal proves `trade:t-1` was booked: the observation is
    // discarded, nothing moves, the id is remembered.
    let replay = |status: &str| UserEvent::Trade {
        trade_id: "t-1".into(),
        taker_order_id: Some("0xrep".into()),
        maker_fills: Vec::new(),
        asset_id: YES.into(),
        market: CONDITION.into(),
        side: "BUY".into(),
        size: 10.0,
        price: 0.40,
        status: status.into(),
        ts: Some(Utc::now()),
    };
    bot.apply_user_event(&replay("MINED")).await.unwrap();
    bot.apply_user_event(&replay("CONFIRMED")).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 10.0).abs() < 1e-9, "{t:?}");
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert!(t.booked_trade_ids.contains(&"t-1".to_string()));
    assert!((state.position("p_r").await.unwrap().qty - 10.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1);
    assert_eq!(
        oms.get(&order.id).await.unwrap().status,
        OrderStatus::PartiallyFilled
    );

    // The poll confirms the venue at 15 (t-1 + t-2) and names its trades:
    // the 5 of `t-2` are booked once, from venue truth, and the per-trade
    // sum is re-based so later new trades book immediately again.
    let cfg = poly_cfg(&state).await;
    venue.set_order_with_trades("0xrep", "live", 15.0, 25.0, &["t-1", "t-2"]);
    bot.poll_orders_once(&cfg).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 15.0).abs() < 1e-9, "{t:?}");
    assert!((t.trade_matched - 15.0).abs() < 1e-9, "{t:?}");
    assert!((state.position("p_r").await.unwrap().qty - 15.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 2);
    bot.apply_user_event(&fresh("t-3", 5.0, 0.42))
        .await
        .unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 20.0).abs() < 1e-9, "{t:?}");
    assert!((state.position("p_r").await.unwrap().qty - 20.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 3);

    // A CUMULATIVE observation whose fill row already exists (the process
    // died between the fill insert and the order-snapshot upsert) advances
    // the local snapshot to venue truth without booking the ledger twice.
    let mut probe = t.clone();
    probe.size_matched = 22.0;
    let pre_existing = probe.fill_id(None, FillSource::Poll);
    assert!(pre_existing.starts_with("pfill_"));
    assert_eq!(
        store
            .record_fill(PolyFillRecord {
                fill_id: pre_existing.clone(),
                venue_order_id: "0xrep".into(),
                order_id: order.id.clone(),
                token_id: YES.into(),
                side: "buy".into(),
                price: 0.40,
                size_tokens: 2.0,
                quote_usd: 0.8,
                source: "poll".into(),
                position_id: Some("p_r".into()),
                ts: Utc::now(),
            })
            .await,
        Some(true)
    );
    venue.set_order("0xrep", "live", 22.0, 25.0);
    bot.poll_orders_once(&cfg).await.unwrap();
    let t = &bot.tracked_orders().await[0];
    assert!((t.size_matched - 22.0).abs() < 1e-9, "{t:?}");
    assert_eq!(t.state, LocalOrderState::PartiallyFilled);
    assert!(
        (state.position("p_r").await.unwrap().qty - 20.0).abs() < 1e-9,
        "the pre-existing fill row was not booked into the position again"
    );
    assert_eq!(store.fills().await.len(), 4);
    let journaled = store
        .orders()
        .await
        .into_iter()
        .find(|o| o.venue_order_id == "0xrep")
        .unwrap();
    assert!((journaled.size_matched - 22.0).abs() < 1e-9);
}

#[tokio::test]
async fn the_venue_claim_is_journaled_before_the_post() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(OrderedStore {
        inner: MemoryPolyStore::new(),
        venue: venue.clone(),
        first_upsert_posts: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let bot = module_polymarket::PolyBot::new(state.clone())
        .await
        .unwrap()
        .with_signer(test_key())
        .with_api_key(test_api_key())
        .with_store(store.clone());
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
    let id = out.venue_order_id.unwrap();
    let posts_at_first_journal = *store.first_upsert_posts.lock().unwrap().get(&id).unwrap();
    assert_eq!(
        posts_at_first_journal, 0,
        "the order row must exist before the venue is called"
    );
    assert_eq!(venue.posted_orders(), 1);
    let row = store
        .inner
        .orders()
        .await
        .into_iter()
        .find(|o| o.venue_order_id == id)
        .unwrap();
    assert_eq!(
        row.state, "resting",
        "and it is updated after the venue answers"
    );
}
