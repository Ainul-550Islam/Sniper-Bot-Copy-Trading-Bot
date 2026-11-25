//! PROMPT 3/10 — ORDERS cross-tenant attack tests (real PostgreSQL).
//!
//! Matrix: A creates ✓, A reads ✓, B read/update/delete/transition ✗,
//! same tenant-local idempotency key allowed for BOTH tenants,
//! signature lookups stay tenant-scoped, cursor cannot jump tenants.

mod trading_isolation_common;

use bot_core::trading_repository::not_found::ResourceKind;
use bot_core::trading_repository::orders::{TenantOrderRead, TenantOrderWrite};
use bot_core::trading_repository::pagination::TenantPageRequest;
use bot_core::trading_repository::repository_error::RepositoryError;

use trading_isolation_common::{at, org, read_scope, run_id, seed_order, setup, write_scope};

#[tokio::test]
async fn tenant_a_crud_and_tenant_b_denied_everything() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: orders_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let reads = TenantOrderRead::new(db.clone());
    let writes = TenantOrderWrite::new(db.clone());
    let id = format!("ord-a-{run}");

    // 1. A creates.
    let inserted = writes
        .insert_if_absent(
            &write_scope(a),
            &id,
            Some(&format!("idem-a-{run}")),
            "sniper",
            "buy",
            "SOL/USDC",
            "paper",
            "paper",
            "created",
            1.0,
            None,
            &serde_json::json!({}),
            at(0.0),
        )
        .await
        .expect("A insert");
    assert!(inserted, "first insert wins");

    // Idempotent replay for A: same (org, idempotency_key) → false.
    let replay = writes
        .insert_if_absent(
            &write_scope(a),
            &format!("ord-a2-{run}"),
            Some(&format!("idem-a-{run}")),
            "sniper",
            "buy",
            "SOL/USDC",
            "paper",
            "paper",
            "created",
            1.0,
            None,
            &serde_json::json!({}),
            at(0.0),
        )
        .await
        .expect("A replay");
    assert!(!replay, "duplicate idempotency key refused for SAME tenant");

    // 2. A reads its order.
    let got = reads
        .get(&read_scope(a), &id)
        .await
        .expect("A reads own order");
    assert_eq!(got.id, id);
    assert_eq!(got.organization_id, a);

    // 3. B cannot read A's order — tenant-safe not-found, no leak.
    match reads.get(&read_scope(b), &id).await {
        Err(RepositoryError::NotFound(kind)) => {
            assert_eq!(kind, ResourceKind::Order.as_str());
        }
        other => panic!("B must not read A's order: {:?}", other.map(|o| o.id)),
    }

    // 4. B cannot transition A's order (guarded CAS sees zero rows).
    match writes
        .set_status(
            &write_scope(b),
            &id,
            "created",
            "submitted",
            None,
            None,
            None,
            None,
            at(0.0),
        )
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not transition A's order: {other:?}"),
    }
    // A's row is untouched.
    assert_eq!(
        reads.get(&read_scope(a), &id).await.unwrap().status,
        "created"
    );

    // 5. B cannot cancel A's order.
    match writes
        .cancel(&write_scope(b), &id, Some("attack"), at(0.0))
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not cancel A's order: {other:?}"),
    }
    assert_eq!(
        reads.get(&read_scope(a), &id).await.unwrap().status,
        "created"
    );

    // 6. B cannot delete A's order.
    match writes.delete(&write_scope(b), &id).await {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not delete A's order: {other:?}"),
    }
    assert!(
        reads.get(&read_scope(a), &id).await.is_ok(),
        "A's row survives"
    );

    // A's own transitions work.
    writes
        .set_status(
            &write_scope(a),
            &id,
            "created",
            "submitted",
            None,
            None,
            Some(at(0.0)),
            None,
            at(0.0),
        )
        .await
        .expect("A transitions own order");
    writes
        .cancel(&write_scope(a), &id, Some("owner"), at(0.0))
        .await
        .expect("A cancels own order");
    assert_eq!(
        reads.get(&read_scope(a), &id).await.unwrap().status,
        "cancelled"
    );
    let history = reads
        .history(&read_scope(a), &id, 10)
        .await
        .expect("history");
    assert!(history.len() >= 2, "history recorded in-tenant");
    // B sees no history for A's order.
    assert!(reads
        .history(&read_scope(b), &id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn same_tenant_local_idempotency_key_both_tenants_allowed() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: orders_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let writes = TenantOrderWrite::new(db.clone());
    let reads = TenantOrderRead::new(db.clone());
    let shared_key = format!("shared-idem-{run}");

    for (tenant, label) in [(a, "a"), (b, "b")] {
        let ok = writes
            .insert_if_absent(
                &write_scope(tenant),
                &format!("ord-{label}-{run}"),
                Some(&shared_key),
                "sniper",
                "buy",
                "SOL/USDC",
                "paper",
                "paper",
                "created",
                1.0,
                None,
                &serde_json::json!({}),
                at(0.0),
            )
            .await
            .unwrap_or_else(|e| panic!("{label} insert: {e:?}"));
        assert!(ok, "{label} inserts with the SAME idempotency key text");
    }
    // Both rows exist, each attributed to its own tenant.
    let row_a = reads
        .get_by_key(&read_scope(a), &shared_key)
        .await
        .expect("A key lookup")
        .expect("A row present");
    assert_eq!(row_a.organization_id, a);
    let row_b = reads
        .get_by_key(&read_scope(b), &shared_key)
        .await
        .expect("B key lookup")
        .expect("B row present");
    assert_eq!(row_b.organization_id, b);
    assert_ne!(row_a.id, row_b.id);

    // Replay within each tenant is a duplicate.
    for (tenant, label) in [(a, "a"), (b, "b")] {
        let dup = writes
            .insert_if_absent(
                &write_scope(tenant),
                &format!("ord-{label}-dup-{run}"),
                Some(&shared_key),
                "sniper",
                "buy",
                "SOL/USDC",
                "paper",
                "paper",
                "created",
                1.0,
                None,
                &serde_json::json!({}),
                at(0.0),
            )
            .await
            .expect("replay");
        assert!(!dup, "{label} replay refused");
    }
}

#[tokio::test]
async fn signature_lookup_and_pagination_stay_tenant_scoped() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: orders_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let reads = TenantOrderRead::new(db.clone());
    let sig = format!("sig-{run}");
    seed_order(&db, a, &format!("ord-sig-a-{run}"), None, "created").await;
    sqlx::query("UPDATE orders SET signature = $2 WHERE organization_id = $1 AND id = $3")
        .bind(a.as_uuid())
        .bind(&sig)
        .bind(format!("ord-sig-a-{run}"))
        .execute(db.pool())
        .await
        .expect("attach signature");
    seed_order(&db, b, &format!("ord-b-{run}"), None, "created").await;
    // A second A order so a limit-1 page provably has a next page.
    seed_order(&db, a, &format!("ord-sig-a2-{run}"), None, "created").await;

    // A finds its order by signature; B does not (same signature text).
    assert!(reads
        .get_by_signature(&read_scope(a), &sig)
        .await
        .expect("sig lookup A")
        .is_some());
    assert!(reads
        .get_by_signature(&read_scope(b), &sig)
        .await
        .expect("sig lookup B")
        .is_none());

    // list_incomplete only sees the caller's rows.
    let open_a = reads.list_incomplete(&read_scope(a)).await.unwrap();
    let open_b = reads.list_incomplete(&read_scope(b)).await.unwrap();
    assert!(open_a.iter().all(|o| o.organization_id == a));
    assert!(open_b.iter().all(|o| o.organization_id == b));
    assert!(open_a.iter().any(|o| o.id.ends_with(&run)));
    assert!(!open_a.iter().any(|o| o.organization_id == b));

    // count_by_status never counts the other tenant.
    let counts_a = reads.count_by_status(&read_scope(a)).await.unwrap();
    let sum_a: i64 = counts_a.iter().map(|c| c.1).sum();
    let counts_b = reads.count_by_status(&read_scope(b)).await.unwrap();
    let sum_b: i64 = counts_b.iter().map(|c| c.1).sum();
    assert!(sum_a >= 1 && sum_b >= 1);

    // A cursor minted by A is rejected for B (fail closed).
    let page_a = TenantPageRequest::new(a, Some(1), None).expect("page A");
    let listed = reads.list_page(&read_scope(a), &page_a).await.unwrap();
    let cursor = listed.next.expect("cursor minted");
    let err = TenantPageRequest::new(b, Some(10), Some(&cursor.encode()))
        .expect_err("cross-tenant cursor must fail");
    let _ = err;
}

