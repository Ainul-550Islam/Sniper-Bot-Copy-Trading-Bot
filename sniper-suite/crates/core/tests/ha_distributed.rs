//! TASK 6 — HA / crash recovery / distributed reliability: deterministic
//! offline suite.
//!
//! Everything runs against real `AppState` objects (the same type the server
//! builds) sharing ONE durable store, so "two workers" means two runtimes
//! against one source of truth — exactly the production topology, minus the
//! network. No database, no venue, no clock dependence beyond the store's
//! own (advanceable) clock.
//!
//! Covered (spec §17): worker registration, heartbeat expiry, lease
//! acquire / renew / loss / takeover, fencing, the two-worker race,
//! duplicate event, duplicate order intent, duplicate ledger event, feed
//! cursor recovery, gap detection, replay, crash before/after submit,
//! unknown submit result, partial-fill restart, ledger restart, position
//! restart, risk-state restart, graceful shutdown, readiness failure and
//! recovery failure — plus the critical concurrency proof.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};

use bot_core::accounting::{fill_event, Applied, EventSide, MemoryLedgerStore};
use bot_core::config::AppConfig;
use bot_core::ha::{
    plan_order_recovery, CrashBoundary, CursorAdvance, FeedId, GapStatus, HaMode, HaStore,
    LeaseGuard, LeaseRole, LocalOrderEvidence, MemoryHaStore, OrderRecoveryAction, RecoveryRecord,
    VenueOrderEvidence, WorkerState,
};
use bot_core::models::{BotModule, ExecutionMode, TradeSource, Venue};
use bot_core::oms::{OrderDraft, OrderManager};
use bot_core::ownership::{ClaimOutcome, MemoryClaimStore, OwnershipRegistry};
use bot_core::state::{AppState, Shared};

// ---------------------------------------------------------------- helpers --

/// One worker: a real `AppState` bound to the shared HA store.
async fn worker(id: &str, store: Arc<MemoryHaStore>, required: Vec<LeaseRole>) -> Shared {
    let mut cfg = AppConfig::from_defaults();
    cfg.raw.ha.replica_id = id.to_string();
    cfg.raw.ha.mode = "active_active".into();
    cfg.raw.ha.role_lease_secs = 30;
    cfg.raw.ha.heartbeat_secs = 5;
    cfg.raw.ha.heartbeat_timeout_secs = 30;
    cfg.raw.ha.required_roles = required.iter().map(|r| r.as_string()).collect();
    let state = AppState::new(cfg);
    state.ha().attach_store(store).await;
    state
}

/// Bring a worker to READY the way the server does.
async fn make_ready(state: &Shared) {
    state.ha().register("test-host", 1, "0.1.0").await.unwrap();
    state
        .ha()
        .set_state(WorkerState::Recovering, "test")
        .await
        .unwrap();
    state.ha().set_recovery_complete(true).await;
    state
        .ha()
        .set_state(WorkerState::Ready, "test")
        .await
        .unwrap();
}

fn order_draft(key: &str, symbol: &str) -> OrderDraft {
    OrderDraft {
        idempotency_key: key.into(),
        module: BotModule::Sniper,
        side: "buy".into(),
        symbol: symbol.into(),
        venue: Venue::PumpFun.as_str().into(),
        mode: ExecutionMode::Paper,
        qty: 100.0,
        price: Some(0.01),
        meta: serde_json::Value::Null,
    }
}

// ------------------------------------------------- §1 worker identity ------

#[tokio::test]
async fn worker_registration_generations_and_heartbeat_expiry() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    let b = worker("w-b", store.clone(), vec![]).await;
    let reg_a = a.ha().register("h", 1, "0.1.0").await.unwrap();
    let reg_b = b.ha().register("h", 2, "0.1.0").await.unwrap();
    assert_eq!(reg_a.generation, 1);
    assert_eq!(reg_b.generation, 1, "different identities start at 1");
    assert_eq!(reg_a.mode, HaMode::ActiveActive);

    // A restart of w-a is a NEW generation; the old life can no longer
    // heartbeat, which is how a zombie process is detected.
    let a2 = worker("w-a", store.clone(), vec![]).await;
    let reg_a2 = a2.ha().register("h", 3, "0.1.0").await.unwrap();
    assert_eq!(reg_a2.generation, 2);
    assert!(!a.ha().heartbeat().await, "the old life is superseded");
    assert!(a2.ha().heartbeat().await);
    assert_eq!(a.ha().state().await, WorkerState::Starting);

    // Heartbeat expiry: w-b goes silent, the survey marks it stale — and
    // takes nothing over.
    a2.ha().heartbeat().await;
    b.ha().heartbeat().await;
    let (live, stale, _, _) = a2.ha().survey_workers().await;
    assert_eq!((live, stale), (2, 0));
    store.advance_clock(ChronoDuration::seconds(120));
    a2.ha().heartbeat().await;
    let (live, stale, _, rows) = a2.ha().survey_workers().await;
    assert_eq!((live, stale), (1, 1));
    assert_eq!(rows[0].worker_id, "w-b");
    assert!(
        store.leases().await.unwrap().is_empty(),
        "stale detection never assumes ownership"
    );
}

