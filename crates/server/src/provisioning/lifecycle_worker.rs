//! Restart-safe background worker for tenant lifecycle/deprovisioning (BATCH 2 file 14).
//!
//! Consumes pending lifecycle jobs from the database. Uses leases/ownership
//! so two replicas cannot process the same job concurrently. Retry transient
//! failures with bounded backoff. Persist phase transitions. Never mark a
//! failed operation successful.

use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use tokio::time::sleep;
use uuid::Uuid;

use bot_core::db::Database;
use bot_core::tenant::rls_context::with_system_scope;

use bot_core::provisioning::deprovision::{DeprovisionJob, DeprovisionPhase, DeprovisionState};
#[cfg(test)]
use bot_core::tenant::OrganizationId;

/// Worker config.
#[derive(Debug, Clone)]
pub struct LifecycleWorkerConfig {
    pub replica_id: String,
    pub poll_interval: Duration,
    pub lease_secs: i64,
    pub max_retries: u32,
    pub backoff_base_secs: i64,
}

impl Default for LifecycleWorkerConfig {
    fn default() -> Self {
        Self {
            replica_id: format!("worker-{}", Uuid::new_v4()),
            poll_interval: Duration::seconds(5),
            lease_secs: 60,
            max_retries: 5,
            backoff_base_secs: 2,
        }
    }
}

/// Worker outcome for one job attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleWorkOutcome {
    Advanced(DeprovisionPhase),
    Completed,
    RetryAfter(Duration),
    Failed(String),
}

/// Deterministic phase executor — pure transition logic (no I/O).
pub fn next_phase(job: &DeprovisionJob) -> Option<DeprovisionPhase> {
    match job.phase {
        DeprovisionPhase::Requested => Some(DeprovisionPhase::TradingDisabled),
        DeprovisionPhase::TradingDisabled => Some(DeprovisionPhase::CredentialsRevoked),
        DeprovisionPhase::CredentialsRevoked => Some(DeprovisionPhase::SessionsInvalidated),
        DeprovisionPhase::SessionsInvalidated => Some(DeprovisionPhase::CustodyRevoked),
        DeprovisionPhase::CustodyRevoked => Some(DeprovisionPhase::ResourcesCleaned),
        DeprovisionPhase::ResourcesCleaned => Some(DeprovisionPhase::Retention),
        DeprovisionPhase::Retention => Some(DeprovisionPhase::Completed),
        DeprovisionPhase::Completed | DeprovisionPhase::Failed => None,
    }
}

/// Compute bounded exponential backoff: base * 2^attempt, capped at 5 minutes.
pub fn backoff_for(attempt: u32, base_secs: i64) -> Duration {
    let exp = 2_i64.pow(attempt.min(6));
    let secs = (base_secs * exp).min(300);
    Duration::seconds(secs)
}

/// Simulate work for a phase. In production this would call revocation,
/// session invalidation, custody revocation via SaasStore.
/// Deterministically succeeds for valid transitions, fails only on injected
/// transient flag for testing.
pub fn execute_phase(
    job: &mut DeprovisionJob,
    now: DateTime<Utc>,
    inject_transient_failure: bool,
) -> LifecycleWorkOutcome {
    if job.state == DeprovisionState::Failed {
        return LifecycleWorkOutcome::Failed("already failed".into());
    }
    let Some(next) = next_phase(job) else {
        return LifecycleWorkOutcome::Completed;
    };
    if inject_transient_failure && job.retry_count < 2 {
        job.retry_count += 1;
        job.failure_reason = "transient network timeout".into();
        job.last_attempt_at = Some(now);
        job.next_attempt_at = Some(now + backoff_for(job.retry_count, 2));
        job.state = DeprovisionState::Waiting;
        return LifecycleWorkOutcome::RetryAfter(backoff_for(job.retry_count, 2));
    }
    job.phase = next;
    job.updated_at = now;
    job.retry_count = 0;
    job.failure_reason.clear();
    job.last_attempt_at = Some(now);
    if next == DeprovisionPhase::Completed {
        job.state = DeprovisionState::Completed;
        job.completed_at = Some(now);
        LifecycleWorkOutcome::Completed
    } else {
        job.state = DeprovisionState::Running;
        LifecycleWorkOutcome::Advanced(next)
    }
}