#[tokio::test]
async fn status_history_rows_carry_the_order_tenant() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: status_history_rows_carry_the_order_tenant — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("history-{run}")).await;
    let id = format!("history-order-{run}");
    seed_order(&db, tenant, &id, None, "created").await;
    let writer = TenantOrderWrite::new(db.clone());
    writer
        .set_status(
            &write_scope(tenant),
            &id,
            "created",
            "submitted",
            None,
            None,
            None,
            None,
            at(0.0),
        )
        .await
        .unwrap();
    let owners: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM order_status_history WHERE order_id = $1")
            .bind(&id)
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        owners,
        vec![tenant.as_uuid()],
        "history row must carry the same tenant as its order"
    );
}

#[tokio::test]
async fn upsert_foreign_order_collision_must_not_report_success() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: upsert_foreign_order_collision_must_not_report_success — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("upsert-a-{run}")).await;
    let b = org(&db, &format!("upsert-b-{run}")).await;
    let id = format!("upsert-order-{run}");
    seed_order(&db, a, &id, None, "created").await;
    let reader = TenantOrderRead::new(db.clone());
    let writer = TenantOrderWrite::new(db.clone());
    let mut candidate = reader.get(&read_scope(a), &id).await.unwrap();
    candidate.organization_id = b;
    candidate.status = "submitted".to_string();
    assert!(
        writer.upsert(&write_scope(b), &candidate).await.is_err(),
        "foreign id collision must not be silent success"
    );
    assert_eq!(
        reader.get(&read_scope(a), &id).await.unwrap().status,
        "created"
    );
    assert!(reader.get(&read_scope(b), &id).await.is_err());
}

