//! Tenant-scoped copy configuration writes (PROMPT 3/10 §F38).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{TenantCopyLink, TenantLeader, TenantLeaderEvent};

/// Write-side tenant copy repository (`copy_leaders`,
/// `copy_leader_events`, `copy_links`).
pub struct TenantCopyWrite {
    db: Arc<Database>,
}

impl TenantCopyWrite {
    pub fn new(db: Arc<Database>) -> Self {
        TenantCopyWrite { db }
    }

    /// Upsert one of the acting tenant's leader rows on the 0029
    /// composite arbiter `(organization_id, address)`: the SAME
    /// external address configured by ANOTHER tenant never touches
    /// this row; `followed_at` keeps the first-seen time.
    pub async fn upsert_leader(
        &self,
        write: &TenantWriteScope,
        rec: &TenantLeader,
    ) -> Result<(), RepositoryError> {
        if rec.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if rec.address.trim().is_empty() {
            return Err(RepositoryError::Validation("address"));
        }
        if !matches!(rec.status.as_str(), "active" | "paused" | "removed") {
            return Err(RepositoryError::Validation("leader status"));
        }
        self.db
            .timed(
                "tenant_copy_leader_upsert",
                sqlx::query(
                    r#"INSERT INTO copy_leaders
                           (organization_id, address, label, status, source,
                            followed_at, status_since, events_seen, mirrored,
                            rejected, last_event_at, last_slot, updated_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                       ON CONFLICT (organization_id, address) DO UPDATE SET
                           label = EXCLUDED.label,
                           status = EXCLUDED.status,
                           source = EXCLUDED.source,
                           status_since = EXCLUDED.status_since,
                           events_seen = GREATEST(copy_leaders.events_seen, EXCLUDED.events_seen),
                           mirrored = GREATEST(copy_leaders.mirrored, EXCLUDED.mirrored),
                           rejected = GREATEST(copy_leaders.rejected, EXCLUDED.rejected),
                           last_event_at = COALESCE(EXCLUDED.last_event_at, copy_leaders.last_event_at),
                           last_slot = GREATEST(copy_leaders.last_slot, EXCLUDED.last_slot),
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(rec.organization_id.as_uuid())
                .bind(&rec.address)
                .bind(&rec.label)
                .bind(&rec.status)
                .bind(&rec.source)
                .bind(rec.followed_at)
                .bind(rec.status_since)
                .bind(rec.events_seen)
                .bind(rec.mirrored)
                .bind(rec.rejected)
                .bind(rec.last_event_at)
                .bind(rec.last_slot)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Append one lifecycle transition for the acting tenant's leader
    /// (the leader row must exist FOR THIS TENANT — a foreign or
    /// missing leader configuration yields `NotFound`).
    pub async fn append_leader_event(
        &self,
        write: &TenantWriteScope,
        rec: &TenantLeaderEvent,
    ) -> Result<(), RepositoryError> {
        if rec.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        let owned = sqlx::query(
            r#"SELECT 1 FROM copy_leaders
                WHERE organization_id = $1 AND address = $2"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(&rec.address)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        if owned.is_none() {
            tx.rollback().await.map_err(RepositoryError::from)?;
            return Err(RepositoryError::NotFound("copy_leader"));
        }
        sqlx::query(
            r#"INSERT INTO copy_leader_events
                   (organization_id, address, event, reason, replica_id, ts)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(&rec.address)
        .bind(&rec.event)
        .bind(&rec.reason)
        .bind(&rec.replica_id)
        .bind(rec.ts)
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(())
    }

    /// Upsert one of the acting tenant's copy links on the 0029
    /// composite arbiter `(organization_id, position_id)`.
    pub async fn upsert_link(
        &self,
        write: &TenantWriteScope,
        rec: &TenantCopyLink,
    ) -> Result<(), RepositoryError> {
        if rec.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if rec.position_id.trim().is_empty() {
            return Err(RepositoryError::Validation("position_id"));
        }
        if !matches!(
            rec.status.as_str(),
            "open" | "closed" | "orphaned" | "mismatch"
        ) {
            return Err(RepositoryError::Validation("link status"));
        }
        self.db
            .timed(
                "tenant_copy_link_upsert",
                sqlx::query(
                    r#"INSERT INTO copy_links
                           (organization_id, position_id, leader, mint,
                            entry_event_id, entry_signature, intent_id,
                            leader_token_amount, follower_qty, status, opened_at,
                            closed_at, exit_event_id, last_reconciled_at, note,
                            updated_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
                       ON CONFLICT (organization_id, position_id) DO UPDATE SET
                           status = EXCLUDED.status,
                           leader_token_amount = EXCLUDED.leader_token_amount,
                           follower_qty = EXCLUDED.follower_qty,
                           closed_at = EXCLUDED.closed_at,
                           exit_event_id = COALESCE(EXCLUDED.exit_event_id, copy_links.exit_event_id),
                           last_reconciled_at = COALESCE(EXCLUDED.last_reconciled_at, copy_links.last_reconciled_at),
                           note = EXCLUDED.note,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(rec.organization_id.as_uuid())
                .bind(&rec.position_id)
                .bind(&rec.leader)
                .bind(&rec.mint)
                .bind(&rec.entry_event_id)
                .bind(&rec.entry_signature)
                .bind(&rec.intent_id)
                .bind(rec.leader_token_amount)
                .bind(rec.follower_qty)
                .bind(&rec.status)
                .bind(rec.opened_at)
                .bind(rec.closed_at)
                .bind(&rec.exit_event_id)
                .bind(rec.last_reconciled_at)
                .bind(&rec.note)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Mark one of the acting tenant's links reconciled (guarded on
    /// the link being open). Zero rows (absent/foreign/terminal) →
    /// `NotFound`/`StaleWrite`.
    pub async fn mark_link_reconciled(
        &self,
        write: &TenantWriteScope,
        position_id: &str,
        follower_qty: f64,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_copy_link_reconciled",
                sqlx::query(
                    r#"UPDATE copy_links
                          SET follower_qty = $3, last_reconciled_at = $4,
                              updated_at = $4
                        WHERE organization_id = $1 AND position_id = $2
                          AND status = 'open'"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(position_id)
                .bind(follower_qty)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            let row = self
                .db
                .timed(
                    "tenant_copy_link_probe",
                    sqlx::query(
                        r#"SELECT 1 FROM copy_links
                            WHERE organization_id = $1 AND position_id = $2"#,
                    )
                    .bind(write.organization_id().as_uuid())
                    .bind(position_id)
                    .fetch_optional(self.db.pool()),
                )
                .await?;
            match row {
                Some(_) => Err(RepositoryError::StaleWrite("copy link status")),
                None => Err(RepositoryError::NotFound("copy_link")),
            }
        }
    }
}
