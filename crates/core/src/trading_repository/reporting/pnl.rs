//! Tenant PnL reporting (PROMPT 3/10 §I57).
//!
//! Realized PnL mirrors the risk oracle's shape
//! (`SUM(realized_quote - cost_basis)` over closed positions, day
//! window by default) but is computed for the acting tenant IN SQL.
//! The legacy deployment-org path stays in the risk oracle; the data
//! plane serves this tenant-scoped twin.

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
    /// positions closed in the window contribute
    /// `realized_quote - cost_basis`; closed-position count and trade
    /// fees come along.
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
                           COALESCE(SUM(p.realized_quote - p.cost_basis), 0)::double precision
                               AS realized_net,
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
            realized_net: row.try_get("realized_net").unwrap_or(0.0),
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
        let rows = self
            .db
            .timed(
                "tenant_report_pnl_by_symbol",
                sqlx::query(
                    r#"SELECT p.organization_id, p.symbol,
                              COUNT(*)::bigint AS closed_trades,
                              COALESCE(SUM(p.realized_quote - p.cost_basis), 0)::double precision
                                  AS realized_quote,
                              0::double precision AS fees
                         FROM positions p
                        WHERE p.organization_id = $1
                          AND p.closed_at IS NOT NULL
                          AND p.closed_at >= $2 AND p.closed_at < $3
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
