//! Tenant-safe recovery / replay queries (PROMPT 3/10 §E33).
//!
//! Every recovery query here answers "which tenant owns this record?"
//! in the SQL predicate itself. The DANGEROUS global shape
//! `SELECT … FROM execution_intents WHERE status = …` exists ONLY in
//! the legacy deployment operator plane; the tenant data plane uses
//! the scoped forms below.
//!
//! The recovery sweep is serialized per (tenant, purpose) with the
//! tenant-namespaced advisory lock, so two tenants' sweeps run
//! concurrently while two workers of ONE tenant still serialize.
//!
//! **Bounded, paged, paired.** A sweep used to cap at `LIMIT 1000` intents and
//! `LIMIT 5000` orders with no cursor and no truncation signal, and it cloned
//! that one tenant-wide open-order list onto EVERY intent. A caller therefore
//! could not tell that rows existed past the cap, could not reach them, and was
//! handed a pairing that was not evidence. [`TenantRecoveryRepo::sweep_page`]
//! walks a `(created_at, intent_id)` keyset — the full sort key, so ties cannot
//! skip or repeat a row — reports `has_more`/`total_remaining`, and attaches to
//! each intent only the open orders attributed to it (`orders.meta->>'intent_id'`,
//! the attribution the OMS writes). [`TenantRecoveryRepo::sweep`] is the
//! first-page convenience over the same code path.
//!
//! **The lock covers the reads, not the caller.** The advisory lock is
//! transaction-scoped, so it is taken for the two SELECTs and released at
//! commit — before the caller acts. That is deliberate and now documented: it
//! serializes two workers' *reads* (one consistent snapshot each), and each
//! decision is re-fenced by [`TenantRecoveryRepo::abandon_orphan`] /
//! [`TenantRecoveryRepo::link_orphan`], which only transition a still-`pending`
//! row. Holding the lock for the caller's processing would not make those
//! writes safer and would block the tenant's second worker for an unbounded
//! time.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::transaction::TradingTransaction;

use super::model::{intent_from_row, TenantIntent};
use super::read::TenantIntentRead;
use super::write::TenantIntentWrite;

/// Largest orphan page a single `sweep_page` call may return. The cap is a
/// page size, not a silent truncation: [`TenantRecoveryPage::has_more`] and
/// [`TenantRecoveryPage::total_remaining`] report what is left, and the keyset
/// cursor reaches it.
pub const RECOVERY_PAGE_MAX: i64 = 500;

/// One recovery work item for the acting tenant.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantRecoveryItem {
    /// The orphaned intent (pending, older than the cutoff).
    pub intent: TenantIntent,
    /// The non-terminal orders attributed to THIS intent
    /// (`orders.meta->>'intent_id'`), in creation order. Empty when the
    /// broadcast never produced an order — never another intent's rows.
    pub open_order_ids: Vec<String>,
}

/// One page of recovery work for the acting tenant (gap #74).
///
/// `has_more`/`total_remaining` make truncation VISIBLE: a caller that stops
/// early knows it stopped early instead of believing the batch was complete.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantRecoveryPage {
    /// The page, oldest orphan first.
    pub items: Vec<TenantRecoveryItem>,
    /// The tenant's open orders that carry NO `meta->>'intent_id'` attribution.
    /// They belong to no intent on this page, so pairing them would be a guess —
    /// and dropping them would hide trading state. They are reported here so
    /// nothing in the sweep is silently unreachable.
    pub unattributed_open_order_ids: Vec<String>,
    /// True when at least one more orphan exists past this page.
    pub has_more: bool,
    /// Orphans still to visit after this page (same cutoff, excluding the
    /// rows already returned).
    pub total_remaining: i64,
    /// Continuation cursor `(created_at, intent_id)` of the last row in this
    /// page — the full sort key, so a tie cannot skip or repeat a row. `None`
    /// when the page is empty.
    pub next_cursor: Option<(DateTime<Utc>, String)>,
}

impl TenantRecoveryPage {
    /// The `(created_at, intent_id)` pairs in page order — the keyset key of
    /// every row, so a caller can build its own cursor without re-reading.
    pub fn cursor_pairs(&self) -> Vec<(DateTime<Utc>, String)> {
        self.items
            .iter()
            .map(|i| (i.intent.created_at, i.intent.intent_id.clone()))
            .collect()
    }
}

/// Tenant-scoped recovery repository.
pub struct TenantRecoveryRepo {
    db: Arc<Database>,
    intents: TenantIntentRead,
    intent_writes: TenantIntentWrite,
}

