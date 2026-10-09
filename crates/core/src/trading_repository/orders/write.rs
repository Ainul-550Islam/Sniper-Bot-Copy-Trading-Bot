//! Tenant-scoped order writes (PROMPT 3/10 §B11).
//!
//! Every INSERT binds the acting tenant's `organization_id`
//! explicitly; every UPDATE/DELETE carries
//! `organization_id = $1 AND id = $2`. The idempotent insert uses the
//! 0026 tenant-composite arbiter `ON CONFLICT (organization_id,
//! idempotency_key)` — a duplicate key of ANOTHER tenant never
//! collapses this tenant's order, and a duplicate of THIS tenant is
//! reported to the caller as [`RepositoryError::Conflict`] (via
//! `Ok(false)`, mirroring the legacy `insert_if_absent` contract).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::TenantOrder;

/// Write-side tenant order repository (`orders`, `order_status_history`).
pub struct TenantOrderWrite {
    db: Arc<Database>,
}

impl TenantOrderWrite {
    pub fn new(db: Arc<Database>) -> Self {
        TenantOrderWrite { db }
    }

    /// Insert an order for the acting tenant unless its idempotency key
    /// already exists FOR THIS TENANT (0026 arbiter).
    ///
    /// `Ok(true)` = inserted; `Ok(false)` = this tenant already placed
    /// this intent (caller fetches via
    /// [`super::read::TenantOrderRead::get_by_key`]); another tenant's
    /// identical key is invisible and irrelevant to the outcome.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_if_absent(
        &self,
        write: &TenantWriteScope,
        id: &str,
        idempotency_key: Option<&str>,
        module: &str,
        side: &str,
        symbol: &str,
        venue: &str,
        mode: &str,
        status: &str,
        qty: f64,
        price: Option<f64>,
        meta: &serde_json::Value,
        at: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        if id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        if module.trim().is_empty() || side.trim().is_empty() || symbol.trim().is_empty() {
            return Err(RepositoryError::Validation("order fields"));
        }
        let res = self
            .db
            .timed(
                "tenant_order_insert",
                sqlx::query(
                    r#"INSERT INTO orders
                        (organization_id, id, idempotency_key, module, side, symbol,
                         venue, mode, status, qty, price, meta, created_at, updated_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13)
                       ON CONFLICT (organization_id, idempotency_key) DO NOTHING"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(id)
                .bind(idempotency_key)
                .bind(module)
                .bind(side)
                .bind(symbol)
                .bind(venue)
                .bind(mode)
                .bind(status)
                .bind(qty)
                .bind(price)
                .bind(meta)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Full-row upsert for mirror refresh and recovery writes, scoped
    /// to the acting tenant (both the insert attribution and the
    /// conflict update target the tenant's own row).
    pub async fn upsert(
        &self,
        write: &TenantWriteScope,
        order: &TenantOrder,
    ) -> Result<(), RepositoryError> {
        if order.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if order.id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        self.db
            .timed(
                "tenant_order_upsert",
                sqlx::query(
                    r#"INSERT INTO orders
                        (organization_id, id, idempotency_key, module, side, symbol, venue, mode,
                         status, qty, price, external_id, signature, error, meta,
                         created_at, updated_at, submitted_at, finished_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
                       ON CONFLICT (id) DO UPDATE SET
                         status = EXCLUDED.status,
                         qty = EXCLUDED.qty,
                         price = EXCLUDED.price,
                         external_id = COALESCE(EXCLUDED.external_id, orders.external_id),
                         signature = COALESCE(EXCLUDED.signature, orders.signature),
                         error = EXCLUDED.error,
                         meta = EXCLUDED.meta,
                         updated_at = EXCLUDED.updated_at,
                         submitted_at = COALESCE(EXCLUDED.submitted_at, orders.submitted_at),
                         finished_at = EXCLUDED.finished_at
                       WHERE orders.organization_id = $1"#,
                )
                .bind(order.organization_id.as_uuid())
                .bind(&order.id)
                .bind(&order.idempotency_key)
                .bind(&order.module)
                .bind(&order.side)
                .bind(&order.symbol)
                .bind(&order.venue)
                .bind(&order.mode)
                .bind(&order.status)
                .bind(order.qty)
                .bind(order.price)
                .bind(&order.external_id)
                .bind(&order.signature)
                .bind(&order.error)
                .bind(&order.meta)
                .bind(order.created_at)
                .bind(order.updated_at)
                .bind(order.submitted_at)
                .bind(order.finished_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Guarded status transition + history row in ONE transaction,
    /// scoped to the acting tenant. `expected_from` is the CAS guard:
    /// a row that already moved on yields
    /// [`RepositoryError::StaleWrite`] and NOTHING is written.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_status(
        &self,
        write: &TenantWriteScope,
        order_id: &str,
        expected_from: &str,
        to_status: &str,
        reason: Option<&str>,
        error: Option<&str>,
        submitted_at: Option<DateTime<Utc>>,
        finished_at: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        if order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        let updated = sqlx::query(
            r#"UPDATE orders SET status = $3, updated_at = $4,
                     submitted_at = COALESCE(submitted_at, $5),
                     finished_at = $6, error = COALESCE($7, error)
               WHERE organization_id = $1 AND id = $2 AND status = $8"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(order_id)
        .bind(to_status)
        .bind(at)
        .bind(submitted_at)
        .bind(finished_at)
        .bind(error)
        .bind(expected_from)
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(RepositoryError::from)?;
            // Tenant-scoped miss: absent, foreign, or the CAS guard
            // tripped. Distinguish ONLY between "not ours/not found"
            // and "ours but moved on" — never leak which.
            let exists = sqlx::query(
                r#"SELECT 1 FROM orders
                    WHERE organization_id = $1 AND id = $2"#,
            )
            .bind(write.organization_id().as_uuid())
            .bind(order_id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(RepositoryError::from)?;
            return if exists.is_some() {
                Err(RepositoryError::StaleWrite("order status"))
            } else {
                Err(RepositoryError::NotFound("order"))
            };
        }
        sqlx::query(
            r#"INSERT INTO order_status_history
                   (order_id, from_status, to_status, reason)
               SELECT id, $3, $4, $5 FROM orders
                WHERE organization_id = $1 AND id = $2"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(order_id)
        .bind(expected_from)
        .bind(to_status)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(())
    }

    /// Fill in provider-side identifiers on the acting tenant's order.
    /// A foreign order id updates ZERO rows and reports `NotFound`.
    pub async fn update_external(
        &self,
        write: &TenantWriteScope,
        order_id: &str,
        external_id: Option<&str>,
        signature: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_order_update_external",
                sqlx::query(
                    r#"UPDATE orders SET external_id = COALESCE($3, external_id),
                              signature = COALESCE($4, signature), updated_at = $5
                        WHERE organization_id = $1 AND id = $2"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(order_id)
                .bind(external_id)
                .bind(signature)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(RepositoryError::NotFound("order"))
        }
    }

    /// Cancel the acting tenant's order: a guarded transition to
    /// `cancelled` from any NON-TERMINAL status (a filled/failed/
    /// cancelled/expired/reconciled order can never be cancelled again).
    pub async fn cancel(
        &self,
        write: &TenantWriteScope,
        order_id: &str,
        reason: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        if order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        // Lock the acting tenant's row with the tenant condition INSIDE
        // the locking statement (PROMPT 3/10 rule): a foreign id locks
        // nothing and the transaction reports NotFound.
        let current = sqlx::query(
            r#"SELECT status FROM orders
                WHERE organization_id = $1 AND id = $2
                FOR UPDATE"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(order_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        let Some(from_row) = current else {
            tx.rollback().await.map_err(RepositoryError::from)?;
            return Err(RepositoryError::NotFound("order"));
        };
        let from_status: String = from_row
            .try_get("status")
            .unwrap_or_else(|_| "unknown".into());
        if matches!(
            from_status.as_str(),
            "filled" | "failed" | "cancelled" | "expired" | "reconciled"
        ) {
            tx.rollback().await.map_err(RepositoryError::from)?;
            return Err(RepositoryError::StaleWrite("order status"));
        }
        sqlx::query(
            r#"UPDATE orders SET status = 'cancelled', updated_at = $3,
                     finished_at = $3, error = COALESCE($4, error)
               WHERE organization_id = $1 AND id = $2"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(order_id)
        .bind(at)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        sqlx::query(
            r#"INSERT INTO order_status_history
                   (organization_id, order_id, from_status, to_status, reason)
               SELECT organization_id, id, $3, 'cancelled', $4 FROM orders
                WHERE organization_id = $1 AND id = $2"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(order_id)
        .bind(&from_status)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(())
    }

    /// Tenant-scoped delete (maintenance/tests): removes the acting
    /// tenant's order and its cascaded history. A foreign id deletes
    /// ZERO rows and reports `NotFound` — it can never remove another
    /// tenant's order.
    pub async fn delete(
        &self,
        write: &TenantWriteScope,
        order_id: &str,
    ) -> Result<(), RepositoryError> {
        if order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        let res = self
            .db
            .timed(
                "tenant_order_delete",
                sqlx::query("DELETE FROM orders WHERE organization_id = $1 AND id = $2")
                    .bind(write.organization_id().as_uuid())
                    .bind(order_id)
                    .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(RepositoryError::NotFound("order"))
        }
    }

    /// The read scope of this writer's tenant (writes that read first).
    pub fn query_scope(&self, write: &TenantWriteScope) -> TradingQueryScope {
        write.query_scope()
    }
}
