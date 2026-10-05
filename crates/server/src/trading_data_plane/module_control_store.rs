//! Durable, tenant-scoped storage for module control overrides (§S-2).
//!
//! # Why this module exists
//!
//! The tenant kill-switch used to live in a process-global
//! `OnceLock<Mutex<HashMap<(OrganizationId, BotModule), _>>>`. Three
//! consequences, all of them bad for a system that moves money:
//!
//! * a tenant pressing "pause sniper" paused it on exactly ONE replica —
//!   every other replica kept trading;
//! * a process restart silently re-enabled every paused module;
//! * there was no record an auditor could read of who paused what, when.
//!
//! This module makes PostgreSQL the authority (table
//! `tenant_module_controls`, migration 0036) and keeps the in-process map
//! strictly as a **read-through cache** in front of it.
//!
//! # Degraded mode is explicit, not accidental
//!
//! When no database is attached (the memory-only operator/dev mode, and
//! the unit tests) the cache IS the store — byte-for-byte the previous
//! behaviour. [`ModuleControlStore::is_durable`] reports which mode is
//! active so a status surface can tell the truth instead of implying
//! durability it does not have.
//!
//! When a database IS attached but a read fails, the store returns an
//! explicit error and the status surface reports `effective_state: unknown`.
//! A stale replica cache must never be presented as the current kill-switch
//! state, because an unavailable authoritative read cannot prove that a
//! module is enabled or paused.
//!
//! A WRITE also fails CLOSED: if the durable write does not land, the caller
//! is told it did not land. A tenant must never be shown a successful control
//! action when it exists only in one replica's RAM.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bot_core::db::Database;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;
use chrono::{DateTime, Utc};
use sqlx::Row;

/// A tenant-level module control override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleControlOverride {
    /// The override state: `false` = tenant paused the module.
    pub enabled: bool,
    /// Why (for disables; empty for enables).
    pub reason: String,
    /// When the override was last written.
    pub updated_at: DateTime<Utc>,
    /// Non-secret actor label from the authenticated context.
    pub updated_by: String,
    /// Optimistic-concurrency token of the persisted row (`0` in the
    /// memory-only mode, which has no concurrent writers to lose).
    pub version: i64,
}

/// Why a durable control write could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlStoreError {
    /// The database rejected or could not serve the statement. The string
    /// is a sanitised description — it never carries row data.
    Backend(String),
}

impl std::fmt::Display for ControlStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlStoreError::Backend(detail) => {
                write!(f, "module control store unavailable: {detail}")
            }
        }
    }
}

impl std::error::Error for ControlStoreError {}

type CacheKey = (OrganizationId, BotModule);

/// Process-local read-through cache / memory-mode store.
fn cache() -> &'static Mutex<HashMap<CacheKey, ModuleControlOverride>> {
    static S: OnceLock<Mutex<HashMap<CacheKey, ModuleControlOverride>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(key: &CacheKey) -> Option<ModuleControlOverride> {
    cache().lock().ok()?.get(key).cloned()
}

fn cache_put(key: CacheKey, value: ModuleControlOverride) {
    if let Ok(mut map) = cache().lock() {
        map.insert(key, value);
    }
}

fn cache_remove(key: &CacheKey) -> bool {
    match cache().lock() {
        Ok(mut map) => map.remove(key).is_some(),
        Err(_) => false,
    }
}

/// Drop every cached entry. Used by tests, and after a restore so a stale
/// cache cannot outlive the data it mirrors.
pub fn reset_cache() {
    if let Ok(mut map) = cache().lock() {
        map.clear();
    }
}

/// The tenant module-control repository.
///
/// Construct one per [`crate::api::ApiState`]; it is cheap to clone (an
/// `Option<Arc<Database>>`) and holds no connection of its own.
#[derive(Clone, Default)]
pub struct ModuleControlStore {
    db: Option<Arc<Database>>,
}

impl std::fmt::Debug for ModuleControlStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleControlStore")
            .field("durable", &self.db.is_some())
            .finish()
    }
}

impl ModuleControlStore {
    /// Build a store over the attached database. `None` selects the
    /// memory-only mode (dev/tests) with the previous semantics.
    pub fn new(db: Option<Arc<Database>>) -> Self {
        ModuleControlStore { db }
    }

    /// Is PostgreSQL the authority for this store?
    ///
    /// `false` means module controls are process-local and do NOT survive
    /// a restart — a status surface must say so rather than imply
    /// durability.
    pub fn is_durable(&self) -> bool {
        self.db.is_some()
    }

