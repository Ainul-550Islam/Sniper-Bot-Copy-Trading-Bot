//! The tenant decision log (STEP 3 file 55).
//!
//! Append-only record of every gateway decision (allow AND deny), with
//! the machine label, the module/mode/origin of the request and the
//! acting principal. This is the operator's "why is tenant X not
//! trading" answer: `recent(org, 50)` shows the exact deny labels in
//! order. Storage: `tenant_decision_log` (migration 0025) in PG; a vec
//! in memory for tests and detached deployments.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;

use bot_core::error::{BotError, BotResult};
use bot_core::tenant::OrganizationId;

/// One logged decision. Labels are owned strings: the in-process writer
/// copies its `&'static str` vocabulary in, and rows read back from PG
/// arrive owned — no interning, no leaking.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecisionLogEntry {
    /// The tenant.
    pub organization_id: OrganizationId,
    /// `"allow"` or a deny label from the gateway vocabulary.
    pub decision: String,
    /// Human detail (deny reasons carry it; allows say which guards
    /// passed).
    pub detail: String,
    /// The module the request targeted.
    pub module: String,
    /// The trading mode.
    pub mode: String,
    /// The entry path.
    pub origin: String,
    /// The acting principal.
    pub principal: String,
    /// When.
    pub at: DateTime<Utc>,
}

impl DecisionLogEntry {
    /// Build from the gateway's static vocabulary (copies into owned
    /// labels).
    #[allow(clippy::too_many_arguments)]
    pub fn from_static(
        organization_id: OrganizationId,
        decision: &'static str,
        detail: String,
        module: &'static str,
        mode: &'static str,
        origin: &'static str,
        principal: &str,
        at: DateTime<Utc>,
    ) -> Self {
        DecisionLogEntry {
            organization_id,
            decision: decision.to_string(),
            detail,
            module: module.to_string(),
            mode: mode.to_string(),
            origin: origin.to_string(),
            principal: principal.to_string(),
            at,
        }
    }
}

/// The sink contract.
#[async_trait]
pub trait DecisionLogSink: Send + Sync {
    /// Append one entry. Best-effort by design: a logging failure must
    /// never block or fail a decision — callers log the error and move
    /// on. Returns Ok(()) when persisted.
    async fn append(&self, entry: DecisionLogEntry) -> BotResult<()>;

    /// The most recent entries for a tenant (newest first).
    async fn recent(
        &self,
        organization_id: OrganizationId,
        limit: u32,
    ) -> BotResult<Vec<DecisionLogEntry>>;
}

/// PG sink over `tenant_decision_log` (migration 0025).
pub struct PgDecisionLogSink {
    db: Arc<bot_core::db::Database>,
}

impl PgDecisionLogSink {
    /// Bind to the shared database handle.
    pub fn new(db: Arc<bot_core::db::Database>) -> Self {
        PgDecisionLogSink { db }
    }
}

#[async_trait]
impl DecisionLogSink for PgDecisionLogSink {
    async fn append(&self, entry: DecisionLogEntry) -> BotResult<()> {
        sqlx::query(
            "INSERT INTO tenant_decision_log \
                 (organization_id, decision, detail, module, mode, origin, principal, at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(entry.organization_id.as_uuid())
        .bind(entry.decision)
        .bind(&entry.detail)
        .bind(entry.module)
        .bind(entry.mode)
        .bind(entry.origin)
        .bind(&entry.principal)
        .bind(entry.at)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("decision log append failed: {e}")))?;
        Ok(())
    }

    async fn recent(
        &self,
        organization_id: OrganizationId,
        limit: u32,
    ) -> BotResult<Vec<DecisionLogEntry>> {
        let rows = sqlx::query(
            "SELECT decision, detail, module, mode, origin, principal, at \
               FROM tenant_decision_log \
              WHERE organization_id = $1 \
              ORDER BY at DESC, id DESC \
              LIMIT $2",
        )
        .bind(organization_id.as_uuid())
        .bind(limit as i64)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("decision log read failed: {e}")))?;

        let mapped: BotResult<Vec<DecisionLogEntry>> = rows
            .iter()
            .map(|row| {
                let column = |name: &str| {
                    row.try_get::<String, _>(name)
                        .map_err(|e| BotError::db(format!("decision log column unreadable: {e}")))
                };
                Ok(DecisionLogEntry {
                    organization_id,
                    decision: column("decision")?,
                    detail: column("detail")?,
                    module: column("module")?,
                    mode: column("mode")?,
                    origin: column("origin")?,
                    principal: column("principal")?,
                    at: row.try_get("at").unwrap_or_else(|_| Utc::now()),
                })
            })
            .collect();
        mapped
    }
}

