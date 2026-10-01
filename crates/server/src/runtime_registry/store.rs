//! Runtime DB store (STEP 3 file 39).
//!
//! The durable contract for the `tenant_runtimes` table (migration
//! 0025): register, look up, heartbeat, transition, rotate and reap.
//! Two implementations, the same discipline as the SaaS store:
//!
//! * [`PgRuntimeStore`] — PostgreSQL, used whenever the database is
//!   attached (the authoritative path in production);
//! * [`MemoryRuntimeStore`] — process-local, for single-process runs and
//!   tests.
//!
//! The split-brain invariant (at most one live runtime per tenant) is
//! enforced by the partial unique index `tenant_runtimes_one_live_per_org_idx`
//! on the PG side and by construction on the memory side; `rotate` is the
//! only sanctioned way to supersede a live runtime.

use std::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::warn;

use bot_core::db::Database;
use bot_core::error::{BotError, BotResult};
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

use super::model::{RuntimeStatus, TenantRuntimeRecord};

/// The durable runtime registry contract.
#[async_trait]
pub trait RuntimeStore: Send + Sync {
    /// Insert a new runtime row. Fails when it would create a second
    /// live runtime for the tenant (split brain).
    async fn insert(&self, record: &TenantRuntimeRecord) -> BotResult<()>;

    /// The current LIVE (provisioning/active) runtime of a tenant.
    async fn live_for(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<TenantRuntimeRecord>>;

    /// One runtime row by instance id.
    async fn get(&self, runtime_id: RuntimeId) -> BotResult<Option<TenantRuntimeRecord>>;

    /// Record a heartbeat for a runtime. `Ok(false)` = the runtime is no
    /// longer live (stopped/retired) — the caller must stop acting.
    async fn heartbeat(&self, runtime_id: RuntimeId, now: DateTime<Utc>) -> BotResult<bool>;

    /// Transition a runtime's lifecycle status. Terminal statuses stick
    /// (no resurrection: rotation mints a new row).
    async fn set_status(
        &self,
        runtime_id: RuntimeId,
        status: RuntimeStatus,
        now: DateTime<Utc>,
    ) -> BotResult<()>;

    /// The highest generation ever issued for a tenant, `None` when the
    /// tenant has no runtime history (use [`RuntimeStore::next_generation`]).
    async fn max_generation(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<RuntimeGeneration>>;

    /// Atomically supersede the tenant's live runtime (if any) with a new
    /// active row at generation+1, and return the new record. This is the
    /// ONLY sanctioned rotation path.
    async fn rotate(
        &self,
        organization_id: OrganizationId,
        new_runtime_id: RuntimeId,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> BotResult<TenantRuntimeRecord>;

    /// Live runtimes whose heartbeat went stale before `cutoff` (the reaping
    /// set). Callers mark them stopped via [`RuntimeStore::set_status`].
    async fn stale_since(&self, cutoff: DateTime<Utc>) -> BotResult<Vec<TenantRuntimeRecord>>;

    /// The next generation for a tenant (max+1; 1 for a first runtime).
    async fn next_generation(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<RuntimeGeneration> {
        let max = self.max_generation(organization_id).await?;
        Ok(match max {
            None => RuntimeGeneration::first(),
            // Overflow is unreachable in practice (u64 generations); the
            // fallback keeps the contract total rather than panicking.
            Some(max) => max.next().unwrap_or_else(RuntimeGeneration::first),
        })
    }
}

/// PostgreSQL implementation over `tenant_runtimes` (migration 0025).
pub struct PgRuntimeStore {
    db: std::sync::Arc<Database>,
}

impl PgRuntimeStore {
    /// Bind to the shared database handle.
    pub fn new(db: std::sync::Arc<Database>) -> Self {
        PgRuntimeStore { db }
    }

    fn map_row(row: &sqlx::postgres::PgRow) -> TenantRuntimeRecord {
        let status_raw: String = row.try_get("status").unwrap_or_else(|_| "stopped".into());
        TenantRuntimeRecord {
            runtime_id: RuntimeId::from(row.try_get::<uuid::Uuid, _>("id").unwrap_or_default()),
            organization_id: OrganizationId::from(
                row.try_get::<uuid::Uuid, _>("organization_id")
                    .unwrap_or_default(),
            ),
            generation: RuntimeGeneration::from_i64(
                row.try_get::<i64, _>("generation").unwrap_or(1),
            )
            .unwrap_or_else(RuntimeGeneration::first),
            status: RuntimeStatus::parse(&status_raw).unwrap_or(RuntimeStatus::Stopped),
            worker_id: row.try_get("worker_id").unwrap_or_default(),
            started_at: row.try_get("started_at").unwrap_or_else(|_| Utc::now()),
            heartbeat_at: row.try_get("heartbeat_at").unwrap_or_else(|_| Utc::now()),
            lease_expires_at: row.try_get("lease_expires_at").ok().flatten(),
            stopped_at: row.try_get("stopped_at").ok().flatten(),
        }
    }

    const COLS: &'static str = "id, organization_id, generation, status, worker_id, \
                                started_at, heartbeat_at, lease_expires_at, stopped_at";
}

#[async_trait]
impl RuntimeStore for PgRuntimeStore {
    async fn insert(&self, record: &TenantRuntimeRecord) -> BotResult<()> {
        sqlx::query(
            "INSERT INTO tenant_runtimes \
             (id, organization_id, generation, status, worker_id, started_at, heartbeat_at, \
              lease_expires_at, stopped_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(record.runtime_id.as_uuid())
        .bind(record.organization_id.as_uuid())
        .bind(record.generation.to_i64().unwrap_or(1))
        .bind(record.status.as_str())
        .bind(&record.worker_id)
        .bind(record.started_at)
        .bind(record.heartbeat_at)
        .bind(record.lease_expires_at)
        .bind(record.stopped_at)
        .execute(self.db.pool())
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("tenant_runtimes_one_live_per_org_idx") {
                BotError::db("runtime registration refused: the tenant already has a live runtime")
            } else {
                BotError::db(format!("runtime insert failed: {e}"))
            }
        })?;
        Ok(())
    }

    async fn live_for(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<TenantRuntimeRecord>> {
        let row = sqlx::query(&format!(
            "SELECT {} FROM tenant_runtimes \
             WHERE organization_id = $1 AND status IN ('provisioning','active') \
             ORDER BY generation DESC LIMIT 1",
            Self::COLS
        ))
        .bind(organization_id.as_uuid())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("runtime lookup failed: {e}")))?;
        Ok(row.as_ref().map(Self::map_row))
    }

    async fn get(&self, runtime_id: RuntimeId) -> BotResult<Option<TenantRuntimeRecord>> {
        let row = sqlx::query(&format!(
            "SELECT {} FROM tenant_runtimes WHERE id = $1",
            Self::COLS
        ))
        .bind(runtime_id.as_uuid())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("runtime get failed: {e}")))?;
        Ok(row.as_ref().map(Self::map_row))
    }

