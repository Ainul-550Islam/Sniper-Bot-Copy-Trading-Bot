//! PROMPT 3/10 — REPORTING cross-tenant isolation (real PostgreSQL):
//! every dashboard aggregate (order counts, position book, execution
//! stats, realized PnL, balance totals, the composed summary) is
//! computed tenant-scoped IN SQL — tenant A's report never contains a
//! single row, count or cent of tenant B.

mod trading_isolation_common;

use bot_core::trading_repository::executions::TenantExecutionWrite;
use bot_core::trading_repository::positions::{
    model::TenantPosition, TenantBalanceRepo, TenantPositionWrite,
};
use bot_core::trading_repository::reporting::TenantReportingRepo;

use trading_isolation_common::{at, org, read_scope, run_id, seed_order, setup, write_scope};

fn position(tenant: bot_core::tenant::OrganizationId, id: String, status: &str) -> TenantPosition {
    let now = chrono::Utc::now();
    let (status, closed_at, realized) = match status {
        "closed" => ("closed", Some(now), 150.0),
        _ => ("open", None, 0.0),
    };
    TenantPosition {
        organization_id: tenant,
        id,
        source: "sniper".into(),
        venue: "paper".into(),
        mode: "paper".into(),
        status: status.into(),
        symbol: "SOL/USDC".into(),
        symbol_display: "SOL/USDC".into(),
        quote_symbol: "USDC".into(),
        qty: 1.0,
        avg_entry: 100.0,
        cost_basis: 100.0,
        realized_quote: realized,
        last_mark: 100.0,
        stop_loss: None,
        take_profit: None,
        trailing_stop: None,
        trailing_high_water: None,
        max_hold_secs: None,
        entry_signature: None,
        exit_signature: None,
        entry_latency_ms: None,
        copied_wallet: None,
        market_id: None,
        outcome: None,
        reason_closed: None,
        opened_at: now,
        updated_at: now,
        closed_at,
    }
}

