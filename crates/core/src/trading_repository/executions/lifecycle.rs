//! Tenant-safe execution lifecycle persistence (PROMPT 3/10 §C19).
//!
//! `execution_lifecycle` + `execution_lifecycle_events` (0012), scoped
//! to the acting tenant. The 0031 swap makes the lifecycle arbiter
//! `(organization_id, intent_id)`; two tenants deriving the same
//! deterministic intent id hold independent rows.
//!
//! **Recovery reads page and report truncation (gap #78).** The unsettled
//! ("open") list is the crash-recovery input, so a bare `LIMIT 5000` silently
//! dropped everything past the cap — a tenant with more unsettled attempts than
//! the cap recovered only the first 5,000 and nothing said so.
//! [`TenantLifecycleRepo::open_page`] walks a `(updated_at, intent_id)` keyset —
//! the full sort key, so ties are neither skipped nor repeated — and returns
//! `has_more`/`total_remaining`/`next_cursor`.
//!
//! **Organization comes from the ROW.** The mapper used to stamp
//! `scope.organization_id()` on every record, which made the tenant assertion
//! (`assert_rows_org`) decorative: it could never fail, and a mapping mistake
//! would have presented a foreign row as the caller's. Rows now carry the
//! stored column, a NULL organization is rejected, and the assertion has teeth.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::db::Database;
use crate::execution::ExecutionRecord;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_rows_org;
use crate::trading_repository::write_scope::TenantWriteScope;

/// One lifecycle row as returned to the acting tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantLifecycleRecord {
    pub organization_id: crate::tenant::OrganizationId,
    pub intent_id: String,
    pub module: String,
    pub label: String,
    pub wallet: String,
    pub symbol: String,
    pub state: String,
    pub attempts: i32,
    pub signature: Option<String>,
    pub failure_class: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantLifecycleRecord {
    fn row_organization_id(&self) -> crate::tenant::OrganizationId {
        self.organization_id
    }
}

/// Largest open-attempt page `open_page` will return.
pub const LIFECYCLE_PAGE_MAX: i64 = 500;

/// One page of the acting tenant's unsettled attempts (gap #78).
#[derive(Debug, Clone, PartialEq)]
pub struct TenantLifecyclePage {
    /// The page, oldest `updated_at` first.
    pub items: Vec<TenantLifecycleRecord>,
    /// True when at least one more unsettled attempt exists past this page.
    pub has_more: bool,
    /// Unsettled attempts in the same cutoff window, INCLUDING this page, so
    /// `total_remaining - items.len()` is the work left.
    pub total_remaining: i64,
    /// Continuation `(updated_at, intent_id)` of the last row in this page.
    pub next_cursor: Option<(DateTime<Utc>, String)>,
}

const LIFECYCLE_COLUMNS: &str = "organization_id, intent_id, module, label, wallet, symbol, \
     state, attempts, signature, failure_class, error, created_at, updated_at";

/// Map one lifecycle row to the acting tenant's record, taking the
/// organization FROM THE ROW (gap #78).
///
/// The mapper used to stamp the caller's scope onto every row, so
/// [`crate::trading_repository::tenant_assert::assert_rows_org`] could never
/// fire. A row with a NULL organization — written before tenant attribution
/// was enforced — is refused rather than adopted by whoever happens to be
/// asking.
fn record_from_row(row: &sqlx::postgres::PgRow) -> Result<TenantLifecycleRecord, RepositoryError> {
    let organization_id: Option<uuid::Uuid> = row.try_get("organization_id").ok().flatten();
    let organization_id = organization_id
        .map(crate::tenant::OrganizationId::from)
        .ok_or(RepositoryError::TenantMismatch)?;
    Ok(TenantLifecycleRecord {
        organization_id,
        intent_id: row.try_get("intent_id").unwrap_or_default(),
        module: row.try_get("module").unwrap_or_default(),
        label: row.try_get("label").unwrap_or_default(),
        wallet: row.try_get("wallet").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        state: row.try_get("state").unwrap_or_default(),
        attempts: row.try_get("attempts").unwrap_or(1),
        signature: row.try_get("signature").ok().flatten(),
        failure_class: row.try_get("failure_class").ok().flatten(),
        error: row.try_get("error").ok().flatten(),
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
    })
}

/// Tenant-scoped lifecycle repository.
pub struct TenantLifecycleRepo {
    db: Arc<Database>,
}