/// In-memory sink (tests, detached deployments).
#[derive(Default)]
pub struct MemoryDecisionLogSink {
    entries: tokio::sync::RwLock<Vec<DecisionLogEntry>>,
}

impl MemoryDecisionLogSink {
    /// An empty sink.
    pub fn new() -> Self {
        MemoryDecisionLogSink::default()
    }
}

#[async_trait]
impl DecisionLogSink for MemoryDecisionLogSink {
    async fn append(&self, entry: DecisionLogEntry) -> BotResult<()> {
        self.entries.write().await.push(entry);
        Ok(())
    }

    async fn recent(
        &self,
        organization_id: OrganizationId,
        limit: u32,
    ) -> BotResult<Vec<DecisionLogEntry>> {
        let entries = self.entries.read().await;
        let mut out: Vec<DecisionLogEntry> = entries
            .iter()
            .filter(|e| e.organization_id == organization_id)
            .cloned()
            .collect();
        out.reverse(); // newest last appended -> newest first
        out.truncate(limit as usize);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(org: OrganizationId, decision: &str, at: DateTime<Utc>) -> DecisionLogEntry {
        DecisionLogEntry {
            organization_id: org,
            decision: decision.to_string(),
            detail: if decision == "allow" {
                "all guards passed".into()
            } else {
                format!("{decision} detail")
            },
            module: "copy".into(),
            mode: "paper".into(),
            origin: "req".into(),
            principal: "user-1".into(),
            at,
        }
    }

    #[tokio::test]
    async fn entries_append_and_read_back_newest_first() {
        let sink = MemoryDecisionLogSink::new();
        let org = OrganizationId::new();
        let base = Utc::now();
        sink.append(entry(org, "allow", base)).await.unwrap();
        sink.append(entry(
            org,
            "wallet_not_bound",
            base + chrono::Duration::seconds(5),
        ))
        .await
        .unwrap();

        let recent = sink.recent(org, 10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].decision, "wallet_not_bound", "newest first");
        assert_eq!(recent[1].decision, "allow");
    }

    #[tokio::test]
    async fn tenants_never_see_each_others_decisions() {
        let sink = MemoryDecisionLogSink::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        sink.append(entry(a, "allow", Utc::now())).await.unwrap();
        assert!(sink.recent(b, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_limit_truncates() {
        let sink = MemoryDecisionLogSink::new();
        let org = OrganizationId::new();
        for _ in 0..10 {
            sink.append(entry(org, "allow", Utc::now())).await.unwrap();
        }
        assert_eq!(sink.recent(org, 3).await.unwrap().len(), 3);
    }

    #[test]
    fn entries_build_from_the_static_vocabulary_and_serialize() {
        let entry = DecisionLogEntry::from_static(
            OrganizationId::new(),
            "fence_failed",
            "runtime fence failed (superseded)".into(),
            "copy",
            "paper",
            "req",
            "user-1",
            Utc::now(),
        );
        assert_eq!(entry.decision, "fence_failed");
        assert_eq!(entry.module, "copy");
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["decision"], "fence_failed");
    }
}
