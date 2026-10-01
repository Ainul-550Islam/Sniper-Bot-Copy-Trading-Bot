//! Tenant-scoped order reads (PROMPT 3/10 §B10).
//!
//! Every query carries `organization_id = $1` in the SQL predicate.
//! Point lookups by id, idempotency key or signature are tenant-AND-id;
//! a foreign id yields the tenant-safe `NotFound` (indistinguishable
//! from absence), never the row.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::not_found::{tenant_not_found, ResourceKind};
use crate::trading_repository::pagination::{fetch_window, TenantPage, TenantPageRequest};
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::{assert_optional_row_org, assert_rows_org};

use super::model::{order_from_row, status_entry_from_row, TenantOrder, TenantOrderStatusEntry};

/// Read-side tenant order repository (`orders`, `order_status_history`).
pub struct TenantOrderRead {
    db: Arc<Database>,
}

impl TenantOrderRead {
    pub fn new(db: Arc<Database>) -> Self {
        TenantOrderRead { db }
    }

    /// One order by id, scoped to the acting tenant.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        order_id: &str,
    ) -> Result<TenantOrder, RepositoryError> {
        if order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        let row = self
            .db
            .timed(
                "tenant_order_get",
                sqlx::query(
                    r#"SELECT * FROM orders
                        WHERE organization_id = $1 AND id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(order_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(order_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        tenant_not_found(ResourceKind::Order, found)
    }

    /// One order by idempotency key, scoped to the acting tenant. After
    /// the 0026 composite swap the same key MAY exist for another
    /// tenant — this lookup answers only with the acting tenant's row.
    pub async fn get_by_key(
        &self,
        scope: &TradingQueryScope,
        idempotency_key: &str,
    ) -> Result<Option<TenantOrder>, RepositoryError> {
        if idempotency_key.trim().is_empty() {
            return Err(RepositoryError::Validation("idempotency_key"));
        }
        let row = self
            .db
            .timed(
                "tenant_order_by_key",
                sqlx::query(
                    r#"SELECT * FROM orders
                        WHERE organization_id = $1 AND idempotency_key = $2
                        ORDER BY created_at DESC LIMIT 1"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(idempotency_key)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(order_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The most recent order carrying a chain signature, scoped to the
    /// acting tenant.
    pub async fn get_by_signature(
        &self,
        scope: &TradingQueryScope,
        signature: &str,
    ) -> Result<Option<TenantOrder>, RepositoryError> {
        if signature.trim().is_empty() {
            return Err(RepositoryError::Validation("signature"));
        }
        let row = self
            .db
            .timed(
                "tenant_order_by_signature",
                sqlx::query(
                    r#"SELECT * FROM orders
                        WHERE organization_id = $1 AND signature = $2
                        ORDER BY created_at DESC LIMIT 1"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(signature)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(order_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// Non-terminal orders of the acting tenant (recovery and
    /// reconciliation input). Never returns another tenant's rows.
    pub async fn list_incomplete(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantOrder>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_orders_incomplete",
                sqlx::query(
                    r#"SELECT * FROM orders
                        WHERE organization_id = $1
                          AND status NOT IN
                              ('filled','failed','cancelled','expired','reconciled')
                        ORDER BY created_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let orders: Vec<TenantOrder> = rows.iter().map(order_from_row).collect();
        assert_rows_org(scope.organization_id(), &orders)?;
        Ok(orders)
    }

    /// Keyset-paginated recent orders of the acting tenant. The cursor
    /// is tenant-bound; a cursor minted for another organization is
    /// rejected by `TenantPageRequest::new`.
    pub async fn list_page(
        &self,
        scope: &TradingQueryScope,
        page: &TenantPageRequest,
    ) -> Result<TenantPage<TenantOrder>, RepositoryError> {
        let org = scope.organization_id();
        let rows = match &page.cursor {
            None => {
                self.db
                    .timed(
                        "tenant_orders_page_first",
                        sqlx::query(
                            r#"SELECT * FROM orders
                            WHERE organization_id = $1
                            ORDER BY updated_at DESC, id DESC LIMIT $2"#,
                        )
                        .bind(org.as_uuid())
                        .bind(fetch_window(page.limit as i64))
                        .fetch_all(self.db.pool()),
                    )
                    .await?
            }
            Some(cursor) => {
                self.db
                    .timed(
                        "tenant_orders_page_next",
                        sqlx::query(
                            r#"SELECT * FROM orders
                            WHERE organization_id = $1
                              AND (updated_at, id) < ($2, $3)
                            ORDER BY updated_at DESC, id DESC LIMIT $4"#,
                        )
                        .bind(org.as_uuid())
                        .bind(cursor.at)
                        .bind(&cursor.sort_key)
                        .bind(fetch_window(page.limit as i64))
                        .fetch_all(self.db.pool()),
                    )
                    .await?
            }
        };
        let orders: Vec<TenantOrder> = rows.iter().map(order_from_row).collect();
        assert_rows_org(org, &orders)?;
        Ok(TenantPage::from_fetched(
            org,
            orders,
            page.limit,
            |o| o.id.clone(),
            |o| o.updated_at,
        ))
    }

    /// Count of the acting tenant's orders per status (reporting).
    pub async fn count_by_status(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<(String, i64)>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_orders_count_by_status",
                sqlx::query(
                    r#"SELECT status, count(*)::bigint AS n FROM orders
                        WHERE organization_id = $1
                        GROUP BY status ORDER BY status"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("status").unwrap_or_default(),
                    r.try_get::<i64, _>("n").unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Status history of one order of the acting tenant (oldest first).
    /// The history table carries no organization column of its own; the
    /// query joins through the tenant-constrained order row, so a
    /// foreign order id yields the tenant-safe `NotFound`.
    pub async fn history(
        &self,
        scope: &TradingQueryScope,
        order_id: &str,
        limit: i64,
    ) -> Result<Vec<TenantOrderStatusEntry>, RepositoryError> {
        if order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("order_id"));
        }
        let rows = self
            .db
            .timed(
                "tenant_order_history",
                sqlx::query(
                    r#"SELECT h.id, h.order_id, h.from_status, h.to_status,
                              h.reason, h.ts, o.organization_id
                         FROM order_status_history h
                         JOIN orders o ON o.id = h.order_id
                        WHERE o.organization_id = $1 AND h.order_id = $2
                        ORDER BY h.ts ASC, h.id ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(order_id)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let entries: Vec<TenantOrderStatusEntry> = rows.iter().map(status_entry_from_row).collect();
        assert_rows_org(scope.organization_id(), &entries)?;
        Ok(entries)
    }

    /// Orders of the acting tenant created in a time window
    /// (reconciliation/recovery sweeps). `since` inclusive, `until`
    /// exclusive.
    pub async fn list_created_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantOrder>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_orders_created_between",
                sqlx::query(
                    r#"SELECT * FROM orders
                        WHERE organization_id = $1
                          AND created_at >= $2 AND created_at < $3
                        ORDER BY created_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let orders: Vec<TenantOrder> = rows.iter().map(order_from_row).collect();
        assert_rows_org(scope.organization_id(), &orders)?;
        Ok(orders)
    }
}
