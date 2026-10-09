//! Tenant-scoped copy reads (PROMPT 3/10 §F37).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::{assert_optional_row_org, assert_rows_org};

use super::model::{
    copy_event_from_row, leader_event_from_row, leader_from_row, link_from_row, TenantCopyEvent,
    TenantCopyLink, TenantLeader, TenantLeaderEvent,
};

/// Read-side tenant copy repository (`copy_leaders`,
/// `copy_leader_events`, `copy_events`, `copy_links`).
pub struct TenantCopyRead {
    db: Arc<Database>,
}

impl TenantCopyRead {
    pub fn new(db: Arc<Database>) -> Self {
        TenantCopyRead { db }
    }

    /// One leader row by address — only the acting tenant's
    /// configuration of that address (another tenant following the
    /// same leader is invisible).
    pub async fn leader(
        &self,
        scope: &TradingQueryScope,
        address: &str,
    ) -> Result<Option<TenantLeader>, RepositoryError> {
        if address.trim().is_empty() {
            return Err(RepositoryError::Validation("address"));
        }
        let row = self
            .db
            .timed(
                "tenant_copy_leader_get",
                sqlx::query(
                    r#"SELECT * FROM copy_leaders
                        WHERE organization_id = $1 AND address = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(address)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(leader_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's leaders (including removed ones — the
    /// caller decides).
    pub async fn leaders(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantLeader>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_copy_leaders",
                sqlx::query(
                    r#"SELECT * FROM copy_leaders
                        WHERE organization_id = $1
                        ORDER BY followed_at ASC, address ASC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantLeader> = rows.iter().map(leader_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Lifecycle history of one of the acting tenant's leaders.
    pub async fn leader_events(
        &self,
        scope: &TradingQueryScope,
        address: &str,
        limit: i64,
    ) -> Result<Vec<TenantLeaderEvent>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_copy_leader_events",
                sqlx::query(
                    r#"SELECT * FROM copy_leader_events
                        WHERE organization_id = $1 AND address = $2
                        ORDER BY ts ASC, id ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(address)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantLeaderEvent> = rows.iter().map(leader_event_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// One copy event by id — only the acting tenant's (two tenants
    /// following the same leader hold separate rows for the same
    /// leader trade, 0029).
    pub async fn event(
        &self,
        scope: &TradingQueryScope,
        event_id: &str,
    ) -> Result<Option<TenantCopyEvent>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_copy_event_get",
                sqlx::query(
                    r#"SELECT * FROM copy_events
                        WHERE organization_id = $1 AND event_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(event_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(copy_event_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's events observed at/after `since` — the
    /// restart recovery re-seeds the tenant's dedup facade from these.
    pub async fn events_since(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<TenantCopyEvent>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_copy_events_since",
                sqlx::query(
                    r#"SELECT * FROM copy_events
                        WHERE organization_id = $1 AND observed_at >= $2
                        ORDER BY observed_at ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(limit.clamp(1, 10_000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantCopyEvent> = rows.iter().map(copy_event_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Most recent events of one of the acting tenant's leaders,
    /// newest first.
    pub async fn events_for_leader(
        &self,
        scope: &TradingQueryScope,
        leader: &str,
        limit: i64,
    ) -> Result<Vec<TenantCopyEvent>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_copy_events_for_leader",
                sqlx::query(
                    r#"SELECT * FROM copy_events
                        WHERE organization_id = $1 AND leader = $2
                        ORDER BY observed_at DESC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(leader)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantCopyEvent> = rows.iter().map(copy_event_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// One copy link by the follower's position id — only the acting
    /// tenant's.
    pub async fn link(
        &self,
        scope: &TradingQueryScope,
        position_id: &str,
    ) -> Result<Option<TenantCopyLink>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_copy_link_get",
                sqlx::query(
                    r#"SELECT * FROM copy_links
                        WHERE organization_id = $1 AND position_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(position_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let found = row.as_ref().map(link_from_row);
        assert_optional_row_org(scope.organization_id(), &found)?;
        Ok(found)
    }

    /// The acting tenant's OPEN links (reconciliation input).
    pub async fn open_links(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantCopyLink>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_copy_open_links",
                sqlx::query(
                    r#"SELECT * FROM copy_links
                        WHERE organization_id = $1 AND status = 'open'
                        ORDER BY opened_at ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantCopyLink> = rows.iter().map(link_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's open links to one leader+mint pair
    /// (mirror-sizing continuity check).
    pub async fn open_links_for(
        &self,
        scope: &TradingQueryScope,
        leader: &str,
        mint: &str,
    ) -> Result<Vec<TenantCopyLink>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_copy_open_links_for",
                sqlx::query(
                    r#"SELECT * FROM copy_links
                        WHERE organization_id = $1
                          AND leader = $2 AND mint = $3 AND status = 'open'
                        ORDER BY opened_at ASC LIMIT 1000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(leader)
                .bind(mint)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantCopyLink> = rows.iter().map(link_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
