//! Tenant-scoped position reads (PROMPT 3/10 §D24).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::not_found::{tenant_not_found, ResourceKind};
use crate::trading_repository::pagination::{fetch_window, TenantPage, TenantPageRequest};
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::{assert_optional_row_org, assert_rows_org};

use super::model::{position_from_row, TenantPosition};

/// Read-side tenant position repository (`positions`).
pub struct TenantPositionRead {
    db: Arc<Database>,
}

impl TenantPositionRead {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPositionRead { db }
    }

    /// One position by id, scoped to the acting tenant.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        position_id: &str,
    ) -> Result<TenantPosition, RepositoryError> {
        if position_id.trim().is_empty() {
            return Err(RepositoryError::Validation("position_id"));
        }
        let row = self
            .db
            .timed(
                "tenant_position_get",
                sqlx::query(
                    r#"SELECT * FROM positions
                        WHERE organization_id = $1 AND id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(position_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(position_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        tenant_not_found(ResourceKind::Position, found)
    }

    /// The acting tenant's live positions (restart-recovery input).
    pub async fn list_open(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantPosition>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_positions_open",
                sqlx::query(
                    r#"SELECT * FROM positions
                        WHERE organization_id = $1 AND status IN ('open','closing')
                        ORDER BY opened_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPosition> = rows.iter().map(position_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Keyset-paginated positions of the acting tenant, newest update
    /// first. The cursor is tenant-bound (a foreign cursor is rejected
    /// by `TenantPageRequest::new`).
    pub async fn list_page(
        &self,
        scope: &TradingQueryScope,
        page: &TenantPageRequest,
    ) -> Result<TenantPage<TenantPosition>, RepositoryError> {
        let org = scope.organization_id();
        let rows = match &page.cursor {
            None => {
                self.db
                    .timed(
                        "tenant_positions_page_first",
                        sqlx::query(
                            r#"SELECT * FROM positions
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
                        "tenant_positions_page_next",
                        sqlx::query(
                            r#"SELECT * FROM positions
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
        let rows: Vec<TenantPosition> = rows.iter().map(position_from_row).collect();
        assert_rows_org(org, &rows)?;
        Ok(TenantPage::from_fetched(
            org,
            rows,
            page.limit,
            |p| p.id.clone(),
            |p| p.updated_at,
        ))
    }

    /// The acting tenant's positions closed in a window (realized-PnL
    /// reporting input — 0034 index `positions_org_closed_idx`).
    pub async fn list_closed_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantPosition>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_positions_closed_between",
                sqlx::query(
                    r#"SELECT * FROM positions
                        WHERE organization_id = $1
                          AND closed_at IS NOT NULL
                          AND closed_at >= $2 AND closed_at < $3
                        ORDER BY closed_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPosition> = rows.iter().map(position_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
