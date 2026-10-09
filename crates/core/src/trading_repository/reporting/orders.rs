//! Tenant order reporting (PROMPT 3/10 §I54).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

use super::model::{order_status_count_from_row, TenantOrderStatusCounts, TenantOrderSummary};

/// Tenant-scoped order reporting.
pub struct TenantOrderReport {
    db: Arc<Database>,
}

impl TenantOrderReport {
    pub fn new(db: Arc<Database>) -> Self {
        TenantOrderReport { db }
    }

    /// Count of the acting tenant's orders by status — the aggregate
    /// is scoped in SQL.
    pub async fn counts_by_status(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantOrderStatusCounts>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_report_order_counts",
                sqlx::query(
                    r#"SELECT organization_id, status, COUNT(*)::bigint AS count
                         FROM orders
                        WHERE organization_id = $1
                        GROUP BY organization_id, status
                        ORDER BY status ASC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(order_status_count_from_row).collect())
    }

    /// One-row order summary for the acting tenant.
    pub async fn summary(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<TenantOrderSummary, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_report_order_summary",
                sqlx::query(
                    r#"SELECT
                           COUNT(*)::bigint AS total,
                           COUNT(*) FILTER (
                               WHERE status NOT IN
                                   ('filled','failed','cancelled','expired','reconciled')
                           )::bigint AS open,
                           COUNT(*) FILTER (WHERE status = 'filled')::bigint AS filled,
                           COUNT(*) FILTER (WHERE status = 'failed')::bigint AS failed,
                           COUNT(*) FILTER (WHERE status = 'cancelled')::bigint AS cancelled,
                           MIN(created_at) AS first_created_at,
                           MAX(created_at) AS last_created_at
                         FROM orders
                        WHERE organization_id = $1"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_one(self.db.pool()),
            )
            .await?;
        let total: i64 = row.try_get("total").unwrap_or(0);
        let filled: i64 = row.try_get("filled").unwrap_or(0);
        let failed: i64 = row.try_get("failed").unwrap_or(0);
        let cancelled: i64 = row.try_get("cancelled").unwrap_or(0);
        let open: i64 = row.try_get("open").unwrap_or(0);
        Ok(TenantOrderSummary {
            organization_id: scope.organization_id(),
            total,
            open,
            filled,
            failed,
            cancelled,
            other: (total - filled - failed - cancelled - open).max(0),
            first_created_at: row
                .try_get::<Option<DateTime<Utc>>, _>("first_created_at")
                .ok()
                .flatten(),
            last_created_at: row
                .try_get::<Option<DateTime<Utc>>, _>("last_created_at")
                .ok()
                .flatten(),
        })
    }
}
