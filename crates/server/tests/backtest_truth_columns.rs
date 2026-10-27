use chrono::{Duration, Utc};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn postgres_url() -> String {
    std::env::var("POSTGRES_URL")
        .expect("POSTGRES_URL is required for the backtest truth integration test")
}

#[tokio::test]
async fn shared_list_and_detail_serializer_returns_truth_columns() {
    if std::env::var("REQUIRE_POSTGRES").ok().as_deref() == Some("1") {
        assert!(
            std::env::var_os("POSTGRES_URL").is_some(),
            "REQUIRE_POSTGRES=1 requires POSTGRES_URL"
        );
    }
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&postgres_url())
        .await
        .expect("connect to PostgreSQL");
    bot_core::db::MIGRATOR
        .run(&pool)
        .await
        .expect("apply migrations");

    let organization_id = Uuid::new_v4();
    let strategy_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'truth test organization')",
    )
    .bind(organization_id)
    .bind(format!("truth-{}", organization_id.simple()))
    .execute(&pool)
    .await
    .expect("insert organization");
    sqlx::query("INSERT INTO tenant_strategies (id, organization_id, name, module) VALUES ($1, $2, 'truth test strategy', 'sniper')")
        .bind(strategy_id)
        .bind(organization_id)
        .execute(&pool)
        .await
        .expect("insert strategy");

    let period_start = Utc::now() - Duration::days(2);
    let period_end = period_start + Duration::days(1);
    sqlx::query(
        "INSERT INTO backtest_runs
            (id, organization_id, strategy_id, venue, period_start, period_end,
             initial_balance_usd_cents, status, data_source, authoritative)
         VALUES ($1, $2, $3, 'raydium', $4, $5, 100000, 'completed', 'synthetic', false)",
    )
    .bind(run_id)
    .bind(organization_id)
    .bind(strategy_id)
    .bind(period_start)
    .bind(period_end)
    .execute(&pool)
    .await
    .expect("insert backtest with known provenance");

    // The list and detail handlers share run_json. Exercise the exact row
    // shape consumed by that serializer and assert its response JSON values.
    let row = sqlx::query(
        "SELECT b.id, b.organization_id, b.strategy_id, s.name AS strategy_name,
                s.module, b.venue, b.period_start, b.period_end,
                b.initial_balance_usd_cents, b.fee_rate_bps, b.slippage_bps,
                b.status, b.result_json, b.error, b.created_at, b.completed_at,
                b.data_source, b.authoritative, b.blocked_reason, b.dataset_id
           FROM backtest_runs b
           JOIN tenant_strategies s ON s.id = b.strategy_id
          WHERE b.organization_id = $1 AND b.id = $2",
    )
    .bind(organization_id)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("load backtest row through list/detail projection");
    let response = sniper_suite::trading_data_plane::backtests::run_json(&row);
    assert_eq!(response["data_source"], "synthetic");
    assert_eq!(response["authoritative"], false);

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(organization_id)
        .execute(&pool)
        .await
        .expect("clean up truth test organization");
    pool.close().await;
}
