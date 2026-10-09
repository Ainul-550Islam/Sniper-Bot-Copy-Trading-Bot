//! Tenant-safe execution claim acquisition/release (PROMPT 3/10 §C18).
//!
//! The claim RACE SEMANTICS are the legacy `PostgresClaimStore` shape,
//! PRESERVED EXACTLY — one atomic statement, `prev` CTE, conditional
//! `DO UPDATE … WHERE (released | lease expired | handoff grace)`,
//! `RETURNING` — with exactly two deltas required by the 0028 swap:
//!
//! * the arbiter is `ON CONFLICT (organization_id, execution_id)`
//!   (the tenant-composite PRIMARY KEY);
//! * the organization is BOUND from the acting tenant (never defaulted).
//!
//! Consequences (all regression-tested):
//!
//! * two workers of the SAME tenant racing the same execution_id →
//!   exactly one winner (Postgres serialises the conflicting upserts on
//!   the composite key) — the
//!   `two_contexts_pg_claim_race_single_owner` invariant, unchanged;
//! * the same execution_id under TWO different tenants → two
//!   INDEPENDENT claims (different arbiter values never conflict) —
//!   the enterprise requirement: tenant B can never acquire, fence or
//!   release tenant A's claim.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::db::Database;
use crate::ownership::{ClaimDecision, ClaimRequest, ClaimStatus, ExecutionClaim};
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

fn ms(d: Duration) -> i64 {
    d.as_millis().max(1) as i64
}

fn claim_from_row(row: &PgRow) -> ExecutionClaim {
    let status_s: String = row.try_get("status").unwrap_or_default();
    ExecutionClaim {
        execution_id: row.try_get("execution_id").unwrap_or_default(),
        kind: row.try_get("kind").unwrap_or_default(),
        module: row.try_get("module").unwrap_or_default(),
        strategy: row.try_get("strategy").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        owner_id: row.try_get("owner_id").unwrap_or_default(),
        epoch: row.try_get("claim_epoch").unwrap_or(1),
        status: ClaimStatus::parse(&status_s).unwrap_or(ClaimStatus::Claimed),
        acquired_at: row.try_get("claimed_at").unwrap_or_else(|_| Utc::now()),
        lease_until: row
            .try_get("lease_until")
            .unwrap_or_else(|_| Utc::now() + chrono::Duration::seconds(45)),
        takeover_count: row.try_get("takeover_count").unwrap_or(0),
        previous_owner: row.try_get("previous_owner").ok().flatten(),
    }
}

/// Tenant-scoped claim repository (`execution_claims`, 0028 arbiter).
pub struct TenantClaimRepo {
    db: Arc<Database>,
}

