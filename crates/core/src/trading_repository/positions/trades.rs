//! Tenant trade history and execution-linked trades (PROMPT 3/10 §D26).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::pagination::{fetch_window, TenantPage, TenantPageRequest};
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_rows_org;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{trade_from_row, TenantTrade};

/// Tenant-scoped trades repository (`trades`).
pub struct TenantTradeRepo {
    db: Arc<Database>,
}

impl TenantTradeRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantTradeRepo { db }
    }

    /// Append one of the acting tenant's fills. Idempotent on the
    /// app-assigned trade id (`trades.id` stays the GLOBAL primary
    /// key); the insert attributes the acting tenant and the conflict
    /// leg is tenant-constrained.
    #[allow(clippy::too_many_arguments)]
    pub async fn append(
        &self,
        write: &TenantWriteScope,
        trade: &TenantTrade,
    ) -> Result<(), RepositoryError> {
        if trade.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if trade.id.trim().is_empty() {
            return Err(RepositoryError::Validation("trade_id"));
        }
        // A trade may reference one of the tenant's positions (or none).
        if let Some(position_id) = &trade.position_id {
            if position_id.trim().is_empty() {
                return Err(RepositoryError::Validation("position_id"));
            }
        }
        self.db
            .timed(
                "tenant_trade_append",
                sqlx::query(
                    r#"INSERT INTO trades
                        (organization_id, id, ts, source, venue, mode, side, symbol,
                         symbol_display, amount_in, amount_out, quote_symbol, price,
                         fee, slippage_bps, signature, position_id, note, latency_ms)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
                       ON CONFLICT (id) DO NOTHING"#,
                )
                .bind(trade.organization_id.as_uuid())
                .bind(&trade.id)
                .bind(trade.ts)
                .bind(&trade.source)
                .bind(&trade.venue)
                .bind(&trade.mode)
                .bind(&trade.side)
                .bind(&trade.symbol)
                .bind(&trade.symbol_display)
                .bind(trade.amount_in)
                .bind(trade.amount_out)
                .bind(&trade.quote_symbol)
                .bind(trade.price)
                .bind(trade.fee)
                .bind(trade.slippage_bps)
                .bind(&trade.signature)
                .bind(&trade.position_id)
                .bind(&trade.note)
                .bind(trade.latency_ms)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Chronological fill history of one of the acting tenant's
    /// positions — the authoritative execution data PnL reconstruction
    /// replays. A foreign position id yields the tenant's own EMPTY
    /// set (no existence leak).
    pub async fn list_for_position(
        &self,
        scope: &TradingQueryScope,
        position_id: &str,
    ) -> Result<Vec<TenantTrade>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_trades_for_position",
                sqlx::query(
                    r#"SELECT * FROM trades
                        WHERE organization_id = $1 AND position_id = $2
                        ORDER BY ts ASC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(position_id)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantTrade> = rows.iter().map(trade_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Keyset-paginated trade history of the acting tenant.
    pub async fn list_page(
        &self,
        scope: &TradingQueryScope,
        page: &TenantPageRequest,
    ) -> Result<TenantPage<TenantTrade>, RepositoryError> {
        let org = scope.organization_id();
        let rows = match &page.cursor {
            None => {
                self.db
                    .timed(
                        "tenant_trades_page_first",
                        sqlx::query(
                            r#"SELECT * FROM trades
                            WHERE organization_id = $1
                            ORDER BY ts DESC, id DESC LIMIT $2"#,
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
                        "tenant_trades_page_next",
                        sqlx::query(
                            r#"SELECT * FROM trades
                            WHERE organization_id = $1
                              AND (ts, id) < ($2, $3)
                            ORDER BY ts DESC, id DESC LIMIT $4"#,
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
        let rows: Vec<TenantTrade> = rows.iter().map(trade_from_row).collect();
        assert_rows_org(org, &rows)?;
        Ok(TenantPage::from_fetched(
            org,
            rows,
            page.limit,
            |t| t.id.clone(),
            |t| t.ts,
        ))
    }

    /// The acting tenant's trades in a window (volume/fee reporting —
    /// 0034 index `trades_org_symbol_ts_idx`).
    pub async fn list_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantTrade>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_trades_between",
                sqlx::query(
                    r#"SELECT * FROM trades
                        WHERE organization_id = $1 AND ts >= $2 AND ts < $3
                        ORDER BY ts ASC, id ASC LIMIT 10_000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantTrade> = rows.iter().map(trade_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
