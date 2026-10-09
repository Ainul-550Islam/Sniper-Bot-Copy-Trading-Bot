//! PROMPT 3/10 — POLYMARKET cross-tenant isolation (real PostgreSQL):
//! signals, mirror orders and fills are tenant-local on the 0030
//! composite arbiters — the SAME signal id / venue order id / fill id
//! text exists as independent rows per tenant; reconciliation drift
//! and findings never cross tenants.

mod trading_isolation_common;

use bot_core::tenant::OrganizationId;
use bot_core::trading_repository::pagination::TenantPageRequest;
use bot_core::trading_repository::polymarket::{
    model::{TenantPolyFill, TenantPolyOrder, TenantPolySignal},
    TenantPolyRead, TenantPolyReconRepo, TenantPolyWrite,
};
use bot_core::trading_repository::repository_error::RepositoryError;
use chrono::{DateTime, Utc};

use trading_isolation_common::{at, org, read_scope, run_id, setup, write_scope};

fn signal(tenant: OrganizationId, signal_id: &str, stage: &str) -> TenantPolySignal {
    let now = Utc::now();
    TenantPolySignal {
        organization_id: tenant,
        signal_id: signal_id.into(),
        condition_id: "0xcond".into(),
        token_id: "tok-yes".into(),
        outcome: "YES".into(),
        side: "buy".into(),
        strategy: "momentum".into(),
        limit_price: 0.55,
        size_tokens: 100.0,
        stake_usd: 55.0,
        mode: "paper".into(),
        stage: stage.into(),
        reject_reason: None,
        detail: "test".into(),
        order_id: None,
        venue_order_id: None,
        position_id: None,
        created_at: now,
        updated_at: now,
    }
}

fn order(
    tenant: OrganizationId,
    venue_order_id: &str,
    size_matched: f64,
    updated_at: DateTime<Utc>,
) -> TenantPolyOrder {
    TenantPolyOrder {
        organization_id: tenant,
        venue_order_id: venue_order_id.into(),
        order_id: format!("o-{venue_order_id}"),
        signal_id: format!("s-{venue_order_id}"),
        condition_id: "0xcond".into(),
        token_id: "tok-yes".into(),
        outcome: "YES".into(),
        side: "buy".into(),
        order_type: "limit".into(),
        limit_price: 0.55,
        size_tokens: 100.0,
        size_matched,
        mode: "paper".into(),
        state: "live".into(),
        venue_status: "open".into(),
        expiration: 0,
        position_id: None,
        replica_id: "replica-t".into(),
        submitted_at: updated_at,
        updated_at,
        closed_at: None,
    }
}

fn fill(tenant: OrganizationId, fill_id: &str, venue_order_id: &str, size: f64) -> TenantPolyFill {
    TenantPolyFill {
        organization_id: tenant,
        fill_id: fill_id.into(),
        venue_order_id: venue_order_id.into(),
        order_id: format!("o-{venue_order_id}"),
        token_id: "tok-yes".into(),
        side: "buy".into(),
        price: 0.55,
        size_tokens: size,
        quote_usd: size * 0.55,
        source: "venue-ws".into(),
        position_id: None,
        ts: Utc::now(),
    }
}

