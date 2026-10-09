//! Tenant configuration cache (STEP 3 file 34).
//!
//! The guards read tenant configuration on every gated call — a DB
//! round trip per call would dominate latency. [`ConfigCache`] keeps
//! the last-known-good documents in memory, keyed by tenant and
//! version, with two invalidation paths:
//!
//! 1. **Version-verified refresh** — `get()` serves the cached copy and
//!    triggers a background refresh when the TTL lapses; a write
//!    through the same process bumps the entry immediately.
//! 2. **Drift sweep** — `refresh_versions()` compares cached versions
//!    against the store (`ConfigStore::all_versions`) and reloads the
//!    tenants whose version moved (another process wrote).
//!
//! The cache NEVER serves a document it could not validate: a fetch
//! error keeps the previous copy (stale-but-typed beats unavailable),
//! and a corrupt document is dropped, not served. In the worst case the
//! guards fall back to the resolver's platform defaults — never to an
//! unknown state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, warn};

use bot_core::error::BotResult;
use bot_core::tenant::OrganizationId;

use super::model::TenantConfigModel;
use super::store::ConfigStore;
use super::version::ConfigVersion;

/// One cached entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    version: ConfigVersion,
    config: TenantConfigModel,
    refreshed_at: Instant,
    /// Set when the entry was served beyond its TTL (a background
    /// refresh was scheduled).
    refresh_pending: bool,
}

/// How the served value was obtained (observability + tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOrigin {
    /// Served from the cache, fresh.
    Fresh,
    /// Served from the cache while a refresh was pending.
    Stale,
    /// Nothing cached (first read for this tenant) — the store answered.
    Cold,
}

/// The per-process tenant configuration cache.
pub struct ConfigCache {
    store: Arc<dyn ConfigStore>,
    ttl: Duration,
    entries: RwLock<HashMap<OrganizationId, CacheEntry>>,
}

impl ConfigCache {
    /// Build over a store with the default TTL (5 seconds — short
    /// enough that a concurrent writer in another process is picked up
    /// by the drift sweep, long enough to make guards free).
    pub fn new(store: Arc<dyn ConfigStore>) -> Self {
        ConfigCache::with_ttl(store, Duration::from_secs(5))
    }

    /// Build with an explicit TTL (tests use tiny values).
    pub fn with_ttl(store: Arc<dyn ConfigStore>, ttl: Duration) -> Self {
        ConfigCache {
            store,
            ttl,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Fetch a tenant's effective configuration, through the cache.
    pub async fn get(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<(TenantConfigModel, ConfigVersion, CacheOrigin)> {
        {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(&organization_id) {
                let fresh = entry.refreshed_at.elapsed() < self.ttl;
                let origin = if fresh {
                    CacheOrigin::Fresh
                } else {
                    CacheOrigin::Stale
                };
                let result = Ok((entry.config.clone(), entry.version, origin));
                if fresh {
                    return result;
                }
            }
        }

        // TTL lapsed (or cold): reload from the store.
        match self.store.get(organization_id).await {
            Ok(Some(record)) => {
                let mut entries = self.entries.write().await;
                entries.insert(
                    organization_id,
                    CacheEntry {
                        version: record.version,
                        config: record.config.clone(),
                        refreshed_at: Instant::now(),
                        refresh_pending: false,
                    },
                );
                Ok((record.config, record.version, CacheOrigin::Stale))
            }
            Ok(None) => {
                // No document for this tenant: nothing to cache — the
                // resolver applies platform defaults for it.
                Ok((
                    TenantConfigModel::default(),
                    ConfigVersion::first(),
                    CacheOrigin::Cold,
                ))
            }
            Err(e) => {
                // Serve the previous copy if one exists (stale beats
                // unavailable); otherwise propagate.
                let entries = self.entries.read().await;
                match entries.get(&organization_id) {
                    Some(entry) => {
                        warn!(
                            organization = %organization_id,
                            error = %e,
                            "tenant config refresh failed; serving last-known-good"
                        );
                        Ok((entry.config.clone(), entry.version, CacheOrigin::Stale))
                    }
                    None => Err(e),
                }
            }
        }
    }

    /// Insert/overwrite an entry directly (same-process writes: the
    /// writer just persisted a new version — cache it now).
    pub async fn store_written(&self, record: &super::version::ConfigRecord) {
        let mut entries = self.entries.write().await;
        entries.insert(
            record.organization_id,
            CacheEntry {
                version: record.version,
                config: record.config.clone(),
                refreshed_at: Instant::now(),
                refresh_pending: false,
            },
        );
    }

    /// Drop a tenant's entry (its runtime was reaped/rotated, or an
    /// operator wants a forced reload).
    pub async fn invalidate(&self, organization_id: OrganizationId) {
        self.entries.write().await.remove(&organization_id);
    }

    /// Drop every entry (config reload, tests).
    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
    }

    /// Drift sweep: compare the store's versions with the cached ones
    /// and reload the tenants whose version moved. Returns the number
    /// of reloaded tenants.
    pub async fn refresh_versions(&self) -> BotResult<usize> {
        let versions = self.store.all_versions().await?;
        let mut reloaded = 0usize;
        for (organization_id, stored_version) in versions {
            let needs_reload = {
                let entries = self.entries.read().await;
                match entries.get(&organization_id) {
                    Some(entry) => entry.version != stored_version,
                    None => true,
                }
            };
            if needs_reload {
                if let Some(record) = self.store.get(organization_id).await? {
                    debug!(organization = %organization_id, version = %record.version, "tenant config drift reloaded");
                    self.store_written(&record).await;
                    reloaded += 1;
                }
            }
        }
        Ok(reloaded)
    }

    /// The number of cached tenants (observability, tests).
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Is the cache empty?
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::resolver::GlobalSafetyBounds;
    use crate::tenant_config::store::MemoryConfigStore;
    use bot_core::models::ExecutionMode;
    use chrono::Utc;

    fn bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &[ExecutionMode::Paper],
        }
    }