    /// Read THIS organization's override for a module.
    ///
    /// `Ok(None)` is the honest "no tenant override": the effective state
    /// is then the entitlement-governed default. `Err` means the
    /// authoritative state could not be read and must not be replaced by a
    /// cached guess.
    pub async fn override_for(
        &self,
        org: OrganizationId,
        module: BotModule,
    ) -> Result<Option<ModuleControlOverride>, ControlStoreError> {
        let key = (org, module);
        let Some(db) = &self.db else {
            return Ok(cache_get(&key));
        };

        let row = sqlx::query(
            "SELECT enabled, reason, updated_at, updated_by, version
               FROM tenant_module_controls
              WHERE organization_id = $1 AND module = $2",
        )
        .bind(org.0)
        .bind(module.as_str())
        .fetch_optional(db.pool())
        .await
        .map_err(|error| {
            tracing::error!(
                error = %error,
                organization = %org,
                module = module.as_str(),
                "module control read failed; refusing to report a stale cached state"
            );
            ControlStoreError::Backend(error.to_string())
        })?;

        let Some(row) = row else {
            // Authoritative absence — drop any stale cached entry so a
            // cleared override cannot be resurrected by a later read.
            cache_remove(&key);
            return Ok(None);
        };

        let entry = ModuleControlOverride {
            enabled: row.try_get("enabled").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control enabled value: {error}"))
            })?,
            reason: row.try_get("reason").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control reason value: {error}"))
            })?,
            updated_at: row.try_get("updated_at").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control timestamp: {error}"))
            })?,
            updated_by: row.try_get("updated_by").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control actor value: {error}"))
            })?,
            version: row.try_get("version").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control version: {error}"))
            })?,
        };
        cache_put(key, entry.clone());
        Ok(Some(entry))
    }

    /// Record an override for THIS organization's module.
    ///
    /// Fails CLOSED: an `Err` means the override is NOT in effect
    /// anywhere, and the caller must not report success to the tenant.
    /// The `version` column is bumped on every write so a concurrent
    /// replica's update is visible rather than silently overwritten.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_override(
        &self,
        org: OrganizationId,
        module: BotModule,
        enabled: bool,
        reason: &str,
        updated_by: &str,
        correlation_id: &str,
        now: DateTime<Utc>,
    ) -> Result<ModuleControlOverride, ControlStoreError> {
        let key = (org, module);
        let Some(db) = &self.db else {
            let entry = ModuleControlOverride {
                enabled,
                reason: reason.to_string(),
                updated_at: now,
                updated_by: updated_by.to_string(),
                version: 0,
            };
            cache_put(key, entry.clone());
            return Ok(entry);
        };

        // Tenant-composite arbiter, exactly as 0026–0033 established for
        // every tenant-local upsert in this codebase.
        let row = sqlx::query(
            "INSERT INTO tenant_module_controls
                 (organization_id, module, enabled, reason, version,
                  updated_by, correlation_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 1, $5, $6, $7, $7)
             ON CONFLICT (organization_id, module) DO UPDATE
                SET enabled        = EXCLUDED.enabled,
                    reason         = EXCLUDED.reason,
                    updated_by     = EXCLUDED.updated_by,
                    correlation_id = EXCLUDED.correlation_id,
                    updated_at     = EXCLUDED.updated_at,
                    version        = tenant_module_controls.version + 1
             RETURNING enabled, reason, updated_at, updated_by, version",
        )
        .bind(org.0)
        .bind(module.as_str())
        .bind(enabled)
        .bind(reason)
        .bind(updated_by)
        .bind(correlation_id)
        .bind(now)
        .fetch_one(db.pool())
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                organization = %org,
                module = module.as_str(),
                "module control write failed; refusing to report an override that is not durable"
            );
            ControlStoreError::Backend(e.to_string())
        })?;

        let entry = ModuleControlOverride {
            enabled: row.try_get("enabled").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control enabled value: {error}"))
            })?,
            reason: row.try_get("reason").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control reason value: {error}"))
            })?,
            updated_at: row.try_get("updated_at").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control timestamp: {error}"))
            })?,
            updated_by: row.try_get("updated_by").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control actor value: {error}"))
            })?,
            version: row.try_get("version").map_err(|error| {
                ControlStoreError::Backend(format!("invalid module control version: {error}"))
            })?,
        };
        cache_put(key, entry.clone());
        Ok(entry)
    }

    /// Clear the override entirely — the module returns to its
    /// entitlement-governed default. Returns whether a row was removed.
    pub async fn clear_override(
        &self,
        org: OrganizationId,
        module: BotModule,
    ) -> Result<bool, ControlStoreError> {
        let key = (org, module);
        let Some(db) = &self.db else {
            return Ok(cache_remove(&key));
        };

        let result = sqlx::query(
            "DELETE FROM tenant_module_controls
              WHERE organization_id = $1 AND module = $2",
        )
        .bind(org.0)
        .bind(module.as_str())
        .execute(db.pool())
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                organization = %org,
                module = module.as_str(),
                "module control clear failed; the override is still in effect"
            );
            ControlStoreError::Backend(e.to_string())
        })?;

        cache_remove(&key);
        Ok(result.rows_affected() > 0)
    }

    /// Atomically set or clear the tenant emergency stop across every
    /// trading module. A kill switch is a tenant control, not a process-wide
    /// flag: the database transaction makes all module overrides visible
    /// together on every replica.
    pub async fn set_trading_kill_switch(
        &self,
        org: OrganizationId,
        active: bool,
        reason: &str,
        updated_by: &str,
        correlation_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), ControlStoreError> {
        let Some(db) = &self.db else {
            for module in BotModule::TRADING {
                let key = (org, module);
                if active {
                    cache_put(
                        key,
                        ModuleControlOverride {
                            enabled: false,
                            reason: reason.to_string(),
                            updated_at: now,
                            updated_by: updated_by.to_string(),
                            version: 0,
                        },
                    );
                } else {
                    cache_remove(&key);
                }
            }
            return Ok(());
        };

        let mut transaction = db.pool().begin().await.map_err(|error| {
            ControlStoreError::Backend(format!("kill switch transaction could not start: {error}"))
        })?;
        if active {
            for module in BotModule::TRADING {
                sqlx::query(
                    "INSERT INTO tenant_module_controls
                         (organization_id, module, enabled, reason, version,
                          updated_by, correlation_id, created_at, updated_at)
                     VALUES ($1, $2, false, $3, 1, $4, $5, $6, $6)
                     ON CONFLICT (organization_id, module) DO UPDATE
                        SET enabled = false,
                            reason = EXCLUDED.reason,
                            updated_by = EXCLUDED.updated_by,
                            correlation_id = EXCLUDED.correlation_id,
                            updated_at = EXCLUDED.updated_at,
                            version = tenant_module_controls.version + 1",
                )
                .bind(org.0)
                .bind(module.as_str())
                .bind(reason)
                .bind(updated_by)
                .bind(correlation_id)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(|error| {
                    ControlStoreError::Backend(format!(
                        "kill switch disable could not be persisted: {error}"
                    ))
                })?;
            }
        } else {
            sqlx::query(
                "DELETE FROM tenant_module_controls WHERE organization_id = $1 AND module = ANY($2)",
            )
            .bind(org.0)
            .bind(BotModule::TRADING.iter().map(|module| module.as_str().to_string()).collect::<Vec<_>>())
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                ControlStoreError::Backend(format!("kill switch resume could not be persisted: {error}"))
            })?;
        }
        transaction.commit().await.map_err(|error| {
            ControlStoreError::Backend(format!("kill switch transaction could not commit: {error}"))
        })?;
        for module in BotModule::TRADING {
            if active {
                cache_put(
                    (org, module),
                    ModuleControlOverride {
                        enabled: false,
                        reason: reason.to_string(),
                        updated_at: now,
                        updated_by: updated_by.to_string(),
                        version: 0,
                    },
                );
            } else {
                cache_remove(&(org, module));
            }
        }
        Ok(())
    }

    /// The effective module state from the tenant's perspective: a
    /// disable override wins; everything else is the entitlement default
    /// (which the authorization chain has already granted).
    pub async fn effective_state(
        &self,
        org: OrganizationId,
        module: BotModule,
    ) -> Result<(&'static str, Option<ModuleControlOverride>), ControlStoreError> {
        match self.override_for(org, module).await? {
            Some(o) if !o.enabled => Ok(("disabled", Some(o))),
            Some(o) => Ok(("enabled", Some(o))),
            None => Ok(("enabled", None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ModuleControlStore {
        reset_cache();
        ModuleControlStore::new(None)
    }

    /// Memory mode keeps the pre-0036 semantics exactly, and reports
    /// honestly that it is not durable.
    #[tokio::test]
    async fn memory_mode_round_trips_and_admits_it_is_not_durable() {
        let s = store();
        let org = OrganizationId::new();
        assert!(!s.is_durable());

        assert_eq!(
            s.effective_state(org, BotModule::Sniper)
                .await
                .expect("read")
                .0,
            "enabled"
        );
        s.apply_override(
            org,
            BotModule::Sniper,
            false,
            "paused",
            "user:a",
            "corr-1",
            Utc::now(),
        )
        .await
        .expect("memory write");
        let (state, entry) = s
            .effective_state(org, BotModule::Sniper)
            .await
            .expect("read");
        assert_eq!(state, "disabled");
        assert_eq!(entry.expect("entry").reason, "paused");

        assert!(s
            .clear_override(org, BotModule::Sniper)
            .await
            .expect("clear"));
        assert_eq!(
            s.effective_state(org, BotModule::Sniper)
                .await
                .expect("read")
                .0,
            "enabled"
        );
    }

    /// One tenant's pause must never be visible to another tenant, and
    /// must not leak across modules.
    #[tokio::test]
    async fn overrides_are_tenant_and_module_scoped() {
        let s = store();
        let a = OrganizationId::new();
        let b = OrganizationId::new();

        s.apply_override(
            a,
            BotModule::Sniper,
            false,
            "paused",
            "user:a",
            "",
            Utc::now(),
        )
        .await
        .expect("write");

        assert!(s
            .override_for(b, BotModule::Sniper)
            .await
            .expect("read")
            .is_none());
        assert_eq!(
            s.effective_state(b, BotModule::Sniper)
                .await
                .expect("read")
                .0,
            "enabled"
        );
        assert_eq!(
            s.effective_state(a, BotModule::Copy).await.expect("read").0,
            "enabled"
        );
        assert_eq!(
            s.effective_state(a, BotModule::Sniper)
                .await
                .expect("read")
                .0,
            "disabled"
        );
    }
}