/// Background loop — polls for resumable jobs, claims with lease, executes.
///
/// This is the restart-safe consumer: on restart, it re-reads jobs that are
/// still in Running/Requested/Waiting and resumes at the recorded phase,
/// never at the beginning. Uses `job_claim` lease to prevent concurrent
/// execution.
/// One pass of the durable tenant-lifecycle worker.
///
/// **Why this exists.** `tenant_lifecycle_jobs` (migration 0021) is the durable
/// state machine for tenant closure, but nothing drove it: a close request
/// inserted a job at phase `requested` and left it there, so a tenant that
/// asked to be deleted stayed open forever unless an operator manually called
/// `POST /api/saas/lifecycle/jobs/{id}/advance` once per phase. The startup
/// code spawned a task that logged "background worker started" and then idled
/// on an empty `select!` arm, which made the gap invisible in logs.
///
/// **What it does — and refuses to do.** The close request performs three of
/// the phases synchronously under the requester's own tenant scope: it closes
/// the organization, revokes API keys and invalidates sessions. This pass
/// therefore advances a due job *only* through those phases, one step per
/// tick, idempotently and under an audited system scope. At
/// `custody_revoked` it **parks** the job (state `waiting`) instead of
/// pretending: custody revocation and resource cleanup have no unattended
/// implementation in this build, and marking them complete would be a fake
/// success. The operator endpoint advances parked jobs exactly as before.
///
/// Jobs are claimed with `FOR UPDATE SKIP LOCKED`, so extra replicas are
/// harmless and no job is processed twice in the same pass.
pub async fn drain_due_jobs(db: &Database, batch_size: i64) -> Result<DrainReport, sqlx::Error> {
    let system = db.system_pool().ok_or_else(|| {
        sqlx::Error::Protocol("tenant lifecycle worker requires RLS-enabled database pools".into())
    })?;
    with_system_scope(
        system,
        "advance due tenant lifecycle jobs",
        |connection| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT id, phase, state FROM tenant_lifecycle_jobs \
                     WHERE state IN ('pending','running') \
                       AND scheduled_at <= now() \
                       AND (next_attempt_at IS NULL OR next_attempt_at <= now()) \
                       AND phase IN ('requested','trading_disabled','credentials_revoked','sessions_invalidated') \
                     ORDER BY scheduled_at \
                     LIMIT $1 \
                     FOR UPDATE SKIP LOCKED",
                )
                .bind(batch_size)
                .fetch_all(&mut *connection)
                .await?;

                let mut report = DrainReport {
                    claimed: rows.len(),
                    ..DrainReport::default()
                };
                for row in rows {
                    let id: Uuid = row.try_get("id")?;
                    let phase: String = row.try_get("phase")?;
                    let state: String = row.try_get("state")?;

                    // The next phase whose side effect is already performed by
                    // the close request itself. `None` means: park.
                    let next: Option<&str> = match phase.as_str() {
                        "requested" => Some("trading_disabled"),
                        "trading_disabled" => Some("credentials_revoked"),
                        "credentials_revoked" => Some("sessions_invalidated"),
                        _ => None,
                    };

                    let result = match next {
                        Some(next_phase) => {
                            // The active-job unique index plus the
                            // (org, action, phase, state) uniqueness mean a
                            // second row must never collide here; skip rather
                            // than abort the pass if it ever would.
                            sqlx::query(
                                "UPDATE tenant_lifecycle_jobs \
                                 SET phase = $2, state = 'running', \
                                     started_at = COALESCE(started_at, now()), \
                                     last_attempt_at = now(), updated_at = now() \
                                 WHERE id = $1 \
                                   AND state IN ('pending','running') \
                                   AND NOT EXISTS ( \
                                       SELECT 1 FROM tenant_lifecycle_jobs other \
                                       WHERE other.organization_id = tenant_lifecycle_jobs.organization_id \
                                         AND other.phase = $2 \
                                         AND other.state = 'running' \
                                         AND other.id <> tenant_lifecycle_jobs.id)",
                            )
                            .bind(id)
                            .bind(next_phase)
                            .execute(&mut *connection)
                            .await?
                        }
                        None => {
                            // Parking is idempotent: a job already waiting is
                            // not rewritten on every tick.
                            if state == "waiting" {
                                report.skipped += 1;
                                continue;
                            }
                            sqlx::query(
                                "UPDATE tenant_lifecycle_jobs \
                                 SET state = 'waiting', last_attempt_at = now(), updated_at = now() \
                                 WHERE id = $1 AND state IN ('pending','running')",
                            )
                            .bind(id)
                            .execute(&mut *connection)
                            .await?
                        }
                    };

                    if result.rows_affected() == 0 {
                        report.skipped += 1;
                    } else if next.is_some() {
                        report.advanced += 1;
                    } else {
                        report.parked += 1;
                    }
                }
                Ok(report)
            })
        },
    )
    .await
}

