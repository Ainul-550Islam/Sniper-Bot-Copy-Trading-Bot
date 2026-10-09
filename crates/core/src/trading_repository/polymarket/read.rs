//! Tenant-scoped polymarket reads (PROMPT 3/10 §G43).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::pagination::{fetch_window, TenantPage, TenantPageRequest};
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::{assert_optional_row_org, assert_rows_org};

use super::model::{
    fill_from_row, order_from_row, signal_from_row, TenantPolyFill, TenantPolyOrder,
    TenantPolySignal,
};

/// Read-side tenant polymarket repository (`poly_signals`,
/// `poly_orders`, `poly_fills`).
pub struct TenantPolyRead {
    db: Arc<Database>,
}

impl TenantPolyRead {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPolyRead { db }
    }

    /// One signal by id — only the acting tenant's (0030 composite).
    pub async fn signal(
        &self,
        scope: &TradingQueryScope,
        signal_id: &str,
    ) -> Result<Option<TenantPolySignal>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_poly_signal_get",
                sqlx::query(
                    r#"SELECT * FROM poly_signals
                        WHERE organization_id = $1 AND signal_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(signal_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(signal_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's signals updated at/after `since` (pipeline
    /// restart re-seed, newest-stage first).
    pub async fn signals_since(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TenantPolySignal>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_poly_signals_since",
                sqlx::query(
                    r#"SELECT * FROM poly_signals
                        WHERE organization_id = $1 AND updated_at >= $2
                        ORDER BY updated_at ASC, signal_id ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(limit.clamp(1, 10_000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPolySignal> = rows.iter().map(signal_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's mirror-book state for one venue order —
    /// only the tenant's own mirror of that order (0030 composite:
    /// another tenant mirroring the same venue order is invisible).
    pub async fn order(
        &self,
        scope: &TradingQueryScope,
        venue_order_id: &str,
    ) -> Result<Option<TenantPolyOrder>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_poly_order_get",
                sqlx::query(
                    r#"SELECT * FROM poly_orders
                        WHERE organization_id = $1 AND venue_order_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(venue_order_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(order_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's OPEN mirror orders (`closed_at IS NULL` —
    /// restart recovery re-adopts these and asks the venue for truth).
    pub async fn open_orders(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantPolyOrder>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_poly_open_orders",
                sqlx::query(
                    r#"SELECT * FROM poly_orders
                        WHERE organization_id = $1 AND closed_at IS NULL
                        ORDER BY submitted_at ASC, venue_order_id ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPolyOrder> = rows.iter().map(order_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Keyset-paginated mirror orders of the acting tenant.
    pub async fn orders_page(
        &self,
        scope: &TradingQueryScope,
        page: &TenantPageRequest,
    ) -> Result<TenantPage<TenantPolyOrder>, RepositoryError> {
        let org = scope.organization_id();
        let rows = match &page.cursor {
            None => {
                self.db
                    .timed(
                        "tenant_poly_orders_first",
                        sqlx::query(
                            r#"SELECT * FROM poly_orders
                            WHERE organization_id = $1
                            ORDER BY submitted_at DESC, venue_order_id DESC LIMIT $2"#,
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
                        "tenant_poly_orders_next",
                        sqlx::query(
                            r#"SELECT * FROM poly_orders
                            WHERE organization_id = $1
                              AND (submitted_at, venue_order_id) < ($2, $3)
                            ORDER BY submitted_at DESC, venue_order_id DESC LIMIT $4"#,
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
        let rows: Vec<TenantPolyOrder> = rows.iter().map(order_from_row).collect();
        assert_rows_org(org, &rows)?;
        Ok(TenantPage::from_fetched(
            org,
            rows,
            page.limit,
            |o| o.venue_order_id.clone(),
            |o| o.submitted_at,
        ))
    }

    /// One fill by id — only the acting tenant's (0030 composite).
    pub async fn fill(
        &self,
        scope: &TradingQueryScope,
        fill_id: &str,
    ) -> Result<Option<TenantPolyFill>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_poly_fill_get",
                sqlx::query(
                    r#"SELECT * FROM poly_fills
                        WHERE organization_id = $1 AND fill_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(fill_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(fill_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's fills for one of its venue orders.
    pub async fn fills_for_order(
        &self,
        scope: &TradingQueryScope,
        venue_order_id: &str,
    ) -> Result<Vec<TenantPolyFill>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_poly_fills_for_order",
                sqlx::query(
                    r#"SELECT * FROM poly_fills
                        WHERE organization_id = $1 AND venue_order_id = $2
                        ORDER BY ts ASC, fill_id ASC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(venue_order_id)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPolyFill> = rows.iter().map(fill_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's fills in a window (PnL/volume reporting —
    /// 0034 index `poly_fills_org_ts_idx`).
    pub async fn fills_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantPolyFill>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_poly_fills_between",
                sqlx::query(
                    r#"SELECT * FROM poly_fills
                        WHERE organization_id = $1 AND ts >= $2 AND ts < $3
                        ORDER BY ts ASC, fill_id ASC LIMIT 10_000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPolyFill> = rows.iter().map(fill_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
