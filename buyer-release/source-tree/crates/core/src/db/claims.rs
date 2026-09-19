//! Postgres-backed execution-ownership store (Prompt 3 §D/§L/§T) — the
//! AUTHORITATIVE [`ClaimStore`] whenever `[database]` is enabled, plus the
//! durable runtime-flag store backing kill-switch/module-gate propagation
//! (§Q).
//!
//! Acquisition is ONE atomic statement: `INSERT … ON CONFLICT DO UPDATE …
//! WHERE (lease expired | released | handoff grace elapsed) RETURNING …`.
//! Two replicas racing the same `execution_id` can never both get a row
//! back — Postgres serialises the conflicting upserts and the WHERE clause
//! re-evaluates against the winner's committed row. Fencing (renew/verify/
//! release) is compare-and-set on `(execution_id, owner_id, claim_epoch,
//! status='claimed')`.
//!
//! Fail-closed (§K/§L): any error/timeout surfaces as `Err` — the registry
//! converts it to `OwnershipUnavailable` and money paths must abort. A
//! failed release does NOT mean the claim is gone: the row keeps its lease
//! and expires on its own, and the audit history stays intact.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sqlx::postgres::PgRow;
use sqlx::Row;
use tracing::warn;

use crate::db::Database;
use crate::error::{BotError, BotResult};
use crate::models::BotModule;
use crate::ownership::{
    ClaimDecision, ClaimRequest, ClaimStatus, ClaimStore, ExecutionClaim, RuntimeFlag,
    RuntimeFlagsReader, RuntimeFlagsWriter, StoreContext,
};
use crate::risk::GlobalRiskOracle;

fn ms(d: std::time::Duration) -> i64 {
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
            .unwrap_or_else(|_| Utc::now() + ChronoDuration::seconds(45)),
        takeover_count: row.try_get("takeover_count").unwrap_or(0),
        previous_owner: row.try_get("previous_owner").ok().flatten(),
    }
}

/// Authoritative claim store: `execution_claims` (migration 0009).
pub struct PostgresClaimStore {
    db: Arc<Database>,
}

impl PostgresClaimStore {
    pub fn new(db: Arc<Database>) -> Self {
        PostgresClaimStore { db }
    }

    /// Append-only transition history (migration 0011). BEST EFFORT: an
    /// event-write failure is logged and metered but never changes the
    /// outcome of the claim operation itself — the claim row remains the
    /// authority, the events table is the audit trail (§M/§S).
    async fn record_event(
        &self,
        execution_id: &str,
        event: &str,
        owner_id: &str,
        epoch: i64,
        previous_owner: Option<&str>,
        detail: &str,
    ) {
        let res = self
            .db
            .timed(
                "claim_event",
                sqlx::query(
                    r#"INSERT INTO execution_claim_events
                           (execution_id, event, owner_id, claim_epoch, previous_owner, detail)
                       VALUES ($1, $2, $3, $4, $5, $6)"#,
                )
                .bind(execution_id)
                .bind(event)
                .bind(owner_id)
                .bind(epoch)
                .bind(previous_owner)
                .bind(detail)
                .execute(self.db.pool()),
            )
            .await;
        if let Err(e) = res {
            warn!(execution_id, event, error = %e, "claim event audit write failed (claim outcome unaffected)");
        }
    }

    async fn current(&self, execution_id: &str) -> BotResult<Option<ExecutionClaim>> {
        let row = self
            .db
            .timed(
                "claim_get",
                sqlx::query("SELECT * FROM execution_claims WHERE execution_id = $1")
                    .bind(execution_id)
                    .fetch_optional(self.db.pool()),
            )
            .await
            .map_err(BotError::from)?;
        Ok(row.as_ref().map(claim_from_row))
    }
}

