//! PROMPT 3/10 — COPY TRADING cross-tenant isolation (real
//! PostgreSQL): leader config, leader lifecycle events, the copy event
//! log and follower links are all tenant-local (0029 composite
//! arbiters). The SAME external leader address / event id / position id
//! text may exist for both tenants — as independent rows.

mod trading_isolation_common;

use bot_core::tenant::OrganizationId;
use bot_core::trading_repository::copy::{
    model::{TenantCopyEvent, TenantCopyLink, TenantLeader, TenantLeaderEvent},
    TenantCopyEventRepo, TenantCopyRead, TenantCopyWrite,
};
use bot_core::trading_repository::repository_error::RepositoryError;
use chrono::{DateTime, Utc};

use trading_isolation_common::{at, org, read_scope, run_id, setup, write_scope};

fn leader(tenant: OrganizationId, address: &str, label: &str, events_seen: i64) -> TenantLeader {
    let now = Utc::now();
    TenantLeader {
        organization_id: tenant,
        address: address.into(),
        label: label.into(),
        status: "active".into(),
        source: "config".into(),
        followed_at: now,
        status_since: now,
        events_seen,
        mirrored: 0,
        rejected: 0,
        last_event_at: None,
        last_slot: None,
        updated_at: now,
    }
}

fn copy_event(
    tenant: OrganizationId,
    event_id: &str,
    stage: &str,
    source_sequence: i64,
    observed_at: DateTime<Utc>,
) -> TenantCopyEvent {
    TenantCopyEvent {
        organization_id: tenant,
        event_id: event_id.into(),
        leader: "leader-x".into(),
        signature: format!("sig-{event_id}"),
        slot: 100,
        mint: "MINT".into(),
        side: "buy".into(),
        venue: "raydium".into(),
        token_amount: 10.0,
        sol_amount: 1.0,
        source: "poller".into(),
        source_sequence,
        event_at: None,
        observed_at,
        stage: stage.into(),
        reject_reason: None,
        detail: None,
        intent_id: None,
        position_id: None,
        created_at: observed_at,
        updated_at: observed_at,
    }
}

#[tokio::test]
async fn same_leader_address_is_independent_per_tenant() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: copy_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let shared = format!("leader-{run}");
    let a_only = format!("leader-a-only-{run}");
    let writes = TenantCopyWrite::new(db.clone());
    let reads = TenantCopyRead::new(db.clone());

    // Both tenants follow the SAME external address — 0029 composite
    // arbiter (organization_id, address) keeps the rows independent.
    writes
        .upsert_leader(&write_scope(a), &leader(a, &shared, "A's label", 7))
        .await
        .unwrap();
    writes
        .upsert_leader(&write_scope(b), &leader(b, &shared, "B's label", 99))
        .await
        .unwrap();
    // A also follows a leader B never configured.
    writes
        .upsert_leader(&write_scope(a), &leader(a, &a_only, "A only", 1))
        .await
        .unwrap();

    // Each tenant reads only its own configuration of the address.
    let a_row = reads
        .leader(&read_scope(a), &shared)
        .await
        .unwrap()
        .expect("A row");
    assert_eq!(a_row.organization_id, a);
    assert_eq!(a_row.label, "A's label");
    assert_eq!(a_row.events_seen, 7);
    let b_row = reads
        .leader(&read_scope(b), &shared)
        .await
        .unwrap()
        .expect("B row");
    assert_eq!(b_row.organization_id, b);
    assert_eq!(b_row.label, "B's label");
    assert_eq!(b_row.events_seen, 99);

    // B re-upserting the shared address must not touch A's row.
    writes
        .upsert_leader(&write_scope(b), &leader(b, &shared, "B relabel", 100))
        .await
        .unwrap();
    let a_row = reads
        .leader(&read_scope(a), &shared)
        .await
        .unwrap()
        .expect("A row");
    assert_eq!(a_row.label, "A's label", "A's leader config untouched by B");
    assert_eq!(a_row.events_seen, 7);

    // Leader lists stay scoped.
    let a_leaders = reads.leaders(&read_scope(a)).await.unwrap();
    assert!(a_leaders.iter().all(|l| l.organization_id == a));
    assert!(a_leaders.iter().any(|l| l.address == a_only));
    let b_leaders = reads.leaders(&read_scope(b)).await.unwrap();
    assert!(b_leaders.iter().all(|l| l.organization_id == b));
    assert!(
        !b_leaders.iter().any(|l| l.address == a_only),
        "A-only leader invisible to B"
    );

    // Lifecycle events append ONLY against the tenant's own leader row.
    let evt = |tenant: OrganizationId, address: &str| TenantLeaderEvent {
        organization_id: tenant,
        id: 0,
        address: address.into(),
        event: "followed".into(),
        reason: Some("test".into()),
        replica_id: format!("replica-{run}"),
        ts: Utc::now(),
    };
    writes
        .append_leader_event(&write_scope(a), &evt(a, &shared))
        .await
        .unwrap();
    // B appending for a leader it never configured (A-only) → NotFound.
    match writes
        .append_leader_event(&write_scope(b), &evt(b, &a_only))
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not append events for A-only leader: {other:?}"),
    }
    // History is scoped: A's events for the shared address are A's own.
    let a_events = reads
        .leader_events(&read_scope(a), &shared, 100)
        .await
        .unwrap();
    assert!(a_events.iter().all(|e| e.organization_id == a));
    assert_eq!(a_events.len(), 1);
    let b_events = reads
        .leader_events(&read_scope(b), &shared, 100)
        .await
        .unwrap();
    assert!(b_events.is_empty(), "B has no events yet for its own row");
}

