//! Tenant PnL reporting (PROMPT 3/10 §I57).
//!
//! Realized PnL uses persisted cumulative exit PnL over closed positions.
//! Legacy rows without a trustworthy partial-exit allocation and closed
//! lifecycles spanning the report window's start fail closed instead of
//! being reconstructed from remaining cost basis or misattributed by close date.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

use super::model::{symbol_pnl_from_row, TenantRealizedPnl, TenantSymbolPnl};

/// Tenant-scoped PnL reporting.
pub struct TenantPnlReport {
    db: Arc<Database>,
}

impl TenantPnlReport {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPnlReport { db }
    }

    /// Realized PnL of the acting tenant over `[since, until)`:
    /// positions closed in the window contribute their persisted cumulative
    /// `realized_pnl_quote`; unknown legacy rows return a storage error.
    pub async fn realized_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<TenantRealizedPnl, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_report_realized_pnl",
                sqlx::query(
                    r#"SELECT
                           CASE WHEN COUNT(*) FILTER (
                                    WHERE p.realized_pnl_quote IS NULL OR p.opened_at < $2
                                ) > 0 THEN NULL::double precision
                                ELSE COALESCE(SUM(p.realized_pnl_quote), 0)::double precision
                           END AS realized_net,
                           COUNT(*)::bigint AS positions_closed,
                           (SELECT COALESCE(SUM(t.fee), 0)::double precision
                              FROM trades t
                             WHERE t.organization_id = $1
                               AND t.ts >= $2 AND t.ts < $3) AS fees_paid
                         FROM positions p
                        WHERE p.organization_id = $1
                          AND p.closed_at IS NOT NULL
                          AND p.closed_at >= $2 AND p.closed_at < $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_one(self.db.pool()),
            )
            .await?;
        Ok(TenantRealizedPnl {
            organization_id: scope.organization_id(),
            realized_net: row
                .try_get::<Option<f64>, _>("realized_net")
                .ok()
                .flatten()
                .ok_or_else(|| {
                    RepositoryError::Storage("realized PnL projection is unknown".into())
                })?,
            positions_closed: row.try_get("positions_closed").unwrap_or(0),
            fees_paid: row.try_get("fees_paid").unwrap_or(0.0),
        })
    }

    /// The acting tenant's realized PnL for TODAY (UTC) — the
    /// tenant-scoped twin of the risk oracle's realized_today.
    pub async fn realized_today(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<TenantRealizedPnl, RepositoryError> {
        let day_start = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .map(|t| t.and_utc())
            .unwrap_or_else(Utc::now);
        self.realized_between(scope, day_start, day_start + chrono::Duration::days(1))
            .await
    }

    /// Per-symbol realized PnL rollup for the acting tenant over a
    /// window (top symbols by realized amount).
    pub async fn by_symbol(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TenantSymbolPnl>, RepositoryError> {
        let unknown = self
            .db
            .timed(
                "tenant_report_pnl_projection_check",
                sqlx::query(
                    "SELECT EXISTS (SELECT 1 FROM positions p
                     WHERE p.organization_id = $1 AND p.closed_at >= $2 AND p.closed_at < $3
                       AND (p.realized_pnl_quote IS NULL OR p.opened_at < $2)) AS unknown",
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_one(self.db.pool()),
            )
            .await?;
        if unknown.try_get::<bool, _>("unknown").unwrap_or(true) {
            return Err(RepositoryError::Storage(
                "realized PnL projection is unknown".into(),
            ));
        }
        let rows = self
            .db
            .timed(
                "tenant_report_pnl_by_symbol",
                sqlx::query(
                    r#"SELECT p.organization_id, p.symbol,
                              COUNT(*)::bigint AS closed_trades,
                              SUM(p.realized_pnl_quote)::double precision AS realized_quote,
                              0::double precision AS fees
                         FROM positions p
                        WHERE p.organization_id = $1
                          AND p.closed_at IS NOT NULL
                          AND p.closed_at >= $2 AND p.closed_at < $3
                          AND p.realized_pnl_quote IS NOT NULL
                        GROUP BY p.organization_id, p.symbol
                        ORDER BY realized_quote DESC, symbol ASC
                        LIMIT $4"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .bind(limit.clamp(1, 200))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(symbol_pnl_from_row).collect())
    }
}
