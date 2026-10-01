//! PROMPT 3/10 — TENANT WORKER LANES cross-tenant isolation (real
//! PostgreSQL, 0032 `worker_claims`): one lane per
//! (organization_id, purpose). The same purpose text under two tenants
//! is two independent lanes; renew/verify/release are full-identity
//! CAS including the tenant; lapse sweep never crosses tenants.

mod trading_isolation_common;

use bot_core::trading_repository::worker_claim::{ClaimDecision, TenantWorkerClaimRepo};
use chrono::Duration;

use trading_isolation_common::{org, read_scope, run_id, setup, write_scope};

#[tokio::test]
async fn worker_lanes_are_independent_and_fenced_per_tenant() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: workers_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let lanes = TenantWorkerClaimRepo::new(db.clone());
    let purpose = format!("copy-{run}");
    let ttl = Duration::seconds(60);

    // Both tenants run the same-named lane — two independent lanes.
    let a_dec = lanes
        .acquire(&write_scope(a), &purpose, "rep-a1", ttl)
        .await
        .unwrap();
    let ClaimDecision::Acquired(a_claim) = a_dec else {
        panic!("A must acquire its lane")
    };
    assert_eq!(a_claim.organization_id, a);
    assert_eq!(a_claim.generation, 1);

    let b_dec = lanes
        .acquire(&write_scope(b), &purpose, "rep-b1", ttl)
        .await
        .unwrap();
    let ClaimDecision::Acquired(b_claim) = b_dec else {
        panic!("B must acquire its OWN lane of the same purpose")
    };
    assert_eq!(b_claim.organization_id, b);
    assert_eq!(b_claim.generation, 1, "independent fencing counter");

    // A's row is untouched by B's acquisition.
    let a_cur = lanes
        .current(&write_scope(a), &purpose)
        .await
        .unwrap()
        .expect("A lane");
    assert_eq!(a_cur.leader_name, "rep-a1");
    assert_eq!(a_cur.generation, 1);
    assert!(!a_cur.released);

    // A second worker of the SAME tenant is rejected by the live leader.
    let a2_dec = lanes
        .acquire(&write_scope(a), &purpose, "rep-a2", ttl)
        .await
        .unwrap();
    match a2_dec {
        ClaimDecision::Rejected {
            holder, generation, ..
        } => {
            assert_eq!(holder, "rep-a1");
            assert_eq!(generation, 1);
        }
        other => panic!("same-tenant contender must be rejected: {other:?}"),
    }

    // renew / verify / release are full-identity CAS — the tenant is
    // part of the predicate, so B can never act on A's lane even with
    // A's exact (leader, generation) pair.
    assert!(lanes
        .renew(
            &write_scope(a),
            &purpose,
            "rep-a1",
            1,
            ttl,
            chrono::Utc::now()
        )
        .await
        .unwrap());
    assert!(!lanes
        .renew(
            &write_scope(b),
            &purpose,
            "rep-a1",
            1,
            ttl,
            chrono::Utc::now()
        )
        .await
        .unwrap());
    assert!(lanes
        .verify(&write_scope(a), &purpose, "rep-a1", 1, chrono::Utc::now())
        .await
        .unwrap());
    assert!(!lanes
        .verify(&write_scope(b), &purpose, "rep-a1", 1, chrono::Utc::now())
        .await
        .unwrap());
    assert!(!lanes
        .release(&write_scope(b), &purpose, "rep-a1", 1, chrono::Utc::now())
        .await
        .unwrap());
    // A voluntarily steps down; the lane row survives as audit trail.
    assert!(lanes
        .release(&write_scope(a), &purpose, "rep-a1", 1, chrono::Utc::now())
        .await
        .unwrap());
    let a_cur = lanes
        .current(&write_scope(a), &purpose)
        .await
        .unwrap()
        .expect("row kept");
    assert!(a_cur.released);
    // A stale identity/generation cannot release again (the row is
    // already released — the CAS refuses).
    assert!(!lanes
        .release(&write_scope(a), &purpose, "rep-a1", 1, chrono::Utc::now())
        .await
        .unwrap());

    // Re-acquisition after release bumps the fencing token.
    let dec = lanes
        .acquire(&write_scope(a), &purpose, "rep-a2", ttl)
        .await
        .unwrap();
    let ClaimDecision::Acquired(c) = dec else {
        panic!("released lane must be re-acquirable by the same tenant")
    };
    assert_eq!(c.generation, 2, "fencing token strictly increases");
    assert_eq!(c.previous_leader.as_deref(), Some("rep-a1"));
    // The old token is fenced off.
    assert!(!lanes
        .verify(&write_scope(a), &purpose, "rep-a1", 1, chrono::Utc::now())
        .await
        .unwrap());

    // The lanes() triage view is scoped.
    let a_lanes = lanes.lanes(&read_scope(a)).await.unwrap();
    assert!(a_lanes.iter().all(|l| l.organization_id == a));
    let b_lanes = lanes.lanes(&read_scope(b)).await.unwrap();
    assert!(b_lanes.iter().all(|l| l.organization_id == b));
    assert_eq!(b_lanes.len(), 1);
    assert_eq!(b_lanes[0].leader_name, "rep-b1");
}

