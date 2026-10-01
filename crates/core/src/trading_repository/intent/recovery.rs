//! Tenant-safe recovery / replay queries (PROMPT 3/10 §E33).
//!
//! Every recovery query here answers "which tenant owns this record?"
//! in the SQL predicate itself. The DANGEROUS global shape
//! `SELECT … FROM execution_intents WHERE status = …` exists ONLY in
//! the legacy deployment operator plane; the tenant data plane uses
//! the scoped forms below.
//!
//! The recovery sweep is serialized per (tenant, purpose) with the
//! tenant-namespaced advisory lock, so two tenants' sweeps run
//! concurrently while two workers of ONE tenant still serialize.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::transaction::TradingTransaction;

use super::model::{intent_from_row, TenantIntent};
use super::read::TenantIntentRead;
use super::write::TenantIntentWrite;

/// One recovery work item for the acting tenant.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantRecoveryItem {
    /// The orphaned intent (pending, older than the cutoff).
    pub intent: TenantIntent,
    /// Non-terminal orders of the same tenant in the same window —
    /// the recovery/reconciliation inputs travel together so the
    /// caller never has to re-query globally.
    pub open_order_ids: Vec<String>,
}

/// Tenant-scoped recovery repository.
pub struct TenantRecoveryRepo {
    db: Arc<Database>,
    intents: TenantIntentRead,
    intent_writes: TenantIntentWrite,
}

impl TenantRecoveryRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantRecoveryRepo {
            intents: TenantIntentRead::new(db.clone()),
            intent_writes: TenantIntentWrite::new(db.clone()),
            db,
        }
    }

    /// The acting tenant's FULL recovery batch under the tenant
    /// advisory lock: orphaned pending intents plus its non-terminal
    /// orders. Runs on ONE connection inside a transaction; the lock
    /// is released at commit/rollback.
    pub async fn sweep(
        &self,
        scope: &TradingQueryScope,
        orphan_cutoff: DateTime<Utc>,
    ) -> Result<Vec<TenantRecoveryItem>, RepositoryError> {
        let mut tx = TradingTransaction::begin(&self.db, *scope).await?;
        tx.tenant_lock("recovery-sweep").await?;

        let orphaned = sqlx::query(
            r#"SELECT * FROM execution_intents
                WHERE organization_id = $1
                  AND status = 'pending' AND created_at < $2
                ORDER BY created_at ASC LIMIT 1000"#,
        )
        .bind(scope.organization_id().as_uuid())
        .bind(orphan_cutoff)
        .fetch_all(tx.connection())
        .await
        .map_err(RepositoryError::from)?;
        let intents: Vec<TenantIntent> = orphaned.iter().map(intent_from_row).collect();
        crate::trading_repository::tenant_assert::assert_rows_org(
            scope.organization_id(),
            &intents,
        )?;

        let open_orders = sqlx::query(
            r#"SELECT id FROM orders
                WHERE organization_id = $1
                  AND status NOT IN
                      ('filled','failed','cancelled','expired','reconciled')
                ORDER BY created_at ASC LIMIT 5000"#,
        )
        .bind(scope.organization_id().as_uuid())
        .fetch_all(tx.connection())
        .await
        .map_err(RepositoryError::from)?;
        let open_order_ids: Vec<String> = open_orders
            .iter()
            .filter_map(|r| r.try_get::<String, _>("id").ok())
            .collect();

        tx.commit().await?;

        Ok(intents
            .into_iter()
            .map(|intent| TenantRecoveryItem {
                intent,
                open_order_ids: open_order_ids.clone(),
            })
            .collect())
    }

    /// Abandon one of the acting tenant's orphaned intents after the
    /// operator/automation proved it never broadcast.
    pub async fn abandon_orphan(
        &self,
        write: &crate::trading_repository::write_scope::TenantWriteScope,
        intent_id: &str,
    ) -> Result<(), RepositoryError> {
        self.intent_writes.abandon(write, intent_id).await
    }

    /// Link one of the acting tenant's intents to its broadcast
    /// signature (a late `link` during recovery).
    pub async fn link_orphan(
        &self,
        write: &crate::trading_repository::write_scope::TenantWriteScope,
        intent_id: &str,
        signature: &str,
    ) -> Result<(), RepositoryError> {
        self.intent_writes.link(write, intent_id, signature).await
    }

    /// Reconciliation-queue views for the acting tenant
    /// (`reconciliation_state`, 0033 composite PK): due work rows.
    /// The queue's PK is tenant-local, so two tenants can hold the
    /// same subject id without colliding.
    pub async fn due_reconciliation(
        &self,
        scope: &TradingQueryScope,
        limit: i64,
    ) -> Result<Vec<(String, String, String, i32, DateTime<Utc>)>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_recon_due",
                sqlx::query(
                    r#"SELECT kind, subject, status, attempts, next_attempt_at
                         FROM reconciliation_state
                        WHERE organization_id = $1
                          AND status IN ('pending', 'in_progress')
                          AND next_attempt_at <= now()
                        ORDER BY next_attempt_at ASC LIMIT $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some((
                    r.try_get::<String, _>("kind").ok()?,
                    r.try_get::<String, _>("subject").ok()?,
                    r.try_get::<String, _>("status").ok()?,
                    r.try_get::<i32, _>("attempts").ok()?,
                    r.try_get::<DateTime<Utc>, _>("next_attempt_at").ok()?,
                ))
            })
            .collect())
    }

    /// Enqueue reconciliation work for the acting tenant on the 0033
    /// composite arbiter — the same (kind, subject) may be enqueued by
    /// another tenant independently.
    pub async fn enqueue_reconciliation(
        &self,
        write: &crate::trading_repository::write_scope::TenantWriteScope,
        kind: &str,
        subject: &str,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        if subject.trim().is_empty() {
            return Err(RepositoryError::Validation("subject"));
        }
        self.db
            .timed(
                "tenant_recon_enqueue",
                sqlx::query(
                    r#"INSERT INTO reconciliation_state
                           (organization_id, kind, subject, status, next_attempt_at)
                       VALUES ($1, $2, $3, 'pending', $4)
                       ON CONFLICT (organization_id, kind, subject) DO UPDATE SET
                           status = 'pending',
                           next_attempt_at = EXCLUDED.next_attempt_at,
                           updated_at = now()"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(kind)
                .bind(subject)
                .bind(next_attempt_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// The read side for callers that only need the intent queries.
    pub fn intents(&self) -> &TenantIntentRead {
        &self.intents
    }
}
