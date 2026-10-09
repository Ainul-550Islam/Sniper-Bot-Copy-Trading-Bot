//! Tenant lane renewal / verification / release (PROMPT 3/10 §H50).
//!
//! All three are compare-and-set on the FULL lane identity:
//! `(organization_id, purpose, leader_name, generation)` — a stale
//! leader (older fencing token) can neither extend nor release the
//! lane it lost. The tenant is part of every predicate.

use chrono::{DateTime, Duration, Utc};

use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::acquire::TenantWorkerClaimRepo;

impl TenantWorkerClaimRepo {
    /// Heartbeat + extend: only the CURRENT live leader of THIS
    /// tenant's lane succeeds. `Ok(false)` = lost the lane (stale
    /// token, takeover, release, or expiry).
    pub async fn renew(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
        leader_name: &str,
        generation: i64,
        ttl: Duration,
        at: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let ttl_secs = ttl.num_seconds().max(1) as f64;
        let res = self
            .db
            .timed(
                "tenant_worker_claim_renew",
                sqlx::query(
                    r#"UPDATE worker_claims
                          SET heartbeat_at = $5,
                              lease_until = $5 + make_interval(secs => $6)
                        WHERE organization_id = $1 AND purpose = $2
                          AND leader_name = $3 AND generation = $4
                          AND released = false AND lease_until > $5"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .bind(leader_name)
                .bind(generation)
                .bind(at)
                .bind(ttl_secs)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Fencing check: does `(leader_name, generation)` still lead THIS
    /// tenant's lane with a live lease? The downstream write path
    /// calls this (or compares tokens) before acting.
    pub async fn verify(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
        leader_name: &str,
        generation: i64,
        at: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_worker_claim_verify",
                sqlx::query(
                    r#"SELECT 1 FROM worker_claims
                        WHERE organization_id = $1 AND purpose = $2
                          AND leader_name = $3 AND generation = $4
                          AND released = false AND lease_until > $5"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .bind(leader_name)
                .bind(generation)
                .bind(at)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.is_some())
    }

    /// Voluntary release: the current live leader steps down. The row
    /// stays (audit trail: previous_leader/generation), marked
    /// `released` — instantly takeover-able by another worker of the
    /// SAME tenant, invisible to other tenants.
    pub async fn release(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
        leader_name: &str,
        generation: i64,
        at: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_worker_claim_release",
                sqlx::query(
                    r#"UPDATE worker_claims
                          SET released = true, lease_until = $5, heartbeat_at = $5
                        WHERE organization_id = $1 AND purpose = $2
                          AND leader_name = $3 AND generation = $4
                          AND released = false"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .bind(leader_name)
                .bind(generation)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// `Utc` helper for callers that want "now" semantics spelled out.
    pub fn now() -> DateTime<Utc> {
        Utc::now()
    }
}