#[async_trait]
impl ClaimStore for PostgresClaimStore {
    fn backend(&self) -> &'static str {
        "postgres"
    }

    async fn claim(&self, req: &ClaimRequest, ctx: &StoreContext) -> BotResult<ClaimDecision> {
        // One atomic statement. The `prev` CTE captures the pre-upsert status
        // so a takeover (prev='claimed' with a lapsed lease) is distinguishable
        // from a plain re-acquisition (prev='released'/'handed_off') for
        // metrics — without a second round trip or a read-modify-write race.
        const SQL: &str = r#"
WITH prev AS (
    SELECT status AS prev_status FROM execution_claims WHERE execution_id = $1
), ins AS (
    INSERT INTO execution_claims
        (execution_id, kind, module, strategy, symbol, owner_id, claim_epoch,
         status, claimed_at, lease_until, last_heartbeat, takeover_count,
         previous_owner, updated_at)
    VALUES
        ($1, $2, $3, $4, $5, $6, 1, 'claimed', now(),
         now() + $7::bigint * interval '1 millisecond', now(), 0, NULL, now())
    ON CONFLICT (execution_id) DO UPDATE SET
        kind           = EXCLUDED.kind,
        module         = EXCLUDED.module,
        strategy       = EXCLUDED.strategy,
        symbol         = EXCLUDED.symbol,
        owner_id       = EXCLUDED.owner_id,
        claim_epoch    = execution_claims.claim_epoch + 1,
        status         = 'claimed',
        claimed_at     = now(),
        lease_until    = now() + $7::bigint * interval '1 millisecond',
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
               + $8::bigint * interval '1 millisecond' <= now())
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
                    "claim_acquire",
                    sqlx::query(SQL)
                        .bind(&req.execution_id)
                        .bind(&req.kind)
                        .bind(&req.module)
                        .bind(&req.strategy)
                        .bind(&req.symbol)
                        .bind(&ctx.owner_id)
                        .bind(ms(ctx.lease))
                        .bind(ms(ctx.handoff_grace))
                        .fetch_optional(self.db.pool()),
                )
                .await
                .map_err(BotError::from)?;
            if let Some(row) = row {
                let prev_status: String = row.try_get("prev_status").unwrap_or_default();
                let claim = claim_from_row(&row);
                let event = match prev_status.as_str() {
                    "claimed" => {
                        // The prior row was still 'claimed' with a lapsed
                        // lease: an expiry-driven takeover (§J).
                        crate::obs::metrics::global()
                            .counter(
                                "bot_distributed_claim_expired_total",
                                "Claims found expired and taken over by a new replica.",
                                &[("module", req.module.as_str())],
                            )
                            .inc();
                        crate::obs::metrics::global()
                            .counter(
                                "bot_distributed_claim_takeover_total",
                                "Takeovers of expired claims by a new replica.",
                                &[("module", req.module.as_str())],
                            )
                            .inc();
                        "takeover"
                    }
                    "" => "acquired",
                    _ => "reacquired",
                };
                self.record_event(
                    &req.execution_id,
                    event,
                    &ctx.owner_id,
                    claim.epoch,
                    claim.previous_owner.as_deref(),
                    &format!(
                        "kind={} module={} symbol={}",
                        req.kind, req.module, req.symbol
                    ),
                )
                .await;
                return Ok(ClaimDecision::Acquired(claim));
            }
            // No row back: either legitimately held by someone else, or the
            // row was inserted between our upsert and this point (retry once).
            if attempt == 0 && self.current(&req.execution_id).await?.is_none() {
                continue;
            }
            let cur = self
                .current(&req.execution_id)
                .await?
                .ok_or_else(|| BotError::db("claim row vanished mid-race"))?;
            return Ok(ClaimDecision::Rejected {
                owner_id: cur.owner_id,
                epoch: cur.epoch,
                status: cur.status,
                lease_until: cur.lease_until,
            });
        }
        Err(BotError::db("claim race could not be settled"))
    }

    async fn renew(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool> {
        // CAS on (owner, epoch, claimed) AND lease still running: an expired
        // lease is never silently resurrected by a late renewal — it must go
        // through a fresh claim (takeover path) so epoch/audit stay honest.
        // The renewer supplies its lease length via the row's own cadence:
        // extend by the remaining configured lease is impossible here without
        // extra state, so extend by a fixed generous window derived from the
        // current lease span (claimed_at..lease_until).
        const SQL: &str = r#"
UPDATE execution_claims
SET lease_until    = now() + GREATEST(
        EXTRACT(EPOCH FROM (lease_until - claimed_at))::bigint, 1
    ) * interval '1 second',
    last_heartbeat = now(),
    updated_at     = now()
WHERE execution_id = $1 AND owner_id = $2 AND claim_epoch = $3
  AND status = 'claimed' AND lease_until > now()
"#;
        let res = self
            .db
            .timed(
                "claim_renew",
                sqlx::query(SQL)
                    .bind(execution_id)
                    .bind(owner_id)
                    .bind(epoch)
                    .execute(self.db.pool()),
            )
            .await
            .map_err(BotError::from)?;
        let ok = res.rows_affected() == 1;
        if !ok {
            let detail = match self.current(execution_id).await {
                Ok(Some(cur)) => format!(
                    "current: owner={} epoch={} status={} lease_until={}",
                    cur.owner_id,
                    cur.epoch,
                    cur.status.as_str(),
                    cur.lease_until
                ),
                _ => "current: <unavailable>".to_string(),
            };
            self.record_event(
                execution_id,
                "renew_rejected",
                owner_id,
                epoch,
                None,
                &detail,
            )
            .await;
        }
        Ok(ok)
    }

    async fn verify(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool> {
        const SQL: &str = r#"
SELECT 1 FROM execution_claims
WHERE execution_id = $1 AND owner_id = $2 AND claim_epoch = $3
  AND status = 'claimed' AND lease_until > now()
"#;
        let row = self
            .db
            .timed(
                "claim_verify",
                sqlx::query(SQL)
                    .bind(execution_id)
                    .bind(owner_id)
                    .bind(epoch)
                    .fetch_optional(self.db.pool()),
            )
            .await
            .map_err(BotError::from)?;
        if row.is_none() {
            // A money-moving continuation is about to be blocked — record
            // WHY (who holds it now) for the audit trail (§S).
            let detail = match self.current(execution_id).await {
                Ok(Some(cur)) => format!(
                    "current: owner={} epoch={} status={} lease_until={}",
                    cur.owner_id,
                    cur.epoch,
                    cur.status.as_str(),
                    cur.lease_until
                ),
                Ok(None) => "current: <absent>".to_string(),
                Err(_) => "current: <unavailable>".to_string(),
            };
            self.record_event(execution_id, "fenced", owner_id, epoch, None, &detail)
                .await;
        }
        Ok(row.is_some())
    }

    async fn release(
        &self,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
        mode: ClaimStatus,
    ) -> BotResult<bool> {
        debug_assert!(mode != ClaimStatus::Claimed);
        const SQL: &str = r#"
UPDATE execution_claims
SET status = $4, last_heartbeat = now(), updated_at = now()
WHERE execution_id = $1 AND owner_id = $2 AND claim_epoch = $3
  AND status = 'claimed'
"#;
        let res = self
            .db
            .timed(
                "claim_release",
                sqlx::query(SQL)
                    .bind(execution_id)
                    .bind(owner_id)
                    .bind(epoch)
                    .bind(mode.as_str())
                    .execute(self.db.pool()),
            )
            .await
            .map_err(BotError::from)?;
        let ok = res.rows_affected() == 1;
        if ok {
            self.record_event(execution_id, mode.as_str(), owner_id, epoch, None, "")
                .await;
        }
        Ok(ok)
    }

    async fn get(&self, execution_id: &str) -> BotResult<Option<ExecutionClaim>> {
        self.current(execution_id).await
    }
}

/// One row of the append-only claim history (migration 0011).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClaimEvent {
    pub execution_id: String,
    /// acquired | reacquired | takeover | released | handed_off | fenced |
    /// renew_rejected
    pub event: String,
    pub owner_id: String,
    pub epoch: i64,
    pub previous_owner: Option<String>,
    pub detail: String,
    pub created_at: DateTime<Utc>,
}