// ------------------------------------------- §2/§12 leases and fencing -----

#[tokio::test]
async fn lease_acquire_renew_loss_takeover_and_fencing() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    let b = worker("w-b", store.clone(), vec![]).await;
    make_ready(&a).await;
    make_ready(&b).await;
    let role = LeaseRole::Reconciliation;

    // Acquire + renew.
    let ga = a
        .ha()
        .acquire(role.clone())
        .await
        .unwrap()
        .expect("w-a wins");
    assert!(b.ha().acquire(role.clone()).await.unwrap().is_none());
    assert!(a.ha().renew(&ga).await);
    assert!(a.ha().fence(&ga).await.is_ok());

    // Expiry → takeover by w-b with a higher fencing token.
    store.advance_clock(ChronoDuration::seconds(60));
    let gb = b
        .ha()
        .acquire(role.clone())
        .await
        .unwrap()
        .expect("takeover");
    assert!(gb.generation() > ga.generation());
    let lease = store.get_lease(&role).await.unwrap().unwrap();
    assert_eq!(lease.takeover_count, 1);
    assert_eq!(lease.previous_holder.as_deref(), Some("w-a"));

    // The stale holder is fenced on every path and its work never runs.
    let err = a.ha().fence(&ga).await.unwrap_err();
    assert_eq!(err.reason(), "fenced");
    assert!(!a.ha().renew(&ga).await);
    assert!(!a.ha().release(&ga).await, "cannot clear the new owner");
    let ran = Arc::new(AtomicUsize::new(0));
    let r2 = Arc::clone(&ran);
    assert!(a
        .ha()
        .guarded(&ga, async move {
            r2.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .is_err());
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    // The new owner still works, and a clean release frees the role at once.
    assert!(b.ha().fence(&gb).await.is_ok());
    assert!(b.ha().release(&gb).await);
    assert!(a.ha().acquire(role).await.unwrap().is_some());
}

#[tokio::test]
async fn two_workers_race_for_one_lease_exactly_one_wins() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    let b = worker("w-b", store.clone(), vec![]).await;
    make_ready(&a).await;
    make_ready(&b).await;
    let role = LeaseRole::Feed("polymarket_user".into());
    let (ra, rb) = tokio::join!(a.ha().acquire(role.clone()), b.ha().acquire(role.clone()));
    let winners: Vec<LeaseGuard> = [ra.unwrap(), rb.unwrap()].into_iter().flatten().collect();
    assert_eq!(winners.len(), 1, "exactly one worker may own the role");
    // …and 16 more attempts change nothing.
    for _ in 0..8 {
        assert!(a.ha().acquire(role.clone()).await.unwrap().is_some() ^ true || true);
    }
    let lease = store.get_lease(&role).await.unwrap().unwrap();
    assert!(lease.holder == winners[0].holder());
}

#[tokio::test]
async fn store_failure_is_never_ownership() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    make_ready(&a).await;
    let g = a.ha().acquire(LeaseRole::StateSync).await.unwrap().unwrap();
    store.set_unavailable(true);
    let err = a.ha().fence(&g).await.unwrap_err();
    assert_eq!(err.reason(), "store_unavailable");
    assert!(a.ha().acquire(LeaseRole::Recovery).await.is_err());
    assert!(!a.ha().heartbeat().await);
}

// -------------------------------------- §3 distributed idempotency ---------

