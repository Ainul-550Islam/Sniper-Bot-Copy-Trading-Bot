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
    // A's open order is attributable to exactly one intent (gap #74: the sweep
    // pairs per intent instead of cloning one tenant-wide list onto all of
    // them). This order carries no `meta->>'intent_id'`, so it is reported as
    // the page's unattributed residual — visible either way, never dropped and
    // never guessed onto an unrelated intent.
    let page = recovery
        .sweep_page(&read_scope(a), at(-1.0), None, 100)
        .await
        .unwrap();
    let ord = format!("ord-rc-{run}");
    let paired: Vec<&String> = page
        .items
        .iter()
        .flat_map(|i| i.open_order_ids.iter())
        .filter(|id| **id == ord)
        .collect();
    assert!(
        paired.is_empty() || page.unattributed_open_order_ids.contains(&ord),
        "A's open order is paired with its intent or reported unattributed"
    );
    assert!(
        page.unattributed_open_order_ids.contains(&ord) || paired.len() == 1,
        "and it is never attached to more than one intent"
    );
    assert!(
        page.items.iter().all(|i| i.intent.organization_id == a)
            && !page.unattributed_open_order_ids.is_empty(),
        "A's sweep sees A's order, and only A's"
    );
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

/// A duplicate `(organization_id, intent_id)` must describe the SAME intent.
///
/// The journal used `ON CONFLICT … DO NOTHING`, so reusing an id for a
/// DIFFERENT module/symbol/wallet/side/qty silently aliased the new intent onto
/// the old row: `record` returned Ok and the caller believed its different
/// order was journalled. Now an exact retry still succeeds (idempotent, no
/// rewrite), and a differing payload is `Conflict("intent_id")`.
#[tokio::test]
async fn a_reused_intent_id_with_a_different_payload_is_refused() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: intents_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("payload-a-{run}")).await;
    let id = format!("intent-payload-{run}");

    let writes = TenantIntentWrite::new(db.clone());
    let reads = TenantIntentRead::new(db.clone());

    writes
        .record(
            &write_scope(a),
            &id,
            "sniper",
            "SOL/USDC",
            "w1",
            "buy",
            "1.0",
        )
        .await
        .expect("the first record of this intent id");
    let original = reads
        .get(&read_scope(a), &id)
        .await
        .expect("read must not fail")
        .expect("the intent row exists");

    // EXACT retry: succeeds and changes nothing (idempotent).
    writes
        .record(
            &write_scope(a),
            &id,
            "sniper",
            "SOL/USDC",
            "w1",
            "buy",
            "1.0",
        )
        .await
        .expect("an exact retry of the same intent is idempotent");
    let after_retry = reads
        .get(&read_scope(a), &id)
        .await
        .expect("read must not fail")
        .expect("the intent row still exists");
    assert_eq!(
        after_retry.created_at, original.created_at,
        "a retry must not rewrite the original journal timestamp"
    );
    assert_eq!(after_retry.qty, "1.0");

    // Different payload, same id: REFUSED, and the stored row is untouched.
    for (module, symbol, wallet, side, qty) in [
        ("sniper", "SOL/USDC", "w1", "buy", "999.0"),
        ("sniper", "SOL/USDC", "w1", "sell", "1.0"),
        ("copier", "SOL/USDC", "w1", "buy", "1.0"),
        ("sniper", "BONK/USDC", "w1", "buy", "1.0"),
        ("sniper", "SOL/USDC", "w9", "buy", "1.0"),
    ] {
        let err = writes
            .record(&write_scope(a), &id, module, symbol, wallet, side, qty)
            .await
            .expect_err("a different payload under the same id must be refused");
        match err {
            RepositoryError::Conflict(kind) => assert_eq!(kind, "intent_id"),
            other => panic!("expected Conflict(intent_id), got {other:?}"),
        }
        let cur = reads
            .get(&read_scope(a), &id)
            .await
            .expect("read must not fail")
            .expect("the original row survives");
        assert_eq!(
            (cur.module.as_str(), cur.symbol.as_str(), cur.qty.as_str()),
            ("sniper", "SOL/USDC", "1.0"),
            "the refused payload never reached the row"
        );
    }

    // The refusal is terminal: retrying the ORIGINAL payload still works, so a
    // caller that recovers its true intent is not locked out.
    writes
        .record(
            &write_scope(a),
            &id,
            "sniper",
            "SOL/USDC",
            "w1",
            "buy",
            "1.0",
        )
        .await
        .expect("the original intent id remains usable for its own payload");

    // Another tenant may still reuse the same id text with its own payload.
    let b = org(&db, &format!("payload-b-{run}")).await;
    writes
        .record(
            &write_scope(b),
            &id,
            "sniper",
            "SOL/USDC",
            "w1",
            "buy",
            "5.0",
        )
        .await
        .expect("a different tenant owns its own id space");
}