#[tokio::test]
async fn copy_event_log_is_tenant_local_same_event_id() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: copy_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let events = TenantCopyEventRepo::new(db.clone());
    let reads = TenantCopyRead::new(db.clone());
    let ev_id = format!("ev-{run}");

    // Both tenants observe the SAME leader trade (same event id and
    // signature) but reach DIFFERENT outcomes — two independent rows.
    let fresh_a = events
        .record(
            &write_scope(a),
            &copy_event(a, &ev_id, "mirrored", 10, at(0.05)),
        )
        .await
        .unwrap();
    let fresh_b = events
        .record(
            &write_scope(b),
            &copy_event(b, &ev_id, "rejected", 20, at(0.05)),
        )
        .await
        .unwrap();
    assert!(fresh_a, "first observation for A");
    assert!(fresh_b, "first observation for B — independent row");

    // A advances its own stage; B's row must not move.
    let advanced = events
        .record(
            &write_scope(a),
            &copy_event(a, &ev_id, "terminal", 10, at(0.05)),
        )
        .await
        .unwrap();
    assert!(!advanced, "second observation advances A's own row");
    let a_ev = events
        .event(&read_scope(a), &ev_id)
        .await
        .unwrap()
        .expect("A row");
    assert_eq!(a_ev.stage, "terminal");
    assert_eq!(a_ev.source_sequence, 10);
    let b_ev = events
        .event(&read_scope(b), &ev_id)
        .await
        .unwrap()
        .expect("B row");
    assert_eq!(b_ev.stage, "rejected", "no cross-tenant stage overwrite");
    assert_eq!(b_ev.source_sequence, 20);

    // Dedup + sequence watermark are tenant-local.
    assert!(events.seen(&read_scope(a), &ev_id).await.unwrap());
    assert!(events.seen(&read_scope(b), &ev_id).await.unwrap());
    let a_only_ev = format!("ev-a-only-{run}");
    events
        .record(
            &write_scope(a),
            &copy_event(a, &a_only_ev, "terminal", 11, at(0.05)),
        )
        .await
        .unwrap();
    assert!(!events.seen(&read_scope(b), &a_only_ev).await.unwrap());
    let a_seq = events
        .last_source_sequence(&read_scope(a), "leader-x")
        .await
        .unwrap();
    let b_seq = events
        .last_source_sequence(&read_scope(b), "leader-x")
        .await
        .unwrap();
    assert_eq!(a_seq, Some(11));
    assert_eq!(b_seq, Some(20));
    // A leader B never observed → None (no watermark leak).
    assert_eq!(
        events
            .last_source_sequence(&read_scope(b), "leader-never-b")
            .await
            .unwrap(),
        None
    );

    // events_since / events_for_leader stay scoped.
    let since_a = reads
        .events_since(&read_scope(a), at(1.0), 1000)
        .await
        .unwrap();
    assert!(since_a.iter().all(|e| e.organization_id == a));
    assert!(since_a.iter().all(|e| e.source_sequence != 20));
    let for_a = reads
        .events_for_leader(&read_scope(a), "leader-x", 100)
        .await
        .unwrap();
    assert!(for_a.iter().all(|e| e.organization_id == a));

    // Retention prunes only the acting tenant's terminal rows: both
    // tenants hold a terminal `evp-{run}` observed 10 days ago; A's
    // prune must delete A's row and leave B's untouched.
    let evp = format!("evp-{run}");
    events
        .record(
            &write_scope(a),
            &copy_event(a, &evp, "terminal", 12, at(10.0)),
        )
        .await
        .unwrap();
    events
        .record(
            &write_scope(b),
            &copy_event(b, &evp, "terminal", 22, at(10.0)),
        )
        .await
        .unwrap();
    let pruned = events
        .prune_older_than(&write_scope(a), at(1.0))
        .await
        .unwrap();
    assert!(pruned >= 1, "A's own terminal rows pruned");
    assert!(
        !events.seen(&read_scope(a), &evp).await.unwrap(),
        "A's evp gone"
    );
    assert!(
        events.seen(&read_scope(b), &evp).await.unwrap(),
        "B's evp survives A's prune"
    );
}