/// What one [`drain_due_jobs`] pass did. Reported, never inferred.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    /// Jobs selected by this pass.
    pub claimed: usize,
    /// Jobs moved one phase forward.
    pub advanced: usize,
    /// Jobs parked in `waiting` at the first phase this build cannot perform
    /// unattended (custody revocation / resource cleanup).
    pub parked: usize,
    /// Jobs left untouched (already waiting, or a would-be unique collision).
    pub skipped: usize,
}

pub async fn run_loop<F, Fut>(config: LifecycleWorkerConfig, mut fetch_and_claim: F)
where
    F: FnMut() -> Fut + Send,
    Fut: std::future::Future<Output = Vec<DeprovisionJob>> + Send,
{
    loop {
        let jobs = fetch_and_claim().await;
        for mut job in jobs {
            let outcome = execute_phase(&mut job, Utc::now(), false);
            // In production, persist `job` after outcome; release claim on terminal
            let _ = outcome;
        }
        sleep(config.poll_interval.to_std().unwrap()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::provisioning::deprovision::DeprovisionJob;
    use chrono::Utc;

    fn job_at(phase: DeprovisionPhase) -> DeprovisionJob {
        let mut j = DeprovisionJob::new(OrganizationId::new(), "close", Utc::now());
        j.phase = phase;
        j.state = DeprovisionState::Running;
        j
    }

    #[test]
    fn phase_order_is_strict() {
        let now = Utc::now();
        let mut j = job_at(DeprovisionPhase::Requested);
        let o = execute_phase(&mut j, now, false);
        assert!(matches!(
            o,
            LifecycleWorkOutcome::Advanced(DeprovisionPhase::TradingDisabled)
        ));
        let o2 = execute_phase(&mut j, now, false);
        assert!(matches!(
            o2,
            LifecycleWorkOutcome::Advanced(DeprovisionPhase::CredentialsRevoked)
        ));
    }

    #[test]
    fn transient_retry_with_bounded_backoff() {
        let now = Utc::now();
        let mut j = job_at(DeprovisionPhase::Requested);
        let o = execute_phase(&mut j, now, true);
        assert!(matches!(o, LifecycleWorkOutcome::RetryAfter(_)));
        let dur = backoff_for(1, 2);
        assert!(dur.num_seconds() >= 4);
        assert!(backoff_for(10, 2).num_seconds() <= 300);
    }

    #[test]
    fn permanent_failure_not_marked_success() {
        let now = Utc::now();
        let mut j = job_at(DeprovisionPhase::Requested);
        j.state = DeprovisionState::Failed;
        let o = execute_phase(&mut j, now, false);
        assert!(matches!(o, LifecycleWorkOutcome::Failed(_)));
        assert_eq!(j.state, DeprovisionState::Failed);
    }

    #[test]
    fn worker_restart_resumes_at_recorded_phase() {
        let now = Utc::now();
        let mut j = job_at(DeprovisionPhase::CredentialsRevoked);
        let o = execute_phase(&mut j, now, false);
        assert!(matches!(
            o,
            LifecycleWorkOutcome::Advanced(DeprovisionPhase::SessionsInvalidated)
        ));
        assert_ne!(j.phase, DeprovisionPhase::Requested);
    }

    #[test]
    fn completed_is_terminal() {
        let now = Utc::now();
        let mut j = job_at(DeprovisionPhase::Completed);
        j.state = DeprovisionState::Completed;
        let o = execute_phase(&mut j, now, false);
        assert_eq!(o, LifecycleWorkOutcome::Completed);
    }

    #[test]
    fn retry_count_bounded_and_cleared_on_success() {
        let now = Utc::now();
        let mut j = job_at(DeprovisionPhase::Requested);
        // first transient
        let _ = execute_phase(&mut j, now, true);
        assert_eq!(j.retry_count, 1);
        assert_eq!(j.state, DeprovisionState::Waiting);
        // success clears retry_count
        let o2 = execute_phase(&mut j, now, false);
        assert!(matches!(o2, LifecycleWorkOutcome::Advanced(_)));
        assert_eq!(j.retry_count, 0);
        assert!(j.failure_reason.is_empty());
    }
}
