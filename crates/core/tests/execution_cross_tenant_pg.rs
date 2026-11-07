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

/// Move a persisted lease's entire clock backwards between real renewals.
/// This simulates elapsed time deterministically, without sleeping or mocking
/// either renewal implementation. The grant interval must remain constant.
async fn assert_claim_renewal_duration(
    deployment: bool,
    lease_ms: i64,
    elapsed_ms: i64,
    renewals: usize,
) {
    use bot_core::db::claims::PostgresClaimStore;
    use bot_core::ownership::{ClaimStore, StoreContext};

    let Some(db) = setup().await else {
        assert_ne!(std::env::var("REQUIRE_POSTGRES").as_deref(), Ok("1"));
        assert_ne!(std::env::var("REQUIRE_SERVICES").as_deref(), Ok("1"));
        eprintln!("NOT_RUN: claim duration regression requires PostgreSQL");
        return;
    };
    let run = run_id();
    let organization = if deployment {
        bot_core::db::deployment_org::deployment_org_uuid(&db)
            .await
            .unwrap()
            .into()
    } else {
        org(&db, &format!("lease-{run}")).await
    };
    let scope = read_scope(organization);
    let request = claim_req(&format!("lease-renew-{run}"));
    let owner = "lease-owner";
    let lease = Duration::from_millis(lease_ms.try_into().unwrap());
    let grace = Duration::from_secs(30);
    let legacy = PostgresClaimStore::new(db.clone());
    let tenant = TenantClaimRepo::new(db.clone());
    let decision = if deployment {
        legacy
            .claim(
                &request,
                &StoreContext {
                    owner_id: owner.into(),
                    lease,
                    handoff_grace: grace,
                },
            )
            .await
            .unwrap()
    } else {
        tenant
            .claim(&scope, &request, owner, lease, grace)
            .await
            .unwrap()
    };
    let ClaimDecision::Acquired(first) = decision else {
        panic!("fresh claim")
    };

    for renewal in 0..renewals {
        sqlx::query(
            "UPDATE execution_claims
                SET claimed_at = claimed_at - $3::bigint * interval '1 millisecond',
                    last_heartbeat = last_heartbeat - $3::bigint * interval '1 millisecond',
                    lease_until = lease_until - $3::bigint * interval '1 millisecond'
              WHERE organization_id = $1 AND execution_id = $2",
        )
        .bind(organization.as_uuid())
        .bind(&request.execution_id)
        .bind(elapsed_ms)
        .execute(db.pool())
        .await
        .unwrap();
        let renewed = if deployment {
            legacy
                .renew(&request.execution_id, owner, first.epoch)
                .await
                .unwrap()
        } else {
            tenant
                .renew(&scope, &request.execution_id, owner, first.epoch)
                .await
                .unwrap()
        };
        assert!(renewed, "unexpired owner must renew");
        let span_ms: i64 = sqlx::query_scalar(
            "SELECT (EXTRACT(EPOCH FROM (lease_until - last_heartbeat)) * 1000)::bigint
               FROM execution_claims WHERE organization_id = $1 AND execution_id = $2",
        )
        .bind(organization.as_uuid())
        .bind(&request.execution_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(span_ms, lease_ms, "deployment={deployment}, renewal={renewal}: the grant must not inflate or lose millisecond precision");
    }

    // Keep existing fencing guarantees: neither a different owner nor epoch
    // can renew, and an expired lease is not silently resurrected.
    for (candidate_owner, candidate_epoch) in
        [("other-owner", first.epoch), (owner, first.epoch + 1)]
    {
        let renewed = if deployment {
            legacy
                .renew(&request.execution_id, candidate_owner, candidate_epoch)
                .await
                .unwrap()
        } else {
            tenant
                .renew(
                    &scope,
                    &request.execution_id,
                    candidate_owner,
                    candidate_epoch,
                )
                .await
                .unwrap()
        };
        assert!(!renewed);
    }
    sqlx::query(
        "UPDATE execution_claims SET lease_until = now() - interval '1 second'
          WHERE organization_id = $1 AND execution_id = $2",
    )
    .bind(organization.as_uuid())
    .bind(&request.execution_id)
    .execute(db.pool())
    .await
    .unwrap();
    let renewed = if deployment {
        legacy
            .renew(&request.execution_id, owner, first.epoch)
            .await
            .unwrap()
    } else {
        tenant
            .renew(&scope, &request.execution_id, owner, first.epoch)
            .await
            .unwrap()
    };
    assert!(!renewed, "expired leases must remain fenced");
}

#[tokio::test]
async fn claim_renewal_keeps_original_tenant_duration() {
    assert_claim_renewal_duration(false, 10_000, 5_000, 3).await;
}

#[tokio::test]
async fn claim_renewal_keeps_original_deployment_duration() {
    assert_claim_renewal_duration(true, 10_000, 5_000, 3).await;
}

#[tokio::test]
async fn claim_renewal_preserves_subsecond_precision() {
    assert_claim_renewal_duration(false, 1_250, 0, 2).await;
    assert_claim_renewal_duration(true, 1_250, 0, 2).await;
}

#[tokio::test]
async fn claim_renewal_fences_stale_or_expired_owner() {
    assert_claim_renewal_duration(false, 10_000, 0, 0).await;
    assert_claim_renewal_duration(true, 10_000, 0, 0).await;
}

async fn assert_claim_rejects_unrepresentable_duration(deployment: bool, huge_grace: bool) {
    use bot_core::db::claims::PostgresClaimStore;
    use bot_core::ownership::{ClaimStore, StoreContext};

    let Some(db) = setup().await else {
        assert_ne!(std::env::var("REQUIRE_POSTGRES").as_deref(), Ok("1"));
        assert_ne!(std::env::var("REQUIRE_SERVICES").as_deref(), Ok("1"));
        eprintln!("NOT_RUN: claim duration bounds require PostgreSQL");
        return;
    };
    let run = run_id();
    let organization = if deployment {
        bot_core::db::deployment_org::deployment_org_uuid(&db)
            .await
            .unwrap()
            .into()
    } else {
        org(&db, &format!("duration-bound-{run}")).await
    };
    let scope = read_scope(organization);
    let request = claim_req(&format!("duration-bound-{run}"));
    let huge = Duration::from_secs(u64::MAX);
    let (lease, grace) = if huge_grace {
        (Duration::from_secs(10), huge)
    } else {
        (huge, Duration::from_secs(30))
    };
    let owner = "duration-bound-owner";
    let legacy = PostgresClaimStore::new(db.clone());
    let tenant = TenantClaimRepo::new(db.clone());
    let rejected = if deployment {
        let decision = legacy
            .claim(
                &request,
                &StoreContext {
                    owner_id: owner.into(),
                    lease,
                    handoff_grace: grace,
                },
            )
            .await;
        if huge_grace {
            if let Ok(ClaimDecision::Acquired(first)) = &decision {
                assert!(legacy
                    .release(
                        &request.execution_id,
                        owner,
                        first.epoch,
                        ClaimStatus::HandedOff
                    )
                    .await
                    .unwrap());
                let next = legacy
                    .claim(
                        &request,
                        &StoreContext {
                            owner_id: "next-owner".into(),
                            lease,
                            handoff_grace: grace,
                        },
                    )
                    .await
                    .unwrap();
                panic!("overflowing handoff grace was accepted; immediate reacquisition: {next:?}");
            }
        }
        decision.is_err()
    } else {
        let decision = tenant.claim(&scope, &request, owner, lease, grace).await;
        if huge_grace {
            if let Ok(ClaimDecision::Acquired(first)) = &decision {
                assert!(tenant
                    .release(
                        &scope,
                        &request.execution_id,
                        owner,
                        first.epoch,
                        ClaimStatus::HandedOff
                    )
                    .await
                    .unwrap());
                let next = tenant
                    .claim(&scope, &request, "next-owner", lease, grace)
                    .await
                    .unwrap();
                panic!("overflowing handoff grace was accepted; immediate reacquisition: {next:?}");
            }
        }
        decision.is_err()
    };
    assert!(
        rejected,
        "unrepresentable duration must fail before acquiring a lease"
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution_claims WHERE organization_id = $1 AND execution_id = $2",
    )
    .bind(organization.as_uuid())
    .bind(&request.execution_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(rows, 0, "rejected input must not create a claim row");
}

#[tokio::test]
async fn claim_duration_tenant_rejects_unrepresentable_lease() {
    assert_claim_rejects_unrepresentable_duration(false, false).await;
}

#[tokio::test]
async fn claim_duration_deployment_rejects_unrepresentable_lease() {
    assert_claim_rejects_unrepresentable_duration(true, false).await;
}

#[tokio::test]
async fn claim_duration_tenant_rejects_overflowing_handoff_grace() {
    assert_claim_rejects_unrepresentable_duration(false, true).await;
}

#[tokio::test]
async fn claim_duration_deployment_rejects_overflowing_handoff_grace() {
    assert_claim_rejects_unrepresentable_duration(true, true).await;
}
