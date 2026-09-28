//! One authoritative idempotency boundary (TASK 4): the OMS intent key.
//! Concurrent submissions of the same signal collapse onto one order and
//! one venue POST; a restarted bot sharing the OMS refuses the same intent;
//! distinct intents on the same token are stopped by the open-order /
//! in-market gates; ownership claims elect one submitter per token.

mod common;

use std::sync::Arc;

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::oms::OrderStatus;
use common::*;
use module_polymarket::orders::{PolyStage, RejectReason};
use module_polymarket::store::MemoryPolyStore;

#[tokio::test]
async fn concurrent_identical_signals_produce_exactly_one_order() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = Arc::new(live_bot(&state, store.clone()).await);
    let cfg = Arc::new(poly_cfg(&state).await);
    let sig = Arc::new(signal(decision(), ExecutionMode::Live));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let bot = bot.clone();
        let cfg = cfg.clone();
        let sig = sig.clone();
        handles.push(tokio::spawn(async move {
            bot.process_signal(&sig, &market(), &quotes(), &cfg).await
        }));
    }
    let mut outcomes = Vec::new();
    for h in handles {
        outcomes.push(h.await.unwrap());
    }
    let submitted: Vec<_> = outcomes
        .iter()
        .filter(|o| o.stage == PolyStage::Resting)
        .collect();
    let dup: Vec<_> = outcomes
        .iter()
        .filter(|o| o.reject_reason == Some(RejectReason::DuplicateIntent))
        .collect();
    let open: Vec<_> = outcomes
        .iter()
        .filter(|o| o.reject_reason == Some(RejectReason::OrderAlreadyOpen))
        .collect();
    assert_eq!(submitted.len(), 1, "{outcomes:?}");
    assert_eq!(
        dup.len() + open.len(),
        7,
        "every loser is refused by the intent key (or by the open-order gate once the winner is tracked): {outcomes:?}"
    );
    assert_eq!(venue.posted_orders(), 1, "exactly one venue POST");
    assert_eq!(oms.list(50).await.len(), 1, "exactly one OMS order");
    assert_eq!(oms.list(50).await[0].idempotency_key, sig.intent_key());
    assert_eq!(store.orders().await.len(), 1);
    // Rejections were still journaled (one row: same signal id).
    assert_eq!(store.signals().await.len(), 1);
}

#[tokio::test]
async fn a_restarted_bot_sharing_the_oms_refuses_the_same_intent() {
    let venue = mock_venue().await;
    let mut cfg = base_config(&venue);
    // The shared risk engine's re-entry cooldown would fire first after the
    // position below is closed; switch it off so the intent key is what
    // refuses the replay.
    cfg.risk.reentry_cooldown_secs = 0;
    let (state, oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let cfg = poly_cfg(&state).await;
    let sig = signal(decision(), ExecutionMode::Paper);

    {
        let bot = paper_bot(&state, store.clone()).await;
        let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
        assert_eq!(out.stage, PolyStage::Filled);
    }
    // "Restart": a fresh bot, same OMS + journal, same signal → the OMS key
    // is the authority even though the tracker is empty. (The position gate
    // fires first here because the paper fill is already booked; clear it
    // to prove the key alone is enough.)
    let bot2 = paper_bot(&state, store.clone()).await;
    let out = bot2.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::AlreadyInMarket),
        "{out:?}"
    );
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    state
        .close_position(&pos.id, bot_core::models::PositionStatus::Closed, "test")
        .await;
    let out = bot2.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Rejected, "{out:?}");
    assert_eq!(out.reject_reason, Some(RejectReason::DuplicateIntent));
    assert!(
        out.detail.contains("already recorded as order"),
        "{}",
        out.detail
    );
    assert_eq!(oms.list(10).await.len(), 1);
    assert_eq!(oms.list(10).await[0].status, OrderStatus::Filled);
}

#[tokio::test]
async fn intent_identity_is_semantic_not_temporal() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let bot = paper_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let cfg = poly_cfg(&state).await;

    let a = signal(decision(), ExecutionMode::Paper);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let b = signal(decision(), ExecutionMode::Paper);
    assert_eq!(
        a.intent_key(),
        b.intent_key(),
        "created_at is not part of the identity"
    );
    assert_eq!(a.signal_id, b.signal_id);

    // Different price / size / mode / order type / expiry → different intents.
    let c = signal(decision_for(YES, "Yes", 0.39, 25.0), ExecutionMode::Paper);
    assert_ne!(a.intent_key(), c.intent_key());
    let d = signal(decision(), ExecutionMode::Live);
    assert_ne!(a.intent_key(), d.intent_key());
    let mut e = signal(decision(), ExecutionMode::Paper);
    e.expiration = 1_900_000_000;
    let e = module_polymarket::orders::OrderSignal::new(
        e.decision.clone(),
        "value",
        ExecutionMode::Paper,
        "GTD",
        "0.01",
        1_900_000_000,
        "q",
        chrono::Utc::now(),
    );
    assert_ne!(a.intent_key(), e.intent_key());

    let out = bot.process_signal(&a, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Filled);
    let out = bot.process_signal(&b, &market(), &quotes(), &cfg).await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::AlreadyInMarket),
        "the position gate runs first"
    );
    assert_eq!(oms.list(10).await.len(), 1);
}

#[tokio::test]
async fn ownership_registry_elects_one_submitter_per_token() {
    use bot_core::ownership::{MemoryClaimStore, OwnershipRegistry};
    use std::time::Duration;

    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let cfg = poly_cfg(&state).await;

    // Two replicas over one shared claim store: the first claim on
    // `poly:entry:<token>` wins, the second is refused as owned elsewhere.
    let claims = Arc::new(MemoryClaimStore::new());
    let registry = Arc::new(OwnershipRegistry::new(
        claims.clone(),
        "replica-a",
        Duration::from_secs(30),
        Duration::from_secs(900),
    ));
    let bot_a = paper_bot(&state, store.clone())
        .await
        .with_ownership(registry.clone());

    let registry_b = Arc::new(OwnershipRegistry::new(
        claims,
        "replica-b",
        Duration::from_secs(30),
        Duration::from_secs(900),
    ));
    let bot_b = paper_bot(&state, store.clone())
        .await
        .with_ownership(registry_b);

    // Hold A's permit by making A's pipeline claim first: paper fills
    // release the claim immediately, so instead prove the negative path
    // directly — B cannot claim what A holds.
    let mut permit_a = bot_core::ownership::Permit::acquire(
        Some(registry.as_ref()),
        format!("poly:entry:{YES}"),
        "poly_entry",
        "polymarket",
        "clob",
        YES,
    )
    .await
    .unwrap();
    assert!(permit_a.proceed());

    let out = bot_b
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Rejected, "{out:?}");
    assert_eq!(out.reject_reason, Some(RejectReason::OwnedByOtherReplica));
    // The OMS record B created for its intent is failed, not left dangling.
    let orders = oms.list(10).await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].status, OrderStatus::Failed);
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());

    permit_a.finish(false).await;
    // Once released, A (or anyone) proceeds: a distinct intent (different
    // price) because the failed one is already recorded under its key.
    let out = bot_a
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.39, 25.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
}
