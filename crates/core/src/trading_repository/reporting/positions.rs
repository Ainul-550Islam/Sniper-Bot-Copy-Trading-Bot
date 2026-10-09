//! Tenant position reporting (PROMPT 3/10 §I55).

use std::sync::Arc;

use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

use super::model::TenantPositionSummary;

/// Tenant-scoped position reporting.
pub struct TenantPositionReport {
    db: Arc<Database>,
}

impl TenantPositionReport {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPositionReport { db }
    }

    /// One-row position book summary for the acting tenant — every
    /// aggregate lives in the SQL statement.
    pub async fn summary(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<TenantPositionSummary, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_report_position_summary",
                sqlx::query(
                    r#"SELECT
                           COUNT(*) FILTER (WHERE status = 'open')::bigint AS open_count,
                           COUNT(*) FILTER (WHERE status = 'closing')::bigint AS closing_count,
                           COUNT(*) FILTER (WHERE status IN
                               ('closed','stopped_out','failed'))::bigint AS closed_count,
                           COALESCE(SUM(cost_basis) FILTER
                               (WHERE status IN ('open','closing')), 0)::double precision
                               AS open_cost_basis,
                           COALESCE(SUM(realized_quote) FILTER
                               (WHERE status IN ('closed','stopped_out')), 0)::double precision
                               AS realized_quote_total
                         FROM positions
                        WHERE organization_id = $1"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_one(self.db.pool()),
            )
            .await?;
        Ok(TenantPositionSummary {
            organization_id: scope.organization_id(),
            open_count: row.try_get("open_count").unwrap_or(0),
            closing_count: row.try_get("closing_count").unwrap_or(0),
            closed_count: row.try_get("closed_count").unwrap_or(0),
            open_cost_basis: row.try_get("open_cost_basis").unwrap_or(0.0),
            realized_quote_total: row.try_get("realized_quote_total").unwrap_or(0.0),
        })
    }
}
