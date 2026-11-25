//! Tenant lane lapse recovery (PROMPT 3/10 §H51).
//!
//! A crashed leader leaves the lane expired-but-unreleased. The
//! sweep below finds the acting tenant's lapsed lanes and (only for
//! THAT tenant) force-releases them so a replacement worker can take
//! over through the normal `acquire` race. The fencing `generation`
//! guarantees the crashed leader cannot act again even if it wakes
//! mid-sweep: its token is stale the moment the takeover increments
//! it.

use chrono::{DateTime, Utc};

use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::tenant_assert::assert_rows_org;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::acquire::TenantWorkerClaimRepo;
use super::model::{claim_from_row, TenantWorkClaim};

impl TenantWorkerClaimRepo {
    /// The acting tenant's lapsed-but-unreleased lanes (crashed
    /// leaders). Never returns another tenant's lanes.
    pub async fn lapsed_lanes(
        &self,
        scope: &TradingQueryScope,
        as_of: DateTime<Utc>,
    ) -> Result<Vec<TenantWorkClaim>, RepositoryError> {
        // A caller's `as_of` can move the PLANNING horizon later ("what will
        // have lapsed by then"), never earlier — a fast local clock must not
        // hide a lapse, and the sweep itself always uses the database clock.
        let as_of = self.db_now().await?.max(as_of);
        let rows = self
            .db
            .timed(
                "tenant_worker_claim_lapsed",
                sqlx::query(
                    r#"SELECT organization_id, purpose, leader_name, generation,
                              acquired_at, heartbeat_at, lease_until,
                              takeover_count, previous_leader, released
                         FROM worker_claims
                        WHERE organization_id = $1
                          AND released = false AND lease_until <= $2
                        ORDER BY lease_until ASC LIMIT 1000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(as_of)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantWorkClaim> = rows.iter().map(claim_from_row).collect();
        assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }

    /// Force-release one of the acting tenant's lapsed lanes
    /// (operator action through the tenant data plane). Guarded on
    /// lapsed-and-unreleased: a LIVE lane of this tenant cannot be
    /// force-released by mistake, and another tenant's lane is not
    /// addressable at all.
    ///
    /// There is deliberately NO clock parameter: whether a lane has lapsed is
    /// the database's verdict, so a host with a fast (or slow) clock cannot
    /// declare a peer lane dead.
    pub async fn force_release_lapsed(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
    ) -> Result<bool, RepositoryError> {
        let as_of = self.db_now().await?;
        let res = self
            .db
            .timed(
                "tenant_worker_claim_force_release",
                sqlx::query(
                    // Stamped with the DATABASE clock, lapsed-tested against
                    // GREATEST(db now, as_of).
                    r#"UPDATE worker_claims
                          SET released = true, lease_until = now(), heartbeat_at = now()
                        WHERE organization_id = $1 AND purpose = $2
                          AND released = false AND lease_until <= $3"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .bind(as_of)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Convenience: sweep + force-release every lapsed lane of the
    /// acting tenant. Returns the released lane purposes.
    pub async fn sweep_lapsed(
        &self,
        write: &TenantWriteScope,
    ) -> Result<Vec<String>, RepositoryError> {
        let scope = write.query_scope();
        // One database instant for the whole sweep: rows lapsed at T are
        // released against the same T, so a slow sweep cannot change the
        // verdict halfway through (and no local clock is consulted at all).
        let as_of = self.db_now().await?;
        let lapsed = self.lapsed_lanes(&scope, as_of).await?;
        let mut released = Vec::with_capacity(lapsed.len());
        for lane in &lapsed {
            if self.force_release_lapsed(write, &lane.purpose).await? {
                released.push(lane.purpose.clone());
            }
        }
        Ok(released)
    }
}
