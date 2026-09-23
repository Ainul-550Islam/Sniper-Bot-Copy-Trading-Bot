//! Customer provisioning — restart-safe onboarding (TASK 7A file 16).
//!
//! Signing up touches six tables. This module makes that sequence
//! crash-safe and idempotent, the same way TASK 6 makes order recovery
//! deterministic: an ordered cursor of completed steps, a deterministic
//! request key so a retry resumes instead of duplicating, and a repository
//! boundary so the durable store can be PostgreSQL or memory.
//!
//! | file | concern |
//! |---|---|
//! | `state.rs` | the PURE state machine: [`ProvisioningState`], [`ProvisioningStep`], [`ProvisioningJob`] |
//!
//! The runner lives in the server (it needs the tenant, membership and
//! billing stores); this module owns the vocabulary, the transitions and
//! the storage contract.

pub mod state;

pub use state::{ProvisioningJob, ProvisioningState, ProvisioningStep};

use async_trait::async_trait;

use crate::error::BotResult;

/// Durable provisioning storage.
#[async_trait]
pub trait ProvisioningStore: Send + Sync {
    /// Insert a job, or return the existing one with the same
    /// `request_key`. This is what makes a repeated signup resume rather
    /// than create a second tenant.
    async fn upsert_job(&self, job: &ProvisioningJob) -> BotResult<ProvisioningJob>;

    /// One job by its deterministic request key.
    async fn job_by_request_key(&self, request_key: &str) -> BotResult<Option<ProvisioningJob>>;

    /// Persist a changed job (step completion, failure, cancellation).
    async fn update_job(&self, job: &ProvisioningJob) -> BotResult<()>;

    /// Jobs a worker may pick up (requested / running / waiting /
    /// retrying), oldest first.
    async fn resumable_jobs(&self, limit: usize) -> BotResult<Vec<ProvisioningJob>>;
}

/// What the runner should do with a job right now. Pure decision, no I/O —
/// the server's runner performs the effect and reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningAction {
    /// Execute this step.
    Execute(ProvisioningStep),
    /// Nothing to do: the job is finished.
    Done,
    /// The job is blocked or failed; an operator must act.
    Halted(ProvisioningState),
}

impl ProvisioningAction {
    /// Stable label for metrics / audit.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProvisioningAction::Execute(_) => "execute",
            ProvisioningAction::Done => "done",
            ProvisioningAction::Halted(_) => "halted",
        }
    }

    /// The step to run, when there is one.
    pub fn step(&self) -> Option<ProvisioningStep> {
        match self {
            ProvisioningAction::Execute(s) => Some(*s),
            _ => None,
        }
    }
}

/// Decide what to do with a job. Deterministic and total: the same job
/// always yields the same action, which is what lets two workers reach the
/// same conclusion without coordinating.
pub fn plan_next(job: &ProvisioningJob) -> ProvisioningAction {
    if job.is_ready() {
        return ProvisioningAction::Done;
    }
    if !job.state.is_resumable() {
        return ProvisioningAction::Halted(job.state);
    }
    match job.next_step() {
        Some(step) => ProvisioningAction::Execute(step),
        None => ProvisioningAction::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn job() -> ProvisioningJob {
        ProvisioningJob::new(
            ProvisioningJob::request_key_for("a@example.com", "n"),
            "starter",
            Utc::now(),
        )
    }

    #[test]
    fn planning_walks_the_sequence_once() {
        let now = Utc::now();
        let mut j = job();
        let mut executed = Vec::new();
        while let ProvisioningAction::Execute(step) = plan_next(&j) {
            executed.push(step);
            j.begin(now);
            assert!(j.complete_step(step, now));
        }
        assert_eq!(plan_next(&j), ProvisioningAction::Done);
        assert_eq!(
            executed.len(),
            6,
            "signup is the starting point, not a step"
        );
        assert_eq!(executed[0], ProvisioningStep::UserCreated);
        assert_eq!(executed[5], ProvisioningStep::Ready);
        assert!(j.is_ready());
    }

    #[test]
    fn two_workers_planning_the_same_job_agree() {
        let now = Utc::now();
        let mut j = job();
        j.begin(now);
        j.complete_step(ProvisioningStep::UserCreated, now);
        let a = plan_next(&j);
        let b = plan_next(&j.clone());
        assert_eq!(a, b);
        assert_eq!(a.step(), Some(ProvisioningStep::OrganizationCreated));
        assert_eq!(a.as_str(), "execute");
    }

    #[test]
    fn halted_jobs_are_not_resumed_silently() {
        let now = Utc::now();
        let mut j = job();
        for _ in 0..ProvisioningJob::MAX_ATTEMPTS {
            j.record_failure("boom", now);
        }
        assert_eq!(
            plan_next(&j),
            ProvisioningAction::Halted(ProvisioningState::Failed)
        );
        let mut c = job();
        c.cancel("customer withdrew", now);
        assert_eq!(
            plan_next(&c),
            ProvisioningAction::Halted(ProvisioningState::Cancelled)
        );
    }
}