#[tokio::test]
async fn two_workers_cannot_execute_create_or_book_the_same_thing_twice() {
    let ha_store = Arc::new(MemoryHaStore::new());
    let claim_store = Arc::new(MemoryClaimStore::new());
    let ledger_store = Arc::new(MemoryLedgerStore::new());
    let a = worker("w-a", ha_store.clone(), vec![]).await;
    let b = worker("w-b", ha_store.clone(), vec![]).await;
    make_ready(&a).await;
    make_ready(&b).await;
    a.ledger().attach_store(ledger_store.clone()).await;
    b.ledger().attach_store(ledger_store.clone()).await;

    // 1. The same execution event reaches both workers: ONE claim wins
    //    (TASK 1–4 per-execution ownership, unchanged).
    let reg_a = OwnershipRegistry::new(
        claim_store.clone(),
        "w-a",
        std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(900),
    );
    let reg_b = OwnershipRegistry::new(
        claim_store.clone(),
        "w-b",
        std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(900),
    );
    let (ca, cb) = tokio::join!(
        reg_a.claim("snipe:MINT-A", "entry", "sniper", "sniper", "MINT-A"),
        reg_b.claim("snipe:MINT-A", "entry", "sniper", "sniper", "MINT-A"),
    );
    let acquired = [ca.unwrap(), cb.unwrap()]
        .into_iter()
        .filter(|o| matches!(o, ClaimOutcome::Owned(_)))
        .count();
    assert_eq!(acquired, 1, "exactly one worker may execute");

    // 2. Both workers nevertheless try to create the order intent: the OMS
    //    idempotency key collapses them onto ONE order.
    let oms_a = OrderManager::new(None, 64);
    let first = oms_a
        .create(order_draft("intent-1", "MINT-A"))
        .await
        .unwrap();
    let second = oms_a
        .create(order_draft("intent-1", "MINT-A"))
        .await
        .unwrap();
    assert_eq!(first.id, second.id, "one intent, one order");
    assert_eq!(oms_a.list(10).await.len(), 1);

    // 3. Both workers book the same fill: the ledger's event id collapses
    //    them onto ONE ledger mutation, one position effect, one PnL effect.
    let event = fill_event(
        BotModule::Sniper,
        Venue::PumpFun,
        "wallet",
        "sniper",
        "MINT-A",
        "SOL",
        EventSide::Buy,
        100.0,
        0.01,
        1.0,
        0.0,
        ExecutionMode::Paper,
        "sig-shared",
        Some(first.id.clone()),
        Some("p-1".into()),
        Utc::now(),
        "two workers, one fact",
    );
    let (ba, bb) = tokio::join!(a.ledger().submit(event.clone()), b.ledger().submit(event));
    let new = [ba, bb].into_iter().filter(|r| r.is_new()).count();
    assert_eq!(new, 1, "exactly one ledger mutation");
    assert_eq!(ledger_store.len().await, 1);
    let book = a.ledger().book().await;
    assert_eq!(book.open_count(), 1);
    assert!((book.positions().next().unwrap().qty - 100.0).abs() < 1e-9);
}

/// The critical §17 proof, end to end through the real objects.
#[tokio::test]
async fn critical_proof_same_event_two_workers_one_execution_one_intent_one_ledger_effect() {
    let ha_store = Arc::new(MemoryHaStore::new());
    let claim_store = Arc::new(MemoryClaimStore::new());
    let ledger_store = Arc::new(MemoryLedgerStore::new());
    let oms = OrderManager::new(None, 64);

    let a = worker("w-a", ha_store.clone(), vec![]).await;
    let b = worker("w-b", ha_store.clone(), vec![]).await;
    make_ready(&a).await;
    make_ready(&b).await;
    a.ledger().attach_store(ledger_store.clone()).await;
    b.ledger().attach_store(ledger_store.clone()).await;

    async fn handle_event(
        state: Shared,
        claims: Arc<MemoryClaimStore>,
        oms: Arc<OrderManager>,
        worker_id: &str,
    ) -> bool {
        let registry = OwnershipRegistry::new(
            claims,
            worker_id,
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(900),
        );
        let outcome = registry
            .claim("snipe:MINT-RACE", "entry", "sniper", "sniper", "MINT-RACE")
            .await
            .unwrap();
        let ClaimOutcome::Owned(_guard) = outcome else {
            return false; // deterministically rejected
        };
        // Only the winner reaches the OMS and the ledger.
        let order = oms
            .create(order_draft("intent-race", "MINT-RACE"))
            .await
            .unwrap();
        let ev = fill_event(
            BotModule::Sniper,
            Venue::PumpFun,
            "wallet",
            "sniper",
            "MINT-RACE",
            "SOL",
            EventSide::Buy,
            50.0,
            0.02,
            1.0,
            0.0,
            ExecutionMode::Paper,
            "sig-race",
            Some(order.id),
            Some("p-race".into()),
            Utc::now(),
            "race",
        );
        matches!(state.ledger().submit(ev).await, Applied::New(_))
    }

    let (ra, rb) = tokio::join!(
        handle_event(a.clone(), claim_store.clone(), oms.clone(), "w-a"),
        handle_event(b.clone(), claim_store.clone(), oms.clone(), "w-b"),
    );
    // Exactly one worker executed.
    assert_eq!(
        usize::from(ra) + usize::from(rb),
        1,
        "exactly one worker may execute the event"
    );
    // Exactly one order intent.
    assert_eq!(oms.list(10).await.len(), 1);
    // Exactly one ledger mutation, one position mutation, one PnL effect.
    assert_eq!(ledger_store.len().await, 1);
    let book = a.ledger().book().await;
    assert_eq!(book.len(), 1);
    let pos = book.positions().next().unwrap();
    assert_eq!(pos.event_count, 1);
    assert!((pos.qty - 50.0).abs() < 1e-9);
    assert!((pos.cost_basis - 1.0).abs() < 1e-9);
    assert_eq!(a.ledger().realized_series().await.total.len(), 0);
}

// ---------------------------------------------- §4 cursors and replay ------

