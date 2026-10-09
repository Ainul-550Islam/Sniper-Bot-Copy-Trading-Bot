//! PROMPT 3/10 — EXECUTIONS / TRANSACTIONS / CLAIMS / IDEMPOTENCY
//! cross-tenant attack tests (real PostgreSQL).

mod trading_isolation_common;

use std::time::Duration;

use bot_core::ownership::{ClaimDecision, ClaimRequest, ClaimStatus};
use bot_core::trading_repository::executions::{
    TenantClaimRepo, TenantExecutionRead, TenantExecutionWrite, TenantIdempotencyRepo,
};
use bot_core::trading_repository::repository_error::RepositoryError;

use trading_isolation_common::{at, org, read_scope, run_id, seed_order, setup, write_scope};

fn claim_req(execution_id: &str) -> ClaimRequest {
    ClaimRequest {
        execution_id: execution_id.to_string(),
        kind: "entry".to_string(),
        module: "sniper".to_string(),
        strategy: "s1".to_string(),
        symbol: "SOL/USDC".to_string(),
    }
}

#[tokio::test]
async fn executions_reads_and_writes_are_tenant_fenced() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: execution_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let order_id = format!("ord-ex-{run}");
    seed_order(&db, a, &order_id, None, "submitted").await;

    let reads = TenantExecutionRead::new(db.clone());
    let writes = TenantExecutionWrite::new(db.clone());

    // A appends an execution under its own order.
    let exec_id = writes
        .append(
            &write_scope(a),
            &order_id,
            "send",
            Some("https://rpc"),
            Some(12),
            true,
            Some("ok"),
            at(0.0),
        )
        .await
        .expect("A append execution");

    // 7. A reads A execution.
    let got = reads
        .get(&read_scope(a), exec_id)
        .await
        .expect("A read execution")
        .expect("present");
    assert_eq!(got.organization_id, a);
    assert!(reads
        .list_for_order(&read_scope(a), &order_id, 10)
        .await
        .unwrap()
        .iter()
        .any(|e| e.id == exec_id));

    // 8. B cannot read A execution (absent, not leaked).
    assert!(reads.get(&read_scope(b), exec_id).await.unwrap().is_none());
    assert!(reads
        .list_for_order(&read_scope(b), &order_id, 10)
        .await
        .unwrap()
        .is_empty());

    // B cannot append under A's order (in-tx ownership check).
    match writes
        .append(
            &write_scope(b),
            &order_id,
            "send",
            None,
            None,
            false,
            Some("attack"),
            at(0.0),
        )
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not append under A's order: {other:?}"),
    }

    // Transactions: signature stays globally unique, attribution is
    // the acting tenant's; the lookups answer only for the owner.
    let txs = TenantExecutionWrite::new(db.clone());
    let sig = format!("sig-{run}");
    let fresh = txs
        .record_transaction_submitted(
            &write_scope(a),
            "solana",
            &sig,
            Some(&order_id),
            None,
            None,
            1,
        )
        .await
        .expect("A records transaction");
    assert!(fresh);
    // A replay with the same signature does not duplicate.
    let replay = txs
        .record_transaction_submitted(
            &write_scope(a),
            "solana",
            &sig,
            Some(&order_id),
            None,
            None,
            2,
        )
        .await
        .expect("replay");
    assert!(!replay, "signature stays globally unique (no second row)");
    // A can read its transaction.
    assert!(reads
        .transaction(&read_scope(a), &sig)
        .await
        .unwrap()
        .is_some());
    // B's scoped lookup does not surface A's row.
    assert!(reads
        .transaction(&read_scope(b), &sig)
        .await
        .unwrap()
        .is_none());
    // B cannot transition A's transaction: the scoped UPDATE matches
    // zero rows and the repository fails CLOSED with NotFound.
    match txs
        .set_transaction_status(&write_scope(b), &sig, "failed", None, Some("attack"))
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B's cross-tenant transition must fail closed: {other:?}"),
    }
    let a_tx = reads
        .transaction(&read_scope(a), &sig)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a_tx.status, "submitted", "B's write touched nothing of A's");
}

