//! Guard 1/7: organization lifecycle state (STEP 3 file 17).
//!
//! The first gate in the chain: a tenant that is not ACTIVE may not
//! execute anything, in any mode, through any module. The verdict comes
//! from the PURE core policy (`bot_core::tenant::policy`) — this guard
//! adds no rules of its own, it only maps the policy's answer onto the
//! gateway's deny vocabulary.

use bot_core::tenant::{Organization, OrganizationStatus};

use super::decision::{DenyReason, GuardOutcome};

/// Check the organization's lifecycle state.
pub fn check(organization: &Organization) -> GuardOutcome {
    let status = organization.status;
    match status {
        // Active and trialing tenants are fully operational.
        OrganizationStatus::Active | OrganizationStatus::Trialing => {
            GuardOutcome::Allow("tenant_active")
        }
        // Payment failed: read access continues, new trading is refused.
        OrganizationStatus::PastDue => deny("past_due"),
        OrganizationStatus::Suspended => deny("suspended"),
        OrganizationStatus::Closed => deny("closed"),
    }
}

fn deny(state: &'static str) -> GuardOutcome {
    GuardOutcome::Deny(DenyReason::TenantState { state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::{Organization, OrganizationId};
    use chrono::Utc;

    fn org(status: OrganizationStatus) -> Organization {
        let mut organization = Organization::new(
            OrganizationId::new(),
            "test-org",
            "Test Org",
            None,
            Utc::now(),
        );
        organization.status = status;
        organization
    }

    #[test]
    fn only_active_tenants_pass() {
        assert!(check(&org(OrganizationStatus::Active)).is_allow());
    }

    #[test]
    fn trialing_tenants_are_operational_too() {
        assert!(check(&org(OrganizationStatus::Trialing)).is_allow());
    }

    #[test]
    fn every_non_operational_state_denies_with_its_label() {
        for (status, label) in [
            (OrganizationStatus::PastDue, "past_due"),
            (OrganizationStatus::Suspended, "suspended"),
            (OrganizationStatus::Closed, "closed"),
        ] {
            let outcome = check(&org(status));
            let reason = outcome.deny_reason().expect("must deny");
            assert_eq!(reason.as_str(), "tenant_state");
            assert!(
                matches!(reason, DenyReason::TenantState { state } if *state == label),
                "{status:?} must map to {label}"
            );
        }
    }
}
