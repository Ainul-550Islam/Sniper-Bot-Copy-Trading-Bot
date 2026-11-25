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

/// Today is a bounded UTC calendar day; future positions must not offset
/// today's loss or turn its complete projection into an unknown value.
#[tokio::test]
async fn daily_risk_pnl_excludes_future_utc_days() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: daily_risk_pnl_excludes_future_utc_days — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("utc-window-{run}")).await;
    let writes = TenantPositionWrite::new(db.clone());
    let day = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let mut today = position(tenant, format!("today-{run}"), day);
    today.status = "closed".into();
    today.closed_at = Some(day + chrono::Duration::seconds(1));
    today.realized_at = today.closed_at;
    today.realized_pnl_quote = Some(-5.0);
    writes.upsert(&write_scope(tenant), &today).await.unwrap();

    let future_time = day + chrono::Duration::days(2);
    let mut future = position(tenant, format!("future-{run}"), future_time);
    future.status = "closed".into();
    future.closed_at = Some(future_time + chrono::Duration::seconds(1));
    future.realized_at = future.closed_at;
    future.realized_pnl_quote = Some(100.0);
    writes.upsert(&write_scope(tenant), &future).await.unwrap();
    let oracle = PostgresRiskOracle::new(db);
    let actual = oracle
        .realized_today_status_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
        .await;
    assert_eq!(actual, bot_core::risk::RealizedPnlRead::Known(-5.0));

    future.realized_pnl_quote = None;
    writes.upsert(&write_scope(tenant), &future).await.unwrap();
    let actual = oracle
        .realized_today_status_for_module(Some(tenant), bot_core::models::BotModule::Sniper)
        .await;
    assert_eq!(actual, bot_core::risk::RealizedPnlRead::Known(-5.0));
}

/// One process per replica gets a fresh AppState; ids persisted in a global
/// primary key must not restart from the same short local counter.
#[tokio::test]
async fn independent_states_persist_distinct_position_ids() {
    let Some(db) = setup().await else {
        eprintln!(
            "NOT_RUN: independent_states_persist_distinct_position_ids — POSTGRES_URL missing"
        );
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("position-id-{run}")).await;
    let a = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
    let b = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
    let id_a = a.next_persistent_id("p");
    let id_b = b.next_persistent_id("p");
    let trade_a = a.next_persistent_id("t");
    let trade_b = b.next_persistent_id("t");
    assert_ne!(trade_a, trade_b);
    assert_ne!(
        id_a, id_b,
        "independent states must not allocate the same persisted position id"
    );
    let writes = TenantPositionWrite::new(db.clone());
    let reads = TenantPositionRead::new(db.clone());
    let trades = TenantTradeRepo::new(db);
    let mut first = position(tenant, id_a.clone(), chrono::Utc::now());
    first.symbol = "MINT-A".into();
    first.cost_basis = 1.0;
    let mut second = position(tenant, id_b.clone(), chrono::Utc::now());
    second.symbol = "MINT-B".into();
    second.cost_basis = 2.0;
    writes.upsert(&write_scope(tenant), &first).await.unwrap();
    writes.upsert(&write_scope(tenant), &second).await.unwrap();
    let row_a = reads.get(&read_scope(tenant), &id_a).await.unwrap();
    let row_b = reads.get(&read_scope(tenant), &id_b).await.unwrap();
    assert_eq!((row_a.symbol.as_str(), row_a.cost_basis), ("MINT-A", 1.0));
    assert_eq!((row_b.symbol.as_str(), row_b.cost_basis), ("MINT-B", 2.0));
    for (id, position_id, symbol) in [(&trade_a, &id_a, "MINT-A"), (&trade_b, &id_b, "MINT-B")] {
        let trade = bot_core::trading_repository::positions::model::TenantTrade {
            organization_id: tenant,
            id: id.clone(),
            ts: chrono::Utc::now(),
            source: "sniper".into(),
            venue: "paper".into(),
            mode: "paper".into(),
            side: "long".into(),
            symbol: symbol.into(),
            symbol_display: symbol.into(),
            amount_in: 1.0,
            amount_out: 1.0,
            quote_symbol: "SOL".into(),
            price: 1.0,
            fee: 0.0,
            slippage_bps: 0,
            signature: None,
            position_id: Some(position_id.clone()),
            note: None,
            latency_ms: None,
        };
        trades.append(&write_scope(tenant), &trade).await.unwrap();
    }
    let first_trades = trades
        .list_for_position(&read_scope(tenant), &id_a)
        .await
        .unwrap();
    let second_trades = trades
        .list_for_position(&read_scope(tenant), &id_b)
        .await
        .unwrap();
    assert_eq!(first_trades.len(), 1);
    assert_eq!(first_trades[0].id, trade_a);
    assert_eq!(second_trades.len(), 1);
    assert_eq!(second_trades[0].id, trade_b);
}

