//! Tenant configuration storage (STEP 3 file 33).
//!
//! `tenant_configs` (migration 0025): one row per organization, the
//! typed document as jsonb, a monotonically increasing version and
//! compare-and-swap updates. Writes go through validation; reads are
//! used by the cache and the guards.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::warn;

use bot_core::db::Database;
use bot_core::error::{BotError, BotResult};
use bot_core::tenant::OrganizationId;

use super::model::TenantConfigModel;
use super::resolver::GlobalSafetyBounds;
use super::validator::validate_or_issues;
use super::version::{ConfigRecord, ConfigVersion};

/// The durable contract for tenant configuration storage.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Fetch a tenant's current document.
    async fn get(&self, organization_id: OrganizationId) -> BotResult<Option<ConfigRecord>>;

    /// Insert or update a tenant's document.
    ///
    /// * No existing row → insert at version 1.
    /// * Existing row → `expected` must match the stored version, else
    ///   [`ConfigWriteError::StaleVersion`] (optimistic concurrency).
    /// The candidate is validated against `bounds` first; an invalid
    /// document is [`ConfigWriteError::Invalid`].
    async fn put(
        &self,
        organization_id: OrganizationId,
        candidate: &TenantConfigModel,
        expected: Option<ConfigVersion>,
        updated_by: Option<&str>,
        now: DateTime<Utc>,
        bounds: &GlobalSafetyBounds,
    ) -> Result<ConfigRecord, ConfigWriteError>;

    /// Every stored document version, oldest tenant first (cache warmup
    /// and drift checks).
    async fn all_versions(&self) -> BotResult<Vec<(OrganizationId, ConfigVersion)>>;
}

/// Why a configuration write was refused.
#[derive(Debug)]
pub enum ConfigWriteError {
    /// The document violates platform bounds (the issues, verbatim).
    Invalid(Vec<super::validator::ConfigIssue>),
    /// Another writer got there first: the version the caller expected
    /// and the version actually stored.
    StaleVersion { expected: u64, stored: u64 },
    /// Storage failure.
    Storage(BotError),
}

impl std::fmt::Display for ConfigWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigWriteError::Invalid(issues) => write!(
                f,
                "tenant configuration rejected: {}",
                issues
                    .iter()
                    .map(|i| i.detail())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            ConfigWriteError::StaleVersion { expected, stored } => write!(
                f,
                "tenant configuration was modified concurrently (expected v{expected}, stored v{stored})"
            ),
            ConfigWriteError::Storage(e) => write!(f, "tenant configuration storage error: {e}"),
        }
    }
}

impl PartialEq for ConfigWriteError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ConfigWriteError::Invalid(a), ConfigWriteError::Invalid(b)) => a == b,
            (
                ConfigWriteError::StaleVersion {
                    expected: ea,
                    stored: sa,
                },
                ConfigWriteError::StaleVersion {
                    expected: eb,
                    stored: sb,
                },
            ) => ea == eb && sa == sb,
            (ConfigWriteError::Storage(a), ConfigWriteError::Storage(b)) => {
                a.to_string() == b.to_string()
            }
            _ => false,
        }
    }
}

impl From<ConfigWriteError> for BotError {
    fn from(e: ConfigWriteError) -> Self {
        match e {
            ConfigWriteError::Storage(inner) => inner,
            other => BotError::db(other.to_string()),
        }
    }
}

/// PostgreSQL implementation over `tenant_configs` (migration 0025).
pub struct PgConfigStore {
    db: Arc<Database>,
}

impl PgConfigStore {
    /// Bind to the shared database handle.
    pub fn new(db: Arc<Database>) -> Self {
        PgConfigStore { db }
    }

    fn map_row(row: &sqlx::postgres::PgRow) -> BotResult<ConfigRecord> {
        let organization_id = OrganizationId::from(
            row.try_get::<uuid::Uuid, _>("organization_id")
                .unwrap_or_default(),
        );
        let version = ConfigVersion::from_raw(
            u64::try_from(row.try_get::<i64, _>("version").unwrap_or(1)).unwrap_or(1),
        )
        .unwrap_or_else(ConfigVersion::first);
        let document: serde_json::Value = row
            .try_get("config")
            .map_err(|e| BotError::db(format!("tenant config column unreadable: {e}")))?;
        let config = serde_json::from_value(document)
            .map_err(|e| BotError::db(format!("tenant config document is corrupt: {e}")))?;
        Ok(ConfigRecord {
            organization_id,
            version,
            config,
            updated_by: row.try_get("updated_by").ok().flatten(),
            updated_at: row.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
        })
    }
}