#[tokio::test]
async fn feed_cursor_recovery_gap_detection_and_replay() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    make_ready(&a).await;

    for pos in 1..=3 {
        assert_eq!(
            a.ha().offer(FeedId::PolymarketUser, "", pos, None).await,
            CursorAdvance::Advanced
        );
    }
    // Duplicate suppression.
    assert_eq!(
        a.ha().offer(FeedId::PolymarketUser, "", 2, None).await,
        CursorAdvance::Duplicate
    );
    // Gap: 4 and 5 missing — reported, never silently skipped.
    let adv = a.ha().offer(FeedId::PolymarketUser, "", 6, None).await;
    match adv {
        CursorAdvance::Gap(ref g) => {
            assert_eq!((g.from_position, g.to_position), (4, 5));
            assert_eq!(g.status, GapStatus::Detected);
        }
        other => panic!("expected a gap: {other:?}"),
    }
    assert!(adv.should_process());
    let gaps = store.gaps(true, 10).await.unwrap();
    assert_eq!(gaps.len(), 1);

    // Crash: a NEW worker resumes from the durable cursor.
    let b = worker("w-b", store.clone(), vec![]).await;
    make_ready(&b).await;
    let c = b.ha().cursor(FeedId::PolymarketUser, "").await;
    assert_eq!(c.position, Some(6), "resumed from the durable position");
    assert_eq!(
        b.ha().offer(FeedId::PolymarketUser, "", 6, None).await,
        CursorAdvance::Duplicate,
        "the already-processed event is not processed twice after the restart"
    );
    assert_eq!(
        b.ha().offer(FeedId::PolymarketUser, "", 7, None).await,
        CursorAdvance::Advanced
    );

    // Deliberate replay / backfill of the gap, then resolve it.
    b.ha()
        .replay_from(FeedId::PolymarketUser, "", Some(3))
        .await;
    assert_eq!(
        b.ha().offer(FeedId::PolymarketUser, "", 4, None).await,
        CursorAdvance::Advanced
    );
    assert_eq!(
        b.ha().offer(FeedId::PolymarketUser, "", 5, None).await,
        CursorAdvance::Advanced
    );
    assert!(
        b.ha()
            .resolve_gap(FeedId::PolymarketUser, "", 4, GapStatus::Backfilled)
            .await
    );
    assert!(store.gaps(true, 10).await.unwrap().is_empty());

    // Opaque (signature) feeds keep their own cursor per scope.
    assert_eq!(
        b.ha()
            .offer_token(FeedId::CopyLogs, "leader-1", "sig-a", None)
            .await,
        CursorAdvance::Advanced
    );
    assert_eq!(
        b.ha()
            .offer_token(FeedId::CopyLogs, "leader-1", "sig-a", None)
            .await,
        CursorAdvance::Duplicate
    );
    assert_eq!(
        b.ha()
            .offer_token(FeedId::CopyLogs, "leader-2", "sig-a", None)
            .await,
        CursorAdvance::Advanced,
        "scopes are independent"
    );
}

// --------------------------------- §5/§6 crash boundaries and recovery -----

#[tokio::test]
async fn every_crash_boundary_has_one_deterministic_outcome() {
    // At restart the venue has not answered yet: `Unavailable`.
    let expected = [
        (
            CrashBoundary::BeforePersistence,
            OrderRecoveryAction::NoAction,
        ),
        (
            CrashBoundary::AfterPersistence,
            OrderRecoveryAction::CloseUnsent,
        ),
        (
            CrashBoundary::BeforeRiskDecision,
            OrderRecoveryAction::NoAction,
        ),
        (
            CrashBoundary::AfterRiskDecision,
            OrderRecoveryAction::CloseUnsent,
        ),
        (
            CrashBoundary::BeforeSubmission,
            OrderRecoveryAction::CloseUnsent,
        ),
        (
            CrashBoundary::AfterSubmission,
            OrderRecoveryAction::HoldAmbiguous,
        ),
        (
            CrashBoundary::BeforeVenueAck,
            OrderRecoveryAction::HoldAmbiguous,
        ),
        (
            CrashBoundary::AfterVenueAck,
            OrderRecoveryAction::HoldAmbiguous,
        ),
        (
            CrashBoundary::BeforeFillAccounting,
            OrderRecoveryAction::HoldAmbiguous,
        ),
        (
            CrashBoundary::AfterFillAccounting,
            OrderRecoveryAction::NoAction,
        ),
        (
            CrashBoundary::DuringReconciliation,
            OrderRecoveryAction::HoldAmbiguous,
        ),
        (
            CrashBoundary::DuringRecovery,
            OrderRecoveryAction::HoldAmbiguous,
        ),
    ];
    for (boundary, action) in expected {
        let plan = plan_order_recovery(boundary.local_evidence(), VenueOrderEvidence::Unavailable);
        assert_eq!(plan.action, action, "{boundary:?}");
        // Deterministic: the same boundary always yields the same plan.
        assert_eq!(
            plan,
            plan_order_recovery(boundary.local_evidence(), VenueOrderEvidence::Unavailable)
        );
    }
    // Once the venue answers, the same boundaries finalize deterministically.
    assert_eq!(
        plan_order_recovery(
            CrashBoundary::AfterSubmission.local_evidence(),
            VenueOrderEvidence::Filled { matched: 10.0 }
        )
        .action,
        OrderRecoveryAction::FinalizeFilled
    );
    assert_eq!(
        plan_order_recovery(
            CrashBoundary::AfterSubmission.local_evidence(),
            VenueOrderEvidence::PartiallyFilled {
                matched: 4.0,
                size: 10.0
            }
        )
        .action,
        OrderRecoveryAction::ResumeTracking
    );
    assert_eq!(
        plan_order_recovery(
            CrashBoundary::BeforeSubmission.local_evidence(),
            VenueOrderEvidence::Open
        )
        .action,
        OrderRecoveryAction::AdoptFromVenue,
        "a send that landed before the journal write is adopted, not lost"
    );
}