#[tokio::test]
async fn lapsed_lane_sweep_never_crosses_tenants() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: workers_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let lanes = TenantWorkerClaimRepo::new(db.clone());
    let short = format!("short-{run}");
    let live = format!("live-{run}");
    let a_only = format!("a-only-{run}");
    let as_of = chrono::Utc::now() + Duration::seconds(2);

    // Both tenants hold a 1-second lane with the SAME purpose text;
    // A additionally holds a live lane and an A-only lapsed lane.
    for scope in [write_scope(a), write_scope(b)] {
        let dec = lanes
            .acquire(&scope, &short, "rep-1", Duration::seconds(1))
            .await
            .unwrap();
        assert!(matches!(dec, ClaimDecision::Acquired(_)));
    }
    lanes
        .acquire(&write_scope(a), &live, "rep-a1", Duration::minutes(60))
        .await
        .unwrap();
    lanes
        .acquire(&write_scope(a), &a_only, "rep-a1", Duration::seconds(1))
        .await
        .unwrap();

    // Lapsed lists are scoped and carry the owning organization.
    let lapsed_a = lanes.lapsed_lanes(&read_scope(a), as_of).await.unwrap();
    assert!(lapsed_a.iter().all(|l| l.organization_id == a));
    let a_purposes: Vec<&str> = lapsed_a.iter().map(|l| l.purpose.as_str()).collect();
    assert!(
        a_purposes.contains(&short.as_str()),
        "A's short lane lapsed"
    );
    assert!(
        a_purposes.contains(&a_only.as_str()),
        "A's A-only lane lapsed"
    );
    assert!(
        !a_purposes.contains(&live.as_str()),
        "A's live lane must not be lapsed"
    );
    let lapsed_b = lanes.lapsed_lanes(&read_scope(b), as_of).await.unwrap();
    assert!(lapsed_b.iter().all(|l| l.organization_id == b));
    assert_eq!(lapsed_b.len(), 1, "B sees only its own short lane");
    assert_eq!(lapsed_b[0].purpose, short);

    // B cannot force-release A's A-only lane (0 rows, no error leak).
    assert!(!lanes
        .force_release_lapsed(&write_scope(b), &a_only, as_of)
        .await
        .unwrap());

    // sweep_lapsed evaluates lapsedness against REAL now — let the
    // 1-second leases actually expire before sweeping.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    // A's sweep releases ONLY A's lanes; B's same-purpose lane stays.
    let released = lanes.sweep_lapsed(&write_scope(a)).await.unwrap();
    assert!(
        released.contains(&short),
        "A's short lane swept: {released:?}"
    );
    assert!(
        released.contains(&a_only),
        "A's A-only lane swept: {released:?}"
    );
    assert!(!released.contains(&live));
    let b_cur = lanes
        .current(&write_scope(b), &short)
        .await
        .unwrap()
        .expect("B lane row");
    assert!(!b_cur.released, "B's lane survives A's sweep");

    // A's replacement takes over with a fresh fencing token; B's
    // crashed leader (old token) is fenced off in B's own lane.
    let dec = lanes
        .acquire(&write_scope(a), &short, "rep-a2", Duration::minutes(5))
        .await
        .unwrap();
    let ClaimDecision::Acquired(c) = dec else {
        panic!("A's replacement must take over the swept lane")
    };
    assert_eq!(c.generation, 2);
    let dec_b = lanes
        .acquire(&write_scope(b), &short, "rep-b2", Duration::minutes(5))
        .await
        .unwrap();
    let ClaimDecision::Acquired(cb) = dec_b else {
        panic!("B's replacement must take over B's lapsed lane")
    };
    assert_eq!(cb.generation, 2, "takeover bumps B's own fencing token");
    assert_eq!(cb.previous_leader.as_deref(), Some("rep-1"));
    assert!(!lanes
        .verify(&write_scope(b), &short, "rep-1", 1, chrono::Utc::now())
        .await
        .unwrap());
}
