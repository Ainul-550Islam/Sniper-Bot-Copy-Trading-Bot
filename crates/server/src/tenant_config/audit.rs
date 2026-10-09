//! Tenant configuration audit trail (STEP 3 file 36).
//!
//! Every configuration write lands in `tenant_config_audit` (migration
//! 0025) with who/when, from-version, to-version and the
//! machine-readable diff. The trail is append-only: no update or delete
//! path exists anywhere for it. Reads answer "who changed what, when"
//! for support and compliance without exposing secrets — the audited
//! document contains none by construction (see `model.rs`).

use chrono::{DateTime, Utc};
use serde::Serialize;

use bot_core::error::{BotError, BotResult};
use bot_core::tenant::OrganizationId;

use super::diff::ConfigDiff;
use super::model::TenantConfigModel;
use super::version::ConfigVersion;

/// One audit entry, as persisted.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigAuditEntry {
    /// The tenant whose configuration changed.
    pub organization_id: OrganizationId,
    /// The version BEFORE the change (None = first write).
    pub from_version: Option<ConfigVersion>,
    /// The version AFTER the change.
    pub to_version: ConfigVersion,
    /// The recorded diff (empty = no observable change — the writer
    /// persisted an identical document).
    pub changes: Vec<super::diff::Change>,
    /// Who made the change (user id, operator label or "system").
    pub updated_by: Option<String>,
    /// When.
    pub at: DateTime<Utc>,
}

impl ConfigAuditEntry {
    /// Human summary for logs and support tooling.
    pub fn summary(&self) -> String {
        let who = self.updated_by.as_deref().unwrap_or("system");
        match self.changes.len() {
            0 => format!(
                "[{}] tenant {} config rewritten unchanged at {} by {who}",
                self.at, self.organization_id, self.to_version
            ),
            n => format!(
                "[{}] tenant {} config {} -> {} by {who}: {} change{}",
                self.at,
                self.organization_id,
                self.from_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "none".into()),
                self.to_version,
                n,
                if n == 1 { "" } else { "s" }
            ),
        }
    }
}

/// The audit sink contract (PG in production; a vec in tests).
#[async_trait::async_trait]
pub trait ConfigAuditSink: Send + Sync {
    /// Append one entry. Failures are the caller's choice: the store
    /// treats audit failure as fatal for the WRITE (a configuration
    /// change that cannot be audited does not happen).
    async fn append(&self, entry: ConfigAuditEntry) -> BotResult<()>;

    /// The most recent entries for a tenant (newest first).
    async fn recent(
        &self,
        organization_id: OrganizationId,
        limit: u32,
    ) -> BotResult<Vec<ConfigAuditEntry>>;
}

/// Build the entry for a write (pure — shared by both stores).
pub fn entry_for(
    organization_id: OrganizationId,
    old: Option<&TenantConfigModel>,
    old_version: Option<ConfigVersion>,
    new: &TenantConfigModel,
    new_version: ConfigVersion,
    updated_by: Option<&str>,
    now: DateTime<Utc>,
) -> ConfigAuditEntry {
    let changes = match old {
        Some(old) => ConfigDiff::between(old, new).changes,
        None => Vec::new(), // first write: nothing to diff against
    };
    ConfigAuditEntry {
        organization_id,
        from_version: old_version,
        to_version: new_version,
        changes,
        updated_by: updated_by.map(str::to_string),
        at: now,
    }
}

/// PostgreSQL sink over `tenant_config_audit` (migration 0025).
pub struct PgConfigAuditSink {
    db: std::sync::Arc<bot_core::db::Database>,
}

impl PgConfigAuditSink {
    /// Bind to the shared database handle.
    pub fn new(db: std::sync::Arc<bot_core::db::Database>) -> Self {
        PgConfigAuditSink { db }
    }
}