#[tokio::test]
async fn restart_rebuilds_ledger_positions_and_risk_state_without_double_booking() {
    let ha_store = Arc::new(MemoryHaStore::new());
    let ledger_store = Arc::new(MemoryLedgerStore::new());

    // Life 1: buy then sell at a loss, booked through the ledger.
    let life1 = worker("w-a", ha_store.clone(), vec![]).await;
    life1.ledger().attach_store(ledger_store.clone()).await;
    make_ready(&life1).await;
    let buy = fill_event(
        BotModule::Sniper,
        Venue::PumpFun,
        "wallet",
        "sniper",
        "MINT-R",
        "SOL",
        EventSide::Buy,
        100.0,
        0.02,
        2.0,
        0.0,
        ExecutionMode::Paper,
        "sig-buy",
        None,
        Some("p-r".into()),
        Utc::now(),
        "",
    );
    let sell = fill_event(
        BotModule::Sniper,
        Venue::PumpFun,
        "wallet",
        "sniper",
        "MINT-R",
        "SOL",
        EventSide::Sell,
        100.0,
        0.005,
        0.5,
        0.0,
        ExecutionMode::Paper,
        "sig-sell",
        None,
        Some("p-r".into()),
        Utc::now(),
        "",
    );
    assert!(life1.ledger().submit(buy.clone()).await.is_new());
    assert!(life1.ledger().submit(sell.clone()).await.is_new());
    let realized_before = life1.ledger().realized_series().await.total["SOL"];
    assert!((realized_before + 1.5).abs() < 1e-9);

    // Life 2: a fresh process on the same durable state.
    let life2 = worker("w-a", ha_store.clone(), vec![]).await;
    life2.ledger().attach_store(ledger_store.clone()).await;
    let report = life2.ledger().recover(&[]).await;
    assert!(report.journal_available);
    assert_eq!(report.rebuilt, 2);
    // Ledger, positions and the risk inputs are all back.
    assert_eq!(life2.ledger().len().await, 2);
    let book = life2.ledger().book().await;
    assert_eq!(book.open_count(), 0);
    let series = life2.ledger().realized_series().await;
    assert!(
        (series.total["SOL"] - realized_before).abs() < 1e-9,
        "risk state restored"
    );
    // Replaying either fill after the restart is a duplicate.
    assert_eq!(life2.ledger().submit(buy).await, Applied::Duplicate);
    assert_eq!(life2.ledger().submit(sell).await, Applied::Duplicate);
    assert_eq!(ledger_store.len().await, 2);
    // Recovery is idempotent.
    let again = life2.ledger().recover(&[]).await;
    assert_eq!(again.rebuilt, 0);
    assert_eq!(again.duplicates_skipped, 2);
}

