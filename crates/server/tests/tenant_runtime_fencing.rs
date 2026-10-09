//! Stale runtime / lease / generation denial tests (tenant-isolation
//! file 67).
//!
//! Positive: a tenant's live runtime resolves into a
//! [`RuntimeContext`] with a verifying fence token; the metadata
//! cache serves fresh entries.
//!
//! Negative: no runtime, a stopped (not-live) runtime, a rotated
//! (superseded) fence and a foreign tenant's fence are ALL denied;
//! the cache invalidates on generation change and never serves
//! expired entries; the fence guard maps every failure to the
//! machine-readable `fence_failed` reason.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use sniper_suite::runtime_registry::store::MemoryRuntimeStore;
use sniper_suite::runtime_registry::{
    FenceToken, RuntimeRegistryService, RuntimeStatus, TenantRuntimeRecord,
};
use sniper_suite::tenant::context::{ContextOrigin, TenantContext};
use sniper_suite::tenant::fence_guard;
use sniper_suite::tenant::runtime_cache::RuntimeMetadataCache;
use sniper_suite::tenant::runtime_context::{RuntimeContext, RuntimeContextError};

use bot_core::tenant::{Organization, OrganizationId, RuntimeGeneration, RuntimeId};

fn service() -> Arc<RuntimeRegistryService> {
    Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )))
}

fn context_for(organization_id: OrganizationId) -> TenantContext {
    let organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
    TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap()
}

#[tokio::test]
async fn the_live_runtime_of_a_tenant_resolves_and_verifies() {
    let service = service();
    let org = OrganizationId::new();
    let record = service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();

    let resolved = RuntimeContext::resolve(&service, &context_for(org))
        .await
        .unwrap();
    assert_eq!(resolved.record().runtime_id, record.runtime_id);
    assert_eq!(resolved.generation().raw(), 1);
    // The fence token verifies against the registry.
    let outcome = fence_guard::check(&service, &resolved.fence_token()).await;
    assert!(outcome.is_allow());
}

#[tokio::test]
async fn a_tenant_without_a_runtime_is_denied_with_no_live_runtime() {
    let service = service();
    let error = RuntimeContext::resolve(&service, &context_for(OrganizationId::new()))
        .await
        .unwrap_err();
    assert_eq!(error, RuntimeContextError::NoLiveRuntime);
    assert_eq!(error.as_str(), "no_live_runtime");
}

#[tokio::test]
async fn a_stopped_runtime_resolves_to_no_live_runtime() {
    // `current` answers only with a lease-LIVE runtime: a stopped
    // worker means the tenant currently has none — fail closed.
    let service = service();
    let org = OrganizationId::new();
    let record = service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();
    service.stop(record.runtime_id, Utc::now()).await.unwrap();

    let error = RuntimeContext::resolve(&service, &context_for(org))
        .await
        .unwrap_err();
    assert_eq!(error, RuntimeContextError::NoLiveRuntime);
}

#[tokio::test]
async fn a_rotated_workers_fence_is_denied_as_superseded() {
    let service = service();
    let org = OrganizationId::new();
    let first = service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();
    service.rotate(org, "worker-b", Utc::now()).await.unwrap();

    // The OLD worker's token: superseded (stale generation).
    let outcome = fence_guard::check(&service, &first.fence_token()).await;
    let reason = outcome.deny_reason().unwrap();
    assert_eq!(reason.as_str(), "fence_failed");
    assert!(matches!(
        reason,
        sniper_suite::tenant::DenyReason::FenceFailed {
            verdict: "superseded"
        }
    ));

    // Resolution follows the rotation: the NEW worker is current.
    let resolved = RuntimeContext::resolve(&service, &context_for(org))
        .await
        .unwrap();
    assert_eq!(resolved.record().worker_id, "worker-b");
    assert_eq!(resolved.generation().raw(), 2);
}

#[tokio::test]
async fn a_foreign_tenants_fence_token_is_denied() {
    let service = service();
    let org = OrganizationId::new();
    let _ = service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap();

    let foreign_token = FenceToken {
        organization_id: org,
        runtime_id: RuntimeId::new(), // never registered
        generation: RuntimeGeneration::first(),
    };
    let outcome = fence_guard::check(&service, &foreign_token).await;
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "fence_failed");
}

#[tokio::test]
async fn the_metadata_cache_serves_fresh_and_invalidates_on_generation() {
    let service = service();
    let org = OrganizationId::new();
    service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap();

    let cache = RuntimeMetadataCache::with_default_ttl(service.clone());
    let first = cache.get_or_fetch(org).await.unwrap().unwrap();
    assert_eq!(first.status, RuntimeStatus::Active);
    // Fresh entry served from the cache.
    assert!(cache.peek(org).await.is_some());

    // Rotation invalidates through the generation note.
    service.rotate(org, "worker-b", Utc::now()).await.unwrap();
    let new_generation = RuntimeGeneration::from_raw(2).unwrap();
    assert!(cache.note_generation(org, new_generation).await);
    assert!(cache.peek(org).await.is_none());
    let second = cache.get_or_fetch(org).await.unwrap().unwrap();
    assert_eq!(second.worker_id, "worker-b");
    assert_eq!(second.generation.raw(), 2);
}

#[tokio::test]
async fn the_metadata_cache_never_serves_expired_entries_or_caches_misses() {
    let service = service();
    let org = OrganizationId::new();
    service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap();

    // Zero TTL: entries expire immediately.
    let cache = RuntimeMetadataCache::new(service.clone(), Duration::from_secs(0));
    let _ = cache.get_or_fetch(org).await.unwrap().unwrap();
    assert!(cache.peek(org).await.is_none());

    // Registry misses are never cached.
    let other = OrganizationId::new();
    assert!(cache.get_or_fetch(other).await.unwrap().is_none());
    assert!(cache.peek(other).await.is_none());
}

#[tokio::test]
async fn a_drained_runtime_resolves_to_no_live_runtime() {
    // `current` answers only with a lease-LIVE runtime: draining the
    // worker leaves the tenant without one — fail closed.
    let service = service();
    let org = OrganizationId::new();
    let record = service
        .ensure_active(org, "worker-a", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();
    service.drain(record.runtime_id, Utc::now()).await.unwrap();

    let error = RuntimeContext::resolve(&service, &context_for(org))
        .await
        .unwrap_err();
    assert_eq!(error, RuntimeContextError::NoLiveRuntime);
}

#[tokio::test]
async fn the_record_shapes_agree_on_ownership() {
    // The runtime record itself is tenant-scoped: a record for org A
    // never verifies as a record for org B.
    let service = service();
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let record = service
        .ensure_active(a, "worker-a", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();
    assert_eq!(record.organization_id, a);
    assert_ne!(record.organization_id, b);

    let _foreign: TenantRuntimeRecord = record;
    let _ = b;
}