#[tokio::test]
async fn claims_race_and_cross_tenant_independence() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: execution_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let claims = TenantClaimRepo::new(db.clone());
    let execution_id = format!("exec-{run}");

    // 9. B cannot claim A's execution: same execution_id text is a
    // DIFFERENT lane for B (tenant-composite arbiter) — and A's row
    // is never mutated by B's claim.
    let a_decision = claims
        .claim(
            &read_scope(a),
            &claim_req(&execution_id),
            "worker-a1",
            Duration::from_secs(30),
            Duration::from_secs(5),
        )
        .await
        .expect("A claims");
    let ClaimDecision::Acquired(a_claim) = a_decision else {
        panic!("A must acquire");
    };
    assert_eq!(a_claim.owner_id, "worker-a1");

    // B claims "the same id" — succeeds for ITS OWN lane, A untouched.
    let b_decision = claims
        .claim(
            &read_scope(b),
            &claim_req(&execution_id),
            "worker-b1",
            Duration::from_secs(30),
            Duration::from_secs(5),
        )
        .await
        .expect("B claims its own lane");
    let ClaimDecision::Acquired(b_claim) = b_decision else {
        panic!("B must acquire its own tenant-local lane")
    };
    assert_eq!(b_claim.owner_id, "worker-b1");
    assert_eq!(b_claim.epoch, 1, "independent epoch counter per tenant");

    let a_row = claims
        .get(&read_scope(a), &execution_id)
        .await
        .unwrap()
        .expect("A row");
    assert_eq!(a_row.owner_id, "worker-a1", "A's claim untouched by B");
    assert_eq!(a_row.epoch, 1);

    // 11. Same-tenant race: two workers of A, one winner.
    let exec2 = format!("exec2-{run}");
    let race_scope = read_scope(a);
    let race_req = claim_req(&exec2);
    let (d1, d2) = tokio::join!(
        claims.claim(
            &race_scope,
            &race_req,
            "worker-a1",
            Duration::from_secs(30),
            Duration::from_secs(5),
        ),
        claims.claim(
            &race_scope,
            &race_req,
            "worker-a2",
            Duration::from_secs(30),
            Duration::from_secs(5),
        ),
    );
    let winners = [d1.expect("race 1"), d2.expect("race 2")]
        .into_iter()
        .filter(|d| matches!(d, ClaimDecision::Acquired(_)))
        .count();
    assert_eq!(winners, 1, "exactly one winner in the same-tenant race");
    let final_row = claims
        .get(&read_scope(a), &exec2)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(final_row.epoch, 1, "single insert (no double-bump)");

    // renew/verify/release are owner-CAS'd and tenant-fenced.
    assert!(claims
        .renew(
            &read_scope(a),
            &exec2,
            final_row.owner_id.as_str(),
            final_row.epoch
        )
        .await
        .unwrap());
    assert!(claims
        .verify(
            &read_scope(a),
            &exec2,
            final_row.owner_id.as_str(),
            final_row.epoch
        )
        .await
        .unwrap());
    // B cannot renew or release A's lane.
    assert!(!claims
        .renew(
            &read_scope(b),
            &exec2,
            final_row.owner_id.as_str(),
            final_row.epoch
        )
        .await
        .unwrap());
    assert!(!claims
        .release(
            &read_scope(b),
            &exec2,
            final_row.owner_id.as_str(),
            final_row.epoch,
            ClaimStatus::Released
        )
        .await
        .unwrap());
    assert!(claims
        .release(
            &read_scope(a),
            &exec2,
            final_row.owner_id.as_str(),
            final_row.epoch,
            ClaimStatus::Released
        )
        .await
        .unwrap());

    // Lapsed leases stay tenant-scoped: A's lapsed list carries A's
    // short-lease claim but never B's (and vice versa).
    let exec_a_lapse = format!("exec-a-lapse-{run}");
    let exec_b_lapse = format!("exec-b-lapse-{run}");
    for (scope, exec) in [
        (read_scope(a), &exec_a_lapse),
        (read_scope(b), &exec_b_lapse),
    ] {
        let decision = claims
            .claim(
                &scope,
                &claim_req(exec),
                "worker-lapse",
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .await
            .expect("short-lease claim");
        assert!(matches!(decision, ClaimDecision::Acquired(_)));
    }
    // `as_of` in the future ⇒ both leases have lapsed by then.
    let lapsed_a = claims.list_lapsed(&read_scope(a), at(-0.01)).await.unwrap();
    let lapsed_b = claims.list_lapsed(&read_scope(b), at(-0.01)).await.unwrap();
    let a_ids: Vec<&str> = lapsed_a.iter().map(|c| c.execution_id.as_str()).collect();
    let b_ids: Vec<&str> = lapsed_b.iter().map(|c| c.execution_id.as_str()).collect();
    assert!(
        a_ids.contains(&exec_a_lapse.as_str()),
        "A sees own lapsed lease"
    );
    assert!(
        !a_ids.contains(&exec_b_lapse.as_str()),
        "A must never see B's lapsed lease"
    );
    assert!(
        b_ids.contains(&exec_b_lapse.as_str()),
        "B sees own lapsed lease"
    );
    assert!(
        !b_ids.contains(&exec_a_lapse.as_str()),
        "B must never see A's lapsed lease"
    );
}

#[tokio::test]
async fn idempotency_keys_are_tenant_local_namespaces() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: execution_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let idem = TenantIdempotencyRepo::new(db.clone());
    let key = format!("idem-{run}");

    // 12. A consumes and records a response.
    assert!(idem.try_consume(&read_scope(a), "api", &key).await.unwrap());
    idem.record_response(
        &read_scope(a),
        "api",
        &key,
        &serde_json::json!({ "result": "A-secret" }),
    )
    .await
    .expect("record A response");

    // 13. B's same text key is an independent namespace.
    assert!(idem.try_consume(&read_scope(b), "api", &key).await.unwrap());
    // B replay: duplicate.
    assert!(!idem.try_consume(&read_scope(b), "api", &key).await.unwrap());

    // 14. B cannot retrieve A's idempotency response.
    assert!(idem
        .response(&read_scope(b), "api", &key)
        .await
        .unwrap()
        .is_none());
    let a_resp = idem
        .response(&read_scope(a), "api", &key)
        .await
        .unwrap()
        .expect("A reads own response");
    assert_eq!(a_resp["result"], "A-secret");
    assert!(idem.held(&read_scope(a), "api", &key).await.unwrap());
    assert!(!idem
        .held(&read_scope(b), "api", &format!("other-{run}"))
        .await
        .unwrap());

    // Cleanup only drops the acting tenant's keys.
    idem.cleanup_older_than(&read_scope(a), chrono::Duration::days(365), at(0.0))
        .await
        .unwrap();
    assert!(
        idem.held(&read_scope(b), "api", &key).await.unwrap(),
        "B survives A's cleanup"
    );
}