    async fn heartbeat(&self, runtime_id: RuntimeId, now: DateTime<Utc>) -> BotResult<bool> {
        let row = sqlx::query(
            "UPDATE tenant_runtimes SET heartbeat_at = $2 \
             WHERE id = $1 AND status IN ('provisioning','active') \
             RETURNING id",
        )
        .bind(runtime_id.as_uuid())
        .bind(now)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("runtime heartbeat failed: {e}")))?;
        Ok(row.is_some())
    }

    async fn set_status(
        &self,
        runtime_id: RuntimeId,
        status: RuntimeStatus,
        now: DateTime<Utc>,
    ) -> BotResult<()> {
        let stopped_at = status.is_terminal().then_some(now);
        sqlx::query(
            "UPDATE tenant_runtimes SET status = $2, stopped_at = COALESCE($3, stopped_at) \
             WHERE id = $1 AND status NOT IN ('stopped','retired')",
        )
        .bind(runtime_id.as_uuid())
        .bind(status.as_str())
        .bind(stopped_at)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("runtime status change failed: {e}")))?;
        Ok(())
    }

    async fn max_generation(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<RuntimeGeneration>> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT MAX(generation) FROM tenant_runtimes WHERE organization_id = $1",
        )
        .bind(organization_id.as_uuid())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("runtime generation lookup failed: {e}")))?;
        Ok(row.and_then(|(g,)| g).and_then(RuntimeGeneration::from_i64))
    }

    async fn rotate(
        &self,
        organization_id: OrganizationId,
        new_runtime_id: RuntimeId,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> BotResult<TenantRuntimeRecord> {
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|e| BotError::db(format!("runtime rotate begin failed: {e}")))?;
        // Supersede the live runtime (if any).
        sqlx::query(
            "UPDATE tenant_runtimes SET status = 'stopped', stopped_at = $2 \
             WHERE organization_id = $1 AND status IN ('provisioning','active')",
        )
        .bind(organization_id.as_uuid())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("runtime rotate supersede failed: {e}")))?;
        // Issue the next generation.
        let (max,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(MAX(generation), 0) FROM tenant_runtimes WHERE organization_id = $1",
        )
        .bind(organization_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("runtime rotate generation failed: {e}")))?;
        let generation = RuntimeGeneration::from_i64(max.max(0))
            .and_then(|g| g.next())
            .unwrap_or_else(RuntimeGeneration::first);
        let record = TenantRuntimeRecord::new_active(
            organization_id,
            new_runtime_id,
            generation,
            worker_id,
            now,
        );
        sqlx::query(
            "INSERT INTO tenant_runtimes \
             (id, organization_id, generation, status, worker_id, started_at, heartbeat_at) \
             VALUES ($1, $2, $3, 'active', $4, $5, $5)",
        )
        .bind(record.runtime_id.as_uuid())
        .bind(record.organization_id.as_uuid())
        .bind(record.generation.to_i64().unwrap_or(1))
        .bind(&record.worker_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("runtime rotate insert failed: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| BotError::db(format!("runtime rotate commit failed: {e}")))?;
        Ok(record)
    }

    async fn stale_since(&self, cutoff: DateTime<Utc>) -> BotResult<Vec<TenantRuntimeRecord>> {
        let rows = sqlx::query(&format!(
            "SELECT {} FROM tenant_runtimes \
             WHERE status IN ('provisioning','active') AND heartbeat_at < $1",
            Self::COLS
        ))
        .bind(cutoff)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("runtime stale sweep failed: {e}")))?;
        Ok(rows.iter().map(Self::map_row).collect())
    }
}

