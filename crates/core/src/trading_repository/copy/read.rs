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

/// Largest page of open copy links a single read will return.
pub const COPY_LINK_PAGE_MAX: i64 = 500;

/// One page of the acting tenant's open links (gap #78).
#[derive(Debug, Clone, PartialEq)]
pub struct CopyLinkPage {
    /// The page, oldest `opened_at` first.
    pub items: Vec<TenantCopyLink>,
    /// True when a further page exists.
    pub has_more: bool,
    /// Open links in this cursor window, INCLUDING this page.
    pub total_remaining: i64,
    /// Continuation `(opened_at, position_id)` of the last row in this page.
    pub next_cursor: Option<(DateTime<Utc>, String)>,
}

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
                    // Full sort key (observed_at, event_id): a restart re-seed
                    // that is capped must at least be DETERMINISTIC, or two
                    // replicas re-seed different prefixes of the same backlog.
                    r#"SELECT * FROM copy_events
                        WHERE organization_id = $1 AND observed_at >= $2
                        ORDER BY observed_at ASC, event_id ASC LIMIT $3"#,
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

    /// One PAGE of the acting tenant's OPEN links — the reconciliation input
    /// (gap #78).
    ///
    /// `cursor` is the `(opened_at, id)` of the previous page's last row, so
    /// rows sharing a timestamp are neither skipped nor repeated, and
    /// `has_more`/`total_remaining` state what the page is not returning.
    pub async fn open_links_page(
        &self,
        scope: &TradingQueryScope,
        cursor: Option<(DateTime<Utc>, String)>,
        limit: i64,
    ) -> Result<CopyLinkPage, RepositoryError> {
        let page_size = limit.clamp(1, COPY_LINK_PAGE_MAX);
        let (cursor_at, cursor_id) = match cursor {
            Some((at, id)) => (Some(at), Some(id)),
            None => (None, None),
        };
        let organization_id = scope.organization_id().as_uuid();
        let rows = self
            .db
            .timed(
                "tenant_copy_open_links_page",
                sqlx::query(
                    r#"SELECT * FROM copy_links
                        WHERE organization_id = $1 AND status = 'open'
                          AND ($2::timestamptz IS NULL
                               OR opened_at > $2
                               OR (opened_at = $2 AND position_id > $3))
                        ORDER BY opened_at ASC, position_id ASC LIMIT $4"#,
                )
                .bind(organization_id)
                .bind(cursor_at)
                .bind(cursor_id.clone())
                .bind(page_size + 1)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = rows.len() as i64 > page_size;
        let items: Vec<TenantCopyLink> = rows
            .iter()
            .take(page_size as usize)
            .map(link_from_row)
            .collect();
        assert_rows_org(scope.organization_id(), &items)?;
        let total_remaining: i64 = sqlx::query_scalar(
            r#"SELECT count(*) FROM copy_links
                WHERE organization_id = $1 AND status = 'open'
                  AND ($2::timestamptz IS NULL
                       OR opened_at > $2
                       OR (opened_at = $2 AND position_id > $3))"#,
        )
        .bind(organization_id)
        .bind(cursor_at)
        .bind(cursor_id)
        .fetch_one(self.db.pool())
        .await?;
        let next_cursor = items.last().map(|l| (l.opened_at, l.position_id.clone()));
        Ok(CopyLinkPage {
            items,
            has_more,
            total_remaining,
            next_cursor,
        })
    }

    /// The acting tenant's OPEN links (reconciliation input) — the FIRST PAGE
    /// of [`Self::open_links_page`]. A truncated page is logged, never silent.
    pub async fn open_links(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantCopyLink>, RepositoryError> {
        let page = self
            .open_links_page(scope, None, COPY_LINK_PAGE_MAX)
            .await?;
        if page.has_more {
            tracing::warn!(
                organization_id = %scope.organization_id(),
                returned = page.items.len(),
                remaining = page.total_remaining,
                "open copy links TRUNCATED: walk open_links_page for the rest"
            );
        }
        Ok(page.items)
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