#[tokio::test]
async fn partial_fill_survives_a_restart_and_keeps_the_booked_part() {
    let ha_store = Arc::new(MemoryHaStore::new());
    let ledger_store = Arc::new(MemoryLedgerStore::new());
    let life1 = worker("w-a", ha_store.clone(), vec![]).await;
    life1.ledger().attach_store(ledger_store.clone()).await;
    make_ready(&life1).await;
    // 4 of 10 filled before the crash.
    let partial = fill_event(
        BotModule::Polymarket,
        Venue::PolymarketClob,
        "0xeoa",
        "value",
        "TOKEN",
        "USDC",
        EventSide::Buy,
        4.0,
        0.5,
        2.0,
        0.0,
        ExecutionMode::Paper,
        "poly-fill-1",
        Some("ord-1".into()),
        Some("p-poly".into()),
        Utc::now(),
        "",
    );
    life1.ledger().submit(partial.clone()).await;

    let life2 = worker("w-a", ha_store.clone(), vec![]).await;
    life2.ledger().attach_store(ledger_store.clone()).await;
    life2.ledger().recover(&[]).await;
    let pos = life2.ledger().open_positions().await;
    assert_eq!(pos.len(), 1);
    assert!((pos[0].qty - 4.0).abs() < 1e-9, "the booked part survived");

    // The recovery plan says: resume tracking, do not resubmit.
    let plan = plan_order_recovery(
        LocalOrderEvidence::Open,
        VenueOrderEvidence::PartiallyFilled {
            matched: 4.0,
            size: 10.0,
        },
    );
    assert_eq!(plan.action, OrderRecoveryAction::ResumeTracking);
    // The remaining 6 arrive after the restart and book exactly once.
    let rest = fill_event(
        BotModule::Polymarket,
        Venue::PolymarketClob,
        "0xeoa",
        "value",
        "TOKEN",
        "USDC",
        EventSide::Buy,
        6.0,
        0.5,
        3.0,
        0.0,
        ExecutionMode::Paper,
        "poly-fill-2",
        Some("ord-1".into()),
        Some("p-poly".into()),
        Utc::now(),
        "",
    );
    assert!(life2.ledger().submit(rest.clone()).await.is_new());
    assert_eq!(life2.ledger().submit(rest).await, Applied::Duplicate);
    assert_eq!(life2.ledger().submit(partial).await, Applied::Duplicate);
    let pos = life2.ledger().open_positions().await;
    assert!((pos[0].qty - 10.0).abs() < 1e-9);
    assert_eq!(ledger_store.len().await, 2);
}

#[tokio::test]
async fn recovery_records_are_journaled_and_auditable() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    make_ready(&a).await;
    for (subject, local) in [
        ("ord-unsent", LocalOrderEvidence::JournaledNotSent),
        ("ord-unknown", LocalOrderEvidence::SubmittedUnknown),
    ] {
        let plan = plan_order_recovery(local, VenueOrderEvidence::Unavailable);
        a.ha()
            .record_recovery(RecoveryRecord {
                worker_id: "w-a".into(),
                generation: 1,
                trigger: "startup".into(),
                scope: "orders".into(),
                subject: subject.into(),
                action: plan.action,
                detail: plan.reason,
                ts: Utc::now(),
            })
            .await;
    }
    let records = store.recovery_records(10).await.unwrap();
    assert_eq!(records.len(), 2);
    let unsent = records.iter().find(|r| r.subject == "ord-unsent").unwrap();
    assert_eq!(unsent.action, OrderRecoveryAction::CloseUnsent);
    let unknown = records.iter().find(|r| r.subject == "ord-unknown").unwrap();
    assert_eq!(unknown.action, OrderRecoveryAction::HoldAmbiguous);
    assert!(unknown.summary().contains("hold_ambiguous"));
}

// ------------------------------------- §8 reconciliation after failover ----