/// This test deliberately uses a colliding id to demonstrate the legacy
/// failure mode. It is a witness to unsafe behavior, not desired semantics.
#[tokio::test]
async fn colliding_legacy_position_ids_can_mix_distinct_positions() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: colliding_legacy_position_ids_can_mix_distinct_positions — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("position-alias-{run}")).await;
    let id = format!("legacy-position-{run}");
    let writes = TenantPositionWrite::new(db.clone());
    let reads = TenantPositionRead::new(db);
    let mut first = position(tenant, id.clone(), chrono::Utc::now());
    first.symbol = "MINT-A".into();
    first.cost_basis = 1.0;
    let mut second = position(tenant, id.clone(), chrono::Utc::now());
    second.symbol = "MINT-B".into();
    second.cost_basis = 2.0;
    writes.upsert(&write_scope(tenant), &first).await.unwrap();
    writes.upsert(&write_scope(tenant), &second).await.unwrap();
    let row = reads.get(&read_scope(tenant), &id).await.unwrap();
    assert_eq!(
        (row.symbol.as_str(), row.cost_basis),
        ("MINT-A", 2.0),
        "old symbol combined with new cost demonstrates aliasing"
    );
}

#[tokio::test]
async fn tenant_trade_rejects_foreign_or_missing_position() {
    let Some(db) = setup().await else {
        eprintln!(
            "NOT_RUN: tenant_trade_rejects_foreign_or_missing_position — POSTGRES_URL missing"
        );
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("trade-owner-a-{run}")).await;
    let b = org(&db, &format!("trade-owner-b-{run}")).await;
    let owner_position = format!("foreign-position-{run}");
    TenantPositionWrite::new(db.clone())
        .upsert(
            &write_scope(a),
            &position(a, owner_position.clone(), chrono::Utc::now()),
        )
        .await
        .unwrap();
    let trades = TenantTradeRepo::new(db.clone());
    let mut trade = bot_core::trading_repository::positions::model::TenantTrade {
        organization_id: b,
        id: format!("trade-foreign-{run}"),
        ts: chrono::Utc::now(),
        source: "sniper".into(),
        venue: "paper".into(),
        mode: "paper".into(),
        side: "long".into(),
        symbol: "TOKEN/SOL".into(),
        symbol_display: "TOKEN/SOL".into(),
        amount_in: 1.0,
        amount_out: 1.0,
        quote_symbol: "SOL".into(),
        price: 1.0,
        fee: 0.0,
        slippage_bps: 0,
        signature: None,
        position_id: Some(owner_position.clone()),
        note: None,
        latency_ms: None,
    };
    assert!(matches!(
        trades.append(&write_scope(b), &trade).await,
        Err(RepositoryError::NotFound("position"))
    ));
    assert!(trades
        .list_for_position(&read_scope(b), &owner_position)
        .await
        .unwrap()
        .is_empty());
    trade.id = format!("trade-missing-{run}");
    trade.position_id = Some(format!("missing-position-{run}"));
    assert!(matches!(
        trades.append(&write_scope(b), &trade).await,
        Err(RepositoryError::NotFound("position"))
    ));
    trade.position_id = None;
    trade.id = format!("trade-unlinked-{run}");
    trades.append(&write_scope(b), &trade).await.unwrap();
}

