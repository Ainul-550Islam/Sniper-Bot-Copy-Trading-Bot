//! PROMPT 3/10 — POSITIONS / TRADES / BALANCES cross-tenant isolation
//! (real PostgreSQL): reads, writes, lists and SQL aggregates must all
//! stay inside the acting tenant (§D23–D28).
//!
//! Gated like `db_integration.rs` (NOT_RUN without POSTGRES_URL);
//! run with `--test-threads=1`.

mod trading_isolation_common;

use bot_core::db::claims::PostgresRiskOracle;
use bot_core::risk::GlobalRiskOracle;
use bot_core::trading_repository::polymarket::{
    model::{TenantPolyFill, TenantPolyOrder},
    write::TenantPolyWrite,
};
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
        symbol: "TOKEN/SOL".into(),
        symbol_display: "TOKEN/SOL".into(),
        quote_symbol: "SOL".into(),
        qty: 1.0,
        avg_entry: 100.0,
        cost_basis: 100.0,
        realized_quote: 0.0,
        realized_pnl_quote: Some(0.0),
        realized_at: None,
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

    // Per tenant: one position with a known same-day PnL, one that stays
    // open, one trade, and one balance snapshot. Keep the closed position
    // inside today's UTC window: the oracle intentionally rejects cumulative
    // PnL for positions opened on an earlier day.
    let utc_day_start = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("valid UTC midnight")
        .and_utc();
    let closed_opened_at = utc_day_start + chrono::Duration::minutes(1);
    let closed_at = utc_day_start + chrono::Duration::minutes(2);
    for (tenant, label, realized) in [(a, "a", 150.0_f64), (b, "b", -40.0_f64)] {
        let closing_id = format!("pos-{label}-{run}");
        let open_id = format!("pos2-{label}-{run}");
        // Seed a fully projected closed row. The close-only repository API
        // deliberately clears realized_pnl_quote because it cannot infer
        // canonical net PnL from realized proceeds alone.
        let mut closed = position(tenant, closing_id.clone(), closed_opened_at);
        closed.status = "closed".into();
        closed.realized_quote = 100.0 + realized;
        closed.realized_pnl_quote = Some(realized);
        closed.realized_at = Some(closed_at);
        closed.closed_at = Some(closed_at);
        closed.updated_at = closed_at;
        pos_write
            .upsert(&write_scope(tenant), &closed)
            .await
            .unwrap();
        pos_write
            .upsert(&write_scope(tenant), &position(tenant, open_id, at(0.5)))
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

    // The risk oracle's tenant projections use the same durable rows:
    // counts, exposure, duplicate symbols and realized PnL stay per-org.
    let oracle = PostgresRiskOracle::new(db.clone());
    assert_eq!(
        oracle
            .count_open_for(Some(a), bot_core::models::BotModule::Sniper)
            .await,
        Some(1)
    );
    assert_eq!(
        oracle
            .count_open_for(Some(b), bot_core::models::BotModule::Sniper)
            .await,
        Some(1)
    );
    assert_eq!(
        oracle
            .open_exposure_for(a, bot_core::models::BotModule::Sniper)
            .await,
        Some(100.0)
    );
    assert_eq!(
        oracle
            .has_open_symbol_for(a, bot_core::models::BotModule::Sniper, "TOKEN/SOL")
            .await,
        Some(true)
    );
    assert_eq!(
        oracle
            .realized_today_for_module(Some(a), bot_core::models::BotModule::Sniper)
            .await,
        Some(150.0)
    );
    assert_eq!(
        oracle
            .realized_today_for_module(Some(b), bot_core::models::BotModule::Sniper)
            .await,
        Some(-40.0)
    );

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
    // the tenant-guarded conflict update affects zero rows. That must be
    // surfaced as a tenant mismatch, not reported as a successful write.
    let mut b_pos = position(b, id.clone(), at(0.1));
    b_pos.qty = 2.0;
    b_pos.cost_basis = 20.0;
    assert!(matches!(
        pos_write.upsert(&write_scope(b), &b_pos).await,
        Err(RepositoryError::TenantMismatch)
    ));

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

#[tokio::test]
async fn risk_exposure_fails_closed_when_positions_use_an_unexpected_quote_unit() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: risk_quote_unit_projection_pg — POSTGRES_URL missing");
        return;
    };
    let tenant = org(&db, &format!("risk-quote-{}", run_id())).await;
    let write = write_scope(tenant);
    let mut sniper_position = position(tenant, format!("wrong-quote-{}", run_id()), at(0.0));
    sniper_position.quote_symbol = "USDC".into();
    sniper_position.cost_basis = 10.0;
    TenantPositionWrite::new(db.clone())
        .upsert(&write, &sniper_position)
        .await
        .expect("persist open position with unexpected quote unit");
    let mut poly_position = position(tenant, format!("wrong-poly-quote-{}", run_id()), at(0.0));
    poly_position.source = "polymarket".into();
    poly_position.quote_symbol = "SOL".into();
    TenantPositionWrite::new(db.clone())
        .upsert(&write, &poly_position)
        .await
        .expect("persist Polymarket position with unexpected quote unit");

    let oracle = PostgresRiskOracle::new(db);
    assert_eq!(
        oracle
            .open_exposure_for(tenant, bot_core::models::BotModule::Sniper)
            .await,
        None,
        "silently omitting a quote unit would understate tenant exposure"
    );
    assert_eq!(
        oracle
            .open_exposure_for(tenant, bot_core::models::BotModule::Polymarket)
            .await,
        None,
        "Polymarket exposure must also fail closed outside USDC"
    );
}