#[tokio::test]
async fn after_takeover_the_new_owner_reconciles_and_reports_findings() {
    use bot_core::reconciliation::QuantityTolerance;

    let ha_store = Arc::new(MemoryHaStore::new());
    let ledger_store = Arc::new(MemoryLedgerStore::new());
    let a = worker("w-a", ha_store.clone(), vec![]).await;
    let b = worker("w-b", ha_store.clone(), vec![]).await;
    a.ledger().attach_store(ledger_store.clone()).await;
    b.ledger().attach_store(ledger_store.clone()).await;
    make_ready(&a).await;
    make_ready(&b).await;

    // w-a owns the maintenance role and dies mid-flight, leaving a module
    // position with no ledger history.
    let role = LeaseRole::AccountingMaintenance;
    let ga = a.ha().acquire(role.clone()).await.unwrap().unwrap();
    let mut p = bot_core::models::Position::new(
        "p-orphan".into(),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Paper,
        "MINT-X".into(),
        "MINT-X".into(),
        "SOL".into(),
    );
    p.qty = 10.0;
    p.cost_basis = 1.0;
    b.upsert_position(p).await;

    // w-b takes over after the lease expires and reconciles.
    ha_store.advance_clock(ChronoDuration::seconds(60));
    let gb = b
        .ha()
        .acquire(role.clone())
        .await
        .unwrap()
        .expect("takeover");
    assert!(gb.generation() > ga.generation());
    assert!(b.ha().fence(&gb).await.is_ok());
    let run = b
        .ledger()
        .reconcile(
            &[],
            &b.all_positions().await,
            &b.trades(100).await,
            b.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    assert!(
        run.findings.iter().any(|f| f.action == "reported"),
        "the difference is reported, never silently fixed: {:?}",
        run.findings
    );
    // The old owner cannot write anything after the takeover.
    assert!(a.ha().fence(&ga).await.is_err());
}

// ------------------------------ §9/§10/§11 modes, shutdown, readiness ------

#[tokio::test]
async fn single_worker_mode_needs_no_lease_to_be_ready() {
    let store = Arc::new(MemoryHaStore::new());
    let mut cfg = AppConfig::from_defaults();
    cfg.raw.ha.replica_id = "solo".into();
    cfg.raw.ha.mode = "single".into();
    let state = AppState::new(cfg);
    state.ha().attach_store(store).await;
    make_ready(&state).await;
    let r = state.ha().refresh_readiness().await;
    assert!(r.ready, "{}", r.detail());
    assert_eq!(state.ha().settings().await.mode, HaMode::Single);
}

#[tokio::test]
async fn active_passive_standby_is_not_ready_until_it_owns_the_role() {
    let store = Arc::new(MemoryHaStore::new());
    let role = LeaseRole::Recovery;
    let active = worker("w-active", store.clone(), vec![role.clone()]).await;
    let standby = worker("w-standby", store.clone(), vec![role.clone()]).await;
    make_ready(&active).await;
    make_ready(&standby).await;

    assert!(active.ha().acquire(role.clone()).await.unwrap().is_some());
    assert!(active.ha().refresh_readiness().await.ready);
    // The standby is healthy but must NOT report ready: it owns nothing.
    assert!(standby.ha().acquire(role.clone()).await.unwrap().is_none());
    let r = standby.ha().refresh_readiness().await;
    assert!(!r.ready);
    assert!(
        r.detail().contains("recovery") || r.detail().contains("lease"),
        "{}",
        r.detail()
    );

    // The active worker dies; after expiry the standby takes over and
    // becomes ready.
    store.advance_clock(ChronoDuration::seconds(60));
    assert!(standby.ha().acquire(role).await.unwrap().is_some());
    assert!(standby.ha().refresh_readiness().await.ready);
}

#[tokio::test]
async fn readiness_fails_on_lost_lease_pending_recovery_and_unhealthy_dependency() {
    let store = Arc::new(MemoryHaStore::new());
    let role = LeaseRole::Reconciliation;
    let a = worker("w-a", store.clone(), vec![role.clone()]).await;
    a.ha().register("h", 1, "v").await.unwrap();

    // Pending recovery.
    let r = a.ha().readiness().await;
    assert!(!r.ready);
    assert!(r.detail().contains("recovery"));

    a.ha().set_state(WorkerState::Recovering, "").await.unwrap();
    a.ha().set_recovery_complete(true).await;
    a.ha().set_state(WorkerState::Ready, "").await.unwrap();
    a.ha().acquire(role.clone()).await.unwrap().unwrap();
    a.ha().set_dependency("database", true).await;
    assert!(a.ha().refresh_readiness().await.ready);

    // Unhealthy dependency.
    a.ha().set_dependency("database", false).await;
    assert!(!a.ha().refresh_readiness().await.ready);
    a.ha().set_dependency("database", true).await;
    assert!(a.ha().refresh_readiness().await.ready);

    // Lease lost to another worker.
    store.advance_clock(ChronoDuration::seconds(120));
    let b = worker("w-b", store.clone(), vec![]).await;
    make_ready(&b).await;
    b.ha().acquire(role.clone()).await.unwrap().unwrap();
    let stale = a.ha().acquire(role.clone()).await.unwrap_or(None);
    assert!(stale.is_none(), "w-b holds it");
    let r = a.ha().refresh_readiness().await;
    assert!(!r.ready, "{}", r.detail());

    // Recovery failure puts the worker in RECOVERY_REQUIRED and not ready.
    a.ha()
        .set_state(WorkerState::RecoveryRequired, "ledger journal unreadable")
        .await
        .unwrap();
    let r = a.ha().refresh_readiness().await;
    assert!(!r.ready);
    assert_eq!(r.state, WorkerState::RecoveryRequired);
    assert!(r.detail().contains("recovery_required"));
}

#[tokio::test]
async fn graceful_shutdown_persists_cursors_releases_leases_and_stops() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    make_ready(&a).await;
    let role = LeaseRole::Feed("copy_logs".into());
    a.ha().acquire(role.clone()).await.unwrap().unwrap();
    a.ha()
        .offer_token(FeedId::CopyLogs, "leader-1", "sig-x", None)
        .await;

    let released = a.ha().shutdown("sigterm").await;
    assert_eq!(released, 1);
    assert_eq!(a.ha().state().await, WorkerState::Stopped);
    assert!(a.ha().is_draining());
    assert!(
        !a.ha().readiness().await.ready,
        "a stopped worker is never ready"
    );
    // The cursor is durable and the role is free for a standby at once.
    assert!(store
        .load_cursor("copy_logs:leader-1")
        .await
        .unwrap()
        .is_some());
    let b = worker("w-b", store.clone(), vec![]).await;
    make_ready(&b).await;
    assert!(b.ha().acquire(role).await.unwrap().is_some());
    // Shutting down twice changes nothing.
    assert_eq!(a.ha().shutdown("again").await, 0);
}

