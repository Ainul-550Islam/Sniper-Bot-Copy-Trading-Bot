//! Cached tenant runtime metadata (tenant-isolation file 20).
//!
//! [`RuntimeMetadataCache`] is a concurrency-safe cache of runtime
//! RECORDS (worker id, generation, status, timestamps) in front of the
//! runtime registry. It exists so hot paths (listing dashboards,
//! subscription scopes, logging fields) do not hit the store on every
//! read.
//!
//! Security model — read carefully:
//!
//! * the cache NEVER authorizes. Fencing and execution authorization
//!   always go to the registry ([`RuntimeRegistryService::verify`]);
//!   the cache is metadata only;
//! * every entry carries the runtime's generation and is invalidated
//!   the moment that generation is superseded
//!   ([`RuntimeMetadataCache::note_generation`]);
//! * entries expire on a TTL even if nobody rotated them (a runtime's
//!   lease can lapse without a rotation event reaching this process);
//! * `get_or_fetch` refreshes from the registry when the cache is
//!   empty, expired, or superseded.
//!
//! Fail-soft on reads (a cache miss falls through to the registry) and
//! fail-closed on authorization (which never reads the cache).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use bot_core::error::BotResult;
use bot_core::tenant::{OrganizationId, RuntimeGeneration};

use crate::runtime_registry::{RuntimeRegistryService, TenantRuntimeRecord};

/// One cached runtime record with its freshness bookkeeping.
#[derive(Debug, Clone)]
struct CachedRuntime {
    record: TenantRuntimeRecord,
    cached_at: Instant,
}

/// The default freshness window for cached metadata.
pub const DEFAULT_RUNTIME_CACHE_TTL: Duration = Duration::from_secs(15);

/// Concurrency-safe tenant runtime metadata cache.
pub struct RuntimeMetadataCache {
    service: Arc<RuntimeRegistryService>,
    ttl: Duration,
    entries: RwLock<HashMap<OrganizationId, CachedRuntime>>,
}