#[tokio::test]
async fn polymarket_risk_oracle_counts_orders_and_fails_closed_on_unprojected_fills() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: polymarket_risk_projection_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("poly-risk-{run}")).await;
    let write = write_scope(tenant);
    let writes = TenantPolyWrite::new(db.clone());
    let order_id = format!("poly-order-{run}");
    let token_id = format!("poly-token-{run}");
    let now = at(0.0);
    writes
        .upsert_order(
            &write,
            &TenantPolyOrder {
                organization_id: tenant,
                venue_order_id: order_id.clone(),
                order_id: format!("oms-{run}"),
                signal_id: format!("signal-{run}"),
                condition_id: format!("condition-{run}"),
                token_id: token_id.clone(),
                outcome: "YES".into(),
                side: "buy".into(),
                order_type: "GTC".into(),
                limit_price: 0.5,
                size_tokens: 10.0,
                size_matched: 0.0,
                mode: "paper".into(),
                state: "resting".into(),
                venue_status: "live".into(),
                expiration: 0,
                position_id: None,
                replica_id: "risk-projection-test".into(),
                submitted_at: now,
                updated_at: now,
                closed_at: None,
            },
        )
        .await
        .expect("upsert tenant order");

    let oracle = PostgresRiskOracle::new(db.clone());
    assert_eq!(
        oracle
            .count_open_for(Some(tenant), bot_core::models::BotModule::Polymarket)
            .await,
        Some(1)
    );
    assert_eq!(
        oracle
            .open_exposure_for(tenant, bot_core::models::BotModule::Polymarket)
            .await,
        Some(5.0)
    );
    assert_eq!(
        oracle
            .has_open_symbol_for(tenant, bot_core::models::BotModule::Polymarket, &token_id)
            .await,
        Some(true)
    );

    // A fill without its generic tenant-position projection makes PnL
    // incomplete; the oracle returns unknown so the tenant gate fails closed.
    writes
        .record_fill(
            &write,
            &TenantPolyFill {
                organization_id: tenant,
                fill_id: format!("fill-{run}"),
                venue_order_id: order_id.clone(),
                order_id: format!("oms-{run}"),
                token_id,
                side: "buy".into(),
                price: 0.5,
                size_tokens: 2.0,
                quote_usd: 1.0,
                source: "test".into(),
                position_id: None,
                ts: now,
            },
        )
        .await
        .expect("record fill");
    assert_eq!(
        oracle
            .realized_today_for_module(Some(tenant), bot_core::models::BotModule::Polymarket)
            .await,
        None
    );
    assert_eq!(
        oracle
            .realized_today_status_for_module(
                Some(tenant),
                bot_core::models::BotModule::Polymarket,
            )
            .await,
        bot_core::risk::RealizedPnlRead::Incomplete
    );

    // Legacy closed rows with no trustworthy cumulative PnL must not be
    // interpreted as zero or reconstructed from the remaining basis.
    let mut closed = position(tenant, format!("legacy-pnl-{run}"), now);
    closed.status = "closed".into();
    closed.closed_at = Some(now);
    closed.realized_quote = 11.0;
    closed.realized_pnl_quote = None;
    let position_writes = TenantPositionWrite::new(db.clone());
    position_writes
        .upsert(&write, &closed)
        .await
        .expect("persist unknown legacy pnl projection");
    assert_eq!(
        oracle
            .realized_today_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
            .await,
        None
    );
    assert_eq!(
        oracle
            .realized_today_status_for_module(Some(tenant), bot_core::models::BotModule::Sniper,)
            .await,
        bot_core::risk::RealizedPnlRead::Incomplete
    );

    // A same-day lifecycle with a known cumulative result is reportable.
    closed.realized_pnl_quote = Some(2.0);
    position_writes
        .upsert(&write, &closed)
        .await
        .expect("persist known cumulative pnl projection");
    assert_eq!(
        oracle
            .realized_today_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
            .await,
        Some(2.0)
    );

    // Mark-only updates must not make yesterday's open partial exit look like
    // a realization today; the dedicated fill timestamp carries that day.
    let mut partial = position(tenant, format!("open-partial-{run}"), now);
    partial.updated_at = now;
    partial.realized_quote = 4.0;
    partial.realized_pnl_quote = Some(0.0);
    partial.realized_at = Some(now - chrono::Duration::days(1));
    position_writes
        .upsert(&write, &partial)
        .await
        .expect("persist prior-day partial exit");
    assert_eq!(
        oracle
            .realized_today_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
            .await,
        Some(2.0)
    );
    partial.realized_at = Some(now);
    position_writes
        .upsert(&write, &partial)
        .await
        .expect("persist same-day partial exit");
    assert_eq!(
        oracle
            .realized_today_status_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
            .await,
        bot_core::risk::RealizedPnlRead::Incomplete
    );

    // A lifecycle spanning UTC days has cumulative rather than daily PnL,
    // so its close-day contribution is unknown.
    closed.opened_at = now - chrono::Duration::days(1);
    position_writes
        .upsert(&write, &closed)
        .await
        .expect("persist cross-day closed lifecycle");
    assert_eq!(
        oracle
            .realized_today_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
            .await,
        None
    );

    // Partial exits on a still-open position are not silently omitted from
    // the durable daily-loss view.
    closed.status = "open".into();
    closed.opened_at = now;
    closed.closed_at = None;
    closed.realized_quote = 11.0;
    closed.realized_pnl_quote = Some(1.0);
    position_writes
        .upsert(&write, &closed)
        .await
        .expect("persist open partial-exit lifecycle");
    assert_eq!(
        oracle
            .realized_today_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
            .await,
        None
    );
}