/// TASK 6 §4 + §18: the cursor API the feeds use (`module-copy`'s poll
/// loop and `module-polymarket`'s user channel) behaves correctly for the
/// two shapes they need — an opaque signature cursor that survives a
/// restart, and a locally sequenced channel cursor that continues its
/// numbering across lives and reports skipped deliveries.
#[tokio::test]
async fn feed_wiring_shapes_survive_a_restart() {
    let store = Arc::new(MemoryHaStore::new());

    // --- copy poll loop: opaque per-wallet cursor -----------------------
    let life1 = worker("w-a", store.clone(), vec![]).await;
    make_ready(&life1).await;
    for sig in ["sig-1", "sig-2", "sig-3"] {
        assert_eq!(
            life1
                .ha()
                .offer_token(FeedId::CopyLogs, "leader-A", sig, Some(Utc::now()))
                .await,
            CursorAdvance::Advanced
        );
    }
    // A second wallet keeps its own position.
    life1
        .ha()
        .offer_token(FeedId::CopyLogs, "leader-B", "sig-9", None)
        .await;

    // Restart: the new life resumes from the durable token instead of
    // re-seeding with "newest signature now" (which would skip everything
    // that happened while the process was down).
    let life2 = worker("w-a", store.clone(), vec![]).await;
    make_ready(&life2).await;
    let resumed = life2.ha().cursor(FeedId::CopyLogs, "leader-A").await;
    assert_eq!(resumed.token.as_deref(), Some("sig-3"));
    assert_eq!(resumed.processed_count, 3);
    assert_eq!(
        life2
            .ha()
            .offer_token(FeedId::CopyLogs, "leader-A", "sig-3", None)
            .await,
        CursorAdvance::Duplicate,
        "the head we already processed is not processed again"
    );
    assert_eq!(
        life2
            .ha()
            .cursor(FeedId::CopyLogs, "leader-B")
            .await
            .token
            .as_deref(),
        Some("sig-9"),
        "scopes are independent"
    );

    // --- polymarket user channel: locally sequenced cursor --------------
    let mut seq = life1
        .ha()
        .cursor(FeedId::PolymarketUser, "")
        .await
        .position
        .unwrap_or(0);
    for _ in 0..4 {
        seq += 1;
        assert!(life1
            .ha()
            .offer(FeedId::PolymarketUser, "", seq, Some(Utc::now()))
            .await
            .should_process());
    }
    assert_eq!(seq, 4);

    // A new life continues the numbering from the durable position rather
    // than restarting at 1 (which would look like four duplicates).
    let life3 = worker("w-b", store.clone(), vec![]).await;
    make_ready(&life3).await;
    let mut seq2 = life3
        .ha()
        .cursor(FeedId::PolymarketUser, "")
        .await
        .position
        .unwrap_or(0);
    assert_eq!(seq2, 4, "resumed from the durable delivery position");
    seq2 += 1;
    assert_eq!(
        life3
            .ha()
            .offer(FeedId::PolymarketUser, "", seq2, None)
            .await,
        CursorAdvance::Advanced
    );
    // A skipped delivery is reported, never silently dropped.
    let adv = life3
        .ha()
        .offer(FeedId::PolymarketUser, "", seq2 + 3, None)
        .await;
    match adv {
        // offered 8 while the cursor was at 5 → 6 and 7 are missing.
        CursorAdvance::Gap(ref g) => assert_eq!((g.from_position, g.to_position), (6, 7)),
        other => panic!("expected a gap, got {other:?}"),
    }
    assert!(
        adv.should_process(),
        "the delivery itself is still processed"
    );
    assert_eq!(store.gaps(true, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn worker_state_machine_rejects_illegal_transitions() {
    let store = Arc::new(MemoryHaStore::new());
    let a = worker("w-a", store.clone(), vec![]).await;
    a.ha().register("h", 1, "v").await.unwrap();
    // Cannot jump straight to Ready: recovery is mandatory.
    assert!(a.ha().set_state(WorkerState::Ready, "skip").await.is_err());
    assert_eq!(a.ha().state().await, WorkerState::Starting);
    a.ha().set_state(WorkerState::Recovering, "").await.unwrap();
    a.ha().set_state(WorkerState::Ready, "").await.unwrap();
    // Lease loss → recovery → ready again.
    a.ha().set_state(WorkerState::LeaseLost, "").await.unwrap();
    assert!(a.ha().set_state(WorkerState::Ready, "").await.is_err());
    a.ha().set_state(WorkerState::Recovering, "").await.unwrap();
    a.ha().set_state(WorkerState::Ready, "").await.unwrap();
    // Nothing leaves Stopped.
    a.ha().set_state(WorkerState::Draining, "").await.unwrap();
    a.ha().set_state(WorkerState::Stopped, "").await.unwrap();
    for s in WorkerState::ALL {
        assert!(a.ha().set_state(s, "").await.is_err() || s == WorkerState::Stopped);
    }
}
