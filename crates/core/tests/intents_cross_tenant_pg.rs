//! PROMPT 3/10 — INTENT JOURNAL + RECOVERY cross-tenant attack tests
//! (real PostgreSQL).

mod trading_isolation_common;

use bot_core::trading_repository::intent::{
    TenantIntentRead, TenantIntentWrite, TenantRecoveryRepo,
};
use bot_core::trading_repository::repository_error::RepositoryError;

use trading_isolation_common::{at, org, read_scope, run_id, seed_order, setup, write_scope};

#[tokio::test]
async fn intents_are_tenant_fenced_end_to_end() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: intents_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let writes = TenantIntentWrite::new(db.clone());
    let reads = TenantIntentRead::new(db.clone());
    let recovery = TenantRecoveryRepo::new(db.clone());
    let intent_id = format!("intent-{run}");

    // 15. A journals + recovers its own intent (orphaned = pending and
    // older than the cutoff).
    writes
        .record(
            &write_scope(a),
            &intent_id,
            "sniper",
            "SOL/USDC",
            "w1",
            "buy",
            "1.0",
        )
        .await
        .expect("A record");
    let orphans_a = reads
        .list_orphaned(&read_scope(a), at(-1.0), 100)
        .await
        .unwrap();
    assert!(orphans_a.iter().any(|i| i.intent_id == intent_id));

    // Same intent_id text for B: a completely independent row.
    writes
        .record(
            &write_scope(b),
            &intent_id,
            "sniper",
            "SOL/USDC",
            "w2",
            "sell",
            "2.0",
        )
        .await
        .expect("B record with same id text");
    let orphans_b = reads
        .list_orphaned(&read_scope(b), at(-1.0), 100)
        .await
        .unwrap();
    assert!(orphans_b.iter().any(|i| i.intent_id == intent_id));

    // 16. B cannot mutate an A-ONLY intent (no same-id row exists for
    // B — the scoped update matches zero rows and fails closed).
    let a_only = format!("intent-a2-{run}");
    writes
        .record(
            &write_scope(a),
            &a_only,
            "sniper",
            "SOL/USDC",
            "w1",
            "buy",
            "1.0",
        )
        .await
        .expect("A-only record");
    match recovery.abandon_orphan(&write_scope(b), &a_only).await {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not abandon A's intent: {other:?}"),
    }
    match recovery
        .link_orphan(&write_scope(b), &a_only, &format!("sig-{run}"))
        .await
    {
        Err(RepositoryError::StaleWrite(_)) | Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not link A's intent: {other:?}"),
    }
    assert_eq!(
        reads
            .get(&read_scope(a), &a_only)
            .await
            .unwrap()
            .unwrap()
            .status,
        "pending",
        "A's A-only intent untouched by both B attacks"
    );

    // Same-id independence: B's legitimate abandon of its OWN copy
    // never touches A's row with the same intent_id text.
    recovery
        .abandon_orphan(&write_scope(b), &intent_id)
        .await
        .expect("B abandons its OWN same-id copy");
    let a_after = reads
        .get(&read_scope(a), &intent_id)
        .await
        .unwrap()
        .expect("A intent");
    assert_eq!(a_after.status, "pending", "A's intent untouched by B");
    assert_eq!(
        reads
            .get(&read_scope(b), &intent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "abandoned",
        "B's own copy is the one that moved"
    );

    // A links its own; B's copy is unaffected.
    recovery
        .link_orphan(&write_scope(a), &intent_id, &format!("sig-{run}"))
        .await
        .expect("A links own intent");
    assert_eq!(
        reads
            .get(&read_scope(a), &intent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "submitted"
    );
    assert_eq!(
        reads
            .get(&read_scope(b), &intent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "abandoned",
        "B's own copy keeps its own state"
    );

    // The recovery SWEEP is tenant-scoped: A's sweep lists only A's
    // orphans and A's non-terminal orders.
    seed_order(&db, a, &format!("ord-rc-{run}"), None, "submitted").await;
    let items = recovery.sweep(&read_scope(a), at(-1.0)).await.unwrap();
    assert!(items.iter().all(|i| i.intent.organization_id == a));
    assert!(items.iter().all(|i| i.open_order_ids.iter().all(|_| true)));
    // A's sweep CAN see its own open order ids.
    assert!(items
        .iter()
        .any(|i| i.open_order_ids.contains(&format!("ord-rc-{run}"))));
    let items_b = recovery.sweep(&read_scope(b), at(-1.0)).await.unwrap();
    assert!(items_b.iter().all(|i| i.intent.organization_id == b));
}

#[tokio::test]
async fn reconciliation_queue_is_tenant_local() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: intents_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let recovery = TenantRecoveryRepo::new(db.clone());
    let subject = format!("subject-{run}");

    // Both tenants enqueue the SAME (kind, subject): independent rows
    // on the 0033 composite PK.
    recovery
        .enqueue_reconciliation(&write_scope(a), "order", &subject, at(0.0))
        .await
        .expect("A enqueue");
    recovery
        .enqueue_reconciliation(&write_scope(b), "order", &subject, at(0.0))
        .await
        .expect("B enqueue same text");

    let due_a = recovery
        .due_reconciliation(&read_scope(a), 100)
        .await
        .unwrap();
    let due_b = recovery
        .due_reconciliation(&read_scope(b), 100)
        .await
        .unwrap();
    assert!(due_a.iter().any(|(k, s, ..)| k == "order" && s == &subject));
    assert!(due_b.iter().any(|(k, s, ..)| k == "order" && s == &subject));
    assert!(!due_a.is_empty() && !due_b.is_empty());
}
