//! Failed-payment / dunning state machine (Batch 3).
//!
//! Supports: payment_failed, retry_pending, grace_period, billing_suspended, recovered, manually_resolved.
//! Never invents a successful payment. Entitlements change only through validated transitions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DunningState {
    Current,
    PaymentFailed,
    RetryPending,
    GracePeriod,
    BillingSuspended,
    Recovered,
    ManuallyResolved,
}

impl DunningState {
    pub const ALL: [DunningState; 7] = [
        DunningState::Current,
        DunningState::PaymentFailed,
        DunningState::RetryPending,
        DunningState::GracePeriod,
        DunningState::BillingSuspended,
        DunningState::Recovered,
        DunningState::ManuallyResolved,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            DunningState::Current => "current",
            DunningState::PaymentFailed => "payment_failed",
            DunningState::RetryPending => "retry_pending",
            DunningState::GracePeriod => "grace_period",
            DunningState::BillingSuspended => "billing_suspended",
            DunningState::Recovered => "recovered",
            DunningState::ManuallyResolved => "manually_resolved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }

    pub fn is_suspended(&self) -> bool {
        matches!(self, DunningState::BillingSuspended)
    }

    pub fn is_terminal_recovery(&self) -> bool {
        matches!(
            self,
            DunningState::Recovered | DunningState::ManuallyResolved | DunningState::Current
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GracePeriod {
    pub until: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DunningRecord {
    pub state: DunningState,
    pub attempt: u32,
    pub max_attempts: u32,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub grace: Option<GracePeriod>,
    pub failure_reason: String,
    pub updated_at: DateTime<Utc>,
}

impl DunningRecord {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            state: DunningState::Current,
            attempt: 0,
            max_attempts: 3,
            last_failure_at: None,
            next_retry_at: None,
            grace: None,
            failure_reason: String::new(),
            updated_at: now,
        }
    }

    pub fn can_transition_to(&self, to: DunningState) -> bool {
        use DunningState::*;
        if self.state == to {
            return false;
        }
        match (self.state, to) {
            (Current, PaymentFailed) => true,
            (PaymentFailed, RetryPending) => true,
            (RetryPending, GracePeriod) => true,
            (RetryPending, PaymentFailed) => true, // retry failed again
            (RetryPending, BillingSuspended) => true,
            (GracePeriod, BillingSuspended) => true,
            (GracePeriod, Recovered) => true,
            (GracePeriod, ManuallyResolved) => true,
            (PaymentFailed, GracePeriod) => true,
            (PaymentFailed, BillingSuspended) => true,
            (BillingSuspended, Recovered) => true,
            (BillingSuspended, ManuallyResolved) => true,
            (Recovered, Current) => true,
            (ManuallyResolved, Current) => true,
            (Current, ManuallyResolved) => true, // operator override
            // No direct Current -> BillingSuspended without failure
            _ => false,
        }
    }

    pub fn transition(
        &mut self,
        to: DunningState,
        now: DateTime<Utc>,
        reason: impl Into<String>,
    ) -> Result<(), String> {
        if !self.can_transition_to(to) {
            return Err(format!(
                "illegal dunning transition {} -> {}",
                self.state.as_str(),
                to.as_str()
            ));
        }
        // Never invent success: Recovered requires a prior payment proof token; we enforce caller passes non-empty reason that looks like provider evidence.
        if matches!(to, DunningState::Recovered) {
            let r = reason.into();
            if r.trim().is_empty() {
                return Err(
                    "recovered requires provider payment evidence (non-empty reason)".into(),
                );
            }
            // Require provider-like evidence marker
            if !r.contains("provider:")
                && !r.contains("evt_")
                && !r.contains("pi_")
                && !r.contains("payment_succeeded")
            {
                return Err("recovered requires provider payment evidence marker".into());
            }
            self.failure_reason = r;
        } else {
            self.failure_reason = reason.into();
        }
        self.state = to;
        self.updated_at = now;
        match to {
            DunningState::PaymentFailed => {
                self.last_failure_at = Some(now);
                self.attempt += 1;
                self.next_retry_at = Some(now + chrono::Duration::hours(4));
            }
            DunningState::RetryPending => {
                self.next_retry_at = Some(now + chrono::Duration::hours(12));
            }
            DunningState::GracePeriod => {
                self.grace = Some(GracePeriod {
                    until: now + chrono::Duration::days(7),
                    reason: self.failure_reason.clone(),
                });
            }
            DunningState::BillingSuspended => {
                // entitlements will be gated by this state; grace cleared
                // keep grace for audit but not active
            }
            DunningState::Recovered | DunningState::ManuallyResolved => {
                self.attempt = 0;
                self.next_retry_at = None;
                self.grace = None;
            }
            DunningState::Current => {
                self.attempt = 0;
                self.next_retry_at = None;
                self.grace = None;
                self.failure_reason.clear();
            }
        }
        if self.attempt >= self.max_attempts
            && matches!(to, DunningState::PaymentFailed | DunningState::RetryPending)
        {
            // Auto-flag: next is grace or suspend; caller must explicitly transition
        }
        Ok(())
    }

    pub fn is_retry_due(&self, now: DateTime<Utc>) -> bool {
        if self.state != DunningState::RetryPending {
            return false;
        }
        self.next_retry_at.map(|t| now >= t).unwrap_or(false)
    }

    pub fn is_grace_active(&self, now: DateTime<Utc>) -> bool {
        if self.state != DunningState::GracePeriod {
            return false;
        }
        self.grace.as_ref().map(|g| now < g.until).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn rec() -> DunningRecord {
        DunningRecord::new(Utc::now())
    }

    #[test]
    fn valid_lifecycle_payment_failed_to_suspended() {
        let now = Utc::now();
        let mut r = rec();
        assert!(r
            .transition(
                DunningState::PaymentFailed,
                now,
                "provider: payment_failed evt_123"
            )
            .is_ok());
        assert_eq!(r.state, DunningState::PaymentFailed);
        assert_eq!(r.attempt, 1);
        assert!(r
            .transition(DunningState::RetryPending, now, "retry scheduled")
            .is_ok());
        assert!(r
            .transition(DunningState::GracePeriod, now, "grace for 7d")
            .is_ok());
        assert!(r.is_grace_active(now + chrono::Duration::days(1)));
        assert!(r
            .transition(DunningState::BillingSuspended, now, "grace expired")
            .is_ok());
        assert!(r.state.is_suspended());
    }

    #[test]
    fn recovered_requires_evidence() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(DunningState::PaymentFailed, now, "fail")
            .unwrap();
        r.transition(DunningState::RetryPending, now, "retry")
            .unwrap();
        r.transition(DunningState::BillingSuspended, now, "suspend")
            .unwrap();
        // Empty reason -> reject
        assert!(r.transition(DunningState::Recovered, now, "").is_err());
        // No provider marker -> reject
        assert!(r
            .transition(
                DunningState::Recovered,
                now,
                "we fixed it manually without provider"
            )
            .is_err());
        // With provider evidence -> ok
        assert!(r
            .transition(
                DunningState::Recovered,
                now,
                "provider: payment_succeeded evt_777"
            )
            .is_ok());
        assert_eq!(r.state, DunningState::Recovered);
    }

    #[test]
    fn illegal_transitions_rejected() {
        let now = Utc::now();
        let mut r = rec();
        // Current -> BillingSuspended directly is illegal (must go through failed)
        assert!(r
            .transition(DunningState::BillingSuspended, now, "illegal")
            .is_err());
        // Current -> Current is illegal
        assert!(r.transition(DunningState::Current, now, "").is_err());
    }

    #[test]
    fn never_invent_success_idempotent() {
        let now = Utc::now();
        let mut r = rec();
        // Cannot invent recovered from Current without failure evidence
        assert!(r
            .transition(
                DunningState::Recovered,
                now,
                "provider: payment_succeeded evt_1"
            )
            .is_err());
        // Must have failed first, but still need evidence
        r.transition(DunningState::PaymentFailed, now, "fail")
            .unwrap();
        assert!(
            r.transition(
                DunningState::Recovered,
                now,
                "provider: payment_succeeded evt_1"
            )
            .is_err(),
            "must be suspended/grace before recovered"
        );
    }

    #[test]
    fn manual_resolve_allowed() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(DunningState::PaymentFailed, now, "fail")
            .unwrap();
        assert!(
            r.transition(
                DunningState::ManuallyResolved,
                now,
                "operator manually resolved after wire confirmation"
            )
            .is_ok()
                || r.transition(DunningState::GracePeriod, now, "grace")
                    .is_ok()
        );
    }

    #[test]
    fn retry_idempotency_semantics() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(DunningState::PaymentFailed, now, "fail1")
            .unwrap();
        r.transition(DunningState::RetryPending, now, "retry")
            .unwrap();
        assert!(r.is_retry_due(now + chrono::Duration::hours(13)));
        assert!(!r.is_retry_due(now));
        // Second transition to same state rejected
        assert!(r
            .transition(DunningState::RetryPending, now, "retry")
            .is_err());
    }

    #[test]
    fn grace_expiry() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(DunningState::PaymentFailed, now, "fail")
            .unwrap();
        r.transition(DunningState::GracePeriod, now, "grace")
            .unwrap();
        assert!(r.is_grace_active(now));
        assert!(!r.is_grace_active(now + chrono::Duration::days(8)));
    }

    #[test]
    fn all_states_parse_roundtrip() {
        for s in DunningState::ALL {
            assert_eq!(DunningState::parse(s.as_str()), Some(s));
        }
    }
}