#[tokio::test]
async fn deployment_lifecycle_prune_preserves_other_tenants() {
    let Some(db) = setup().await else {
        eprintln!(
            "NOT_RUN: deployment_lifecycle_prune_preserves_other_tenants — POSTGRES_URL missing"
        );
        return;
    };
    let tenant = org(&db, &format!("prune-foreign-{}", run_id())).await;
    let deployment = bot_core::db::deployment_org::deployment_org_uuid(&db)
        .await
        .unwrap();
    assert_ne!(tenant.as_uuid(), deployment);
    let own = format!("prune-own-{}", run_id());
    let foreign = format!("prune-foreign-{}", run_id());
    for (owner, id) in [(deployment, &own), (tenant.as_uuid(), &foreign)] {
        sqlx::query("INSERT INTO execution_lifecycle (organization_id,intent_id,state,updated_at) VALUES ($1,$2,'confirmed',now()-interval '101 years')")
            .bind(owner).bind(id).execute(db.pool()).await.unwrap();
    }
    let repo = bot_core::db::execution::ExecutionRepo::new(db.clone());
    repo.delete_settled_older_than(chrono::Duration::days(36525))
        .await
        .unwrap();
    let surviving: Vec<(uuid::Uuid,String)> = sqlx::query_as("SELECT organization_id,intent_id FROM execution_lifecycle WHERE intent_id=ANY($1) ORDER BY intent_id")
        .bind(vec![own.clone(),foreign.clone()]).fetch_all(db.pool()).await.unwrap();
    assert_eq!(
        surviving,
        vec![(tenant.as_uuid(), foreign)],
        "deployment cleanup must not delete another tenant's history"
    );
}
