//! STEP 3 integration: the tenant runtime registry over memory and
//! (when POSTGRES_URL is set) PostgreSQL.

use std::sync::Arc;

use bot_core::db::Database;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};
use chrono::Utc;
use sniper_suite::runtime_registry::{
    MemoryRuntimeStore, PgRuntimeStore, RuntimeRegistryService, RuntimeStatus, RuntimeStore,
};

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

#[tokio::test]
async fn the_memory_registry_lifecycle() {
    let service = RuntimeRegistryService::new(Arc::new(MemoryRuntimeStore::new()));
    let org = OrganizationId::new();

    // First registration is generation 1.
    let first = service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap();
    assert_eq!(first.record().generation, RuntimeGeneration::first());

    // A healthy peer is refused (split brain).
    assert!(service
        .ensure_active(org, "worker-b", Utc::now())
        .await
        .is_err());

    // Heartbeating keeps it live; the fence verifies.
    assert!(service
        .heartbeat(first.record().runtime_id, Utc::now())
        .await
        .unwrap());
    assert!(service
        .verify(&first.record().fence_token())
        .await
        .unwrap()
        .is_current());

    // Rotation: the old runtime stops, the new one holds generation 2
    // and the old fence no longer verifies.
    let second = service.rotate(org, "worker-b", Utc::now()).await.unwrap();
    assert_eq!(second.generation.raw(), 2);
    assert!(!service
        .verify(&first.record().fence_token())
        .await
        .unwrap()
        .is_current());
    assert!(service
        .verify(&second.fence_token())
        .await
        .unwrap()
        .is_current());

    // Draining then stopping the successor leaves no live runtime.
    service.drain(second.runtime_id, Utc::now()).await.unwrap();
    service.stop(second.runtime_id, Utc::now()).await.unwrap();
    assert!(service.current(org).await.unwrap().is_none());
}

#[tokio::test]
async fn reaping_cleans_up_stale_workers() {
    let service = RuntimeRegistryService::new(Arc::new(MemoryRuntimeStore::new()));
    let org = OrganizationId::new();
    service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap();

    let later = Utc::now() + chrono::Duration::seconds(3_600);
    let reaped = service.reap_stale(later).await.unwrap();
    assert_eq!(reaped.len(), 1);

    // The returned rows are the pre-reap snapshot; the STORE now shows
    // the runtime stopped and no live runtime resolves.
    let after = service
        .store()
        .get(reaped[0].runtime_id)
        .await
        .unwrap()
        .expect("the row still exists (terminal, not deleted)");
    assert_eq!(after.status, RuntimeStatus::Stopped);
    assert!(service.current(org).await.unwrap().is_none());
}

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[tokio::test]
async fn pg_registry_when_available() {
    let Some(url) = pg_url() else {
        eprintln!("NOT_RUN: pg runtime registry — POSTGRES_URL missing");
        return;
    };
    let cfg = bot_core::config::DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(Database::connect(&cfg, &url).await.unwrap());
    db.migrate().await.unwrap();

    let store = PgRuntimeStore::new(db.clone());
    let org = seed_org(&db).await;
    let runtime_id = RuntimeId::new();
    let now = Utc::now();

    let record = sniper_suite::runtime_registry::TenantRuntimeRecord::new_active(
        org,
        runtime_id,
        RuntimeGeneration::first(),
        "it-worker",
        now,
    );
    store.insert(&record).await.unwrap();

    let live = store.live_for(org).await.unwrap().expect("live runtime");
    assert_eq!(live.runtime_id, runtime_id);
    assert!(store.heartbeat(runtime_id, Utc::now()).await.unwrap());
    assert_eq!(store.max_generation(org).await.unwrap().unwrap().raw(), 1);

    // Rotation supersedes atomically.
    let rotated = store
        .rotate(org, RuntimeId::new(), "it-worker-2", Utc::now())
        .await
        .unwrap();
    assert_eq!(rotated.generation.raw(), 2);
    let old = store.get(runtime_id).await.unwrap().unwrap();
    assert!(old.status.is_terminal());
}
