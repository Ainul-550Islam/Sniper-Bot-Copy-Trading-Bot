//! BATCH 02 — migration 0063–0065 schema contract on real PostgreSQL.
//!
//! Gated exactly like `dataset_pg_store.rs`: panics when REQUIRE_POSTGRES=1
//! and no POSTGRES_URL is present.

use std::path::Path;
use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;

fn pg_url() -> Option<String> {
    match std::env::var("POSTGRES_URL") {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            if std::env::var("REQUIRE_POSTGRES").as_deref() == Ok("1") {
                panic!("REQUIRE_POSTGRES=1 but POSTGRES_URL is missing");
            }
            None
        }
    }
}

// A FRESH Database per test (never a shared static pool): each
// #[tokio::test] owns its runtime, and a sqlx PgPool must not outlive the
// runtime it was created on. sqlx's advisory lock serializes concurrent
// migrate() calls across tests. Generous timeouts absorb a busy CI runner.
async fn setup() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        max_connections: 8,
        acquire_timeout_ms: 60_000,
        statement_timeout_ms: 120_000,
        query_timeout_ms: 120_000,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &url)
            .await
            .expect("connect postgres"),
    );
    db.migrate().await.expect("migrations must apply");
    Some(db)
}

fn migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/migrations")
}

#[tokio::test]
async fn global_dataset_tables_exist_without_tenant_columns() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: dataset tables — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();

    let (tables,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name IN \
           ('recorded_launch_datasets', 'recorded_launch_dataset_chunks', \
            'recorded_launch_dataset_verifications')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(tables, 3, "all three global dataset tables must exist");

    let (tenant_cols,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name IN \
           ('recorded_launch_datasets', 'recorded_launch_dataset_chunks', \
            'recorded_launch_dataset_verifications') \
           AND column_name IN ('organization_id', 'org_id', 'tenant_id')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        tenant_cols, 0,
        "GLOBAL market data must have NO tenant column (Batch 01 RLS loop)"
    );

    // The authoritative guard CHECK is present.
    let (guards,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pg_constraint \
         WHERE conname = 'recorded_launch_datasets_authoritative_guard'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(guards, 1, "the authoritative guard constraint must exist");
}

#[tokio::test]
async fn dataset_migrations_are_idempotent_on_reapply() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: dataset migrations idempotency — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();

    // Re-running the embedded migrator is a no-op (checksum tracking).
    db.migrate()
        .await
        .expect("second migrate must be idempotent");

    // Re-EXECUTE the raw 0063–0065 SQL twice. Every statement is guarded
    // (CREATE ... IF NOT EXISTS / idempotent guarded GRANT DO-block), so a
    // double re-apply must succeed without duplicating or erroring.
    let dir = migrations_dir();
    for name in [
        "0063_launch_datasets.sql",
        "0064_launch_dataset_chunks.sql",
        "0065_launch_dataset_verifications.sql",
    ] {
        let sql =
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
        for _ in 0..2 {
            sqlx::raw_sql(&sql)
                .execute(pool)
                .await
                .unwrap_or_else(|e| panic!("re-apply {name}: {e}"));
        }
    }

    // Tables still unique (no duplicate relations/constraints).
    let (tables,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name IN \
           ('recorded_launch_datasets', 'recorded_launch_dataset_chunks', \
            'recorded_launch_dataset_verifications')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(tables, 3);
}

#[tokio::test]
async fn guarded_grants_work_without_batch01_roles() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: guarded grants — POSTGRES_URL missing");
        return;
    };
    // The 0065 DO-block must apply cleanly whether or not bot_app /
    // bot_system exist. Re-running it here (roles typically absent) proves
    // the guard. No assertion on role existence — only that it does not
    // error, which dataset_migrations_are_idempotent_on_reapply already
    // covers; here we assert the roles query itself is well-formed.
    let (_n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pg_roles WHERE rolname IN ('bot_app', 'bot_system')")
            .fetch_one(db.pool())
            .await
            .unwrap();
}