/// Exact retries are idempotent; distinct trades must not be silently lost.
#[tokio::test]
async fn tenant_trade_id_collision_fails_closed() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: tenant_trade_id_collision_fails_closed — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("trade-collision-a-{run}")).await;
    let b = org(&db, &format!("trade-collision-b-{run}")).await;
    let repo = TenantTradeRepo::new(db);
    let trade = bot_core::trading_repository::positions::model::TenantTrade {
        organization_id: a,
        id: format!("colliding-trade-{run}"),
        ts: chrono::Utc::now(),
        source: "sniper".into(),
        venue: "paper".into(),
        mode: "paper".into(),
        side: "long".into(),
        symbol: "MINT-A".into(),
        symbol_display: "MINT-A".into(),
        amount_in: 1.0,
        amount_out: 1.0,
        quote_symbol: "SOL".into(),
        price: 1.0,
        fee: 0.0,
        slippage_bps: 0,
        signature: Some("signature-a".into()),
        position_id: None,
        note: None,
        latency_ms: None,
    };
    repo.append(&write_scope(a), &trade).await.unwrap();
    repo.append(&write_scope(a), &trade).await.unwrap(); // legitimate retry
    let mut changed = trade.clone();
    changed.signature = Some("signature-b".into());
    changed.symbol = "MINT-B".into();
    assert!(matches!(
        repo.append(&write_scope(a), &changed).await,
        Err(RepositoryError::Conflict("trade_id"))
    ));
    changed.organization_id = b;
    assert!(matches!(
        repo.append(&write_scope(b), &changed).await,
        Err(RepositoryError::Conflict("trade_id"))
    ));
    let since = trade.ts - chrono::Duration::minutes(1);
    let until = trade.ts + chrono::Duration::minutes(1);
    let rows_a = repo
        .list_between(&read_scope(a), since, until)
        .await
        .unwrap();
    let rows_b = repo
        .list_between(&read_scope(b), since, until)
        .await
        .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].signature.as_deref(), Some("signature-a"));
    assert!(rows_b.is_empty());

    // Separate simultaneous pool operations with the same payload both
    // succeed, while competing different payloads have exactly one winner.
    let mut retry = trade.clone();
    retry.id = format!("retry-race-{run}");
    let scope_a = write_scope(a);
    let (first, second) =
        tokio::join!(repo.append(&scope_a, &retry), repo.append(&scope_a, &retry));
    assert!(first.is_ok() && second.is_ok());
    let mut competing_a = retry.clone();
    competing_a.id = format!("collision-race-{run}");
    let mut competing_b = competing_a.clone();
    competing_b.signature = Some("other-signature".into());
    let (first, second) = tokio::join!(
        repo.append(&scope_a, &competing_a),
        repo.append(&scope_a, &competing_b)
    );
    assert!(matches!(
        (&first, &second),
        (Ok(()), Err(RepositoryError::Conflict("trade_id")))
            | (Err(RepositoryError::Conflict("trade_id")), Ok(()))
    ));
    let winner = if first.is_ok() {
        &competing_a
    } else {
        &competing_b
    };
    repo.append(&scope_a, winner).await.unwrap();
}

/// A live position beyond the old 5,000-row cap must be available to
/// deployment recovery and book sync.
#[tokio::test]
async fn deployment_open_position_read_includes_more_than_5000() {
    let Some(db) = setup().await else {
        eprintln!(
            "NOT_RUN: deployment_open_position_read_includes_more_than_5000 — POSTGRES_URL missing"
        );
        return;
    };
    let org = bot_core::db::deployment_org::deployment_org_uuid(&db)
        .await
        .unwrap();
    let prefix = format!("limit-witness-{}-", run_id());
    sqlx::query(
        r#"INSERT INTO positions
        (organization_id,id,source,venue,mode,status,symbol,opened_at,updated_at)
        SELECT $1,$2 || i::text,'sniper','paper','paper','open','LIMIT-WITNESS',
               '2001-01-01 00:00:00+00'::timestamptz + i * interval '1 second', now()
        FROM generate_series(1,5001) AS i"#,
    )
    .bind(org)
    .bind(&prefix)
    .execute(db.pool())
    .await
    .unwrap();
    let rows = bot_core::db::repo::PositionRepo::new(db.clone())
        .list_open()
        .await
        .unwrap();
    let tenant_rows = TenantPositionRead::new(db.clone())
        .list_open(&read_scope(bot_core::tenant::OrganizationId::from(org)))
        .await
        .unwrap();
    let tenant_last_found = tenant_rows.iter().any(|p| p.id == format!("{prefix}5001"));
    let first_found = rows.iter().any(|p| p.id == format!("{prefix}1"));
    let last_found = rows.iter().any(|p| p.id == format!("{prefix}5001"));
    let count = rows.len();
    sqlx::query("DELETE FROM positions WHERE organization_id = $1 AND left(id, length($2)) = $2")
        .bind(org)
        .bind(&prefix)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        count >= 5001,
        "live position recovery must not silently truncate"
    );
    assert!(first_found);
    assert!(last_found, "the 5001st open position must be recoverable");
    assert!(
        tenant_last_found,
        "tenant recovery must not silently truncate"
    );
}