#[async_trait]
impl ConfigStore for PgConfigStore {
    async fn get(&self, organization_id: OrganizationId) -> BotResult<Option<ConfigRecord>> {
        let row = sqlx::query(
            "SELECT organization_id, version, config, updated_by, updated_at \
             FROM tenant_configs WHERE organization_id = $1",
        )
        .bind(organization_id.as_uuid())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("tenant config fetch failed: {e}")))?;
        match row {
            Some(row) => Ok(Some(Self::map_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn put(
        &self,
        organization_id: OrganizationId,
        candidate: &TenantConfigModel,
        expected: Option<ConfigVersion>,
        updated_by: Option<&str>,
        now: DateTime<Utc>,
        bounds: &GlobalSafetyBounds,
    ) -> Result<ConfigRecord, ConfigWriteError> {
        if let Err(issues) = validate_or_issues(candidate, bounds) {
            return Err(ConfigWriteError::Invalid(issues));
        }

        let document = serde_json::to_value(candidate).map_err(|e| {
            ConfigWriteError::Storage(BotError::db(format!("config serialization failed: {e}")))
        })?;

        // Upsert with a compare-and-swap guard on the version: a first
        // write lands at v1; an update must name the stored version.
        let inserted = sqlx::query(
            "INSERT INTO tenant_configs (organization_id, version, config, updated_by, updated_at) \
             VALUES ($1, 1, $2, $3, $4) \
             ON CONFLICT (organization_id) DO UPDATE \
                SET version = tenant_configs.version + 1, \
                    config = EXCLUDED.config, \
                    updated_by = EXCLUDED.updated_by, \
                    updated_at = EXCLUDED.updated_at \
              WHERE tenant_configs.version = COALESCE($5, 1) \
             RETURNING version, updated_at",
        )
        .bind(organization_id.as_uuid())
        .bind(document)
        .bind(updated_by.map(str::to_string))
        .bind(now)
        .bind(expected.map(|v| v.raw() as i64))
        .fetch_optional(self.db.pool())
        .await;

        match inserted {
            Ok(Some(row)) => Ok(ConfigRecord {
                organization_id,
                version: ConfigVersion::from_raw(
                    u64::try_from(row.try_get::<i64, _>("version").unwrap_or(1)).unwrap_or(1),
                )
                .unwrap_or_else(ConfigVersion::first),
                config: candidate.clone(),
                updated_by: updated_by.map(str::to_string),
                updated_at: row.try_get("updated_at").unwrap_or(now),
            }),
            Ok(None) => {
                // The CAS guard rejected the write: a concurrent writer
                // moved the version. Report what is actually stored.
                let current = self
                    .get(organization_id)
                    .await
                    .map_err(ConfigWriteError::Storage)?;
                Err(match (expected, current) {
                    (Some(expected), Some(current)) => ConfigWriteError::StaleVersion {
                        expected: expected.raw(),
                        stored: current.version.raw(),
                    },
                    (_, Some(current)) => ConfigWriteError::StaleVersion {
                        expected: 1,
                        stored: current.version.raw(),
                    },
                    (expected, None) => ConfigWriteError::StaleVersion {
                        expected: expected.map(|v| v.raw()).unwrap_or(1),
                        stored: 1,
                    },
                })
            }
            Err(e) => {
                warn!(organization = %organization_id, error = %e, "tenant config write failed");
                Err(ConfigWriteError::Storage(BotError::db(format!(
                    "tenant config write failed: {e}"
                ))))
            }
        }
    }

    async fn all_versions(&self) -> BotResult<Vec<(OrganizationId, ConfigVersion)>> {
        let rows = sqlx::query(
            "SELECT organization_id, version FROM tenant_configs ORDER BY organization_id",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("tenant config version scan failed: {e}")))?;
        Ok(rows
            .iter()
            .map(|row| {
                (
                    OrganizationId::from(
                        row.try_get::<uuid::Uuid, _>("organization_id")
                            .unwrap_or_default(),
                    ),
                    ConfigVersion::from_raw(
                        u64::try_from(row.try_get::<i64, _>("version").unwrap_or(1)).unwrap_or(1),
                    )
                    .unwrap_or_else(ConfigVersion::first),
                )
            })
            .collect())
    }
}

/// In-memory store (tests and detached deployments).
#[derive(Default)]
pub struct MemoryConfigStore {
    docs: tokio::sync::RwLock<HashMap<OrganizationId, ConfigRecord>>,
}

impl MemoryConfigStore {
    /// An empty store.
    pub fn new() -> Self {
        MemoryConfigStore::default()
    }
}

#[async_trait]
impl ConfigStore for MemoryConfigStore {
    async fn get(&self, organization_id: OrganizationId) -> BotResult<Option<ConfigRecord>> {
        Ok(self.docs.read().await.get(&organization_id).cloned())
    }

    async fn put(
        &self,
        organization_id: OrganizationId,
        candidate: &TenantConfigModel,
        expected: Option<ConfigVersion>,
        updated_by: Option<&str>,
        now: DateTime<Utc>,
        bounds: &GlobalSafetyBounds,
    ) -> Result<ConfigRecord, ConfigWriteError> {
        if let Err(issues) = validate_or_issues(candidate, bounds) {
            return Err(ConfigWriteError::Invalid(issues));
        }
        let mut docs = self.docs.write().await;
        match docs.get(&organization_id) {
            Some(existing) => {
                let expected = expected.unwrap_or_else(ConfigVersion::first);
                if existing.version != expected {
                    return Err(ConfigWriteError::StaleVersion {
                        expected: expected.raw(),
                        stored: existing.version.raw(),
                    });
                }
                let record =
                    existing.successor(candidate.clone(), updated_by.map(str::to_string), now);
                docs.insert(organization_id, record.clone());
                Ok(record)
            }
            None => {
                let record = ConfigRecord::initial(
                    organization_id,
                    candidate.clone(),
                    updated_by.map(str::to_string),
                    now,
                );
                docs.insert(organization_id, record.clone());
                Ok(record)
            }
        }
    }

    async fn all_versions(&self) -> BotResult<Vec<(OrganizationId, ConfigVersion)>> {
        let docs = self.docs.read().await;
        let mut out: Vec<_> = docs
            .iter()
            .map(|(org, record)| (*org, record.version))
            .collect();
        out.sort_by_key(|(org, _)| *org);
        Ok(out)
    }
}

/// Share a store.
pub fn shared(store: impl ConfigStore + 'static) -> Arc<dyn ConfigStore> {
    Arc::new(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::ExecutionMode;

    fn bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &[ExecutionMode::Paper],
        }
    }

    #[tokio::test]
    async fn first_write_lands_at_version_one() {
        let store = MemoryConfigStore::new();
        let org = OrganizationId::new();
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
        assert_eq!(record.version.raw(), 1);
        assert_eq!(store.get(org).await.unwrap().unwrap().version.raw(), 1);
    }

    #[tokio::test]
    async fn updates_cas_on_the_expected_version() {
        let store = MemoryConfigStore::new();
        let org = OrganizationId::new();
        let first = store
            .put(
                org,
                &TenantConfigModel::default(),
                None,
                Some("a"),
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();

        let second = store
            .put(
                org,
                &TenantConfigModel::default(),
                Some(first.version),
                Some("b"),
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();
        assert_eq!(second.version.raw(), 2);

        // Re-playing the first writer's CAS must now fail.
        let stale = store
            .put(
                org,
                &TenantConfigModel::default(),
                Some(first.version),
                Some("a-again"),
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            stale,
            ConfigWriteError::StaleVersion {
                expected: 1,
                stored: 2
            }
        );
    }

    #[tokio::test]
    async fn invalid_documents_are_refused_before_persist() {
        let store = MemoryConfigStore::new();
        let org = OrganizationId::new();
        let mut bad = TenantConfigModel::default();
        bad.risk.max_position_usd = Some(999_999.0);
        let err = store
            .put(org, &bad, None, Some("op"), Utc::now(), &bounds())
            .await
            .unwrap_err();
        assert!(matches!(err, ConfigWriteError::Invalid(_)));
        assert!(store.get(org).await.unwrap().is_none(), "nothing persisted");
    }

    #[tokio::test]
    async fn version_scan_lists_every_tenant() {
        let store = MemoryConfigStore::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        store
            .put(
                a,
                &TenantConfigModel::default(),
                None,
                None,
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();
        store
            .put(
                b,
                &TenantConfigModel::default(),
                None,
                None,
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();
        let versions = store.all_versions().await.unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions.iter().all(|(_, v)| v.raw() == 1));
    }
}
