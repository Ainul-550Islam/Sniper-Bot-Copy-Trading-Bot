//! Tenant-scoped execution + transaction reads (PROMPT 3/10 §C16).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_rows_org;

use super::model::{execution_from_row, transaction_from_row, TenantExecution, TenantTransaction};

/// Read-side tenant execution repository (`executions`, `transactions`).
pub struct TenantExecutionRead {
    db: Arc<Database>,
}

impl TenantExecutionRead {
    pub fn new(db: Arc<Database>) -> Self {
        TenantExecutionRead { db }
    }

    /// One execution row by id, scoped to the acting tenant. The id is
    /// a bigserial — globally unique — but the row is only ever
    /// returned when it belongs to the acting organization.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        execution_id: i64,
    ) -> Result<Option<TenantExecution>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_execution_get",
                sqlx::query(
                    r#"SELECT * FROM executions
                        WHERE organization_id = $1 AND id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(execution_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(execution_from_row))
    }

    /// Chronological execution log of one of the acting tenant's orders
    /// (foreign order ids return the tenant's own empty set — never
    /// another tenant's rows, never an existence leak).
    pub async fn list_for_order(
        &self,
        scope: &TradingQueryScope,
        order_id: &str,
        limit: i64,
    ) -> Result<Vec<TenantExecution>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_executions_for_order",
                sqlx::query(
                    r#"SELECT * FROM executions
                        WHERE organization_id = $1 AND order_id = $2
                        ORDER BY ts ASC, id ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(order_id)
                .bind(limit.clamp(1, 10_000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantExecution> = rows.iter().map(execution_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's executions in a time window (latency
    /// reporting / incident review).
    pub async fn list_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TenantExecution>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_executions_between",
                sqlx::query(
                    r#"SELECT * FROM executions
                        WHERE organization_id = $1 AND ts >= $2 AND ts < $3
                        ORDER BY ts ASC, id ASC LIMIT $4"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .bind(limit.clamp(1, 10_000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantExecution> = rows.iter().map(execution_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// One transaction by chain signature — GLOBAL identity, TENANT
    /// ownership: the row is returned only when its organization_id
    /// matches the acting tenant. A foreign attribution yields `None`
    /// (no existence leak).
    pub async fn transaction(
        &self,
        scope: &TradingQueryScope,
        signature: &str,
    ) -> Result<Option<TenantTransaction>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_transaction_get",
                sqlx::query(
                    r#"SELECT * FROM transactions
                        WHERE organization_id = $1 AND signature = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(signature)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(transaction_from_row))
    }

    /// Transactions of the acting tenant linked to one of its orders.
    pub async fn transactions_for_order(
        &self,
        scope: &TradingQueryScope,
        order_id: &str,
    ) -> Result<Vec<TenantTransaction>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_transactions_for_order",
                sqlx::query(
                    r#"SELECT * FROM transactions
                        WHERE organization_id = $1 AND order_id = $2
                        ORDER BY submitted_at ASC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(order_id)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantTransaction> = rows.iter().map(transaction_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Transactions of the acting tenant in a non-terminal status
    /// (reconciliation input) — never another tenant's.
    pub async fn transactions_in_flight(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantTransaction>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_transactions_in_flight",
                sqlx::query(
                    r#"SELECT * FROM transactions
                        WHERE organization_id = $1
                          AND status IN ('submitted','confirmed')
                          AND landed = false
                        ORDER BY submitted_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantTransaction> = rows.iter().map(transaction_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
