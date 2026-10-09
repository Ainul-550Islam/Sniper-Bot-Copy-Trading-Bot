//! Durable DCA scheduler (remediation tree, Part 3).
//!
//! Each due schedule yields at most ONE intent per interval slot. The intent
//! row, the spend increment, and the clock advance commit in one statement, so
//! a crash cannot double-spend and cannot record a run without also charging it.
//!
//! Guarantees:
//! * **lease-based claim** with `FOR UPDATE SKIP LOCKED`; only the lease owner
//!   can write a run for that schedule;
//! * **exactly-once per slot**: `dca_runs (schedule_id, slot_at)` is UNIQUE and
//!   the insert is `ON CONFLICT DO NOTHING`;
//! * **hard budget**: `spent_sol` never exceeds `budget_sol` (schema CHECK), and
//!   the final run is clamped to the remaining budget;
//! * **no catch-up burst**: after a run the clock moves to the first whole
//!   interval strictly after `now`, so a worker that was down for a week
//!   produces one run, not 168;
//! * **intent, not fill**: `dca_runs.status = 'intent'`. Execution is a separate
//!   outbox step that moves a row to `submitted`/`failed`; this worker never
//!   reports a fill it did not observe.

use std::time::Duration as StdDuration;

use sqlx::{PgPool, Row};
use tokio::sync::watch;
use uuid::Uuid;

/// Failed attempts before a schedule is parked and no longer claimed.
pub const MAX_ATTEMPTS: i32 = 5;
/// Schedules claimed per tick.
pub const CLAIM_BATCH: i64 = 100;

/// Worker tuning.
#[derive(Debug, Clone)]
pub struct DcaWorkerConfig {
    pub worker_id: String,
    pub lease_secs: f64,
    pub tick: StdDuration,
}

/// A schedule claimed under a lease.
#[derive(Debug, Clone, PartialEq)]
pub struct DueSchedule {
    pub id: String,
    pub organization_id: Uuid,
    pub mint: String,
    pub amount_per_run_sol: f64,
    pub budget_sol: f64,
    pub spent_sol: f64,
    pub interval_secs: i64,
    pub next_run_at: chrono::DateTime<chrono::Utc>,
}

/// Result of processing one claimed schedule.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// An intent for the slot was created and the clock advanced.
    Recorded { amount_sol: f64, completed: bool },
    /// The budget was already spent: the schedule is now `completed`.
    BudgetExhausted,
    /// The slot was already recorded by an earlier transaction. Nothing was
    /// written; the lease is released so the next claim sees fresh state.
    AlreadyRecorded,
}

/// Claim due schedules under a lease.
pub async fn claim_due(
    pool: &PgPool,
    worker_id: &str,
    lease_secs: f64,
) -> Result<Vec<DueSchedule>, sqlx::Error> {
    let rows = sqlx::query(
        "UPDATE dca_schedules \
         SET lease_owner = $1, lease_expires_at = now() + make_interval(secs => $2::float8) \
         WHERE id IN ( \
             SELECT id FROM dca_schedules \
             WHERE status = 'active' \
               AND next_run_at <= now() \
               AND (lease_expires_at IS NULL OR lease_expires_at < now()) \
               AND attempts < $3 \
             ORDER BY next_run_at, id \
             LIMIT $4 \
             FOR UPDATE SKIP LOCKED) \
         RETURNING id, organization_id, mint, amount_per_run_sol::float8 AS amount_per_run_sol, \
                   budget_sol::float8 AS budget_sol, spent_sol::float8 AS spent_sol, \
                   interval_secs, next_run_at",
    )
    .bind(worker_id)
    .bind(lease_secs)
    .bind(MAX_ATTEMPTS)
    .bind(CLAIM_BATCH)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| DueSchedule {
            id: row.get("id"),
            organization_id: row.get("organization_id"),
            mint: row.get("mint"),
            amount_per_run_sol: row.get("amount_per_run_sol"),
            budget_sol: row.get("budget_sol"),
            spent_sol: row.get("spent_sol"),
            interval_secs: row.get("interval_secs"),
            next_run_at: row.get("next_run_at"),
        })
        .collect())
}

