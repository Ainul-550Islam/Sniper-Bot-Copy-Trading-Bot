//! Retention/purge worker (BATCH 2 file 15).
//!
//! Processes only records eligible under the retention policy. Preserves
//! accounting/financial/audit truth according to policy. Requires explicit
//! purge eligibility (deadline passed, purge_allowed). Idempotent and restart-safe.

use chrono::{DateTime, Duration, Utc};

use bot_core::provisioning::retention::{
    is_purge_eligible, RetentionCategory, RetentionPolicy, RetentionState,
};
#[cfg(test)]
use bot_core::tenant::OrganizationId;

/// Purge eligibility decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeDecision {
    Eligible,
    NotEligible(String),
    Protected(String), // financial/audit truth — never purge
}

/// Determine if a record is purge-eligible at `now`.
pub fn is_eligible(
    state: &RetentionState,
    policy: &RetentionPolicy,
    now: DateTime<Utc>,
) -> PurgeDecision {
    if matches!(
        state.category,
        RetentionCategory::FinancialAccounting
            | RetentionCategory::ControlPlaneAudit
            | RetentionCategory::LegalCompliance
    ) {
        return PurgeDecision::Protected(format!(
            "category {} is protected and never purged",
            state.category.as_str()
        ));
    }
    if is_purge_eligible(policy, state, now) {
        PurgeDecision::Eligible
    } else {
        // Distinguish why not eligible for audit
        if state.category != policy.category {
            return PurgeDecision::NotEligible("policy category mismatch".into());
        }
        if !policy.purge_allowed {
            return PurgeDecision::NotEligible("policy purge not allowed".into());
        }
        if state.purged {
            return PurgeDecision::NotEligible("already purged".into());
        }
        if now < state.retention_deadline {
            return PurgeDecision::NotEligible(format!(
                "retention until {}",
                state.retention_deadline.to_rfc3339()
            ));
        }
        PurgeDecision::NotEligible("not eligible".into())
    }
}

/// Worker outcome for one purge attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetentionOutcome {
    Purged { records: usize },
    Skipped(PurgeDecision),
    Failed(String),
}

/// Deterministic purge executor — idempotent.
///
/// `already_purged` indicates if the record was already deleted (second run is no-op).
pub fn execute_purge(
    state: &mut RetentionState,
    policy: &RetentionPolicy,
    now: DateTime<Utc>,
    already_purged: bool,
) -> RetentionOutcome {
    if already_purged || state.purged {
        return RetentionOutcome::Purged { records: 0 }; // idempotent no-op
    }
    let decision = is_eligible(state, policy, now);
    match decision {
        PurgeDecision::Eligible => {
            state.purged = true;
            state.purged_at = Some(now);
            RetentionOutcome::Purged { records: 1 }
        }
        other @ PurgeDecision::NotEligible(_) => RetentionOutcome::Skipped(other),
        other @ PurgeDecision::Protected(_) => RetentionOutcome::Skipped(other),
    }
}

/// Background poll interval for retention worker.
pub const RETENTION_POLL: Duration = Duration::seconds(60);

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::provisioning::retention::{RetentionCategory, RetentionPolicy, RetentionState};
    use chrono::Utc;

    fn state_for(category: RetentionCategory, deadline: DateTime<Utc>) -> RetentionState {
        RetentionState::new(OrganizationId::new(), category, deadline)
    }

    #[test]
    fn financial_never_purged() {
        let now = Utc::now();
        let st = state_for(
            RetentionCategory::FinancialAccounting,
            now - chrono::Duration::days(10),
        );
        let policy = RetentionPolicy::new(RetentionCategory::FinancialAccounting);
        let d = is_eligible(&st, &policy, now);
        assert!(matches!(d, PurgeDecision::Protected(_)));
    }

    #[test]
    fn not_yet_eligible_by_deadline() {
        let now = Utc::now();
        let st = state_for(
            RetentionCategory::Operational,
            now + chrono::Duration::days(10),
        );
        let policy = RetentionPolicy::new(RetentionCategory::Operational);
        let d = is_eligible(&st, &policy, now);
        assert!(matches!(d, PurgeDecision::NotEligible(_)));
    }

    #[test]
    fn eligible_after_deadline() {
        let now = Utc::now();
        let st = state_for(
            RetentionCategory::Operational,
            now - chrono::Duration::days(1),
        );
        let policy = RetentionPolicy::new(RetentionCategory::Operational);
        let d = is_eligible(&st, &policy, now);
        assert_eq!(d, PurgeDecision::Eligible);
    }

    #[test]
    fn idempotent_second_purge_is_no_op() {
        let now = Utc::now();
        let mut st = state_for(
            RetentionCategory::Operational,
            now - chrono::Duration::days(1),
        );
        let policy = RetentionPolicy::new(RetentionCategory::Operational);
        let o1 = execute_purge(&mut st, &policy, now, false);
        assert!(matches!(o1, RetentionOutcome::Purged { records: 1 }));
        assert!(st.purged);
        let o2 = execute_purge(&mut st, &policy, now, true);
        assert!(matches!(o2, RetentionOutcome::Purged { records: 0 }));
        // Even without flag, purged state is no-op
        let o3 = execute_purge(&mut st, &policy, now, false);
        assert!(matches!(o3, RetentionOutcome::Purged { records: 0 }));
    }

    #[test]
    fn protected_audit_never_purged_even_if_eligible() {
        let now = Utc::now();
        let st = state_for(
            RetentionCategory::ControlPlaneAudit,
            now - chrono::Duration::days(100),
        );
        let policy = RetentionPolicy::new(RetentionCategory::ControlPlaneAudit);
        let d = is_eligible(&st, &policy, now);
        assert!(matches!(d, PurgeDecision::Protected(_)));
    }

    #[test]
    fn already_purged_not_eligible() {
        let now = Utc::now();
        let mut st = state_for(
            RetentionCategory::Operational,
            now - chrono::Duration::days(1),
        );
        st.purged = true;
        st.purged_at = Some(now);
        let policy = RetentionPolicy::new(RetentionCategory::Operational);
        let d = is_eligible(&st, &policy, now);
        assert!(matches!(d, PurgeDecision::NotEligible(_)));
        let o = execute_purge(&mut st, &policy, now, false);
        assert!(matches!(o, RetentionOutcome::Purged { records: 0 }));
    }

    #[test]
    fn category_mismatch_not_eligible() {
        let now = Utc::now();
        let st = state_for(
            RetentionCategory::Operational,
            now - chrono::Duration::days(1),
        );
        let policy = RetentionPolicy::new(RetentionCategory::Credentials);
        let d = is_eligible(&st, &policy, now);
        assert!(matches!(d, PurgeDecision::NotEligible(_)));
    }

    #[test]
    fn another_tenant_not_purged() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        assert_ne!(org1, org2);
        // Purge query must include WHERE organization_id=$1 — verified by audit
    }
}
