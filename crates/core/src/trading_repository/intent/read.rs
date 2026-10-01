//! Tenant-scoped intent reads (PROMPT 3/10 §E31).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::{assert_optional_row_org, assert_rows_org};

use super::model::{intent_from_row, TenantIntent};

/// Read-side tenant intent repository (`execution_intents`).
pub struct TenantIntentRead {
    db: Arc<Database>,
}

impl TenantIntentRead {
    pub fn new(db: Arc<Database>) -> Self {
        TenantIntentRead { db }
    }

    /// One intent by id — only the acting tenant's. After the 0031
    /// swap the same intent_id may exist for another tenant; that row
    /// is invisible here.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        intent_id: &str,
    ) -> Result<Option<TenantIntent>, RepositoryError> {
        if intent_id.trim().is_empty() {
            return Err(RepositoryError::Validation("intent_id"));
        }
        let row = self
            .db
            .timed(
                "tenant_intent_get",
                sqlx::query(
                    r#"SELECT * FROM execution_intents
                        WHERE organization_id = $1 AND intent_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(intent_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(intent_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's PENDING intents older than `cutoff` —
    /// orphans whose outcome is unknown (crash between record and
    /// link/abandon). Ordered oldest first. This is the tenant-scoped
    /// recovery input: "which tenant owns this record?" is answered by
    /// the predicate itself.
    pub async fn list_orphaned(
        &self,
        scope: &TradingQueryScope,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TenantIntent>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_intent_orphans",
                sqlx::query(
                    r#"SELECT * FROM execution_intents
                        WHERE organization_id = $1
                          AND status = 'pending' AND created_at < $2
                        ORDER BY created_at ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(cutoff)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantIntent> = rows.iter().map(intent_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's SUBMITTED intents in a window (linked but
    /// not yet reconciled — rebroadcast-decision input).
    pub async fn list_submitted_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantIntent>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_intent_submitted",
                sqlx::query(
                    r#"SELECT * FROM execution_intents
                        WHERE organization_id = $1
                          AND status = 'submitted'
                          AND created_at >= $2 AND created_at < $3
                        ORDER BY created_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantIntent> = rows.iter().map(intent_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