impl RuntimeMetadataCache {
    /// Build with a custom TTL.
    pub fn new(service: Arc<RuntimeRegistryService>, ttl: Duration) -> Self {
        RuntimeMetadataCache {
            service,
            ttl,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Build with the default TTL.
    pub fn with_default_ttl(service: Arc<RuntimeRegistryService>) -> Self {
        Self::new(service, DEFAULT_RUNTIME_CACHE_TTL)
    }

    /// The configured TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Read the cached record for a tenant WITHOUT touching the
    /// registry. Returns `None` when nothing is cached or the entry
    /// expired (an expired entry is dropped, not served).
    pub async fn peek(&self, organization_id: OrganizationId) -> Option<TenantRuntimeRecord> {
        let mut entries = self.entries.write().await;
        match entries.get(&organization_id) {
            Some(cached) if cached.cached_at.elapsed() < self.ttl => Some(cached.record.clone()),
            Some(_) => {
                entries.remove(&organization_id);
                None
            }
            None => None,
        }
    }

    /// Read through: fresh cached record when available, otherwise
    /// fetch from the registry and cache it. A registry miss
    /// (`Ok(None)`) is NOT cached — there is no such thing as a cached
    /// "no runtime" answer; the next caller re-checks.
    pub async fn get_or_fetch(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<TenantRuntimeRecord>> {
        if let Some(record) = self.peek(organization_id).await {
            return Ok(Some(record));
        }
        let fetched = self.service.current(organization_id).await?;
        if let Some(record) = &fetched {
            let mut entries = self.entries.write().await;
            entries.insert(
                organization_id,
                CachedRuntime {
                    record: record.clone(),
                    cached_at: Instant::now(),
                },
            );
        }
        Ok(fetched)
    }

    /// The tenant's cached/known generation, when one is cached and
    /// fresh (a convenience for "did the generation move" checks).
    pub async fn known_generation(
        &self,
        organization_id: OrganizationId,
    ) -> Option<RuntimeGeneration> {
        self.peek(organization_id)
            .await
            .map(|record| record.generation)
    }

    /// Note a generation change for a tenant. If the cache holds an
    /// older generation, the entry is invalidated immediately — a
    /// superseded runtime must never be served as current metadata.
    /// Returns `true` when an entry was invalidated.
    pub async fn note_generation(
        &self,
        organization_id: OrganizationId,
        generation: RuntimeGeneration,
    ) -> bool {
        let mut entries = self.entries.write().await;
        if let Some(cached) = entries.get(&organization_id) {
            if cached.record.generation != generation {
                entries.remove(&organization_id);
                return true;
            }
        }
        false
    }

    /// Invalidate one tenant's entry unconditionally (rotation events,
    /// supervisor sweeps, manual operator action).
    pub async fn invalidate(&self, organization_id: OrganizationId) {
        self.entries.write().await.remove(&organization_id);
    }

    /// Drop every entry (configuration reload, shutdown).
    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
    }

    /// How many entries are currently cached (any age — for tests and
    /// observability, never for authorization).
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Whether the cache is empty.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::tenant::RuntimeGeneration;
    use chrono::Utc;

    fn service() -> Arc<RuntimeRegistryService> {
        Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )))
    }

    #[tokio::test]
    async fn a_fetch_caches_and_a_peek_serves() {
        let service = service();
        let org = bot_core::tenant::OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();

        let cache = RuntimeMetadataCache::with_default_ttl(service.clone());
        assert!(cache.is_empty().await);

        let fetched = cache.get_or_fetch(org).await.unwrap().unwrap();
        assert_eq!(fetched.worker_id, "worker-a");
        assert_eq!(cache.len().await, 1);

        // Second read is served from the cache.
        let peeked = cache.peek(org).await.unwrap();
        assert_eq!(peeked.runtime_id, fetched.runtime_id);
    }

    #[tokio::test]
    async fn a_registry_miss_is_never_cached() {
        let service = service();
        let cache = RuntimeMetadataCache::with_default_ttl(service);
        let org = bot_core::tenant::OrganizationId::new();
        assert!(cache.get_or_fetch(org).await.unwrap().is_none());
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn a_generation_change_invalidates_the_entry() {
        let service = service();
        let org = bot_core::tenant::OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();

        let cache = RuntimeMetadataCache::with_default_ttl(service.clone());
        let first = cache.get_or_fetch(org).await.unwrap().unwrap();

        // Rotate: the registry's current generation moves past the
        // cached one. Noting the new generation must invalidate.
        service.rotate(org, "worker-b", Utc::now()).await.unwrap();
        let new_generation = RuntimeGeneration::from_raw(2).unwrap();
        assert!(cache.note_generation(org, new_generation).await);
        assert!(cache.is_empty().await);

        // The same note again is a no-op (already invalidated).
        assert!(!cache.note_generation(org, new_generation).await);

        // A re-fetch observes the new worker.
        let second = cache.get_or_fetch(org).await.unwrap().unwrap();
        assert_eq!(second.worker_id, "worker-b");
        assert_ne!(first.generation, second.generation);
    }

    #[tokio::test]
    async fn an_expired_entry_is_dropped_not_served() {
        let service = service();
        let org = bot_core::tenant::OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();

        // A zero TTL: every entry is instantly expired.
        let cache = RuntimeMetadataCache::new(service.clone(), Duration::from_secs(0));
        let _ = cache.get_or_fetch(org).await.unwrap().unwrap();
        // The fetch itself cached the entry, but zero TTL means the
        // next peek must drop it and answer None.
        assert!(cache.peek(org).await.is_none());
        // Read-through still works.
        assert!(cache.get_or_fetch(org).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn invalidate_all_clears_every_tenant() {
        let service = service();
        let cache = RuntimeMetadataCache::with_default_ttl(service.clone());
        for i in 0..3 {
            let org = bot_core::tenant::OrganizationId::new();
            service
                .ensure_active(org, &format!("worker-{i}"), Utc::now())
                .await
                .unwrap();
            let _ = cache.get_or_fetch(org).await.unwrap().unwrap();
        }
        assert_eq!(cache.len().await, 3);
        cache.invalidate_all().await;
        assert!(cache.is_empty().await);
    }
}
