//! Restart-safe tenant deprovisioning state machine (BATCH file 09).
//!
//! Handles:
//! close requested -> trading disabled -> credentials revoked ->
//! WebSocket/session invalidation -> custody bindings revoked ->
//! background-resource cleanup -> retention state.
//! Preserve accounting/audit evidence according to retention policy.
//! Never destroy immutable financial truth merely because a tenant is closed.
//! Every transition must be idempotent.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::{OrganizationId, OrganizationStatus};

/// Deprovisioning phase — ordered cursor, resumable after crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeprovisionPhase {
    Requested,
    TradingDisabled,
    CredentialsRevoked,
    SessionsInvalidated,
    CustodyRevoked,
    ResourcesCleaned,
    Retention,
    Completed,
    Failed,
}

impl DeprovisionPhase {
    pub const ALL: [DeprovisionPhase; 9] = [
        DeprovisionPhase::Requested,
        DeprovisionPhase::TradingDisabled,
        DeprovisionPhase::CredentialsRevoked,
        DeprovisionPhase::SessionsInvalidated,
        DeprovisionPhase::CustodyRevoked,
        DeprovisionPhase::ResourcesCleaned,
        DeprovisionPhase::Retention,
        DeprovisionPhase::Completed,
        DeprovisionPhase::Failed,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            DeprovisionPhase::Requested => "requested",
            DeprovisionPhase::TradingDisabled => "trading_disabled",
            DeprovisionPhase::CredentialsRevoked => "credentials_revoked",
            DeprovisionPhase::SessionsInvalidated => "sessions_invalidated",
            DeprovisionPhase::CustodyRevoked => "custody_revoked",
            DeprovisionPhase::ResourcesCleaned => "resources_cleaned",
            DeprovisionPhase::Retention => "retention",
            DeprovisionPhase::Completed => "completed",
            DeprovisionPhase::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }

    pub fn next(&self) -> Option<DeprovisionPhase> {
        let idx = Self::ALL.iter().position(|x| x == self)?;
        // Terminal phases have no successor
        match self {
            DeprovisionPhase::Completed | DeprovisionPhase::Failed => None,
            _ => Self::ALL
                .get(idx + 1)
                .copied()
                .filter(|n| !matches!(n, DeprovisionPhase::Failed)),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, DeprovisionPhase::Completed | DeprovisionPhase::Failed)
    }
}

/// Deprovisioning job — durable, restart-safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeprovisionJob {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub requested_action: String, // "close" or "purge" etc.
    pub phase: DeprovisionPhase,
    pub state: DeprovisionState,
    pub retry_count: u32,
    pub max_retries: u32,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub failure_reason: String,
    pub retention_deadline: Option<DateTime<Utc>>,
    pub requested_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeprovisionState {
    Pending,
    Running,
    Waiting,
    Completed,
    Failed,
    Canceled,
}

impl DeprovisionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeprovisionState::Pending => "pending",
            DeprovisionState::Running => "running",
            DeprovisionState::Waiting => "waiting",
            DeprovisionState::Completed => "completed",
            DeprovisionState::Failed => "failed",
            DeprovisionState::Canceled => "canceled",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            DeprovisionState::Completed | DeprovisionState::Failed | DeprovisionState::Canceled
        )
    }
    pub fn is_resumable(&self) -> bool {
        matches!(
            self,
            DeprovisionState::Pending | DeprovisionState::Running | DeprovisionState::Waiting
        )
    }
}

