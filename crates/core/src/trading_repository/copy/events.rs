//! Tenant-scoped copy EVENT ingestion (PROMPT 3/10 §F39).
//!
//! The copy pipeline's persistent event log: durable pre-processed
//! records keyed by `(organization_id, event_id)` since 0029. The
//! leader SIGNATURE remains a global on-chain identity inside the
//! payload; the ROW (stage, reject_reason, position linkage) is the
//! tenant's own processing state.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_optional_row_org;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{copy_event_from_row, TenantCopyEvent};

/// Tenant-scoped copy event ingestion (`copy_events`).
pub struct TenantCopyEventRepo {
    db: Arc<Database>,
}

impl TenantCopyEventRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantCopyEventRepo { db }
    }

    /// Record/advance one of the acting tenant's copy events on the
    /// 0029 composite arbiter `(organization_id, event_id)`.
    ///
    /// Idempotency + progress discipline (the legacy `seens`/
    /// `record_raw` semantics, tenant-scoped):
    /// * first insert → the given stage;
    /// * re-observe the same event for THIS tenant → update the stage
    ///   and reject reason (progress is monotonic in the caller, the
    ///   row simply reflects the latest stage);
    /// * ANOTHER tenant processing the SAME leader trade (same
    ///   event_id/signature) is a DIFFERENT row — no cross-tenant
    ///   stage overwrite is possible, by the arbiter.
    ///
    /// Returns `Ok(true)` when this call created the row (first
    /// observation for THIS tenant), `Ok(false)` when it advanced an
    /// existing row.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        write: &TenantWriteScope,
        ev: &TenantCopyEvent,
    ) -> Result<bool, RepositoryError> {
        if ev.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if ev.event_id.trim().is_empty() || ev.signature.trim().is_empty() {
            return Err(RepositoryError::Validation("event fields"));
        }
        if !matches!(ev.side.as_str(), "buy" | "sell") {
            return Err(RepositoryError::Validation("side"));
        }
        let row = self
            .db
            .timed(
                "tenant_copy_event_upsert",
                sqlx::query(
                    r#"INSERT INTO copy_events
                           (organization_id, event_id, leader, signature, slot,
                            mint, side, venue, token_amount, sol_amount, source,
                            source_sequence, event_at, observed_at, stage,
                            reject_reason, detail, intent_id, position_id)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
                       ON CONFLICT (organization_id, event_id) DO UPDATE SET
                           stage = EXCLUDED.stage,
                           reject_reason = EXCLUDED.reject_reason,
                           detail = COALESCE(EXCLUDED.detail, copy_events.detail),
                           intent_id = COALESCE(EXCLUDED.intent_id, copy_events.intent_id),
                           position_id = COALESCE(EXCLUDED.position_id, copy_events.position_id),
                           updated_at = now()
                       RETURNING (xmax = 0)"#,
                )
                .bind(ev.organization_id.as_uuid())
                .bind(&ev.event_id)
                .bind(&ev.leader)
                .bind(&ev.signature)
                .bind(ev.slot)
                .bind(&ev.mint)
                .bind(&ev.side)
                .bind(&ev.venue)
                .bind(ev.token_amount)
                .bind(ev.sol_amount)
                .bind(&ev.source)
                .bind(ev.source_sequence)
                .bind(ev.event_at)
                .bind(ev.observed_at)
                .bind(&ev.stage)
                .bind(&ev.reject_reason)
                .bind(&ev.detail)
                .bind(&ev.intent_id)
                .bind(&ev.position_id)
                .fetch_one(self.db.pool()),
            )
            .await?;
        let fresh: bool = row
            .try_get::<bool, _>("?column?")
            .or_else(|_| row.try_get::<bool, _>("xmax"))
            .unwrap_or(true);
        Ok(fresh)
    }

    /// Has THIS tenant already observed the event? (pipeline dedup
    /// after restart — the tenant's own rows only).
    pub async fn seen(
        &self,
        scope: &TradingQueryScope,
        event_id: &str,
    ) -> Result<bool, RepositoryError> {
        Ok(self.event(scope, event_id).await?.is_some())
    }

    /// One event row by id (tenant-scoped).
    pub async fn event(
        &self,
        scope: &TradingQueryScope,
        event_id: &str,
    ) -> Result<Option<TenantCopyEvent>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_copy_event_get",
                sqlx::query(
                    r#"SELECT * FROM copy_events
                        WHERE organization_id = $1 AND event_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(event_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(copy_event_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's last-seen source sequence for a leader —
    /// restart re-seed of the in-memory sequence gate. A leader never
    /// followed by this tenant yields `None` (no leak of another
    /// tenant's watermark).
    pub async fn last_source_sequence(
        &self,
        scope: &TradingQueryScope,
        leader: &str,
    ) -> Result<Option<i64>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_copy_last_seq",
                sqlx::query(
                    r#"SELECT MAX(source_sequence)::bigint AS max_seq
                         FROM copy_events
                        WHERE organization_id = $1 AND leader = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(leader)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row
            .and_then(|r| r.try_get::<Option<i64>, _>("max_seq").ok())
            .flatten())
    }

    /// Retention: drop the acting tenant's terminal-stage events older
    /// than `cutoff` (tenant-scoped twin of the legacy retention).
    pub async fn prune_older_than(
        &self,
        write: &TenantWriteScope,
        cutoff: DateTime<Utc>,
    ) -> Result<u64, RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_copy_event_prune",
                sqlx::query(
                    r#"DELETE FROM copy_events
                        WHERE organization_id = $1
                          AND stage IN ('terminal', 'dropped', 'no_op')
                          AND observed_at < $2"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(cutoff)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}