impl TenantClaimRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantClaimRepo { db }
    }

    /// The acting tenant's claim row for one logical execution, if any.
    /// Another tenant's claim with the same execution_id is invisible.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        execution_id: &str,
    ) -> Result<Option<ExecutionClaim>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_claim_get",
                sqlx::query(
                    r#"SELECT * FROM execution_claims
                        WHERE organization_id = $1 AND execution_id = $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(execution_id)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(claim_from_row))
    }

    /// Atomic acquisition for the acting tenant. `ClaimDecision`
    /// mirrors the legacy store: `Acquired` / `Rejected { owner, epoch,
    /// status, lease }` (the OWNER reported on rejection is always a
    /// worker of the ACTING tenant — cross-tenant owners are never
    /// visible).
    pub async fn claim(
        &self,
        scope: &TradingQueryScope,
        req: &ClaimRequest,
        owner_id: &str,
        lease: Duration,
        handoff_grace: Duration,
    ) -> Result<ClaimDecision, RepositoryError> {
        if req.execution_id.trim().is_empty() {
            return Err(RepositoryError::Validation("execution_id"));
        }
        if owner_id.trim().is_empty() {
            return Err(RepositoryError::Validation("owner_id"));
        }
        const SQL: &str = r#"
WITH prev AS (
    SELECT status AS prev_status FROM execution_claims
    WHERE organization_id = $1 AND execution_id = $2
), ins AS (
    INSERT INTO execution_claims
        (organization_id, execution_id, kind, module, strategy, symbol,
         owner_id, claim_epoch, status, claimed_at, lease_until,
         last_heartbeat, takeover_count, previous_owner, updated_at)
    VALUES
        ($1, $2, $3, $4, $5, $6, $7, 1, 'claimed', now(),
         now() + $8::bigint * interval '1 millisecond', now(), 0, NULL, now())
    ON CONFLICT (organization_id, execution_id) DO UPDATE SET
        kind           = EXCLUDED.kind,
        module         = EXCLUDED.module,
        strategy       = EXCLUDED.strategy,
        symbol         = EXCLUDED.symbol,
        owner_id       = EXCLUDED.owner_id,
        claim_epoch    = execution_claims.claim_epoch + 1,
        status         = 'claimed',
        claimed_at     = now(),
        lease_until    = now() + $8::bigint * interval '1 millisecond',
        last_heartbeat = now(),
        takeover_count = execution_claims.takeover_count
                         + CASE WHEN execution_claims.status = 'claimed' THEN 1 ELSE 0 END,
        previous_owner = execution_claims.owner_id,
        updated_at     = now()
    WHERE execution_claims.status = 'released'
       OR (execution_claims.status = 'claimed'
           AND execution_claims.lease_until <= now())
       OR (execution_claims.status = 'handed_off'
           AND execution_claims.updated_at
               + $9::bigint * interval '1 millisecond' <= now())
    RETURNING execution_id, kind, module, strategy, symbol, owner_id,
              claim_epoch, status, claimed_at, lease_until, takeover_count,
              previous_owner
)
SELECT ins.*, COALESCE(prev.prev_status, '') AS prev_status
FROM ins LEFT JOIN prev ON TRUE
"#;
        for attempt in 0..2 {
            let row = self
                .db
                .timed(
                    "tenant_claim_acquire",
                    sqlx::query(SQL)
                        .bind(scope.organization_id().as_uuid())
                        .bind(&req.execution_id)
                        .bind(&req.kind)
                        .bind(&req.module)
                        .bind(&req.strategy)
                        .bind(&req.symbol)
                        .bind(owner_id)
                        .bind(ms(lease))
                        .bind(ms(handoff_grace))
                        .fetch_optional(self.db.pool()),
                )
                .await?;
            if let Some(row) = row {
                let _prev: String = row.try_get("prev_status").unwrap_or_default();
                return Ok(ClaimDecision::Acquired(claim_from_row(&row)));
            }
            // No row: held by another worker OF THIS TENANT, or the row
            // appeared between upsert and now (one retry, legacy shape).
            if attempt == 0 && self.get(scope, &req.execution_id).await?.is_none() {
                continue;
            }
            let cur = self
                .get(scope, &req.execution_id)
                .await?
                .ok_or_else(|| RepositoryError::Storage("claim row vanished mid-race".into()))?;
            return Ok(ClaimDecision::Rejected {
                owner_id: cur.owner_id,
                epoch: cur.epoch,
                status: cur.status,
                lease_until: cur.lease_until,
            });
        }
        Err(RepositoryError::Storage(
            "claim race could not be settled".into(),
        ))
    }

    /// Renew (CAS on owner + epoch + claimed + lease running), scoped to
    /// the acting tenant. A cross-tenant row never matches.
    pub async fn renew(
        &self,
        scope: &TradingQueryScope,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
    ) -> Result<bool, RepositoryError> {
        const SQL: &str = r#"
UPDATE execution_claims
SET lease_until    = now() + GREATEST(
        EXTRACT(EPOCH FROM (lease_until - claimed_at))::bigint, 1
    ) * interval '1 second',
    last_heartbeat = now(),
    updated_at     = now()
WHERE organization_id = $1 AND execution_id = $2
  AND owner_id = $3 AND claim_epoch = $4
  AND status = 'claimed' AND lease_until > now()
"#;
        let res = self
            .db
            .timed(
                "tenant_claim_renew",
                sqlx::query(SQL)
                    .bind(scope.organization_id().as_uuid())
                    .bind(execution_id)
                    .bind(owner_id)
                    .bind(epoch)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Verify the fencing token, scoped to the acting tenant.
    pub async fn verify(
        &self,
        scope: &TradingQueryScope,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
    ) -> Result<bool, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_claim_verify",
                sqlx::query(
                    r#"SELECT 1 FROM execution_claims
                        WHERE organization_id = $1 AND execution_id = $2
                          AND owner_id = $3 AND claim_epoch = $4
                          AND status = 'claimed' AND lease_until > now()"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(execution_id)
                .bind(owner_id)
                .bind(epoch)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.is_some())
    }

    /// Release / hand off (CAS), scoped to the acting tenant.
    pub async fn release(
        &self,
        scope: &TradingQueryScope,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
        mode: ClaimStatus,
    ) -> Result<bool, RepositoryError> {
        debug_assert!(mode != ClaimStatus::Claimed);
        const SQL: &str = r#"
UPDATE execution_claims
SET status = $5, last_heartbeat = now(), updated_at = now()
WHERE organization_id = $1 AND execution_id = $2
  AND owner_id = $3 AND claim_epoch = $4 AND status = 'claimed'
"#;
        let res = self
            .db
            .timed(
                "tenant_claim_release",
                sqlx::query(SQL)
                    .bind(scope.organization_id().as_uuid())
                    .bind(execution_id)
                    .bind(owner_id)
                    .bind(epoch)
                    .bind(mode.as_str())
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// The acting tenant's claims held by one worker (recovery view).
    pub async fn list_for_owner(
        &self,
        scope: &TradingQueryScope,
        owner_id: &str,
    ) -> Result<Vec<ExecutionClaim>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_claims_for_owner",
                sqlx::query(
                    r#"SELECT * FROM execution_claims
                        WHERE organization_id = $1 AND owner_id = $2
                        ORDER BY updated_at DESC LIMIT 1000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(owner_id)
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(claim_from_row).collect())
    }

    /// Lapsed leases of the acting tenant (takeover candidates) —
    /// `as_of` lets tests freeze time; production passes `Utc::now()`.
    pub async fn list_lapsed(
        &self,
        scope: &TradingQueryScope,
        as_of: DateTime<Utc>,
    ) -> Result<Vec<ExecutionClaim>, RepositoryError> {
        let rows = self
            .db
            .timed(
                "tenant_claims_lapsed",
                sqlx::query(
                    r#"SELECT * FROM execution_claims
                        WHERE organization_id = $1 AND status = 'claimed'
                          AND lease_until <= $2
                        ORDER BY lease_until ASC LIMIT 1000"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(as_of)
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(claim_from_row).collect())
    }
}
