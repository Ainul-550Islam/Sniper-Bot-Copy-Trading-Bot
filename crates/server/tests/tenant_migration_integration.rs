//! STEP 3 integration: migration 0025's schema contract — the tables
//! and indexes the tenant layer depends on. Requires POSTGRES_URL.

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use std::sync::Arc;

async fn seed_org(db: &Arc<Database>) -> OrganizationId {
    let org = OrganizationId::new();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) \
         VALUES ($1, $2, $3)",
    )
    .bind(org.as_uuid())
    .bind(format!("it-{org:?}"))
    .bind("Integration Tenant")
    .execute(db.pool())
    .await
    .unwrap();
    org
}

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[tokio::test]
async fn migration_0025_tables_and_indexes_exist() {
    let Some(url) = pg_url() else {
        eprintln!("NOT_RUN: migration 0025 schema — POSTGRES_URL missing");
        return;
    };
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(Database::connect(&cfg, &url).await.unwrap());
    db.migrate().await.unwrap();

    // Every table the tenant layer writes to exists.
    for table in [
        "tenant_runtimes",
        "tenant_configs",
        "tenant_config_audit",
        "tenant_decision_log",
        "tenant_bindings",
    ] {
        let found: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name = '{table}'"
        ))
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(found.0, 1, "table {table} must exist after migration");
    }

    // The split-brain guard: a PARTIAL UNIQUE index enforcing one live
    // runtime per organization.
    let (idx,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pg_indexes \
         WHERE indexname = 'tenant_runtimes_one_live_per_org_idx'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        idx, 1,
        "the one-live-per-org partial unique index must exist"
    );

    // The status CHECK vocabulary is the closed set (probe with a REAL
    // organization so only the status violates the constraint).
    let probe_org = seed_org(&db).await;
    let bad = sqlx::query(
        "INSERT INTO tenant_runtimes \
            (organization_id, generation, status, worker_id, started_at, heartbeat_at) \
         VALUES ($1, 1, 'zombie', 'probe', now(), now())",
    )
    .bind(probe_org.as_uuid())
    .execute(db.pool())
    .await;
    assert!(
        bad.is_err(),
        "an unknown runtime status must be rejected by the CHECK"
    );

    // Config versions start at 1.
    let bad_version = sqlx::query(
        "INSERT INTO tenant_configs (organization_id, version, config) \
         VALUES ($1, 0, '{}'::jsonb)",
    )
    .bind(probe_org.as_uuid())
    .execute(db.pool())
    .await;
    assert!(bad_version.is_err(), "version 0 must be rejected");
}