impl TenantLifecycleRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantLifecycleRepo { db }
    }

    /// Upsert the current state of one of the acting tenant's intents
    /// (0031 composite arbiter; `created_at` keeps first-seen time).
    pub async fn upsert(
        &self,
        write: &TenantWriteScope,
        rec: &ExecutionRecord,
    ) -> Result<(), RepositoryError> {
        if rec.intent_id.trim().is_empty() {
            return Err(RepositoryError::Validation("intent_id"));
        }
        self.db
            .timed(
                "tenant_lifecycle_upsert",
                sqlx::query(
                    r#"INSERT INTO execution_lifecycle
                           (organization_id, intent_id, module, label, wallet, symbol,
                            state, attempts, signature, blockhash, last_valid_block_height,
                            priority_fee_micro_lamports, failure_class, error,
                            created_at, updated_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
                       ON CONFLICT (organization_id, intent_id) DO UPDATE SET
                           module = EXCLUDED.module,
                           label = EXCLUDED.label,
                           wallet = EXCLUDED.wallet,
                           symbol = EXCLUDED.symbol,
                           state = EXCLUDED.state,
                           attempts = GREATEST(execution_lifecycle.attempts, EXCLUDED.attempts),
                           signature = EXCLUDED.signature,
                           blockhash = EXCLUDED.blockhash,
                           last_valid_block_height = EXCLUDED.last_valid_block_height,
                           priority_fee_micro_lamports = EXCLUDED.priority_fee_micro_lamports,
                           failure_class = EXCLUDED.failure_class,
                           error = EXCLUDED.error,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(&rec.intent_id)
                .bind(&rec.module)
                .bind(&rec.label)
                .bind(&rec.wallet)
                .bind(&rec.symbol)
                .bind(rec.state.as_str())
                .bind(rec.attempts as i32)
                .bind(&rec.signature)
                .bind(&rec.blockhash)
                .bind(rec.last_valid_block_height.map(|h| h as i64))
                .bind(rec.priority_fee_micro_lamports as i64)
                .bind(rec.failure.map(|f| f.as_str()))
                .bind(&rec.error)
                .bind(rec.created_at)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// One lifecycle row by intent id — only the acting tenant's.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        intent_id: &str,
    ) -> Result<Option<TenantLifecycleRecord>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_lifecycle_get",
                sqlx::query(
                    r#"SELECT organization_id, intent_id, module, label, wallet, symbol,
                              state, attempts, signature, failure_class, error,
                              created_at, updated_at
                         FROM execution_lifecycle
                        WHERE organization_id = $1 AND intent_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(intent_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        match row.as_ref() {
            None => Ok(None),
            Some(r) => {
                let record = record_from_row(r)?;
                crate::trading_repository::tenant_assert::assert_optional_row_org(
                    scope.organization_id(),
                    &Some(record.clone()),
                )?;
                Ok(Some(record))
            }
        }
    }

    /// One PAGE of the acting tenant's NON-settled attempts — the
    /// crash-recovery input (gap #78).
    ///
    /// `cursor` is the `(updated_at, intent_id)` of the previous page's last
    /// row: the full sort key, so rows sharing a timestamp are neither skipped
    /// nor repeated. `limit` is clamped to [`LIFECYCLE_PAGE_MAX`] and the
    /// response states what is left (`has_more`, `total_remaining`).
    pub async fn open_page(
        &self,
        scope: &TradingQueryScope,
        cursor: Option<(DateTime<Utc>, String)>,
        limit: i64,
    ) -> Result<TenantLifecyclePage, RepositoryError> {
        let page_size = limit.clamp(1, LIFECYCLE_PAGE_MAX);
        let organization_id = scope.organization_id().as_uuid();
        let (cursor_at, cursor_id) = match cursor {
            Some((at, id)) => (Some(at), Some(id)),
            None => (None, None),
        };
        // One extra row is the has-more proof; the count is the honest total.
        let rows = self
            .db
            .timed(
                "tenant_lifecycle_open_page",
                sqlx::query(&format!(
                    r#"SELECT {LIFECYCLE_COLUMNS}
                         FROM execution_lifecycle
                        WHERE organization_id = $1
                          AND state IN ('created', 'validated', 'submitted', 'pending')
                          AND ($2::timestamptz IS NULL
                               OR updated_at > $2
                               OR (updated_at = $2 AND intent_id > $3))
                        ORDER BY updated_at ASC, intent_id ASC LIMIT $4"#
                ))
                .bind(organization_id)
                .bind(cursor_at)
                .bind(cursor_id.clone())
                .bind(page_size + 1)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = rows.len() as i64 > page_size;
        let items: Vec<TenantLifecycleRecord> = rows
            .iter()
            .take(page_size as usize)
            .map(record_from_row)
            .collect::<Result<_, _>>()?;
        assert_rows_org(scope.organization_id(), &items)?;
        let total_remaining: i64 = sqlx::query_scalar(
            r#"SELECT count(*) FROM execution_lifecycle
                WHERE organization_id = $1
                  AND state IN ('created', 'validated', 'submitted', 'pending')
                  AND ($2::timestamptz IS NULL
                       OR updated_at > $2
                       OR (updated_at = $2 AND intent_id > $3))"#,
        )
        .bind(organization_id)
        .bind(cursor_at)
        .bind(cursor_id)
        .fetch_one(self.db.pool())
        .await?;
        let next_cursor = items.last().map(|r| (r.updated_at, r.intent_id.clone()));
        Ok(TenantLifecyclePage {
            items,
            has_more,
            total_remaining,
            next_cursor,
        })
    }

    /// The acting tenant's NON-settled attempts (crash-recovery input) — the
    /// FIRST PAGE of [`Self::open_page`], never another tenant's.
    ///
    /// A tenant with more unsettled attempts than one page gets a prefix here
    /// and a warning; callers that must see everything walk `open_page`.
    pub async fn list_open(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantLifecycleRecord>, RepositoryError> {
        let page = self.open_page(scope, None, LIFECYCLE_PAGE_MAX).await?;
        if page.has_more {
            tracing::warn!(
                organization_id = %scope.organization_id(),
                returned = page.items.len(),
                remaining = page.total_remaining,
                "unsettled lifecycle list TRUNCATED: walk open_page for the rest"
            );
        }
        Ok(page.items)
    }

    /// Ownership check with teeth (gap #78): a stored organization that is not
    /// the scope's — or a NULL one — is refused, never relabelled.
    pub fn ownership_from_row(
        &self,
        scope: &TradingQueryScope,
        stored: Option<uuid::Uuid>,
    ) -> Result<crate::tenant::OrganizationId, RepositoryError> {
        let stored = stored
            .map(crate::tenant::OrganizationId::from)
            .ok_or(RepositoryError::TenantMismatch)?;
        if stored != scope.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        Ok(stored)
    }

    /// Append one transition to the acting tenant's immutable history.
    /// The intent must already exist FOR THIS TENANT (ownership check
    /// inside the transaction; foreign intent ids → NotFound).
    #[allow(clippy::too_many_arguments)]
    pub async fn append_event(
        &self,
        write: &TenantWriteScope,
        intent_id: &str,
        attempt: u32,
        from_state: Option<&str>,
        to_state: &str,
        signature: Option<&str>,
        failure_class: Option<&str>,
        reason: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        let owned = sqlx::query(
            r#"SELECT 1 FROM execution_lifecycle
                WHERE organization_id = $1 AND intent_id = $2"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(intent_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        if owned.is_none() {
            tx.rollback().await.map_err(RepositoryError::from)?;
            return Err(RepositoryError::NotFound("lifecycle"));
        }
        sqlx::query(
            r#"INSERT INTO execution_lifecycle_events
                   (organization_id, intent_id, attempt, from_state, to_state,
                    signature, failure_class, reason, ts)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(write.organization_id().as_uuid())
        .bind(intent_id)
        .bind(attempt as i32)
        .bind(from_state)
        .bind(to_state)
        .bind(signature)
        .bind(failure_class)
        .bind(reason)
        .bind(at)
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::from)?;
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(())
    }

    /// The acting tenant's transition history for one intent, oldest
    /// first (JSON rows, mirroring the legacy read shape).
    pub async fn events(
        &self,
        scope: &TradingQueryScope,
        intent_id: &str,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_lifecycle_events",
                sqlx::query(
                    r#"SELECT e.id, e.intent_id, e.attempt, e.from_state, e.to_state,
                              e.signature, e.failure_class, e.reason, e.ts
                         FROM execution_lifecycle_events e
                        WHERE e.organization_id = $1 AND e.intent_id = $2
                        ORDER BY e.ts ASC, e.id ASC LIMIT $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(intent_id)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or_default(),
                    "intent_id": r.try_get::<String, _>("intent_id").unwrap_or_default(),
                    "attempt": r.try_get::<i32, _>("attempt").unwrap_or(1),
                    "from": r.try_get::<Option<String>, _>("from_state").unwrap_or_default(),
                    "to": r.try_get::<String, _>("to_state").unwrap_or_default(),
                    "signature": r.try_get::<Option<String>, _>("signature").unwrap_or_default(),
                    "failure_class": r.try_get::<Option<String>, _>("failure_class").unwrap_or_default(),
                    "reason": r.try_get::<Option<String>, _>("reason").unwrap_or_default(),
                    "ts": r.try_get::<DateTime<Utc>, _>("ts").ok(),
                })
            })
            .collect())
    }

    /// Prune the acting tenant's settled rows older than `age` — the
    /// tenant-scoped twin of the legacy maintenance path.
    pub async fn delete_settled_older_than(
        &self,
        write: &TenantWriteScope,
        age: chrono::Duration,
    ) -> Result<u64, RepositoryError> {
        let cutoff = Utc::now() - age;
        let res = self
            .db
            .timed(
                "tenant_lifecycle_prune",
                sqlx::query(
                    r#"DELETE FROM execution_lifecycle
                        WHERE organization_id = $1
                          AND state IN ('confirmed', 'failed', 'expired', 'reconciled')
                          AND updated_at < $2"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(cutoff)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}

// Re-export: callers build transitions through the tenant adapter.
pub use crate::execution::ExecutionTransition;
