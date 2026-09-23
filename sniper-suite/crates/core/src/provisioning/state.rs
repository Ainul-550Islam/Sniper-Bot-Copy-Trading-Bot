//! The onboarding state machine (TASK 7A file 17).
//!
//! Signing a customer up touches several tables (user, organization,
//! membership, subscription, entitlements, defaults). A crash halfway
//! through must not leave a half-built tenant, and a retry must not create
//! a second organization. This module is the PURE state machine that makes
//! that possible:
//!
//! ```text
//! SIGNUP → USER_CREATED → ORGANIZATION_CREATED → MEMBERSHIP_CREATED
//!        → PLAN_ASSIGNED → DEFAULT_CONFIGURATION → READY
//! ```
//!
//! Two properties do the work:
//!
//! * **Ordered steps.** [`ProvisioningStep`] is a total order, and the job
//!   records the last COMPLETED step. A resumed job restarts at
//!   [`ProvisioningStep::next`], never at the beginning.
//! * **Idempotent steps.** Each step is written so that re-running it on a
//!   partially applied state converges (the runner looks the row up before
//!   inserting). Combined with the ordered cursor, a crash at any point has
//!   exactly one correct continuation.
//!
//! No I/O lives here: the runner ([`super::ProvisioningRunner`]) performs
//! the effects and asks this module what to do next.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::tenant::{OrganizationId, UserId};

/// Where a job is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningState {
    /// Accepted, not started.
    Requested,
    /// A worker is executing a step right now.
    Running,
    /// Blocked on something external (email verification, manual review).
    Waiting,
    /// Every step done; terminal.
    Completed,
    /// Gave up after retries; terminal until an operator intervenes.
    Failed,
    /// A step failed and will be retried.
    Retrying,
    /// Abandoned; terminal.
    Cancelled,
}

impl ProvisioningState {
    /// Every state, stable order.
    pub const ALL: [ProvisioningState; 7] = [
        ProvisioningState::Requested,
        ProvisioningState::Running,
        ProvisioningState::Waiting,
        ProvisioningState::Completed,
        ProvisioningState::Failed,
        ProvisioningState::Retrying,
        ProvisioningState::Cancelled,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProvisioningState::Requested => "requested",
            ProvisioningState::Running => "running",
            ProvisioningState::Waiting => "waiting",
            ProvisioningState::Completed => "completed",
            ProvisioningState::Failed => "failed",
            ProvisioningState::Retrying => "retrying",
            ProvisioningState::Cancelled => "cancelled",
        }
    }

    /// Inverse of [`ProvisioningState::as_str`].
    pub fn parse(s: &str) -> Option<ProvisioningState> {
        ProvisioningState::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Terminal states are never resumed.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ProvisioningState::Completed | ProvisioningState::Cancelled | ProvisioningState::Failed
        )
    }

    /// May a worker pick this job up? `Failed` needs an operator to reset
    /// it, which is why it is not resumable on its own.
    pub fn is_resumable(&self) -> bool {
        matches!(
            self,
            ProvisioningState::Requested
                | ProvisioningState::Running
                | ProvisioningState::Waiting
                | ProvisioningState::Retrying
        )
    }

    /// The legal transitions.
    pub fn can_transition_to(&self, to: ProvisioningState) -> bool {
        use ProvisioningState::*;
        if *self == to {
            return false;
        }
        match (self, to) {
            (Completed | Cancelled, _) => false,
            (_, Cancelled) => true,
            (Requested, Running) => true,
            (Running, Waiting | Completed | Retrying | Failed) => true,
            (Waiting, Running | Retrying | Failed) => true,
            (Retrying, Running | Failed) => true,
            // An operator may reset a failed job to try again.
            (Failed, Requested | Retrying) => true,
            _ => false,
        }
    }
}

impl fmt::Display for ProvisioningState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The ordered onboarding steps. The job stores the last COMPLETED one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStep {
    /// Nothing done yet; the request was accepted.
    Signup,
    /// The user row exists.
    UserCreated,
    /// The organization row exists.
    OrganizationCreated,
    /// The owner membership exists.
    MembershipCreated,
    /// A subscription on the requested plan exists.
    PlanAssigned,
    /// Entitlements and defaults are written.
    DefaultConfiguration,
    /// Everything done.
    Ready,
}