#[async_trait::async_trait]
impl ConfigAuditSink for PgConfigAuditSink {
    async fn append(&self, entry: ConfigAuditEntry) -> BotResult<()> {
        let payload = serde_json::to_value(&entry.changes)
            .map_err(|e| BotError::db(format!("config audit serialization failed: {e}")))?;
        sqlx::query(
            "INSERT INTO tenant_config_audit \
                 (organization_id, from_version, to_version, changes, updated_by, at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(entry.organization_id.as_uuid())
        .bind(entry.from_version.map(|v| v.raw() as i64))
        .bind(entry.to_version.raw() as i64)
        .bind(payload)
        .bind(entry.updated_by.clone())
        .bind(entry.at)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("config audit append failed: {e}")))?;
        Ok(())
    }

    async fn recent(
        &self,
        organization_id: OrganizationId,
        limit: u32,
    ) -> BotResult<Vec<ConfigAuditEntry>> {
        let rows = sqlx::query(
            "SELECT from_version, to_version, changes, updated_by, at \
               FROM tenant_config_audit \
              WHERE organization_id = $1 \
              ORDER BY at DESC, to_version DESC \
              LIMIT $2",
        )
        .bind(organization_id.as_uuid())
        .bind(limit as i64)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("config audit read failed: {e}")))?;

        use sqlx::Row;
        let mapped: BotResult<Vec<ConfigAuditEntry>> = rows
            .iter()
            .map(|row| {
                let changes: serde_json::Value = row
                    .try_get("changes")
                    .map_err(|e| BotError::db(format!("config audit column unreadable: {e}")))?;
                Ok(ConfigAuditEntry {
                    organization_id,
                    from_version: row
                        .try_get::<Option<i64>, _>("from_version")
                        .ok()
                        .flatten()
                        .and_then(|v| ConfigVersion::from_raw(v as u64)),
                    to_version: ConfigVersion::from_raw(
                        row.try_get::<i64, _>("to_version").unwrap_or(1) as u64,
                    )
                    .unwrap_or_else(ConfigVersion::first),
                    changes: serde_json::from_value(changes).unwrap_or_default(),
                    updated_by: row.try_get("updated_by").ok().flatten(),
                    at: row.try_get("at").unwrap_or_else(|_| Utc::now()),
                })
            })
            .collect();
        mapped
    }
}

/// In-memory sink (tests, detached deployments).
#[derive(Default)]
pub struct MemoryConfigAuditSink {
    entries: tokio::sync::RwLock<Vec<ConfigAuditEntry>>,
}

impl MemoryConfigAuditSink {
    /// An empty sink.
    pub fn new() -> Self {
        MemoryConfigAuditSink::default()
    }
}

#[async_trait::async_trait]
impl ConfigAuditSink for MemoryConfigAuditSink {
    async fn append(&self, entry: ConfigAuditEntry) -> BotResult<()> {
        self.entries.write().await.push(entry);
        Ok(())
    }

    async fn recent(
        &self,
        organization_id: OrganizationId,
        limit: u32,
    ) -> BotResult<Vec<ConfigAuditEntry>> {
        let entries = self.entries.read().await;
        let mut out: Vec<ConfigAuditEntry> = entries
            .iter()
            .filter(|e| e.organization_id == organization_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.at.cmp(&a.at).then(b.to_version.cmp(&a.to_version)));
        out.truncate(limit as usize);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::diff::Change;
    use bot_core::models::BotModule;

    #[tokio::test]
    async fn entries_append_and_read_back_newest_first() {
        let sink = MemoryConfigAuditSink::new();
        let org = OrganizationId::new();
        let old = TenantConfigModel::default();
        let mut new = old.clone();
        new.disable_module(
            BotModule::Sniper,
            bot_core::tenant::ModuleDisableReason::Operator,
        );

        let first = entry_for(
            org,
            Some(&old),
            Some(ConfigVersion::first()),
            &new,
            ConfigVersion::first().next(),
            Some("op"),
            Utc::now(),
        );
        assert_eq!(first.changes.len(), 1);
        assert!(matches!(first.changes[0], Change::ModuleDisabled { .. }));

        sink.append(first).await.unwrap();
        let older = entry_for(
            org,
            None,
            None,
            &old,
            ConfigVersion::first(),
            None,
            Utc::now() - chrono::Duration::seconds(300),
        );
        sink.append(older).await.unwrap();

        let recent = sink.recent(org, 10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].to_version.raw(), 2, "newest first");
        assert_eq!(recent[1].to_version.raw(), 1);
    }

    #[test]
    fn summary_names_the_actor_and_versions() {
        let entry = entry_for(
            OrganizationId::new(),
            None,
            None,
            &TenantConfigModel::default(),
            ConfigVersion::first(),
            Some("operator-7"),
            Utc::now(),
        );
        let text = entry.summary();
        assert!(text.contains("operator-7"));
        assert!(text.contains("v1"));
    }

    #[test]
    fn first_write_has_an_empty_diff() {
        let entry = entry_for(
            OrganizationId::new(),
            None,
            None,
            &TenantConfigModel::default(),
            ConfigVersion::first(),
            None,
            Utc::now(),
        );
        assert!(entry.changes.is_empty());
    }
}