#[tokio::test]
async fn signals_orders_fills_same_ids_are_independent_rows() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: polymarket_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let writes = TenantPolyWrite::new(db.clone());
    let reads = TenantPolyRead::new(db.clone());

    // ── Signals: same signal_id text, independent stage machines. ──
    let sig_id = format!("sig-{run}");
    let mut a_sig = signal(a, &sig_id, "ordered");
    a_sig.order_id = Some(format!("oa-{run}"));
    writes.record_signal(&write_scope(a), &a_sig).await.unwrap();
    let mut b_sig = signal(b, &sig_id, "rejected");
    b_sig.reject_reason = Some("sizing".into());
    writes.record_signal(&write_scope(b), &b_sig).await.unwrap();

    let mut a_sig2 = signal(a, &sig_id, "filled");
    a_sig2.order_id = Some(format!("oa-{run}"));
    a_sig2.position_id = Some(format!("pa-{run}"));
    writes
        .record_signal(&write_scope(a), &a_sig2)
        .await
        .unwrap();

    let a_row = reads
        .signal(&read_scope(a), &sig_id)
        .await
        .unwrap()
        .expect("A signal");
    assert_eq!(a_row.stage, "filled");
    assert_eq!(a_row.organization_id, a);
    let b_row = reads
        .signal(&read_scope(b), &sig_id)
        .await
        .unwrap()
        .expect("B signal");
    assert_eq!(
        b_row.stage, "rejected",
        "A's advance must not touch B's row"
    );
    assert_eq!(b_row.position_id, None);
    let since_a = reads
        .signals_since(&read_scope(a), at(1.0), 100)
        .await
        .unwrap();
    assert!(since_a.iter().all(|s| s.organization_id == a));
    assert_eq!(since_a.len(), 1);

    // ── Mirror orders: same venue order id, two independent books. ──
    let vo = format!("vo-{run}");
    writes
        .upsert_order(&write_scope(a), &order(a, &vo, 5.0, at(0.02)))
        .await
        .unwrap();
    writes
        .upsert_order(&write_scope(b), &order(b, &vo, 6.0, at(0.02)))
        .await
        .unwrap();
    // B's re-upsert with a LOWER size_matched must not move either book.
    writes
        .upsert_order(&write_scope(b), &order(b, &vo, 2.0, at(0.01)))
        .await
        .unwrap();
    let a_ord = reads
        .order(&read_scope(a), &vo)
        .await
        .unwrap()
        .expect("A order");
    assert_eq!(a_ord.organization_id, a);
    assert!(
        (a_ord.size_matched - 5.0).abs() < 1e-9,
        "A's book untouched by B"
    );
    let b_ord = reads
        .order(&read_scope(b), &vo)
        .await
        .unwrap()
        .expect("B order");
    assert!(
        (b_ord.size_matched - 6.0).abs() < 1e-9,
        "GREATEST keeps B's monotonic fill"
    );
    assert!(b_ord.closed_at.is_none());

    // open_orders stays scoped.
    let open_a = reads.open_orders(&read_scope(a)).await.unwrap();
    assert!(open_a.iter().all(|o| o.organization_id == a));
    assert!(open_a
        .iter()
        .any(|o| o.venue_order_id == vo && (o.size_matched - 5.0).abs() < 1e-9));

    // B closes ITS mirror of vo; A's stays open.
    writes
        .close_order(&write_scope(b), &vo, "filled", Some("matched"), Utc::now())
        .await
        .unwrap();
    match writes
        .close_order(&write_scope(b), &vo, "filled", None, Utc::now())
        .await
    {
        Err(RepositoryError::StaleWrite(_)) => {}
        other => panic!("B re-closing own closed order must be StaleWrite: {other:?}"),
    }
    let a_ord = reads
        .order(&read_scope(a), &vo)
        .await
        .unwrap()
        .expect("A order");
    assert!(
        a_ord.closed_at.is_none(),
        "B's close must not terminalize A's mirror"
    );
    // B cannot close an A-only venue order id.
    let vo_a_only = format!("vo-a-only-{run}");
    writes
        .upsert_order(&write_scope(a), &order(a, &vo_a_only, 0.0, at(0.02)))
        .await
        .unwrap();
    match writes
        .close_order(&write_scope(b), &vo_a_only, "cancelled", None, Utc::now())
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not close A-only order: {other:?}"),
    }

    // Keyset pagination: A's page carries only A's rows, and a cursor
    // minted by A is rejected for B (fail closed).
    let page_req = TenantPageRequest::new(a, Some(1), None).expect("page A");
    let listed = reads.orders_page(&read_scope(a), &page_req).await.unwrap();
    assert!(listed.items.iter().all(|o| o.organization_id == a));
    let cursor = listed.next.expect("cursor minted");
    assert!(
        TenantPageRequest::new(b, Some(10), Some(&cursor.encode())).is_err(),
        "cross-tenant cursor must be rejected"
    );

    // ── Fills: same fill_id text, insert-once per tenant. ──
    let fill_id = format!("fill-{run}");
    writes
        .record_fill(&write_scope(a), &fill(a, &fill_id, &vo, 5.0))
        .await
        .unwrap();
    writes
        .record_fill(&write_scope(b), &fill(b, &fill_id, &vo, 6.0))
        .await
        .unwrap();
    // Replay with a DIFFERENT size: idempotent per tenant (DO NOTHING).
    writes
        .record_fill(&write_scope(a), &fill(a, &fill_id, &vo, 99.0))
        .await
        .unwrap();
    let a_fill = reads
        .fill(&read_scope(a), &fill_id)
        .await
        .unwrap()
        .expect("A fill");
    assert!(
        (a_fill.size_tokens - 5.0).abs() < 1e-9,
        "A's fill replay is a no-op"
    );
    assert_eq!(a_fill.organization_id, a);
    let b_fill = reads
        .fill(&read_scope(b), &fill_id)
        .await
        .unwrap()
        .expect("B fill");
    assert!((b_fill.size_tokens - 6.0).abs() < 1e-9);
    // Fill lists per venue order stay scoped.
    let a_fills = reads.fills_for_order(&read_scope(a), &vo).await.unwrap();
    assert_eq!(a_fills.len(), 1);
    assert_eq!(a_fills[0].organization_id, a);
    let b_fills = reads.fills_for_order(&read_scope(b), &vo).await.unwrap();
    assert_eq!(b_fills.len(), 1);
    assert_eq!(b_fills[0].organization_id, b);
    let window_a = reads
        .fills_between(&read_scope(a), at(1.0), at(-1.0))
        .await
        .unwrap();
    assert!(window_a.iter().all(|f| f.organization_id == a));
    assert_eq!(window_a.len(), 1);
}

