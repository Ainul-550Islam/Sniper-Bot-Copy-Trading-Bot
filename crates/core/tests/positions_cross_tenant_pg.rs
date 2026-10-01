//! PROMPT 3/10 — POSITIONS / TRADES / BALANCES cross-tenant isolation
//! (real PostgreSQL): reads, writes, lists and SQL aggregates must all
//! stay inside the acting tenant (§D23–D28).
//!
//! Gated like `db_integration.rs` (NOT_RUN without POSTGRES_URL);
//! run with `--test-threads=1`.

mod trading_isolation_common;

use bot_core::trading_repository::positions::{
    model::TenantPosition, TenantBalanceRepo, TenantPositionRead, TenantPositionWrite,
    TenantTradeRepo,
};
use bot_core::trading_repository::reporting::TenantReportingRepo;
use bot_core::trading_repository::repository_error::RepositoryError;

use trading_isolation_common::{at, org, read_scope, run_id, setup, write_scope};

fn position(
    tenant: bot_core::tenant::OrganizationId,
    id: String,
    opened_at: chrono::DateTime<chrono::Utc>,
) -> TenantPosition {
    TenantPosition {
        organization_id: tenant,
        id,
        source: "sniper".into(),
        venue: "paper".into(),
        mode: "paper".into(),
        status: "open".into(),
        symbol: "SOL/USDC".into(),
        symbol_display: "SOL/USDC".into(),
        quote_symbol: "USDC".into(),
        qty: 1.0,
        avg_entry: 100.0,
        cost_basis: 100.0,
        realized_quote: 0.0,
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
        opened_at,
        updated_at: opened_at,
        closed_at: None,
    }
}