impl TenantRecoveryRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantRecoveryRepo {
            intents: TenantIntentRead::new(db.clone()),
            intent_writes: TenantIntentWrite::new(db.clone()),
            db,
        }
    }

    /// One bounded PAGE of the acting tenant's recovery work, under the
    /// tenant advisory lock (which covers the two reads, not the caller).
    ///
    /// `cursor` is the `(created_at, intent_id)` of the last row of the
    /// previous page — the full sort key, so rows sharing a timestamp are
    /// neither skipped nor repeated. `limit` is clamped to
    /// [`RECOVERY_PAGE_MAX`]; the response says whether more rows exist.
    pub async fn sweep_page(
        &self,
        scope: &TradingQueryScope,
        orphan_cutoff: DateTime<Utc>,
        cursor: Option<(DateTime<Utc>, String)>,
        limit: i64,
    ) -> Result<TenantRecoveryPage, RepositoryError> {
        let page_size = limit.clamp(1, RECOVERY_PAGE_MAX);
        let organization_id = scope.organization_id().as_uuid();
        let (cursor_at, cursor_id) = match cursor {
            Some((at, id)) => (Some(at), Some(id)),
            None => (None, None),
        };

        let mut tx = TradingTransaction::begin(&self.db, *scope).await?;
        tx.tenant_lock("recovery-sweep").await?;

        // One extra row tells us whether a next page exists without a second
        // COUNT over the same predicate.
        let rows = sqlx::query(
            r#"SELECT * FROM execution_intents
                WHERE organization_id = $1
                  AND status = 'pending' AND created_at < $2
                  AND ($3::timestamptz IS NULL
                       OR created_at > $3
                       OR (created_at = $3 AND intent_id > $4))
                ORDER BY created_at ASC, intent_id ASC LIMIT $5"#,
        )
        .bind(organization_id)
        .bind(orphan_cutoff)
        .bind(cursor_at)
        .bind(cursor_id.clone())
        .bind(page_size + 1)
        .fetch_all(tx.connection())
        .await
        .map_err(RepositoryError::from)?;
        let has_more = rows.len() as i64 > page_size;
        let intents: Vec<TenantIntent> = rows
            .iter()
            .take(page_size as usize)
            .map(intent_from_row)
            .collect();
        crate::trading_repository::tenant_assert::assert_rows_org(
            scope.organization_id(),
            &intents,
        )?;

        // Exactly the open orders attributed to the intents in THIS page — not
        // one tenant-wide list cloned onto every item.
        let page_intent_ids: Vec<String> = intents.iter().map(|i| i.intent_id.clone()).collect();
        // Unattributed open orders are tenant-wide: fetch them even when this
        // page holds no intents, so a caller always sees them.
        let unattributed_rows = sqlx::query(
            r#"SELECT id FROM orders
                WHERE organization_id = $1
                  AND (meta->>'intent_id') IS NULL
                  AND status NOT IN
                      ('filled','failed','cancelled','expired','reconciled')
                ORDER BY created_at ASC, id ASC LIMIT $2"#,
        )
        .bind(organization_id)
        .bind(RECOVERY_PAGE_MAX)
        .fetch_all(tx.connection())
        .await
        .map_err(RepositoryError::from)?;
        let mut unattributed_open_order_ids: Vec<String> = unattributed_rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("id").ok())
            .collect();
        let open_orders = if page_intent_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query(
                r#"SELECT id, meta->>'intent_id' AS intent_id FROM orders
                    WHERE organization_id = $1
                      AND meta->>'intent_id' = ANY($2)
                      AND status NOT IN
                          ('filled','failed','cancelled','expired','reconciled')
                    ORDER BY created_at ASC, id ASC"#,
            )
            .bind(organization_id)
            .bind(&page_intent_ids)
            .fetch_all(tx.connection())
            .await
            .map_err(RepositoryError::from)?
        };
        let mut orders_by_intent: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for row in &open_orders {
            let order_id: String = row.try_get("id").map_err(RepositoryError::from)?;
            match row.try_get::<Option<String>, _>("intent_id") {
                Ok(Some(intent_id)) if !intent_id.is_empty() => {
                    orders_by_intent
                        .entry(intent_id)
                        .or_default()
                        .push(order_id);
                }
                // No attribution: reported, never guessed onto an intent.
                _ => unattributed_open_order_ids.push(order_id),
            }
        }

        let total_remaining: i64 = sqlx::query_scalar(
            r#"SELECT count(*) FROM execution_intents
                WHERE organization_id = $1
                  AND status = 'pending' AND created_at < $2
                  AND ($3::timestamptz IS NULL
                       OR created_at > $3
                       OR (created_at = $3 AND intent_id > $4))"#,
        )
        .bind(organization_id)
        .bind(orphan_cutoff)
        .bind(cursor_at)
        .bind(cursor_id)
        .fetch_one(tx.connection())
        .await
        .map_err(RepositoryError::from)?;

        // Released HERE, at commit: the caller's processing is not under the
        // lock, and its decisions are re-fenced by the guarded transitions.
        tx.commit().await?;

        let next_cursor = intents.last().map(|i| (i.created_at, i.intent_id.clone()));
        let items = intents
            .into_iter()
            .map(|intent| {
                let open_order_ids = orders_by_intent
                    .get(&intent.intent_id)
                    .cloned()
                    .unwrap_or_default();
                TenantRecoveryItem {
                    intent,
                    open_order_ids,
                }
            })
            .collect();
        Ok(TenantRecoveryPage {
            items,
            unattributed_open_order_ids,
            has_more,
            total_remaining,
            next_cursor,
        })
    }

    /// The acting tenant's first recovery page (the old one-shot batch shape,
    /// now bounded AND honest about truncation: `items` may be a prefix, and
    /// `has_more`/`total_remaining` say so).
    pub async fn sweep(
        &self,
        scope: &TradingQueryScope,
        orphan_cutoff: DateTime<Utc>,
    ) -> Result<Vec<TenantRecoveryItem>, RepositoryError> {
        Ok(self
            .sweep_page(scope, orphan_cutoff, None, RECOVERY_PAGE_MAX)
            .await?
            .items)
    }

    /// Abandon one of the acting tenant's orphaned intents after the
    /// operator/automation proved it never broadcast.
    pub async fn abandon_orphan(
        &self,
        write: &crate::trading_repository::write_scope::TenantWriteScope,
        intent_id: &str,
    ) -> Result<(), RepositoryError> {
        self.intent_writes.abandon(write, intent_id).await
    }

    /// Link one of the acting tenant's intents to its broadcast
    /// signature (a late `link` during recovery).
    pub async fn link_orphan(
        &self,
        write: &crate::trading_repository::write_scope::TenantWriteScope,
        intent_id: &str,
        signature: &str,
    ) -> Result<(), RepositoryError> {
        self.intent_writes.link(write, intent_id, signature).await
    }

    /// Reconciliation-queue views for the acting tenant
    /// (`reconciliation_state`, 0033 composite PK): due work rows.
    /// The queue's PK is tenant-local, so two tenants can hold the
    /// same subject id without colliding.
    pub async fn due_reconciliation(
        &self,
        scope: &TradingQueryScope,
        limit: i64,
    ) -> Result<Vec<(String, String, String, i32, DateTime<Utc>)>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_recon_due",
                sqlx::query(
                    r#"SELECT kind, subject, status, attempts, next_attempt_at
                         FROM reconciliation_state
                        WHERE organization_id = $1
                          AND status IN ('pending', 'in_progress')
                          AND next_attempt_at <= now()
                        ORDER BY next_attempt_at ASC LIMIT $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some((
                    r.try_get::<String, _>("kind").ok()?,
                    r.try_get::<String, _>("subject").ok()?,
                    r.try_get::<String, _>("status").ok()?,
                    r.try_get::<i32, _>("attempts").ok()?,
                    r.try_get::<DateTime<Utc>, _>("next_attempt_at").ok()?,
                ))
            })
            .collect())
    }

    /// Enqueue reconciliation work for the acting tenant on the 0033
    /// composite arbiter — the same (kind, subject) may be enqueued by
    /// another tenant independently.
    pub async fn enqueue_reconciliation(
        &self,
        write: &crate::trading_repository::write_scope::TenantWriteScope,
        kind: &str,
        subject: &str,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        if subject.trim().is_empty() {
            return Err(RepositoryError::Validation("subject"));
        }
        self.db
            .timed(
                "tenant_recon_enqueue",
                sqlx::query(
                    r#"INSERT INTO reconciliation_state
                           (organization_id, kind, subject, status, next_attempt_at)
                       VALUES ($1, $2, $3, 'pending', $4)
                       ON CONFLICT (organization_id, kind, subject) DO UPDATE SET
                           status = 'pending',
                           next_attempt_at = EXCLUDED.next_attempt_at,
                           updated_at = now()"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(kind)
                .bind(subject)
                .bind(next_attempt_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// The read side for callers that only need the intent queries.
    pub fn intents(&self) -> &TenantIntentRead {
        &self.intents
    }
}