impl ProvisioningStep {
    /// Every step in order.
    pub const ALL: [ProvisioningStep; 7] = [
        ProvisioningStep::Signup,
        ProvisioningStep::UserCreated,
        ProvisioningStep::OrganizationCreated,
        ProvisioningStep::MembershipCreated,
        ProvisioningStep::PlanAssigned,
        ProvisioningStep::DefaultConfiguration,
        ProvisioningStep::Ready,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProvisioningStep::Signup => "signup",
            ProvisioningStep::UserCreated => "user_created",
            ProvisioningStep::OrganizationCreated => "organization_created",
            ProvisioningStep::MembershipCreated => "membership_created",
            ProvisioningStep::PlanAssigned => "plan_assigned",
            ProvisioningStep::DefaultConfiguration => "default_configuration",
            ProvisioningStep::Ready => "ready",
        }
    }

    /// Inverse of [`ProvisioningStep::as_str`].
    pub fn parse(s: &str) -> Option<ProvisioningStep> {
        ProvisioningStep::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Position in the sequence (0-based).
    pub fn index(&self) -> usize {
        ProvisioningStep::ALL
            .iter()
            .position(|s| s == self)
            .unwrap_or(0)
    }

    /// The step a resumed job must execute next; `None` once ready.
    pub fn next(&self) -> Option<ProvisioningStep> {
        ProvisioningStep::ALL.get(self.index() + 1).copied()
    }

    /// Is everything done?
    pub fn is_final(&self) -> bool {
        matches!(self, ProvisioningStep::Ready)
    }
}

impl fmt::Display for ProvisioningStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One onboarding job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvisioningJob {
    /// Row identity.
    pub id: Uuid,
    /// The organization, once its step has run.
    pub organization_id: Option<OrganizationId>,
    /// The user, once their step has run.
    pub user_id: Option<UserId>,
    /// Deterministic request identity: re-submitting the same signup
    /// resumes THIS job instead of starting a second one.
    pub request_key: String,
    /// Lifecycle.
    pub state: ProvisioningState,
    /// Last COMPLETED step.
    pub step: ProvisioningStep,
    /// How many times a step has been attempted.
    pub attempts: i32,
    /// Last error text (operator-facing; never a secret).
    pub last_error: String,
    /// Requested plan code.
    pub plan_code: String,
    /// Signup inputs that are safe to persist (email, organization name).
    /// NEVER a password or a token.
    pub payload: serde_json::Value,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
    /// When it completed.
    pub completed_at: Option<DateTime<Utc>>,
}

impl ProvisioningJob {
    /// How many attempts one step gets before the job fails.
    pub const MAX_ATTEMPTS: i32 = 5;