impl PostgresClaimStore {
    /// Full transition lineage of one logical execution (§S): every
    /// generation, oldest first. Empty when the id was never claimed.
    pub async fn events(&self, execution_id: &str) -> BotResult<Vec<ClaimEvent>> {
        let rows = self
            .db
            .timed(
                "claim_events",
                sqlx::query(
                    "SELECT * FROM execution_claim_events WHERE execution_id = $1 ORDER BY id",
                )
                .bind(execution_id)
                .fetch_all(self.db.pool()),
            )
            .await
            .map_err(BotError::from)?;
        Ok(rows
            .iter()
            .map(|row| ClaimEvent {
                execution_id: row.try_get("execution_id").unwrap_or_default(),
                event: row.try_get("event").unwrap_or_default(),
                owner_id: row.try_get("owner_id").unwrap_or_default(),
                epoch: row.try_get("claim_epoch").unwrap_or(0),
                previous_owner: row.try_get("previous_owner").ok().flatten(),
                detail: row.try_get("detail").unwrap_or_default(),
                created_at: row
                    .try_get::<DateTime<Utc>, _>("created_at")
                    .unwrap_or_else(|_| Utc::now()),
            })
            .collect())
    }
}

/// Durable runtime flags (§Q): `runtime_flags` (migration 0010).
pub struct PostgresFlags {
    db: Arc<Database>,
}