#[tokio::test]
async fn legacy_trade_repo_rejects_distinct_fill_with_same_id() {
    let Some(db) = setup().await else {
        eprintln!(
            "NOT_RUN: legacy_trade_repo_rejects_distinct_fill_with_same_id — POSTGRES_URL missing"
        );
        return;
    };
    let run = run_id();
    let repo = bot_core::db::repo::TradeRepo::new(db.clone());
    let trade = bot_core::models::Trade {
        id: format!("legacy-trade-{run}"),
        ts: chrono::Utc::now(),
        source: bot_core::models::TradeSource::Sniper,
        venue: bot_core::models::Venue::Paper,
        mode: bot_core::models::ExecutionMode::Paper,
        side: bot_core::models::PositionSide::Long,
        symbol: "MINT-A".into(),
        symbol_display: "MINT-A".into(),
        amount_in: 1.0,
        amount_out: 1.0,
        quote_symbol: "SOL".into(),
        price: 1.0,
        fee: 0.0,
        slippage_bps: 0,
        signature: Some("sig-a".into()),
        position_id: None,
        note: None,
        latency_ms: None,
    };
    repo.append(&trade).await.unwrap();
    repo.append(&trade).await.unwrap();
    let mut changed = trade.clone();
    changed.signature = Some("sig-b".into());
    assert!(matches!(
        repo.append(&changed).await,
        Err(bot_core::db::TimedDbError::Conflict("trade_id"))
    ));
    let original = repo
        .list_recent(1000)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == trade.id)
        .unwrap();
    assert_eq!(original.signature.as_deref(), Some("sig-a"));

    let mut same = trade.clone();
    same.id = format!("legacy-retry-race-{run}");
    let (first, second) = tokio::join!(repo.append(&same), repo.append(&same));
    assert!(first.is_ok() && second.is_ok());
    let mut race_a = same.clone();
    race_a.id = format!("legacy-collision-race-{run}");
    let mut race_b = race_a.clone();
    race_b.signature = Some("sig-other".into());
    let (first, second) = tokio::join!(repo.append(&race_a), repo.append(&race_b));
    assert!(matches!(
        (&first, &second),
        (
            Ok(()),
            Err(bot_core::db::TimedDbError::Conflict("trade_id"))
        ) | (
            Err(bot_core::db::TimedDbError::Conflict("trade_id")),
            Ok(())
        )
    ));
    let winner = if first.is_ok() { &race_a } else { &race_b };
    repo.append(winner).await.unwrap();

    let other_org = org(&db, &format!("legacy-collision-foreign-{run}")).await;
    let foreign_id = format!("legacy-foreign-{run}");
    sqlx::query(
        r#"INSERT INTO trades (organization_id,id,ts,source,venue,mode,side,symbol)
                  VALUES ($1,$2,now(),'sniper','paper','paper','long','FOREIGN')"#,
    )
    .bind(other_org.as_uuid())
    .bind(&foreign_id)
    .execute(db.pool())
    .await
    .unwrap();
    let mut foreign_collision = trade.clone();
    foreign_collision.id = foreign_id.clone();
    assert!(matches!(
        repo.append(&foreign_collision).await,
        Err(bot_core::db::TimedDbError::Conflict("trade_id"))
    ));
    let stored: (uuid::Uuid, String) =
        sqlx::query_as("SELECT organization_id, symbol FROM trades WHERE id = $1")
            .bind(&foreign_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stored, (other_org.as_uuid(), "FOREIGN".into()));
}

#[tokio::test]
async fn balance_latest_separates_chains_and_breaks_timestamp_ties() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: balance_latest_separates_chains_and_breaks_timestamp_ties — POSTGRES_URL missing");
        return;
    };
    let tenant = org(&db, &format!("balance-chain-{}", run_id())).await;
    let balances = TenantBalanceRepo::new(db.clone());
    let at = chrono::Utc::now();
    let write = write_scope(tenant);
    balances
        .record(
            &write,
            "chain-a",
            "same-address",
            "USD",
            1.0,
            Some(1.0),
            "test",
            at,
        )
        .await
        .unwrap();
    balances
        .record(
            &write,
            "chain-b",
            "same-address",
            "USD",
            2.0,
            Some(2.0),
            "test",
            at,
        )
        .await
        .unwrap();
    balances
        .record(
            &write,
            "chain-a",
            "same-address",
            "USD",
            3.0,
            Some(3.0),
            "test",
            at,
        )
        .await
        .unwrap();
    let mut rows = balances
        .latest_per_asset(&read_scope(tenant))
        .await
        .unwrap();
    rows.sort_by(|a, b| a.chain.cmp(&b.chain));
    assert_eq!(
        rows.len(),
        2,
        "same address and asset on distinct chains are different balances"
    );
    assert_eq!((rows[0].chain.as_str(), rows[0].amount), ("chain-a", 3.0));
    assert_eq!((rows[1].chain.as_str(), rows[1].amount), ("chain-b", 2.0));
    assert_eq!(
        balances
            .total_usd_latest(&read_scope(tenant))
            .await
            .unwrap(),
        Some(5.0)
    );
    balances
        .record(
            &write,
            "chain-c",
            "same-address",
            "OTHER",
            7.0,
            None,
            "test",
            at,
        )
        .await
        .unwrap();
    // The former SUM ignored the unknown asset and returned a partial value.
    let old_total: Option<f64> = sqlx::query_scalar(r#"SELECT SUM(usd_value)::double precision FROM (
       SELECT DISTINCT ON (address, asset) usd_value FROM balance_snapshots WHERE organization_id = $1
       ORDER BY address, asset, ts DESC
    ) latest"#).bind(tenant.as_uuid()).fetch_one(db.pool()).await.unwrap();
    assert!(
        old_total.is_some(),
        "the prior SQL returned a misleading partial sum"
    );
    assert_eq!(
        balances
            .total_usd_latest(&read_scope(tenant))
            .await
            .unwrap(),
        None,
        "unvalued latest balance must not produce a misleading partial total"
    );
}

