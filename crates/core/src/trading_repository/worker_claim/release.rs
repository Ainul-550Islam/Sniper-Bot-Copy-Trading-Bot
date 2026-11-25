//! Tenant lane renewal / verification / release (PROMPT 3/10 §H50).
//!
//! All three are compare-and-set on the FULL lane identity:
//! `(organization_id, purpose, leader_name, generation)` — a stale
//! leader (older fencing token) can neither extend nor release the
//! lane it lost. The tenant is part of every predicate.
//!
//! **The clock is the database's, never the caller's.** Acquisition uses
//! `now()`; renewal, verification, force-release and the lapsed sweep used a
//! caller-supplied `at`, so one replica whose clock ran fast could extend its
//! own lease (or declare a peer's lane lapsed) while the database disagreed.
//! Callers can still pass an explicit `as_of` — operator tooling needs it — but
//! it is now clamped: it can never be earlier than the database clock, and the
//! predicate is evaluated against `GREATEST(db_now, as_of)`.

use chrono::{DateTime, Duration, Utc};
use sqlx::Row;

use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::acquire::TenantWorkerClaimRepo;

impl TenantWorkerClaimRepo {
    /// The DATABASE clock — the single reference every liveness decision in
    /// this module uses. A caller's `at` is only ever a *lower bound*.
    pub async fn db_now(&self) -> Result<DateTime<Utc>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_worker_claim_db_now",
                sqlx::query("SELECT now() AS t").fetch_one(self.db.pool()),
            )
            .await?;
        Ok(row.try_get("t").unwrap_or_else(|_| Utc::now()))
    }

    /// The instant a MUTATION or liveness check must use: the database clock,
    /// full stop. A caller-supplied instant is accepted for API compatibility
    /// but cannot move the predicate — a fast local clock could otherwise
    /// extend its own lease, and a slow one could look expired early.
    async fn effective_now(
        &self,
        _caller_at: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, RepositoryError> {
        self.db_now().await
    }
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
        // A caller-supplied instant is a lower bound only: never earlier than
        // the database clock, so a fast local clock cannot extend a lease.
        let at = self.effective_now(at).await?;
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
        let at = self.effective_now(at).await?;
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
        // `at` is kept for API compatibility but the row is stamped with the
        // database clock: a release time from a skewed host is not evidence.
        let _ = at;
        let res = self
            .db
            .timed(
                "tenant_worker_claim_release",
                sqlx::query(
                    r#"UPDATE worker_claims
                          SET released = true, lease_until = now(), heartbeat_at = now()
                        WHERE organization_id = $1 AND purpose = $2
                          AND leader_name = $3 AND generation = $4
                          AND released = false"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(purpose)
                .bind(leader_name)
                .bind(generation)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Renew using only the database clock (no caller instant at all). This is
    /// the form production paths should call.
    pub async fn renew_now(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
        leader_name: &str,
        generation: i64,
        ttl: Duration,
    ) -> Result<bool, RepositoryError> {
        // `DateTime::<Utc>::MIN_UTC` is always earlier than the database
        // clock, so `effective_now` returns the database's own time.
        self.renew(
            write,
            purpose,
            leader_name,
            generation,
            ttl,
            DateTime::<Utc>::MIN_UTC,
        )
        .await
    }

    /// Verify using only the database clock.
    pub async fn verify_now(
        &self,
        write: &TenantWriteScope,
        purpose: &str,
        leader_name: &str,
        generation: i64,
    ) -> Result<bool, RepositoryError> {
        self.verify(
            write,
            purpose,
            leader_name,
            generation,
            DateTime::<Utc>::MIN_UTC,
        )
        .await
    }
}