#[tokio::test]
async fn positions_trades_balances_isolation_and_pnl_exclusion() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: positions_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let pos_write = TenantPositionWrite::new(db.clone());
    let pos_read = TenantPositionRead::new(db.clone());
    let trades = TenantTradeRepo::new(db.clone());
    let balances = TenantBalanceRepo::new(db.clone());
    let reporting = TenantReportingRepo::new(db.clone());

    // Per tenant: one position that closes at a known PnL, one that
    // stays open, one trade, one balance snapshot.
    for (tenant, label, realized) in [(a, "a", 150.0_f64), (b, "b", -40.0_f64)] {
        let closing_id = format!("pos-{label}-{run}");
        let open_id = format!("pos2-{label}-{run}");
        pos_write
            .upsert(
                &write_scope(tenant),
                &position(tenant, closing_id.clone(), at(1.0)),
            )
            .await
            .unwrap();
        pos_write
            .upsert(&write_scope(tenant), &position(tenant, open_id, at(0.5)))
            .await
            .unwrap();
        // close at realized_quote = 100 + realized ⇒ net = realized.
        pos_write
            .close(
                &write_scope(tenant),
                &closing_id,
                Some(&format!("exit-{label}-{run}")),
                Some("target"),
                100.0 + realized,
                100.0,
                at(0.25),
            )
            .await
            .unwrap();
        let trade = bot_core::trading_repository::positions::model::TenantTrade {
            organization_id: tenant,
            id: format!("trade-{label}-{run}"),
            ts: at(0.25),
            source: "sniper".into(),
            venue: "paper".into(),
            mode: "paper".into(),
            side: "long".into(),
            symbol: "SOL/USDC".into(),
            symbol_display: "SOL/USDC".into(),
            amount_in: 100.0,
            amount_out: 100.0 + realized,
            quote_symbol: "USDC".into(),
            price: 100.0 + realized,
            fee: 1.0,
            slippage_bps: 5,
            signature: None,
            position_id: Some(closing_id),
            note: None,
            latency_ms: None,
        };
        trades.append(&write_scope(tenant), &trade).await.unwrap();
        balances
            .record(
                &write_scope(tenant),
                "solana",
                &format!("wallet-{label}"),
                "USDC",
                1000.0,
                Some(1000.0),
                "test",
                at(0.1),
            )
            .await
            .unwrap();
    }

    // Reads: A sees A, B's read of A's id is NotFound (no leak).
    let a_pos = pos_read
        .get(&read_scope(a), &format!("pos-a-{run}"))
        .await
        .expect("A reads own position");
    assert_eq!(a_pos.organization_id, a);
    assert_eq!(a_pos.status, "closed");
    for (scope, id) in [
        (read_scope(b), format!("pos-a-{run}")),
        (read_scope(a), format!("pos-b-{run}")),
    ] {
        match pos_read.get(&scope, &id).await {
            Err(RepositoryError::NotFound(_)) => {}
            other => panic!("cross-tenant position read must be NotFound: {other:?}"),
        }
    }

    // Lists stay tenant-scoped.
    let open_a = pos_read.list_open(&read_scope(a)).await.unwrap();
    assert!(open_a.iter().all(|p| p.organization_id == a));
    assert!(open_a.iter().any(|p| p.id == format!("pos2-a-{run}")));
    assert!(!open_a.iter().any(|p| p.id == format!("pos2-b-{run}")));
    let closed_b = pos_read
        .list_closed_between(&read_scope(b), at(2.0), at(-1.0))
        .await
        .unwrap();
    assert!(closed_b.iter().all(|p| p.organization_id == b));
    assert_eq!(closed_b.len(), 1);

    // B cannot close or risk-mark A's position (both NotFound).
    match pos_write
        .close(
            &write_scope(b),
            &format!("pos2-a-{run}"),
            None,
            None,
            0.0,
            0.0,
            at(0.0),
        )
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not close A's position: {other:?}"),
    }
    match pos_write
        .update_risk_marks(
            &write_scope(b),
            &format!("pos2-a-{run}"),
            Some(1.0),
            None,
            None,
            None,
            Some(2.0),
            at(0.0),
        )
        .await
    {
        Err(RepositoryError::NotFound(_)) => {}
        other => panic!("B must not mark A's position: {other:?}"),
    }
    // A's position survived both attack writes.
    let a_open = pos_read
        .get(&read_scope(a), &format!("pos2-a-{run}"))
        .await
        .unwrap();
    assert_eq!(a_open.status, "open");
    assert_eq!(a_open.stop_loss, None);

    // Trades: A's history for A's position; B sees the empty set for
    // A's position id (no existence leak either).
    let a_trades = trades
        .list_for_position(&read_scope(a), &format!("pos-a-{run}"))
        .await
        .unwrap();
    assert_eq!(a_trades.len(), 1);
    assert_eq!(a_trades[0].organization_id, a);
    assert!(trades
        .list_for_position(&read_scope(b), &format!("pos-a-{run}"))
        .await
        .unwrap()
        .is_empty());

    // PnL aggregate excludes the other tenant (SQL-side scoping).
    let pnl_a = reporting
        .pnl()
        .realized_between(&read_scope(a), at(2.0), at(-1.0))
        .await
        .unwrap();
    let pnl_b = reporting
        .pnl()
        .realized_between(&read_scope(b), at(2.0), at(-1.0))
        .await
        .unwrap();
    assert!(
        (pnl_a.realized_net - 150.0).abs() < 1e-6,
        "A net: {}",
        pnl_a.realized_net
    );
    assert!(
        (pnl_b.realized_net - (-40.0)).abs() < 1e-6,
        "B net: {}",
        pnl_b.realized_net
    );
    assert_eq!(pnl_a.positions_closed, 1);
    assert_eq!(pnl_b.positions_closed, 1);
    assert!((pnl_a.fees_paid - 1.0).abs() < 1e-6);
    assert!((pnl_b.fees_paid - 1.0).abs() < 1e-6);

    // Balance aggregates exclude the other tenant.
    let total_a = balances.total_usd_latest(&read_scope(a)).await.unwrap();
    let total_b = balances.total_usd_latest(&read_scope(b)).await.unwrap();
    assert!((total_a.unwrap_or(0.0) - 1000.0).abs() < 1e-6);
    assert!((total_b.unwrap_or(0.0) - 1000.0).abs() < 1e-6);
    let latest_a = balances.latest_per_asset(&read_scope(a)).await.unwrap();
    assert!(latest_a.iter().all(|s| s.organization_id == a));
    assert_eq!(latest_a.len(), 1);
    assert_eq!(latest_a[0].address, "wallet-a");
    assert!(balances
        .history_for_address(&read_scope(b), "wallet-a", at(1.0), at(-1.0))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn cross_tenant_position_id_collision_cannot_mutate() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: positions_cross_tenant_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("a-{run}")).await;
    let b = org(&db, &format!("b-{run}")).await;
    let pos_write = TenantPositionWrite::new(db.clone());
    let pos_read = TenantPositionRead::new(db.clone());
    let id = format!("collide-{run}");

    // A seeds a position directly (attack tests exercise the
    // repositories; the seed only arranges the battlefield).
    sqlx::query(
        r#"INSERT INTO positions (organization_id, id, source, venue, mode, status,
                                  symbol, symbol_display, quote_symbol, qty, avg_entry,
                                  cost_basis, realized_quote, last_mark, opened_at, updated_at)
           VALUES ($1, $2, 'sniper', 'paper', 'paper', 'open', 'X/Y', 'X/Y', 'Y', 1, 10, 10, 0, 10, now(), now())"#,
    )
    .bind(a.as_uuid())
    .bind(&id)
    .execute(db.pool())
    .await
    .expect("seed A position");

    // B cannot read it.
    assert!(matches!(
        pos_read.get(&read_scope(b), &id).await,
        Err(RepositoryError::NotFound(_))
    ));

    // B upserts the SAME global id. `positions.id` is the GLOBAL PK;
    // the ON CONFLICT leg is tenant-guarded (`WHERE
    // positions.organization_id = $1`), so the conflict fires but the
    // guarded update matches zero of B's rows: a silent no-op that
    // must NOT mutate A's row and must NOT create a B row.
    let mut b_pos = position(b, id.clone(), at(0.1));
    b_pos.qty = 2.0;
    b_pos.cost_basis = 20.0;
    pos_write
        .upsert(&write_scope(b), &b_pos)
        .await
        .expect("tenant-guarded upsert no-ops instead of failing");

    let a_row = pos_read
        .get(&read_scope(a), &id)
        .await
        .expect("A still reads own position");
    assert_eq!(a_row.organization_id, a);
    assert!(
        (a_row.qty - 1.0).abs() < 1e-9,
        "A's position must be untouched"
    );
    assert!((a_row.cost_basis - 10.0).abs() < 1e-9);
    // B still has no row with that id.
    assert!(matches!(
        pos_read.get(&read_scope(b), &id).await,
        Err(RepositoryError::NotFound(_))
    ));
}
