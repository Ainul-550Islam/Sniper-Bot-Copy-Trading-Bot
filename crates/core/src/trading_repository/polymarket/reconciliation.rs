//! Tenant-safe polymarket reconciliation (PROMPT 3/10 §G45).
//!
//! The legacy reconciler's GLOBAL inputs ("all open mirror orders",
//! "all fills") become tenant-scoped queries: each tenant's drift is
//! computed only from that tenant's mirror book and fills. Cross-
//! tenant modification is impossible at the SQL layer — every
//! predicate carries `organization_id`.
//!
//! Drift shapes (mirroring the legacy invariants on the 0014 schema):
//! * `drift` — an open mirror order whose `size_matched` disagrees
//!   with the tenant's own booked fills for that venue order;
//! * `orphan_fill` — a booked fill whose venue order is not an open
//!   mirror order of this tenant;
//! * `stuck_order` — an open mirror order not updated within the
//!   grace window.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_rows_org;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{finding_from_row, TenantPolyOrder, TenantPolyReconFinding};
use super::read::TenantPolyRead;
use super::write::TenantPolyWrite;

/// Tenant-scoped polymarket reconciliation repository.
pub struct TenantPolyReconRepo {
    db: Arc<Database>,
    reads: TenantPolyRead,
    writes: TenantPolyWrite,
}

/// One drift finding for the acting tenant.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TenantPolyDrift {
    pub venue_order_id: String,
    pub token_id: String,
    /// `drift` | `orphan_fill` | `stuck_order`.
    pub kind: String,
    pub detail: String,
}

impl TenantPolyReconRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPolyReconRepo {
            reads: TenantPolyRead::new(db.clone()),
            writes: TenantPolyWrite::new(db.clone()),
            db,
        }
    }

    /// Detect the acting tenant's mirror drift in one SQL pass. All
    /// inputs and outputs are this tenant's rows.
    pub async fn detect_drift(
        &self,
        scope: &TradingQueryScope,
        stale_after: chrono::Duration,
        now: DateTime<Utc>,
    ) -> Result<Vec<TenantPolyDrift>, RepositoryError> {
        let stale_cutoff = now - stale_after;
        let rows = self
            .db
            .timed(
                "tenant_poly_recon_drift",
                sqlx::query(
                    r#"
                    -- (a) matched-size drift on open mirror orders
                    SELECT o.venue_order_id, o.token_id, 'drift'::text AS kind,
                           format('mirror=%s fills=%s',
                                  o.size_matched::text,
                                  COALESCE(f.filled, 0)::text) AS detail
                      FROM poly_orders o
                      LEFT JOIN (
                          SELECT venue_order_id, SUM(size_tokens) AS filled
                            FROM poly_fills
                           WHERE organization_id = $1
                           GROUP BY venue_order_id
                      ) f ON f.venue_order_id = o.venue_order_id
                     WHERE o.organization_id = $1
                       AND o.closed_at IS NULL
                       AND o.size_matched <> COALESCE(f.filled, 0)
                    UNION ALL
                    -- (b) booked fills with no open mirror row (orphan fills)
                    SELECT f.venue_order_id, f.token_id, 'orphan_fill'::text,
                           'fill booked without an open mirror order'
                      FROM poly_fills f
                      LEFT JOIN poly_orders o
                        ON o.organization_id = f.organization_id
                       AND o.venue_order_id = f.venue_order_id
                       AND o.closed_at IS NULL
                     WHERE f.organization_id = $1
                       AND o.venue_order_id IS NULL
                    UNION ALL
                    -- (c) open mirror orders stale beyond the grace window
                    SELECT o.venue_order_id, o.token_id, 'stuck_order'::text,
                           'open mirror order not updated since ' ||
                               to_char(o.updated_at, 'YYYY-MM-DD HH24:MI:SS')
                      FROM poly_orders o
                     WHERE o.organization_id = $1
                       AND o.closed_at IS NULL
                       AND o.updated_at < $2
                    ORDER BY kind, venue_order_id
                    LIMIT 1000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(stale_cutoff)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let drift: Vec<TenantPolyDrift> = rows
            .iter()
            .filter_map(|r| {
                Some(TenantPolyDrift {
                    venue_order_id: r.try_get::<String, _>("venue_order_id").ok()?,
                    token_id: r.try_get::<String, _>("token_id").ok()?,
                    kind: r.try_get::<String, _>("kind").ok()?,
                    detail: r.try_get::<String, _>("detail").ok()?,
                })
            })
            .collect();
        Ok(drift)
    }

    /// Persist one finding for the acting tenant (append-only log; the
    /// 0014 schema keys findings by bigserial id and attribution).
    #[allow(clippy::too_many_arguments)]
    pub async fn record_finding(
        &self,
        write: &TenantWriteScope,
        kind: &str,
        venue_order_id: Option<&str>,
        order_id: Option<&str>,
        token_id: Option<&str>,
        detail: &str,
        action: &str,
        replica_id: &str,
    ) -> Result<(), RepositoryError> {
        if kind.trim().is_empty() {
            return Err(RepositoryError::Validation("kind"));
        }
        self.db
            .timed(
                "tenant_poly_recon_finding",
                sqlx::query(
                    r#"INSERT INTO poly_recon_findings
                           (organization_id, kind, venue_order_id, order_id, token_id,
                            detail, action, replica_id, ts)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(kind)
                .bind(venue_order_id)
                .bind(order_id)
                .bind(token_id)
                .bind(detail)
                .bind(action)
                .bind(replica_id)
                .bind(Utc::now())
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// The acting tenant's findings in a window (operator triage view
    /// is tenant-scoped in the data plane).
    pub async fn findings_between(
        &self,
        scope: &TradingQueryScope,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantPolyReconFinding>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_poly_findings",
                sqlx::query(
                    r#"SELECT * FROM poly_recon_findings
                        WHERE organization_id = $1
                          AND ts >= $2 AND ts < $3
                        ORDER BY ts ASC LIMIT 5000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantPolyReconFinding> = rows.iter().map(finding_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's open mirror orders (reconciler input,
    /// tenant-safe re-export of the read repo).
    pub async fn open_orders(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantPolyOrder>, RepositoryError> {
        self.reads.open_orders(scope).await
    }

    /// Terminalize one of the acting tenant's stuck mirror orders
    /// (operator action through the tenant data plane — the guard
    /// stays on the tenant's own row).
    pub async fn cancel_stuck_order(
        &self,
        write: &TenantWriteScope,
        venue_order_id: &str,
    ) -> Result<(), RepositoryError> {
        self.writes
            .close_order(
                write,
                venue_order_id,
                "cancelled",
                Some("operator"),
                Utc::now(),
            )
            .await
    }
}
