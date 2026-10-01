//! Tenant balance snapshots and aggregates (PROMPT 3/10 §D27).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_rows_org;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{balance_from_row, TenantBalanceSnapshot};

/// Tenant-scoped balance snapshot repository (`balance_snapshots`).
pub struct TenantBalanceRepo {
    db: Arc<Database>,
}

impl TenantBalanceRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantBalanceRepo { db }
    }

    /// Record one snapshot row for the acting tenant.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        write: &TenantWriteScope,
        chain: &str,
        address: &str,
        asset: &str,
        amount: f64,
        usd_value: Option<f64>,
        source: &str,
        at: DateTime<Utc>,
    ) -> Result<i64, RepositoryError> {
        if address.trim().is_empty() || asset.trim().is_empty() {
            return Err(RepositoryError::Validation("balance fields"));
        }
        let row = self
            .db
            .timed(
                "tenant_balance_record",
                sqlx::query(
                    r#"INSERT INTO balance_snapshots
                           (organization_id, ts, chain, address, asset, amount,
                            usd_value, source)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                       RETURNING id"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(at)
                .bind(chain)
                .bind(address)
                .bind(asset)
                .bind(amount)
                .bind(usd_value)
                .bind(source)
                .fetch_one(self.db.pool()),
            )
            .await?;
        row.try_get::<i64, _>("id").map_err(RepositoryError::from)
    }

    /// The LATEST snapshot per (address, asset) of the acting tenant —
    /// the reconciler's current-truth view. Never returns another
    /// tenant's rows.
    pub async fn latest_per_asset(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantBalanceSnapshot>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_balances_latest",
                sqlx::query(
                    r#"SELECT DISTINCT ON (address, asset) *
                         FROM balance_snapshots
                        WHERE organization_id = $1
                        ORDER BY address, asset, ts DESC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantBalanceSnapshot> = rows.iter().map(balance_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// The acting tenant's snapshots for one wallet address in a window
    /// (equity-curve input). Foreign addresses yield the tenant's own
    /// empty set.
    pub async fn history_for_address(
        &self,
        scope: &TradingQueryScope,
        address: &str,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TenantBalanceSnapshot>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_balance_history",
                sqlx::query(
                    r#"SELECT * FROM balance_snapshots
                        WHERE organization_id = $1 AND address = $2
                          AND ts >= $3 AND ts < $4
                        ORDER BY ts ASC LIMIT 10_000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(address)
                .bind(since)
                .bind(until)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantBalanceSnapshot> = rows.iter().map(balance_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// USD-equivalent aggregate of the acting tenant's latest
    /// snapshots (tenant-safe SUM — the constraint is in the SQL, not a
    /// post-load filter).
    pub async fn total_usd_latest(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Option<f64>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_balance_total_usd",
                sqlx::query(
                    r#"SELECT SUM(l.usd_value)::double precision AS total
                         FROM (
                           SELECT DISTINCT ON (address, asset) usd_value
                             FROM balance_snapshots
                            WHERE organization_id = $1
                            ORDER BY address, asset, ts DESC
                         ) l"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_one(self.db.pool()),
            )
            .await?;
        Ok(row.try_get::<Option<f64>, _>("total").ok().flatten())
    }
}