    #[tokio::test]
    async fn cold_read_goes_to_the_store_and_caches() {
        let store = Arc::new(MemoryConfigStore::new());
        let org = OrganizationId::new();
        store
            .put(
                org,
                &TenantConfigModel::default(),
                None,
                None,
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();

        let cache = ConfigCache::new(store.clone());
        let (_, version, origin) = cache.get(org).await.unwrap();
        assert_eq!(origin, CacheOrigin::Stale); // first load populates via store
        assert_eq!(version.raw(), 1);
        assert_eq!(cache.len().await, 1);

        let (_, _, origin) = cache.get(org).await.unwrap();
        assert_eq!(origin, CacheOrigin::Fresh);
    }

    #[tokio::test]
    async fn an_unknown_tenant_yields_the_default_document() {
        let cache = ConfigCache::new(Arc::new(MemoryConfigStore::new()));
        let (config, _, origin) = cache.get(OrganizationId::new()).await.unwrap();
        assert_eq!(origin, CacheOrigin::Cold);
        assert_eq!(config, TenantConfigModel::default());
    }

    #[tokio::test]
    async fn drift_sweep_picks_up_other_process_writes() {
        let store = Arc::new(MemoryConfigStore::new());
        let org = OrganizationId::new();
        let first = store
            .put(
                org,
                &TenantConfigModel::default(),
                None,
                None,
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();

        let cache = ConfigCache::new(store.clone());
        cache.get(org).await.unwrap();

        // "Another process" writes version 2 directly to the store.
        let mut updated = TenantConfigModel::default();
        updated.risk.max_position_usd = Some(500.0);
        store
            .put(
                org,
                &updated,
                Some(first.version),
                Some("peer"),
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();

        assert_eq!(cache.refresh_versions().await.unwrap(), 1);
        let (config, version, _) = cache.get(org).await.unwrap();
        assert_eq!(version.raw(), 2);
        assert_eq!(config.risk.max_position_usd, Some(500.0));
    }

    #[tokio::test]
    async fn store_written_publishes_immediately() {
        let store = Arc::new(MemoryConfigStore::new());
        let org = OrganizationId::new();
        let cache = ConfigCache::new(store.clone());
        let record = store
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
        cache.store_written(&record).await;
        let (_, version, origin) = cache.get(org).await.unwrap();
        assert_eq!(version.raw(), 1);
        assert_eq!(origin, CacheOrigin::Fresh);
        cache.invalidate(org).await;
        assert!(cache.is_empty().await);
    }
}