#[tokio::test]
async fn copy_links_are_tenant_local_same_position_id() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: copy_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let writes = TenantCopyWrite::new(db.clone());
    let reads = TenantCopyRead::new(db.clone());
    let leader_addr = format!("leader-{run}");
    let mint = format!("mint-{run}");

    let link = |tenant: OrganizationId, position_id: &str, qty: f64| TenantCopyLink {
        organization_id: tenant,
        position_id: position_id.into(),
        leader: leader_addr.clone(),
        mint: mint.clone(),
        entry_event_id: format!("ee-{position_id}"),
        entry_signature: format!("es-{position_id}"),
        intent_id: None,
        leader_token_amount: 10.0,
        follower_qty: qty,
        status: "open".into(),
        opened_at: Utc::now(),
        closed_at: None,
        exit_event_id: None,
        last_reconciled_at: None,
        note: None,
        updated_at: Utc::now(),
    };

    // Same position id text, both tenants — composite arbiter
    // (organization_id, position_id) keeps them independent.
    let shared_pos = format!("pos-{run}");
    writes
        .upsert_link(&write_scope(a), &link(a, &shared_pos, 5.0))
        .await
        .unwrap();
    writes
        .upsert_link(&write_scope(b), &link(b, &shared_pos, 50.0))
        .await
        .unwrap();

    let a_link = reads
        .link(&read_scope(a), &shared_pos)
        .await
        .unwrap()
        .expect("A link");
    assert_eq!(a_link.organization_id, a);
    assert!((a_link.follower_qty - 5.0).abs() < 1e-9);
    let b_link = reads
        .link(&read_scope(b), &shared_pos)
        .await
        .unwrap()
        .expect("B link");
    assert_eq!(b_link.organization_id, b);
    assert!(
        (b_link.follower_qty - 50.0).abs() < 1e-9,
        "B's row is a different row, not an overwrite"
    );

    // A-only link: B's reconciler mark must fail NotFound.
    let a_only_pos = format!("pos-a-only-{run}");
    writes
        .upsert_link(&write_scope(a), &link(a, &a_only_pos, 7.0))
        .await
        .unwrap();
    match writes
        .mark_link_reconciled(&write_scope(b), &a_only_pos, 99.0, Utc::now())
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not reconcile A's link: {other:?}"),
    }
    // A's own reconciler mark works and B's shared-id row is untouched.
    writes
        .mark_link_reconciled(&write_scope(a), &a_only_pos, 7.5, Utc::now())
        .await
        .unwrap();
    let b_link = reads
        .link(&read_scope(b), &shared_pos)
        .await
        .unwrap()
        .expect("B link");
    assert!((b_link.follower_qty - 50.0).abs() < 1e-9);

    // open_links / open_links_for are scoped: A never sees B's link to
    // the same leader+mint pair.
    let open_a = reads.open_links(&read_scope(a)).await.unwrap();
    assert!(open_a.iter().all(|l| l.organization_id == a));
    assert!(open_a
        .iter()
        .any(|l| l.position_id == shared_pos && (l.follower_qty - 5.0).abs() < 1e-9));
    let for_a = reads
        .open_links_for(&read_scope(a), &leader_addr, &mint)
        .await
        .unwrap();
    assert!(for_a.iter().all(|l| l.organization_id == a));
    assert_eq!(for_a.len(), 2, "A's two open links");
    let for_b = reads
        .open_links_for(&read_scope(b), &leader_addr, &mint)
        .await
        .unwrap();
    assert!(for_b.iter().all(|l| l.organization_id == b));
    assert_eq!(for_b.len(), 1, "B sees only its own link");
}
