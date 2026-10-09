//! Tenant-scoped execution + transaction writes (PROMPT 3/10 §C17).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

/// Write-side tenant execution repository (`executions`, `transactions`).
pub struct TenantExecutionWrite {
    db: Arc<Database>,
}

impl TenantExecutionWrite {
    pub fn new(db: Arc<Database>) -> Self {
        TenantExecutionWrite { db }
    }

    /// Append one observation row against one of the acting tenant's
    /// orders. The insert attributes the tenant's organization; a
    /// foreign order_id cannot produce a row attributed to the acting
    /// tenant because the caller-facing data plane resolves order ids
    /// through the tenant-scoped reads first (and the repository never
    /// trusts a cross-tenant order reference: `note_execution` below
    /// verifies ownership transactionally).
    #[allow(clippy::too_many_arguments)]
    pub async fn append(
        &self,
        write: &TenantWriteScope,
        order_id: &str,
        kind: &str,
        endpoint: Option<&str>,
        latency_ms: Option<i64>,
        ok: bool,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<i64, RepositoryError> {
        if order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        // The order must belong to the acting tenant — checked INSIDE
        // the transaction (ownership check, not a post-filter).
        let owned = sqlx::query(r#"SELECT 1 FROM orders WHERE organization_id = $1 AND id = $2"#)
            .bind(write.organization_id().as_uuid())
            .bind(order_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(RepositoryError::from)?;
        if owned.is_none() {
            tx.rollback().await.map_err(RepositoryError::from)?;
            return Err(RepositoryError::NotFound("order"));
        }
        let row = sqlx::query(
            r#"INSERT INTO executions
                   (organization_id, order_id, ts, kind, endpoint, latency_ms, ok, detail)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               RETURNING id"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(order_id)
        .bind(at)
        .bind(kind)
        .bind(endpoint)
        .bind(latency_ms)
        .bind(ok)
        .bind(detail)
        .fetch_one(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        tx.commit().await.map_err(RepositoryError::from)?;
        row.try_get::<i64, _>("id").map_err(RepositoryError::from)
    }

    /// Record a money-moving broadcast attempt for the acting tenant.
    /// The SIGNATURE stays globally unique (chain identity — the
    /// `ON CONFLICT (signature)` arbiter is CORRECT and preserved);
    /// the row's organization attribution is the acting tenant, and a
    /// re-record of a signature another tenant already claimed yields
    /// `Ok(false)` without revealing whose it is.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_transaction_submitted(
        &self,
        write: &TenantWriteScope,
        chain: &str,
        signature: &str,
        order_id: Option<&str>,
        signer: Option<&str>,
        venue: Option<&str>,
        attempts: i32,
    ) -> Result<bool, RepositoryError> {
        if signature.trim().is_empty() {
            return Err(RepositoryError::Validation("signature"));
        }
        if let Some(order) = order_id {
            if order.trim().is_empty() {
                return Err(RepositoryError::Validation("order_id"));
            }
        }
        // The optional order must belong to the acting tenant.
        if let Some(order) = order_id {
            let owned = self
                .db
                .timed(
                    "tenant_tx_order_owned",
                    sqlx::query(
                        r#"SELECT 1 FROM orders
                            WHERE organization_id = $1 AND id = $2"#,
                    )
                    .bind(write.organization_id().as_uuid())
                    .bind(order)
                    .fetch_optional(self.db.pool()),
                )
                .await?;
            if owned.is_none() {
                return Err(RepositoryError::NotFound("order"));
            }
        }
        let row: Option<(bool,)> = self
            .db
            .timed(
                "tenant_tx_submitted",
                sqlx::query_as(
                    r#"INSERT INTO transactions
                           (organization_id, signature, chain, order_id, status,
                            signer, venue, attempts)
                       VALUES ($1, $2, $3, $4, 'submitted', $5, $6, $7)
                       ON CONFLICT (signature) DO UPDATE
                           SET attempts = GREATEST(transactions.attempts, EXCLUDED.attempts),
                               order_id = COALESCE(transactions.order_id, EXCLUDED.order_id),
                               signer   = COALESCE(transactions.signer,   EXCLUDED.signer),
                               venue    = COALESCE(transactions.venue,    EXCLUDED.venue)
                         WHERE transactions.organization_id = $1
                           AND transactions.status = 'submitted'
                       RETURNING (xmax = 0)"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(signature)
                .bind(chain)
                .bind(order_id)
                .bind(signer)
                .bind(venue)
                .bind(attempts)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.map(|(fresh,)| fresh).unwrap_or(false))
    }

    /// Update the acting tenant's transaction status (local side of the
    /// reconciliation matrix). Zero rows for a foreign/absent signature
    /// → `NotFound`.
    pub async fn set_transaction_status(
        &self,
        write: &TenantWriteScope,
        signature: &str,
        status: &str,
        slot: Option<i64>,
        error: Option<&str>,
    ) -> Result<(), RepositoryError> {
        let landed = matches!(status, "confirmed" | "finalized");
        let res = self
            .db
            .timed(
                "tenant_tx_status",
                sqlx::query(
                    r#"UPDATE transactions SET status = $3, landed = $4,
                              slot = COALESCE($5, slot), error = $6,
                              confirmed_at = CASE WHEN $4 AND confirmed_at IS NULL
                                                  THEN now() ELSE confirmed_at END
                        WHERE organization_id = $1 AND signature = $2"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(signature)
                .bind(status)
                .bind(landed)
                .bind(slot)
                .bind(error)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(RepositoryError::NotFound("transaction"))
        }
    }
}