impl DeprovisionJob {
    pub fn new(
        organization_id: OrganizationId,
        requested_action: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id,
            requested_action: requested_action.into(),
            phase: DeprovisionPhase::Requested,
            state: DeprovisionState::Pending,
            retry_count: 0,
            max_retries: 5,
            scheduled_at: now,
            started_at: None,
            completed_at: None,
            last_attempt_at: None,
            next_attempt_at: None,
            failure_reason: String::new(),
            retention_deadline: None,
            requested_by: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn begin(&mut self, now: DateTime<Utc>) -> bool {
        if self.state != DeprovisionState::Pending {
            return false;
        }
        self.state = DeprovisionState::Running;
        self.started_at = Some(now);
        self.updated_at = now;
        true
    }

    /// Advance to next phase idempotently. Returns true if phase changed.
    pub fn advance(&mut self, now: DateTime<Utc>) -> bool {
        if self.phase.is_terminal() {
            return false;
        }
        if let Some(next) = self.phase.next() {
            self.phase = next;
            self.updated_at = now;
            self.last_attempt_at = Some(now);
            if next == DeprovisionPhase::Completed {
                self.state = DeprovisionState::Completed;
                self.completed_at = Some(now);
            } else if next == DeprovisionPhase::Retention {
                // Retention phase is where we calculate deadline; actual purge is separate.
                // keep state Running until explicitly completed
            }
            true
        } else {
            false
        }
    }

    pub fn complete(&mut self, now: DateTime<Utc>) {
        self.phase = DeprovisionPhase::Completed;
        self.state = DeprovisionState::Completed;
        self.completed_at = Some(now);
        self.updated_at = now;
    }

    pub fn fail(&mut self, reason: impl Into<String>, now: DateTime<Utc>) {
        self.state = DeprovisionState::Failed;
        self.phase = DeprovisionPhase::Failed;
        self.failure_reason = reason.into();
        self.updated_at = now;
        self.last_attempt_at = Some(now);
    }

    pub fn record_retry(&mut self, reason: impl Into<String>, now: DateTime<Utc>) {
        self.retry_count += 1;
        self.failure_reason = reason.into();
        self.last_attempt_at = Some(now);
        self.next_attempt_at =
            Some(now + chrono::Duration::seconds(30 * (self.retry_count as i64)));
        self.updated_at = now;
        if self.retry_count >= self.max_retries {
            self.fail(self.failure_reason.clone(), now);
        } else {
            self.state = DeprovisionState::Waiting;
        }
    }

    pub fn is_completed(&self) -> bool {
        self.state == DeprovisionState::Completed && self.phase == DeprovisionPhase::Completed
    }

    /// Idempotent: callers can re-apply same phase without side effects; `advance` only moves forward.
    pub fn ensure_phase(&mut self, target: DeprovisionPhase, now: DateTime<Utc>) -> bool {
        if self.phase == target {
            return false;
        }
        // If target is behind, do nothing (idempotent replay of old phase)
        let current_idx = DeprovisionPhase::ALL
            .iter()
            .position(|x| *x == self.phase)
            .unwrap_or(0);
        let target_idx = DeprovisionPhase::ALL
            .iter()
            .position(|x| *x == target)
            .unwrap_or(0);
        if target_idx <= current_idx {
            return false;
        }
        self.phase = target;
        self.updated_at = now;
        true
    }

    pub fn summary(&self) -> String {
        format!(
            "deprovision={} org={} action={} phase={} state={} retries={}",
            self.id,
            self.organization_id,
            self.requested_action,
            self.phase.as_str(),
            self.state.as_str(),
            self.retry_count
        )
    }
}

/// Pure function: which organization statuses block deprovisioning?
pub fn can_deprovision(status: OrganizationStatus) -> bool {
    // Closed organizations are already deprovisioned; closing again is idempotent but not needed.
    // Suspended/past_due/active/trialing can be closed.
    !matches!(status, OrganizationStatus::Closed)
}

/// What should happen to trading when deprovisioning phase is reached?
pub fn trading_allowed(phase: DeprovisionPhase, status: OrganizationStatus) -> bool {
    if status == OrganizationStatus::Closed {
        return false;
    }
    match phase {
        DeprovisionPhase::Requested => !matches!(
            status,
            OrganizationStatus::Suspended | OrganizationStatus::Closed
        ),
        DeprovisionPhase::TradingDisabled
        | DeprovisionPhase::CredentialsRevoked
        | DeprovisionPhase::SessionsInvalidated
        | DeprovisionPhase::CustodyRevoked
        | DeprovisionPhase::ResourcesCleaned
        | DeprovisionPhase::Retention
        | DeprovisionPhase::Completed => false,
        DeprovisionPhase::Failed => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::OrganizationId;
    use chrono::Utc;

    fn job() -> DeprovisionJob {
        DeprovisionJob::new(OrganizationId::new(), "close", Utc::now())
    }

    #[test]
    fn phase_order_is_strict() {
        let mut j = job();
        assert_eq!(j.phase, DeprovisionPhase::Requested);
        let now = Utc::now();
        j.begin(now);
        assert!(j.advance(now));
        assert_eq!(j.phase, DeprovisionPhase::TradingDisabled);
        // advancing repeatedly reaches completed
        while !j.phase.is_terminal() {
            j.advance(now);
        }
        assert_eq!(j.phase, DeprovisionPhase::Completed);
        assert!(j.is_completed());
        // cannot advance beyond completed
        assert!(!j.advance(now));
    }

    #[test]
    fn idempotent_ensure_phase() {
        let now = Utc::now();
        let mut j = job();
        j.phase = DeprovisionPhase::CredentialsRevoked;
        // replaying same phase is no-op
        assert!(!j.ensure_phase(DeprovisionPhase::CredentialsRevoked, now));
        // moving forward works
        assert!(j.ensure_phase(DeprovisionPhase::Retention, now));
        assert_eq!(j.phase, DeprovisionPhase::Retention);
        // moving backward is idempotent no-op
        assert!(!j.ensure_phase(DeprovisionPhase::Requested, now));
        assert_eq!(j.phase, DeprovisionPhase::Retention);
    }

    #[test]
    fn retry_and_fail_after_max() {
        let now = Utc::now();
        let mut j = job();
        j.max_retries = 2;
        j.record_retry("first", now);
        assert_eq!(j.state, DeprovisionState::Waiting);
        assert_eq!(j.retry_count, 1);
        j.record_retry("second", now);
        assert_eq!(j.state, DeprovisionState::Failed);
        assert_eq!(j.phase, DeprovisionPhase::Failed);
    }

    #[test]
    fn close_is_idempotent() {
        let now = Utc::now();
        let mut j = job();
        j.complete(now);
        assert!(j.is_completed());
        // second complete is still completed (idempotent)
        j.complete(now);
        assert!(j.is_completed());
    }

    #[test]
    fn can_deprovision_closed_is_noop() {
        assert!(!can_deprovision(OrganizationStatus::Closed));
        assert!(can_deprovision(OrganizationStatus::Active));
        assert!(can_deprovision(OrganizationStatus::Suspended));
    }

    #[test]
    fn trading_blocked_after_trading_disabled() {
        assert!(trading_allowed(
            DeprovisionPhase::Requested,
            OrganizationStatus::Active
        ));
        assert!(!trading_allowed(
            DeprovisionPhase::TradingDisabled,
            OrganizationStatus::Active
        ));
        assert!(!trading_allowed(
            DeprovisionPhase::CredentialsRevoked,
            OrganizationStatus::Active
        ));
        assert!(!trading_allowed(
            DeprovisionPhase::Requested,
            OrganizationStatus::Closed
        ));
    }

    #[test]
    fn worker_restart_resumes_state() {
        let now = Utc::now();
        let mut j = job();
        j.begin(now);
        j.advance(now); // TradingDisabled
        j.advance(now); // CredentialsRevoked
        let persisted = j.clone();
        // simulate restart: new worker loads persisted row and continues
        let mut resumed = persisted;
        assert_eq!(resumed.phase, DeprovisionPhase::CredentialsRevoked);
        resumed.advance(now);
        assert_eq!(resumed.phase, DeprovisionPhase::SessionsInvalidated);
    }

    #[test]
    fn summary_is_secret_free() {
        let j = job();
        let s = j.summary();
        assert!(s.contains("close"));
        assert!(!s.to_ascii_lowercase().contains("secret"));
    }
}
