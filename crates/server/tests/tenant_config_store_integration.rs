//! STEP 3 integration: the tenant configuration engine over memory and
//! (when POSTGRES_URL is set) PostgreSQL.

use std::sync::Arc;

use bot_core::db::Database;
use bot_core::models::ExecutionMode;
use bot_core::tenant::ModuleKind;
use bot_core::tenant::{ModuleDisableReason, OrganizationId};
use chrono::Utc;
use sniper_suite::tenant_config::audit::{ConfigAuditSink, MemoryConfigAuditSink};
use sniper_suite::tenant_config::store::{
    ConfigStore, ConfigWriteError, MemoryConfigStore, PgConfigStore,
};
use sniper_suite::tenant_config::version::ConfigVersion;
use sniper_suite::tenant_config::{GlobalSafetyBounds, TenantConfigModel};

/// Insert an organizations row (the FK target every tenant table
/// references) and return its id.
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

const MODES: [ExecutionMode; 1] = [ExecutionMode::Paper];

fn bounds() -> GlobalSafetyBounds {
    GlobalSafetyBounds {
        max_position_usd: 10_000.0,
        daily_loss_usd_cap: 1_000.0,
        max_slippage_bps: 500,
        allowed_modes: &MODES,
    }
}

#[tokio::test]
async fn memory_cas_and_validation_round_trip() {
    let store = MemoryConfigStore::new();
    let org = OrganizationId::new();

    let first = store
        .put(
            org,
            &TenantConfigModel::default(),
            None,
            Some("op"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    assert_eq!(first.version.raw(), 1);

    // CAS: stale expectations fail with the actual stored version.
    let mut updated = TenantConfigModel::default();
    updated.disable_module(ModuleKind::Copy, ModuleDisableReason::Operator);
    let err = store
        .put(
            org,
            &updated,
            Some(ConfigVersion::from_raw(7).unwrap()),
            Some("op"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ConfigWriteError::StaleVersion {
            expected: 7,
            stored: 1
        }
    );

    let second = store
        .put(
            org,
            &updated,
            Some(first.version),
            Some("op"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    assert_eq!(second.version.raw(), 2);
    assert!(!second.config.module_enabled(ModuleKind::Copy).is_enabled());

    // Invalid documents are refused BEFORE persistence.
    let mut bad = TenantConfigModel::default();
    bad.risk.max_position_usd = Some(f64::INFINITY);
    assert!(matches!(
        store
            .put(org, &bad, Some(second.version), None, Utc::now(), &bounds())
            .await
            .unwrap_err(),
        ConfigWriteError::Invalid(_)
    ));
}

#[tokio::test]
async fn audit_entries_carry_the_diff() {
    let sink = MemoryConfigAuditSink::new();
    let org = OrganizationId::new();
    let old = TenantConfigModel::default();
    let mut new = old.clone();
    new.risk.max_position_usd = Some(500.0);

    let entry = sniper_suite::tenant_config::audit::entry_for(
        org,
        Some(&old),
        Some(ConfigVersion::first()),
        &new,
        ConfigVersion::first().next(),
        Some("operator"),
        Utc::now(),
    );
    assert_eq!(entry.changes.len(), 1);
    sink.append(entry).await.unwrap();
    assert_eq!(sink.recent(org, 5).await.unwrap().len(), 1);
}

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[tokio::test]
async fn pg_cas_when_available() {
    let Some(url) = pg_url() else {
        eprintln!("NOT_RUN: pg config CAS — POSTGRES_URL missing");
        return;
    };
    let cfg = bot_core::config::DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(Database::connect(&cfg, &url).await.unwrap());
    db.migrate().await.unwrap();

    let store = PgConfigStore::new(db.clone());
    let org = seed_org(&db).await;
    let first = store
        .put(
            org,
            &TenantConfigModel::default(),
            None,
            Some("it"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    assert_eq!(first.version.raw(), 1);

    // A CAS with the CURRENT version succeeds and moves to v2.
    let second = store
        .put(
            org,
            &TenantConfigModel::default(),
            Some(first.version),
            Some("it"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    assert_eq!(second.version.raw(), 2);

    // Now the FIRST writer's replayed CAS is stale (expected 1, stored 2).
    let err = store
        .put(
            org,
            &TenantConfigModel::default(),
            Some(first.version),
            None,
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ConfigWriteError::StaleVersion {
            expected: 1,
            stored: 2
        }
    );

    // Read back the typed document.
    let record = store.get(org).await.unwrap().unwrap();
    assert_eq!(record.version.raw(), 2);
    assert_eq!(record.config, TenantConfigModel::default());
}