/// Record the due slot as one atomic unit under the lease we hold.
///
/// The lease owner check is repeated inside every write, so a replica whose
/// lease expired mid-tick writes nothing.
pub async fn record_slot(
    pool: &PgPool,
    schedule: &DueSchedule,
    worker_id: &str,
) -> Result<RunOutcome, sqlx::Error> {
    let remaining = (schedule.budget_sol - schedule.spent_sol).max(0.0);
    if remaining <= 0.0 {
        sqlx::query(
            "UPDATE dca_schedules SET status = 'completed', lease_owner = NULL, lease_expires_at = NULL \
             WHERE id = $1 AND organization_id = $2 AND lease_owner = $3",
        )
        .bind(&schedule.id)
        .bind(schedule.organization_id)
        .bind(worker_id)
        .execute(pool)
        .await?;
        return Ok(RunOutcome::BudgetExhausted);
    }

    // One statement: intent insert + spend + clock advance. If the insert hits
    // the slot UNIQUE, the CTE is empty and the UPDATE matches nothing.
    let row = sqlx::query(
        "WITH ins AS ( \
             INSERT INTO dca_runs (organization_id, schedule_id, slot_at, amount_sol, status) \
             SELECT d.organization_id, d.id, d.next_run_at, \
                    LEAST(d.amount_per_run_sol, d.budget_sol - d.spent_sol), 'intent' \
             FROM dca_schedules d \
             WHERE d.id = $1 AND d.organization_id = $2 AND d.lease_owner = $3 \
               AND d.budget_sol - d.spent_sol > 0 \
             ON CONFLICT (schedule_id, slot_at) DO NOTHING \
             RETURNING amount_sol) \
         UPDATE dca_schedules d \
         SET spent_sol = d.spent_sol + ins.amount_sol, \
             last_run_at = d.next_run_at, \
             next_run_at = d.next_run_at + make_interval(secs => \
                 (floor(extract(epoch from (now() - d.next_run_at)) / d.interval_secs::float8) + 1) \
                 * d.interval_secs::float8), \
             status = CASE WHEN d.spent_sol + ins.amount_sol >= d.budget_sol \
                           THEN 'completed' ELSE d.status END, \
             lease_owner = NULL, lease_expires_at = NULL, attempts = 0 \
         FROM ins \
         WHERE d.id = $1 AND d.lease_owner = $3 \
         RETURNING ins.amount_sol::float8 AS amount_sol, d.status",
    )
    .bind(&schedule.id)
    .bind(schedule.organization_id)
    .bind(worker_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => Ok(RunOutcome::Recorded {
            amount_sol: row.get("amount_sol"),
            completed: row.get::<String, _>("status") == "completed",
        }),
        None => {
            sqlx::query(
                "UPDATE dca_schedules SET lease_owner = NULL, lease_expires_at = NULL \
                 WHERE id = $1 AND organization_id = $2 AND lease_owner = $3",
            )
            .bind(&schedule.id)
            .bind(schedule.organization_id)
            .bind(worker_id)
            .execute(pool)
            .await?;
            Ok(RunOutcome::AlreadyRecorded)
        }
    }
}

/// Record a failed tick for one schedule and release the lease.
pub async fn record_failure(
    pool: &PgPool,
    schedule: &DueSchedule,
    worker_id: &str,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    let clamped: String = reason.chars().take(512).collect();
    sqlx::query(
        "UPDATE dca_schedules SET attempts = attempts + 1, last_error = $4, \
             lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = $1 AND organization_id = $2 AND lease_owner = $3",
    )
    .bind(&schedule.id)
    .bind(schedule.organization_id)
    .bind(worker_id)
    .bind(clamped)
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
}

/// One pass over the due schedules. Per-schedule errors are counted, not fatal.
pub async fn tick_once(pool: &PgPool, cfg: &DcaWorkerConfig) -> Result<DcaTickReport, sqlx::Error> {
    let due = claim_due(pool, &cfg.worker_id, cfg.lease_secs).await?;
    let mut report = DcaTickReport {
        claimed: due.len(),
        ..DcaTickReport::default()
    };
    for schedule in &due {
        match record_slot(pool, schedule, &cfg.worker_id).await {
            Ok(RunOutcome::Recorded { .. }) => report.recorded += 1,
            Ok(RunOutcome::BudgetExhausted) => report.completed += 1,
            Ok(RunOutcome::AlreadyRecorded) => report.already_recorded += 1,
            Err(error) => {
                report.failed += 1;
                tracing::error!(schedule = %schedule.id, error = %error, "dca slot recording failed");
                if let Err(e) =
                    record_failure(pool, schedule, &cfg.worker_id, &error.to_string()).await
                {
                    tracing::error!(schedule = %schedule.id, error = %e, "recording dca failure failed");
                }
            }
        }
    }
    Ok(report)
}

/// Counts for one DCA tick.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DcaTickReport {
    pub claimed: usize,
    pub recorded: usize,
    pub completed: usize,
    pub already_recorded: usize,
    pub failed: usize,
}

/// Run until `stop` flips to `true`. Storage errors back off; the loop does not exit.
pub async fn run(pool: PgPool, cfg: DcaWorkerConfig, mut stop: watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            break;
        }
        let pause = match tick_once(&pool, &cfg).await {
            Ok(_) => cfg.tick,
            Err(error) => {
                tracing::error!(error = %error, "dca tick failed; backing off");
                cfg.tick.max(StdDuration::from_secs(5))
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_are_distinct() {
        assert_ne!(
            RunOutcome::AlreadyRecorded,
            RunOutcome::BudgetExhausted,
            "a replayed slot must not look like a finished budget"
        );
    }

    #[test]
    fn attempt_ceiling_is_positive() {
        assert!(MAX_ATTEMPTS > 0 && CLAIM_BATCH > 0);
    }
}