/// Gap #6: a fill dated in the FUTURE must not block today's realized PnL.
///
/// `realized_pnl_read` sums only positions closed inside today's UTC window, but
/// its completeness probe asked a strictly wider question: `poly_fills.ts >=
/// midnight` with no upper bound. A single fill timestamped tomorrow therefore
/// made the probe answer "incomplete", which `effective_realized` turns into
/// `RiskCode::GlobalUnavailable` — every entry refused, today and every day,
/// while the row sits there. The probe now asks about the same window the sum
/// uses, so the answer describes the number it guards.
#[tokio::test]
async fn a_future_dated_poly_fill_does_not_block_todays_pnl() {
    use bot_core::db::claims::PostgresRiskOracle;
    use bot_core::risk::{GlobalRiskOracle, RealizedPnlRead};
    use bot_core::tenant::OrganizationId;

    let Some(db) = setup().await else { return };
    let oracle = PostgresRiskOracle::new(db.clone());
    let org_uuid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(org_uuid)
        .bind(format!("future-fill-{org_uuid}"))
        .bind("Future Fill Witness")
        .execute(db.pool())
        .await
        .expect("create isolated tenant");
    let organization = OrganizationId::from(org_uuid);

    // Clean tenant: the projection is complete and the day reads as zero.
    assert_eq!(
        oracle.realized_today_status_for(Some(organization)).await,
        RealizedPnlRead::Known(0.0),
        "an empty tenant must read a complete zero"
    );

    let fill_id = format!("future-fill-{}", uuid::Uuid::new_v4().simple());
    let insert = |ts: &'static str| {
        let id = fill_id.clone();
        let db = db.clone();
        async move {
            sqlx::query(
                "INSERT INTO poly_fills
                   (fill_id, venue_order_id, order_id, token_id, side, price, size_tokens,
                    quote_usd, source, position_id, ts, organization_id)
                 VALUES ($1, $2, $3, 'token-future', 'buy', 0.5, 10.0, 5.0, 'witness', NULL,
                         now() + $4::interval, $5)",
            )
            .bind(&id)
            .bind(format!("venue-{id}"))
            .bind(format!("order-{id}"))
            .bind(ts)
            .bind(org_uuid)
            .execute(db.pool())
            .await
            .expect("fill insert");
        }
    };

    // A fill that happened earlier today, unprojected, IS today's problem.
    insert("-1 hour").await;
    assert_eq!(
        oracle.realized_today_status_for(Some(organization)).await,
        RealizedPnlRead::Incomplete,
        "an unprojected fill inside today's window must still refuse the read"
    );
    sqlx::query("DELETE FROM poly_fills WHERE fill_id = $1")
        .bind(&fill_id)
        .execute(db.pool())
        .await
        .expect("cleanup today's fill");

    // A fill timestamped tomorrow is OUTSIDE the window the sum uses.
    insert("1 day").await;
    assert_eq!(
        oracle.realized_today_status_for(Some(organization)).await,
        RealizedPnlRead::Known(0.0),
        "a fill dated in the future must not block today's PnL"
    );

    sqlx::query("DELETE FROM poly_fills WHERE fill_id = $1")
        .bind(&fill_id)
        .execute(db.pool())
        .await
        .expect("cleanup future fill");
}
