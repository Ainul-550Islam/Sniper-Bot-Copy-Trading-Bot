//! Fresh-database migration contract.
//!
//! This test is deliberately gated: it must receive a disposable PostgreSQL
//! URL and refuses to run against a database that already has applied
//! migrations. That prevents a buyer's real database from being mutated by a
//! verification test while still catching ordering, duplicate-table and
//! foreign-key mistakes on the clean path.

use std::collections::BTreeSet;

use bot_core::db::MIGRATOR;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn migration_url() -> Option<String> {
    std::env::var("POSTGRES_MIGRATION_URL")
        .or_else(|_| std::env::var("POSTGRES_URL"))
        .ok()
        .filter(|url| !url.trim().is_empty())
}

async fn columns(pool: &PgPool, table: &str) -> BTreeSet<String> {
    sqlx::query_scalar(
        "SELECT column_name::text
           FROM information_schema.columns
          WHERE table_schema = 'public' AND table_name = $1
          ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("information_schema must be queryable")
    .into_iter()
    .collect()
}

#[tokio::test]
async fn fresh_migrations_apply_and_keep_operator_and_tenant_tables_separate() {
    let Some(url) = migration_url() else {
        eprintln!("NOT_RUN: migrations_apply_clean — POSTGRES_MIGRATION_URL/POSTGRES_URL missing");
        return;
    };

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("migration database must be reachable");

    let existing_migration_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations')::text")
            .fetch_one(&pool)
            .await
            .expect("PostgreSQL must support to_regclass");
    if existing_migration_table.is_some() {
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .expect("migration count must be readable");
        assert_eq!(
            applied, 0,
            "use a disposable empty database: this test will not mutate a database with applied migrations"
        );
    }

    MIGRATOR
        .run(&pool)
        .await
        .expect("every embedded migration must apply to an empty database");

    let operator_strategies = columns(&pool, "strategies").await;
    assert!(operator_strategies.contains("module"));
    assert!(operator_strategies.contains("name"));
    assert!(operator_strategies.contains("enabled"));
    assert!(
        !operator_strategies.contains("organization_id"),
        "migration 0037 must not mutate the deployment-level strategies table"
    );

    let tenant_strategies = columns(&pool, "tenant_strategies").await;
    for required in [
        "id",
        "organization_id",
        "name",
        "description",
        "module",
        "mode",
        "status",
        "version",
        "config_json",
    ] {
        assert!(
            tenant_strategies.contains(required),
            "tenant_strategies is missing required column {required}"
        );
    }

    for table in [
        "backtest_runs",
        "webhook_endpoints",
        "webhook_deliveries",
        "user_mfa_devices",
        "tenant_sso_configs",
        "portfolio_snapshots_hourly",
        "tenant_daily_accounting",
    ] {
        assert!(
            !columns(&pool, table).await.is_empty(),
            "missing table {table}"
        );
    }

    let backtest_foreign_keys: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid)::text
           FROM pg_constraint
          WHERE conrelid = 'backtest_runs'::regclass AND contype = 'f'",
    )
    .fetch_all(&pool)
    .await
    .expect("backtest foreign keys must be queryable");
    assert!(
        backtest_foreign_keys
            .iter()
            .any(|definition| definition.contains("tenant_strategies")),
        "backtest_runs.strategy_id must reference tenant_strategies: {backtest_foreign_keys:?}"
    );
}