impl PostgresFlags {
    pub fn new(db: Arc<Database>) -> Self {
        PostgresFlags { db }
    }
}

#[async_trait]
impl RuntimeFlagsWriter for PostgresFlags {
    async fn write(&self, flag: &str, enabled: bool, reason: &str, updated_by: &str) {
        const SQL: &str = r#"
INSERT INTO runtime_flags (flag, enabled, reason, updated_by, updated_at)
VALUES ($1, $2, $3, $4, now())
ON CONFLICT (flag) DO UPDATE SET
    enabled    = EXCLUDED.enabled,
    reason     = EXCLUDED.reason,
    updated_by = EXCLUDED.updated_by,
    updated_at = now()
"#;
        let res = self
            .db
            .timed(
                "flags_write",
                sqlx::query(SQL)
                    .bind(flag)
                    .bind(enabled)
                    .bind(reason)
                    .bind(updated_by)
                    .execute(self.db.pool()),
            )
            .await;
        if let Err(e) = res {
            // Best-effort write side: the local replica already applied the
            // flag; remote replicas will converge on the next successful
            // write or keep their (safe) local view. Never panics, never
            // silently pretends success — logged loudly.
            warn!(flag, enabled, error = %e, "runtime flag publish FAILED (remote replicas will not see this change until a later write succeeds)");
        }
    }
}

#[async_trait]
impl RuntimeFlagsReader for PostgresFlags {
    async fn read_all(&self) -> BotResult<Vec<RuntimeFlag>> {
        let rows = self
            .db
            .timed(
                "flags_read",
                sqlx::query("SELECT * FROM runtime_flags ORDER BY flag").fetch_all(self.db.pool()),
            )
            .await
            .map_err(BotError::from)?;
        Ok(rows
            .iter()
            .map(|row| RuntimeFlag {
                flag: row.try_get("flag").unwrap_or_default(),
                enabled: row.try_get("enabled").unwrap_or(false),
                reason: row.try_get("reason").unwrap_or_default(),
                updated_by: row.try_get("updated_by").unwrap_or_default(),
                updated_at: row
                    .try_get::<DateTime<Utc>, _>("updated_at")
                    .unwrap_or_else(|_| Utc::now()),
            })
            .collect())
    }
}

/// Cluster-wide [`GlobalRiskOracle`] over the shared positions table:
/// open-position capacity and today's realized PnL as seen by ALL replicas.
///
/// Honest approximation (documented in docs/DISTRIBUTED.md): `realized_today`
/// attributes the full lifecycle PnL (`realized_quote - cost_basis`) of
/// positions CLOSED today (UTC) to today; partial exits on still-open
/// positions are only visible through each replica's local accumulator until
/// the position closes. The engine combines views conservatively
/// (`min` of local/global), so this can only tighten the daily-loss gate.
pub struct PostgresRiskOracle {
    db: Arc<Database>,
}

impl PostgresRiskOracle {
    pub fn new(db: Arc<Database>) -> Self {
        PostgresRiskOracle { db }
    }
}

#[async_trait]
impl GlobalRiskOracle for PostgresRiskOracle {
    async fn count_open(&self, module: BotModule) -> Option<usize> {
        let source = match module {
            BotModule::Sniper => "sniper",
            BotModule::Copy => "copy",
            BotModule::Polymarket => "polymarket",
            BotModule::Contract | BotModule::Telegram => return None,
        };
        let row = self
            .db
            .timed(
                "risk_count_open",
                sqlx::query(
                    "SELECT count(*)::bigint AS n FROM positions
                     WHERE status IN ('open','closing') AND source = $1",
                )
                .bind(source)
                .fetch_one(self.db.pool()),
            )
            .await
            .ok()?;
        let n: i64 = row.try_get("n").ok()?;
        Some(n.max(0) as usize)
    }

    async fn realized_today(&self) -> Option<f64> {
        let row = self
            .db
            .timed(
                "risk_realized_today",
                sqlx::query(
                    "SELECT COALESCE(SUM(realized_quote - cost_basis), 0)::double precision AS pnl
                     FROM positions
                     WHERE closed_at IS NOT NULL
                       AND closed_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
                )
                .fetch_one(self.db.pool()),
            )
            .await
            .ok()?;
        row.try_get("pnl").ok()
    }
}