#[tokio::test]
async fn reconciliation_drift_and_findings_are_tenant_scoped() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: polymarket_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let writes = TenantPolyWrite::new(db.clone());
    let recon = TenantPolyReconRepo::new(db.clone());

    // A's book: a drifted order (matched 5, no fills), a stale order
    // (updated 2 days ago), and a clean order (matched 3, fill 3).
    let voa_drift = format!("voa-drift-{run}");
    let voa_stale = format!("voa-stale-{run}");
    let voa_clean = format!("voa-clean-{run}");
    writes
        .upsert_order(&write_scope(a), &order(a, &voa_drift, 5.0, at(0.02)))
        .await
        .unwrap();
    writes
        .upsert_order(&write_scope(a), &order(a, &voa_stale, 0.0, at(2.0)))
        .await
        .unwrap();
    writes
        .upsert_order(&write_scope(a), &order(a, &voa_clean, 3.0, at(0.02)))
        .await
        .unwrap();
    writes
        .record_fill(
            &write_scope(a),
            &fill(a, &format!("fa-{run}"), &voa_clean, 3.0),
        )
        .await
        .unwrap();

    // B's book: a clean order (matched 4, fill 4) plus an ORPHAN fill
    // (booked against a venue order B never mirrored).
    let vob_clean = format!("vob-clean-{run}");
    let vob_orphan = format!("vob-orphan-{run}");
    writes
        .upsert_order(&write_scope(b), &order(b, &vob_clean, 4.0, at(0.02)))
        .await
        .unwrap();
    writes
        .record_fill(
            &write_scope(b),
            &fill(b, &format!("fb-{run}"), &vob_clean, 4.0),
        )
        .await
        .unwrap();
    writes
        .record_fill(
            &write_scope(b),
            &fill(b, &format!("fbo-{run}"), &vob_orphan, 1.0),
        )
        .await
        .unwrap();

    // Drift detection: each tenant sees only its own book.
    let drift_a = recon
        .detect_drift(&read_scope(a), chrono::Duration::hours(1), Utc::now())
        .await
        .unwrap();
    let kinds: Vec<(&str, &str)> = drift_a
        .iter()
        .map(|d| (d.venue_order_id.as_str(), d.kind.as_str()))
        .collect();
    assert!(
        kinds.contains(&(voa_drift.as_str(), "drift")),
        "A's matched-size drift: {kinds:?}"
    );
    assert!(
        kinds.contains(&(voa_stale.as_str(), "stuck_order")),
        "A's stale order: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|(vo, _)| *vo == voa_clean),
        "A's clean order must not be flagged: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|(vo, _)| vo.starts_with("vob-")),
        "B's book must be invisible to A's reconciler: {kinds:?}"
    );

    let drift_b = recon
        .detect_drift(&read_scope(b), chrono::Duration::hours(1), Utc::now())
        .await
        .unwrap();
    let kinds_b: Vec<(&str, &str)> = drift_b
        .iter()
        .map(|d| (d.venue_order_id.as_str(), d.kind.as_str()))
        .collect();
    assert!(
        kinds_b.contains(&(vob_orphan.as_str(), "orphan_fill")),
        "B's orphan fill: {kinds_b:?}"
    );
    assert!(
        !kinds_b.iter().any(|(vo, _)| vo.starts_with("voa-")),
        "A's book must be invisible to B's reconciler: {kinds_b:?}"
    );

    // Findings: recorded per tenant, read back per tenant.
    recon
        .record_finding(
            &write_scope(a),
            "drift",
            Some(&voa_drift),
            Some(&format!("oa-{run}")),
            Some("tok-yes"),
            "mirror=5 fills=0",
            "alert",
            "replica-a",
        )
        .await
        .unwrap();
    recon
        .record_finding(
            &write_scope(b),
            "orphan_fill",
            Some(&vob_orphan),
            None,
            None,
            "fill booked without an open mirror order",
            "alert",
            "replica-b",
        )
        .await
        .unwrap();
    let findings_a = recon
        .findings_between(&read_scope(a), at(1.0), at(-1.0))
        .await
        .unwrap();
    assert!(findings_a.iter().all(|f| f.organization_id == a));
    assert_eq!(findings_a.len(), 1);
    assert_eq!(findings_a[0].kind, "drift");
    let findings_b = recon
        .findings_between(&read_scope(b), at(1.0), at(-1.0))
        .await
        .unwrap();
    assert!(findings_b.iter().all(|f| f.organization_id == b));
    assert_eq!(findings_b.len(), 1);
    assert_eq!(findings_b[0].kind, "orphan_fill");

    // The reconciler's open-order input stays scoped, and B's
    // operator cancel cannot terminalize A's stuck order.
    let open_a = recon.open_orders(&read_scope(a)).await.unwrap();
    assert!(open_a.iter().all(|o| o.organization_id == a));
    assert_eq!(open_a.len(), 3);
    match recon.cancel_stuck_order(&write_scope(b), &voa_drift).await {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not cancel A's stuck order: {other:?}"),
    }
    recon
        .cancel_stuck_order(&write_scope(a), &voa_drift)
        .await
        .unwrap();
    let open_a = recon.open_orders(&read_scope(a)).await.unwrap();
    assert_eq!(open_a.len(), 2, "A's cancel terminalized only A's order");
    let open_b = recon.open_orders(&read_scope(b)).await.unwrap();
    assert_eq!(open_b.len(), 1, "B's book untouched by A's cancel");
}
