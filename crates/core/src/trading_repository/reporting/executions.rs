//! Tenant execution reporting (PROMPT 3/10 §I56).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

use super::model::TenantExecutionStats;

/// Tenant-scoped execution reporting.
pub struct TenantExecutionReport {
    db: Arc<Database>,
}

impl TenantExecutionReport {
    pub fn new(db: Arc<Database>) -> Self {
        TenantExecutionReport { db }
    }

    /// Execution attempts + outcomes for the acting tenant in a
    /// window, plus its in-flight transaction count (the live
    /// exposure signal). All aggregates are tenant-scoped in SQL.
    ///
    /// Outcome mapping on the 0002 schema: an `executions` row IS one
    /// attempt (`kind`), `ok` carries the outcome — `true` counts as
    /// succeeded, `false` as failed; `transactions` remain keyed by
    /// their globally-unique signature with tenant attribution.
    pub async fn stats_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<TenantExecutionStats, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_report_execution_stats",
                sqlx::query(
                    r#"SELECT
                           (SELECT COUNT(*)::bigint FROM executions
                             WHERE organization_id = $1
                               AND ts >= $2 AND ts < $3) AS attempts,
                           (SELECT COUNT(*)::bigint FROM executions
                             WHERE organization_id = $1
                               AND ts >= $2 AND ts < $3
                               AND ok = true) AS succeeded,
                           (SELECT COUNT(*)::bigint FROM executions
                             WHERE organization_id = $1
                               AND ts >= $2 AND ts < $3
                               AND ok = false) AS failed,
                           (SELECT COUNT(*)::bigint FROM transactions
                             WHERE organization_id = $1
                               AND submitted_at >= $2 AND submitted_at < $3) AS submitted_tx,
                           (SELECT COUNT(*)::bigint FROM transactions
                             WHERE organization_id = $1
                               AND status IN ('submitted','confirmed')) AS in_flight_tx"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_one(self.db.pool()),
            )
            .await?;
        Ok(TenantExecutionStats {
            organization_id: scope.organization_id(),
            attempts: row.try_get("attempts").unwrap_or(0),
            succeeded: row.try_get("succeeded").unwrap_or(0),
            failed: row.try_get("failed").unwrap_or(0),
            submitted_transactions: row.try_get("submitted_tx").unwrap_or(0),
            in_flight_transactions: row.try_get("in_flight_tx").unwrap_or(0),
        })
    }
}