/// Process-local implementation (single-process runs and tests).
pub struct MemoryRuntimeStore {
    rows: RwLock<Vec<TenantRuntimeRecord>>,
}

impl Default for MemoryRuntimeStore {
    fn default() -> Self {
        MemoryRuntimeStore {
            rows: RwLock::new(Vec::new()),
        }
    }
}

impl MemoryRuntimeStore {
    /// An empty registry.
    pub fn new() -> Self {
        MemoryRuntimeStore::default()
    }
}

#[async_trait]
impl RuntimeStore for MemoryRuntimeStore {
    async fn insert(&self, record: &TenantRuntimeRecord) -> BotResult<()> {
        let mut rows = self.rows.write().map_err(|e| BotError::db(e.to_string()))?;
        let split_brain = rows.iter().any(|r| {
            r.organization_id == record.organization_id
                && r.status.is_live()
                && record.status.is_live()
                && r.runtime_id != record.runtime_id
        });
        if split_brain {
            return Err(BotError::db(
                "runtime registration refused: the tenant already has a live runtime",
            ));
        }
        rows.push(record.clone());
        Ok(())
    }

    async fn live_for(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<TenantRuntimeRecord>> {
        let rows = self.rows.read().map_err(|e| BotError::db(e.to_string()))?;
        Ok(rows
            .iter()
            .filter(|r| r.organization_id == organization_id && r.status.is_live())
            .max_by_key(|r| r.generation)
            .cloned())
    }

    async fn get(&self, runtime_id: RuntimeId) -> BotResult<Option<TenantRuntimeRecord>> {
        let rows = self.rows.read().map_err(|e| BotError::db(e.to_string()))?;
        Ok(rows.iter().find(|r| r.runtime_id == runtime_id).cloned())
    }

    async fn heartbeat(&self, runtime_id: RuntimeId, now: DateTime<Utc>) -> BotResult<bool> {
        let mut rows = self.rows.write().map_err(|e| BotError::db(e.to_string()))?;
        match rows.iter_mut().find(|r| r.runtime_id == runtime_id) {
            Some(r) if r.status.is_live() => {
                r.heartbeat_at = now;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn set_status(
        &self,
        runtime_id: RuntimeId,
        status: RuntimeStatus,
        now: DateTime<Utc>,
    ) -> BotResult<()> {
        let mut rows = self.rows.write().map_err(|e| BotError::db(e.to_string()))?;
        if let Some(r) = rows.iter_mut().find(|r| r.runtime_id == runtime_id) {
            if !r.status.is_terminal() {
                r.status = status;
                if status.is_terminal() {
                    r.stopped_at = Some(now);
                }
            }
        }
        Ok(())
    }

    async fn max_generation(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<RuntimeGeneration>> {
        let rows = self.rows.read().map_err(|e| BotError::db(e.to_string()))?;
        Ok(rows
            .iter()
            .filter(|r| r.organization_id == organization_id)
            .map(|r| r.generation)
            .max())
    }

    async fn rotate(
        &self,
        organization_id: OrganizationId,
        new_runtime_id: RuntimeId,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> BotResult<TenantRuntimeRecord> {
        let mut rows = self.rows.write().map_err(|e| BotError::db(e.to_string()))?;
        for r in rows.iter_mut() {
            if r.organization_id == organization_id && r.status.is_live() {
                r.status = RuntimeStatus::Stopped;
                r.stopped_at = Some(now);
            }
        }
        let generation = RuntimeGeneration::from_i64(
            rows.iter()
                .filter(|r| r.organization_id == organization_id)
                .map(|r| r.generation.raw() as i64)
                .max()
                .unwrap_or(0),
        )
        .and_then(|g| g.next())
        .unwrap_or_else(RuntimeGeneration::first);
        let record = TenantRuntimeRecord::new_active(
            organization_id,
            new_runtime_id,
            generation,
            worker_id,
            now,
        );
        rows.push(record.clone());
        Ok(record)
    }

    async fn stale_since(&self, cutoff: DateTime<Utc>) -> BotResult<Vec<TenantRuntimeRecord>> {
        let rows = self.rows.read().map_err(|e| BotError::db(e.to_string()))?;
        Ok(rows
            .iter()
            .filter(|r| r.status.is_live() && r.heartbeat_at < cutoff)
            .cloned()
            .collect())
    }
}

/// Log-and-continue wrapper for reaping sweeps (a failed sweep must never
/// take the process down; the next sweep retries).
pub fn log_reap_failures(failed: &[RuntimeId], error: &BotError) {
    if !failed.is_empty() {
        warn!(
            runtimes = ?failed,
            error = %error,
            "runtime reap could not stop some stale runtimes; next sweep retries"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[tokio::test]
    async fn memory_store_round_trip_and_heartbeat() {
        let store = MemoryRuntimeStore::new();
        let org = OrganizationId::new();
        let rec = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker-a",
            now(),
        );
        store.insert(&rec).await.unwrap();
        assert_eq!(
            store.live_for(org).await.unwrap().unwrap().runtime_id,
            rec.runtime_id
        );
        assert_eq!(store.get(rec.runtime_id).await.unwrap().unwrap(), rec);

        let later = now() + chrono::Duration::seconds(30);
        assert!(store.heartbeat(rec.runtime_id, later).await.unwrap());
        assert_eq!(
            store
                .get(rec.runtime_id)
                .await
                .unwrap()
                .unwrap()
                .heartbeat_at,
            later
        );

        // Terminal runtime: heartbeat refuses.
        store
            .set_status(rec.runtime_id, RuntimeStatus::Stopped, later)
            .await
            .unwrap();
        assert!(!store.heartbeat(rec.runtime_id, later).await.unwrap());
        assert!(store.live_for(org).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn memory_store_refuses_split_brain() {
        let store = MemoryRuntimeStore::new();
        let org = OrganizationId::new();
        let a = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "a",
            now(),
        );
        let b = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "b",
            now(),
        );
        store.insert(&a).await.unwrap();
        assert!(store.insert(&b).await.is_err());
    }

    #[tokio::test]
    async fn rotate_supersedes_and_increments_generation() {
        let store = MemoryRuntimeStore::new();
        let org = OrganizationId::new();
        let first = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "a",
            now(),
        );
        store.insert(&first).await.unwrap();

        let new_id = RuntimeId::new();
        let second = store.rotate(org, new_id, "b", now()).await.unwrap();
        assert_eq!(second.runtime_id, new_id);
        assert_eq!(second.generation.raw(), 2);
        assert!(second.status.is_live());

        // The old runtime is stopped; the live one is the new row.
        let live = store.live_for(org).await.unwrap().unwrap();
        assert_eq!(live.runtime_id, new_id);
        assert_eq!(live.generation.raw(), 2);
        let old = store.get(first.runtime_id).await.unwrap().unwrap();
        assert!(old.status.is_terminal());

        // Rotating without a live runtime still works (fresh generation
        // continues the tenant's history).
        let third = store
            .rotate(org, RuntimeId::new(), "c", now())
            .await
            .unwrap();
        assert_eq!(third.generation.raw(), 3);
    }

    #[tokio::test]
    async fn stale_sweep_finds_only_stale_live_runtimes() {
        let store = MemoryRuntimeStore::new();
        let org = OrganizationId::new();
        let fresh = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "fresh",
            now(),
        );
        store.insert(&fresh).await.unwrap();

        // Heartbeats older than 120s ago are stale; the fresh runtime's
        // heartbeat is now, so nothing qualifies yet.
        let cutoff = now() - chrono::Duration::seconds(120);
        assert!(store.stale_since(cutoff).await.unwrap().is_empty());

        let old = TenantRuntimeRecord::new_active(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "old",
            now() - chrono::Duration::seconds(300),
        );
        store.insert(&old).await.unwrap();
        let stale = store.stale_since(cutoff).await.unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].runtime_id, old.runtime_id);
    }
}
