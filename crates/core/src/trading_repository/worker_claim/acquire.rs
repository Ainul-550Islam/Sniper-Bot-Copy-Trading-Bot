//! Tenant lane acquisition — THE concurrent race (PROMPT 3/10 §H49).
//!
//! This is the exact single-statement acquisition shape the legacy
//! `ha_leases` plane uses (`db::ha::HaRepo::acquire_lease`): one
//! `INSERT … ON CONFLICT DO UPDATE … WHERE` decides the winner inside
//! the database, no read-then-write, no advisory-lock prelude. The
//! ONLY change is the arbiter: `(organization_id, purpose)` (0032)
//! instead of the global `(role)`.
//!
//! Race behaviour, unchanged in shape:
//! * two workers of ONE tenant race → exactly one `RETURNING` row;
//!   the loser gets `None` from `fetch_optional` and a `Rejected`
//!   decision naming the live holder;
//! * a worker of tenant A racing a worker of tenant B is NOT a race
//!   at all — different arbiter keys, both insert their own row;
//! * an expired or released lease is takeover-able by anyone IN THAT
//!   TENANT, bumping the fencing `generation` and `takeover_count`;
//! * the incumbent re-acquiring (holder = EXCLUDED.leader_name)
//!   refreshes the lease without a takeover increment.
//!
//! Cross-tenant safety: the arbiter and every predicate carry
//! `organization_id`, so tenant A's CAS can never resolve to tenant
//! B's row — this is the constraint, not a convention.

use std::sync::Arc;

use chrono::Duration;

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{claim_from_row, ClaimDecision, TenantWorkClaim};

/// Tenant-scoped lane acquisition (`worker_claims`).
pub struct TenantWorkerClaimRepo {
    pub(super) db: Arc<Database>,
}

impl TenantWorkerClaimRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantWorkerClaimRepo { db }
    }

    /// Validate a lane name (shared by acquire/renew/verify/release).
    fn check_purpose(purpose: &str) -> Result<(), RepositoryError> {
        if purpose.trim().is_empty() || purpose.len() > 64 {
            return Err(RepositoryError::Validation("purpose must be 1..=64 chars"));
        }
        Ok(())
    }

    /// Atomically acquire (or take over) the acting tenant's lane.
    /// One statement decides; see the module docs for the race
    /// contract.
    pub async fn acquire(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
        leader_name: &str,
        ttl: Duration,
    ) -> Result<ClaimDecision, RepositoryError> {
        Self::check_purpose(purpose)?;
        if leader_name.trim().is_empty() {
            return Err(RepositoryError::Validation("leader_name"));
        }
        let ttl_secs = ttl.num_seconds().max(1) as f64;

        let row = self
            .db
            .timed(
                "tenant_worker_claim_acquire",
                sqlx::query(
                    r#"INSERT INTO worker_claims
                           (organization_id, purpose, leader_name, generation,
                            acquired_at, heartbeat_at, lease_until,
                            takeover_count, previous_leader, released)
                       VALUES ($1, $2, $3, 1, now(), now(),
                               now() + make_interval(secs => $4), 0, NULL, false)
                       ON CONFLICT (organization_id, purpose) DO UPDATE SET
                           leader_name = EXCLUDED.leader_name,
                           generation = worker_claims.generation + 1,
                           acquired_at = now(),
                           heartbeat_at = now(),
                           lease_until = now() + make_interval(secs => $4),
                           takeover_count = worker_claims.takeover_count
                               + CASE WHEN worker_claims.leader_name <> EXCLUDED.leader_name
                                       AND worker_claims.released = false
                                      THEN 1 ELSE 0 END,
                           previous_leader = worker_claims.leader_name,
                           released = false
                       WHERE worker_claims.lease_until <= now()
                          OR worker_claims.released = true
                          OR worker_claims.leader_name = EXCLUDED.leader_name
                       RETURNING organization_id, purpose, leader_name, generation,
                                 acquired_at, heartbeat_at, lease_until,
                                 takeover_count, previous_leader, released"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .bind(leader_name)
                .bind(ttl_secs)
                .fetch_optional(self.db.pool()),
            )
            .await?;

        match row.as_ref().map(claim_from_row) {
            Some(claim) => Ok(ClaimDecision::Acquired(claim)),
            None => {
                // The WHERE refused: a live leader of THIS TENANT holds
                // the lane. (A foreign tenant's lane is a different
                // arbiter key and cannot surface here.)
                let current = self.current(write, purpose).await?;
                match current {
                    Some(c) => Ok(ClaimDecision::Rejected {
                        holder: c.leader_name,
                        generation: c.generation,
                        lease_until: c.lease_until,
                    }),
                    None => Err(RepositoryError::Storage(
                        "worker claim row vanished during acquisition".to_string(),
                    )),
                }
            }
        }
    }

    /// The acting tenant's current lane state (live or lapsed).
    pub async fn current(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
    ) -> Result<Option<TenantWorkClaim>, RepositoryError> {
        Self::check_purpose(purpose)?;
        let row = self
            .db
            .timed(
                "tenant_worker_claim_get",
                sqlx::query(
                    r#"SELECT organization_id, purpose, leader_name, generation,
                              acquired_at, heartbeat_at, lease_until,
                              takeover_count, previous_leader, released
                         FROM worker_claims
                        WHERE organization_id = $1 AND purpose = $2"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(claim_from_row))
    }

    /// All of the acting tenant's lanes (operator triage view).
    pub async fn lanes(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<Vec<TenantWorkClaim>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_worker_claim_lanes",
                sqlx::query(
                    r#"SELECT organization_id, purpose, leader_name, generation,
                              acquired_at, heartbeat_at, lease_until,
                              takeover_count, previous_leader, released
                         FROM worker_claims
                        WHERE organization_id = $1
                        ORDER BY purpose ASC"#,
                )
                .bind(scope.organization_id().as_uuid())
                .fetch_all(self.db.pool()),
            )
            .await?;
        let rows: Vec<TenantWorkClaim> = rows.iter().map(claim_from_row).collect();
        crate::trading_repository::tenant_assert::assert_rows_org(scope.organization_id(), &rows)?;
        Ok(rows)
    }
}