#[tokio::test]
async fn dashboard_aggregates_exclude_the_other_tenant() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: reporting_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let reporting = TenantReportingRepo::new(db.clone());

    // ── Arrange: two tenants with DIFFERENT business volumes. ──
    // A: 3 orders (2 filled, 1 failed), 1 open + 1 closed position,
    //    2 executions (1 ok, 1 failed), 1 submitted transaction.
    // B: 1 cancelled order, nothing else.
    for (i, status) in ["filled", "filled", "failed"].iter().enumerate() {
        seed_order(&db, a, &format!("ord-a{i}-{run}"), None, status).await;
    }
    seed_order(&db, b, &format!("ord-b0-{run}"), None, "cancelled").await;

    let pos_write = TenantPositionWrite::new(db.clone());
    pos_write
        .upsert(
            &write_scope(a),
            &position(a, format!("p-open-{run}"), "open"),
        )
        .await
        .unwrap();
    pos_write
        .upsert(
            &write_scope(a),
            &position(a, format!("p-closed-{run}"), "closed"),
        )
        .await
        .unwrap();

    let exec_write = TenantExecutionWrite::new(db.clone());
    let order_for_exec = format!("ord-a0-{run}");
    exec_write
        .append(
            &write_scope(a),
            &order_for_exec,
            "send",
            Some("https://rpc"),
            Some(12),
            true,
            Some("ok"),
            at(0.0),
        )
        .await
        .unwrap();
    exec_write
        .append(
            &write_scope(a),
            &order_for_exec,
            "send",
            Some("https://rpc"),
            Some(13),
            false,
            Some("slippage"),
            at(0.0),
        )
        .await
        .unwrap();
    exec_write
        .record_transaction_submitted(
            &write_scope(a),
            "solana",
            &format!("sig-rep-{run}"),
            Some(&order_for_exec),
            None,
            None,
            1,
        )
        .await
        .unwrap();

    let balances = TenantBalanceRepo::new(db.clone());
    balances
        .record(
            &write_scope(a),
            "solana",
            "wallet-a",
            "USDC",
            2500.0,
            Some(2500.0),
            "test",
            at(0.1),
        )
        .await
        .unwrap();
    balances
        .record(
            &write_scope(b),
            "solana",
            "wallet-b",
            "USDC",
            100.0,
            Some(100.0),
            "test",
            at(0.1),
        )
        .await
        .unwrap();

    // ── Assert: each section report answers for its tenant only. ──
    let counts_a = reporting
        .orders()
        .counts_by_status(&read_scope(a))
        .await
        .unwrap();
    assert!(counts_a.iter().all(|c| c.organization_id == a));
    assert_eq!(
        counts_a
            .iter()
            .find(|c| c.status == "filled")
            .map(|c| c.count),
        Some(2)
    );
    let counts_b = reporting
        .orders()
        .counts_by_status(&read_scope(b))
        .await
        .unwrap();
    assert!(counts_b.iter().all(|c| c.organization_id == b));
    assert_eq!(
        counts_b
            .iter()
            .find(|c| c.status == "cancelled")
            .map(|c| c.count),
        Some(1)
    );
    assert!(
        counts_b.iter().all(|c| c.status != "filled"),
        "B never sees A's filled orders"
    );

    let ord_sum_a = reporting.orders().summary(&read_scope(a)).await.unwrap();
    assert_eq!(ord_sum_a.total, 3);
    assert_eq!(ord_sum_a.filled, 2);
    assert_eq!(ord_sum_a.failed, 1);
    assert_eq!(ord_sum_a.cancelled, 0);
    let ord_sum_b = reporting.orders().summary(&read_scope(b)).await.unwrap();
    assert_eq!(ord_sum_b.total, 1);
    assert_eq!(ord_sum_b.cancelled, 1);
    assert_eq!(ord_sum_b.filled, 0);

    let pos_sum_a = reporting.positions().summary(&read_scope(a)).await.unwrap();
    assert_eq!(pos_sum_a.organization_id, a);
    assert_eq!(pos_sum_a.open_count, 1);
    assert_eq!(pos_sum_a.closed_count, 1);
    assert!((pos_sum_a.realized_quote_total - 150.0).abs() < 1e-6);
    let pos_sum_b = reporting.positions().summary(&read_scope(b)).await.unwrap();
    assert_eq!(pos_sum_b.open_count, 0);
    assert_eq!(pos_sum_b.closed_count, 0);

    let exec_a = reporting
        .executions()
        .stats_between(&read_scope(a), at(1.0), at(-1.0))
        .await
        .unwrap();
    assert_eq!(exec_a.organization_id, a);
    assert_eq!(exec_a.attempts, 2);
    assert_eq!(exec_a.succeeded, 1);
    assert_eq!(exec_a.failed, 1);
    assert_eq!(exec_a.submitted_transactions, 1);
    assert_eq!(exec_a.in_flight_transactions, 1);
    let exec_b = reporting
        .executions()
        .stats_between(&read_scope(b), at(1.0), at(-1.0))
        .await
        .unwrap();
    assert_eq!(
        exec_b.attempts, 0,
        "B's execution stats exclude A's attempts"
    );
    assert_eq!(exec_b.submitted_transactions, 0);

    // Realized PnL: A's closed position is A's alone.
    let pnl_a = reporting
        .pnl()
        .realized_between(&read_scope(a), at(2.0), at(-1.0))
        .await
        .unwrap();
    assert_eq!(pnl_a.organization_id, a);
    assert_eq!(pnl_a.positions_closed, 1);
    assert!((pnl_a.realized_net - 50.0).abs() < 1e-6);
    let pnl_b = reporting
        .pnl()
        .realized_between(&read_scope(b), at(2.0), at(-1.0))
        .await
        .unwrap();
    assert_eq!(pnl_b.positions_closed, 0);
    assert!((pnl_b.realized_net - 0.0).abs() < 1e-6);

    // Balance totals never mix.
    assert_eq!(
        balances.total_usd_latest(&read_scope(a)).await.unwrap(),
        Some(2500.0)
    );
    assert_eq!(
        balances.total_usd_latest(&read_scope(b)).await.unwrap(),
        Some(100.0)
    );

    // ── The composed dashboard summary stays scoped end-to-end. ──
    let summary_a = reporting.summary(&read_scope(a)).await.unwrap();
    assert_eq!(summary_a.organization_id, a);
    assert_eq!(summary_a.orders.total, 3);
    assert_eq!(summary_a.positions.open_count, 1);
    assert_eq!(summary_a.executions.attempts, 2);
    assert_eq!(summary_a.realized_pnl.positions_closed, 1);
    let summary_b = reporting.summary(&read_scope(b)).await.unwrap();
    assert_eq!(summary_b.organization_id, b);
    assert_eq!(summary_b.orders.total, 1);
    assert_eq!(summary_b.orders.cancelled, 1);
    assert_eq!(summary_b.positions.open_count, 0);
    assert_eq!(summary_b.executions.attempts, 0);
    assert_eq!(summary_b.realized_pnl.positions_closed, 0);
}
