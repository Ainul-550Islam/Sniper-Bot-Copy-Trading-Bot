//! Tenant configuration cross-organization isolation tests
//! (tenant-isolation file 72).
//!
//! Positive: a tenant writes and reads back its OWN configuration
//! (optimistic-concurrency versioning along the way).
//!
//! Negative: tenant B never reads tenant A's document (the store
//! answers per organization); the cache never serves A's entry to B
//! (and a tenant with no document gets the platform default, never
//! another tenant's config); A's updates never change what B
//! resolves; and a stale-version write by A is refused without
//! touching B.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use sniper_suite::tenant_config::cache::{CacheOrigin, ConfigCache};
use sniper_suite::tenant_config::store::ConfigStore;
use sniper_suite::tenant_config::{GlobalSafetyBounds, MemoryConfigStore, TenantConfigModel};

use bot_core::models::ExecutionMode;
use bot_core::tenant::OrganizationId;

const MODES: [ExecutionMode; 2] = [ExecutionMode::Paper, ExecutionMode::Live];

fn bounds() -> GlobalSafetyBounds {
    GlobalSafetyBounds {
        max_position_usd: 100_000.0,
        daily_loss_usd_cap: 50_000.0,
        max_slippage_bps: 2_000,
        allowed_modes: &MODES,
    }
}

fn config_with_mode(mode: ExecutionMode) -> TenantConfigModel {
    TenantConfigModel::deployment_legacy(&[mode])
}

#[tokio::test]
async fn a_tenant_writes_and_reads_back_its_own_config() {
    let store = Arc::new(MemoryConfigStore::new());
    let a = OrganizationId::new();

    store
        .put(
            a,
            &config_with_mode(ExecutionMode::Paper),
            None,
            Some("a"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    let record = store.get(a).await.unwrap().expect("A's document exists");
    assert_eq!(record.organization_id, a);
    assert_eq!(record.version.raw(), 1);
    assert_eq!(record.updated_by.as_deref(), Some("a"));
}

#[tokio::test]
async fn another_tenant_never_reads_a_written_config() {
    let store = Arc::new(MemoryConfigStore::new());
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    store
        .put(
            a,
            &config_with_mode(ExecutionMode::Paper),
            None,
            Some("a"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    // B has no document: A's config is invisible to B.
    assert!(store.get(b).await.unwrap().is_none());
    assert!(store.get(a).await.unwrap().is_some());
}

#[tokio::test]
async fn the_cache_never_serves_one_tenants_entry_to_another() {
    let store = Arc::new(MemoryConfigStore::new());
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    store
        .put(
            a,
            &config_with_mode(ExecutionMode::Live),
            None,
            Some("a"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();

    let cache = ConfigCache::new(store);
    let (a_config, a_version, _) = cache.get(a).await.unwrap();
    assert_eq!(a_version.raw(), 1);
    assert!(a_config.allowed_modes.contains(&ExecutionMode::Live));

    // B resolves its OWN (empty) state: the platform default — never
    // A's document.
    let (b_config, b_version, b_origin) = cache.get(b).await.unwrap();
    assert_eq!(b_version.raw(), 1, "no document: the default at version 1");
    assert!(matches!(b_origin, CacheOrigin::Cold));
    assert!(!b_config.allowed_modes.contains(&ExecutionMode::Live));
    // The cache holds A's entry and B's default separately.
    assert_eq!(cache.len().await, 1, "only A has a cached document");
}

#[tokio::test]
async fn a_config_update_never_changes_what_another_tenant_resolves() {
    let store = Arc::new(MemoryConfigStore::new());
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    // Both tenants start with paper-only documents.
    for org in [a, b] {
        store
            .put(
                org,
                &config_with_mode(ExecutionMode::Paper),
                None,
                Some("seed"),
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();
    }

    let cache = ConfigCache::with_ttl(store.clone(), Duration::from_secs(300));
    let (a_before, _, _) = cache.get(a).await.unwrap();
    let (b_before, _, _) = cache.get(b).await.unwrap();
    assert!(!a_before.allowed_modes.contains(&ExecutionMode::Live));
    assert!(!b_before.allowed_modes.contains(&ExecutionMode::Live));

    // A widens its own document to live through the SAME store the
    // cache reads (Arc-shared), with optimistic concurrency.
    let current = store.get(a).await.unwrap().unwrap();
    store
        .put(
            a,
            &config_with_mode(ExecutionMode::Live),
            Some(current.version),
            Some("a"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();

    // B invalidates/refreshes: B's resolution is unchanged (paper
    // only), A's now allows live.
    cache.invalidate(a).await;
    cache.invalidate(b).await;
    let (a_after, a_version_after, _) = cache.get(a).await.unwrap();
    assert!(a_after.allowed_modes.contains(&ExecutionMode::Live));
    assert_eq!(a_version_after.raw(), 2);
    let (b_after, b_version_after, _) = cache.get(b).await.unwrap();
    assert!(!b_after.allowed_modes.contains(&ExecutionMode::Live));
    assert_eq!(b_version_after.raw(), 1);
}

#[tokio::test]
async fn a_stale_version_write_is_refused_without_side_effects() {
    let store = Arc::new(MemoryConfigStore::new());
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    store
        .put(
            a,
            &config_with_mode(ExecutionMode::Paper),
            None,
            Some("a1"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();
    store
        .put(
            b,
            &config_with_mode(ExecutionMode::Paper),
            None,
            Some("b1"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap();

    // A's document is at version 1; write with the WRONG expected
    // version: refused.
    let stale = sniper_suite::tenant_config::version::ConfigVersion::from_raw(7).unwrap();
    let error = store
        .put(
            a,
            &config_with_mode(ExecutionMode::Live),
            Some(stale),
            Some("a2"),
            Utc::now(),
            &bounds(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        sniper_suite::tenant_config::store::ConfigWriteError::StaleVersion { .. }
    ));

    // Neither tenant's document moved.
    assert_eq!(store.get(a).await.unwrap().unwrap().version.raw(), 1);
    assert_eq!(store.get(b).await.unwrap().unwrap().version.raw(), 1);
}

#[tokio::test]
async fn an_invalid_document_is_refused_for_any_tenant() {
    let store = Arc::new(MemoryConfigStore::new());
    let a = OrganizationId::new();
    // A tenant risk limit that EXCEEDS the platform bound is invalid:
    // refused at persist, not silently clamped (tenants only narrow).
    let mut invalid = TenantConfigModel::deployment_legacy(&[ExecutionMode::Paper]);
    invalid.risk.max_position_usd = Some(1_000_000_000.0);
    let error = store
        .put(a, &invalid, None, Some("a"), Utc::now(), &bounds())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        sniper_suite::tenant_config::store::ConfigWriteError::Invalid(_)
    ));
    // Nothing was persisted.
    assert!(store.get(a).await.unwrap().is_none());
}