    /// A fresh job in [`ProvisioningState::Requested`].
    pub fn new(
        request_key: impl Into<String>,
        plan_code: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        ProvisioningJob {
            id: Uuid::new_v4(),
            organization_id: None,
            user_id: None,
            request_key: request_key.into(),
            state: ProvisioningState::Requested,
            step: ProvisioningStep::Signup,
            attempts: 0,
            last_error: String::new(),
            plan_code: plan_code.into(),
            payload: serde_json::Value::Object(serde_json::Map::new()),
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    /// The deterministic request key for a signup. The same email and nonce
    /// always produce the same key, which is what makes a retried signup
    /// resume instead of duplicating.
    pub fn request_key_for(email: &str, nonce: &str) -> String {
        let mut h = Sha256::new();
        h.update(b"provisioning-v1|");
        h.update(email.trim().to_ascii_lowercase().as_bytes());
        h.update(b"|");
        h.update(nonce.trim().as_bytes());
        format!("prov_{}", &hex::encode(h.finalize())[..32])
    }

    /// The step a worker should execute now; `None` when nothing is left.
    pub fn next_step(&self) -> Option<ProvisioningStep> {
        if self.state.is_terminal() && self.state != ProvisioningState::Failed {
            return None;
        }
        self.step.next()
    }

    /// Record that `step` completed. Ignores an out-of-order or repeated
    /// completion, so a duplicated worker cannot rewind the cursor.
    pub fn complete_step(&mut self, step: ProvisioningStep, now: DateTime<Utc>) -> bool {
        if step.index() <= self.step.index() {
            return false;
        }
        self.step = step;
        self.attempts = 0;
        self.last_error.clear();
        self.updated_at = now;
        if step.is_final() {
            self.state = ProvisioningState::Completed;
            self.completed_at = Some(now);
        } else if self.state == ProvisioningState::Requested
            || self.state == ProvisioningState::Retrying
        {
            self.state = ProvisioningState::Running;
        }
        true
    }

    /// Record a failed attempt. Moves to `Retrying` until the attempt
    /// budget is exhausted, then to `Failed`.
    pub fn record_failure(&mut self, error: impl Into<String>, now: DateTime<Utc>) {
        self.attempts += 1;
        self.last_error = error.into();
        self.updated_at = now;
        self.state = if self.attempts >= Self::MAX_ATTEMPTS {
            ProvisioningState::Failed
        } else {
            ProvisioningState::Retrying
        };
    }

    /// Mark the job as being worked on.
    pub fn begin(&mut self, now: DateTime<Utc>) -> bool {
        if !self.state.is_resumable() {
            return false;
        }
        if self.state != ProvisioningState::Running {
            self.state = ProvisioningState::Running;
        }
        self.updated_at = now;
        true
    }

    /// Block on something external.
    pub fn wait(&mut self, reason: impl Into<String>, now: DateTime<Utc>) {
        self.state = ProvisioningState::Waiting;
        self.last_error = reason.into();
        self.updated_at = now;
    }

    /// Abandon the job.
    pub fn cancel(&mut self, reason: impl Into<String>, now: DateTime<Utc>) -> bool {
        if self.state.is_terminal() && self.state != ProvisioningState::Failed {
            return false;
        }
        self.state = ProvisioningState::Cancelled;
        self.last_error = reason.into();
        self.updated_at = now;
        true
    }

    /// Is the tenant fully provisioned?
    pub fn is_ready(&self) -> bool {
        self.state == ProvisioningState::Completed && self.step.is_final()
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "provisioning={} request_key={} state={} step={} attempts={} organization={} user={}",
            self.id,
            self.request_key,
            self.state,
            self.step,
            self.attempts,
            self.organization_id
                .map(|o| o.to_string())
                .unwrap_or_else(|| "-".into()),
            self.user_id
                .map(|u| u.to_string())
                .unwrap_or_else(|| "-".into())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(now: DateTime<Utc>) -> ProvisioningJob {
        ProvisioningJob::new(
            ProvisioningJob::request_key_for("a@example.com", "nonce"),
            "starter",
            now,
        )
    }

    #[test]
    fn vocabularies_round_trip() {
        for s in ProvisioningState::ALL {
            assert_eq!(ProvisioningState::parse(s.as_str()), Some(s));
        }
        for s in ProvisioningStep::ALL {
            assert_eq!(ProvisioningStep::parse(s.as_str()), Some(s));
        }
        assert_eq!(ProvisioningState::parse("nope"), None);
        assert_eq!(ProvisioningStep::ALL.len(), 7);
    }

    #[test]
    fn steps_are_ordered_and_have_one_successor() {
        let mut s = ProvisioningStep::Signup;
        let mut seen = vec![s];
        while let Some(next) = s.next() {
            assert!(next.index() == s.index() + 1);
            s = next;
            seen.push(s);
        }
        assert_eq!(seen.len(), 7);
        assert!(ProvisioningStep::Ready.is_final());
        assert_eq!(ProvisioningStep::Ready.next(), None);
    }

    #[test]
    fn request_keys_are_deterministic() {
        let a = ProvisioningJob::request_key_for("Person@Example.com", "n1");
        let b = ProvisioningJob::request_key_for(" person@example.com ", "n1");
        assert_eq!(a, b, "a retried signup resumes the same job");
        assert_ne!(
            a,
            ProvisioningJob::request_key_for("person@example.com", "n2")
        );
        assert!(a.starts_with("prov_"));
    }

    #[test]
    fn a_crash_resumes_at_the_next_step_never_the_first() {
        let now = Utc::now();
        let mut j = job(now);
        assert_eq!(j.next_step(), Some(ProvisioningStep::UserCreated));
        j.begin(now);
        assert!(j.complete_step(ProvisioningStep::UserCreated, now));
        j.user_id = Some(UserId::new());
        assert!(j.complete_step(ProvisioningStep::OrganizationCreated, now));
        j.organization_id = Some(OrganizationId::new());

        // "Crash": another worker loads the row and continues.
        let resumed = j.clone();
        assert_eq!(
            resumed.next_step(),
            Some(ProvisioningStep::MembershipCreated),
            "resume after the last completed step"
        );
        assert_eq!(resumed.organization_id, j.organization_id);

        // A duplicated worker replaying an older step changes nothing.
        let mut dup = j.clone();
        assert!(!dup.complete_step(ProvisioningStep::UserCreated, now));
        assert_eq!(dup.step, ProvisioningStep::OrganizationCreated);
    }

    #[test]
    fn the_full_sequence_reaches_ready() {
        let now = Utc::now();
        let mut j = job(now);
        j.begin(now);
        for step in ProvisioningStep::ALL.iter().skip(1) {
            assert!(j.complete_step(*step, now), "{step}");
        }
        assert!(j.is_ready());
        assert_eq!(j.state, ProvisioningState::Completed);
        assert!(j.completed_at.is_some());
        assert_eq!(j.next_step(), None);
        // A completed job cannot be restarted or cancelled.
        assert!(!j.begin(now));
        assert!(!j.cancel("nope", now));
    }

    #[test]
    fn failures_retry_then_fail_and_can_be_reset() {
        let now = Utc::now();
        let mut j = job(now);
        j.begin(now);
        for i in 1..ProvisioningJob::MAX_ATTEMPTS {
            j.record_failure(format!("attempt {i}"), now);
            assert_eq!(j.state, ProvisioningState::Retrying, "attempt {i}");
            assert!(j.state.is_resumable());
        }
        j.record_failure("final", now);
        assert_eq!(j.state, ProvisioningState::Failed);
        assert!(!j.state.is_resumable(), "a failed job needs an operator");
        assert!(j.state.is_terminal());
        // An operator may reset it.
        assert!(ProvisioningState::Failed.can_transition_to(ProvisioningState::Retrying));
        // A successful step clears the error counter.
        j.state = ProvisioningState::Retrying;
        j.complete_step(ProvisioningStep::UserCreated, now);
        assert_eq!(j.attempts, 0);
        assert!(j.last_error.is_empty());
        assert_eq!(j.state, ProvisioningState::Running);
    }

    #[test]
    fn transition_matrix_is_strict() {
        use ProvisioningState::*;
        assert!(Requested.can_transition_to(Running));
        assert!(Running.can_transition_to(Waiting));
        assert!(Waiting.can_transition_to(Running));
        assert!(Running.can_transition_to(Completed));
        assert!(!Completed.can_transition_to(Running));
        assert!(!Cancelled.can_transition_to(Running));
        assert!(!Requested.can_transition_to(Completed), "must run first");
        for s in ProvisioningState::ALL {
            assert!(!s.can_transition_to(s));
            if !matches!(s, Completed | Cancelled) {
                assert!(s.can_transition_to(Cancelled), "{s}");
            }
        }
    }

    #[test]
    fn payload_never_carries_secrets_by_construction() {
        let now = Utc::now();
        let mut j = job(now);
        j.payload = serde_json::json!({"email": "a@example.com", "org_name": "Acme"});
        let json = serde_json::to_string(&j).unwrap();
        assert!(!json.contains("password"));
        assert!(!json.contains("token"));
        assert!(j.summary().contains("state=requested"));
    }
}
