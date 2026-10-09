//! Tenant lifecycle / trading-eligibility state (STEP 3 file 02).
//!
//! This module does NOT invent a second lifecycle vocabulary: the source
//! of truth is [`super::model::OrganizationStatus`] (active / trialing /
//! past_due / suspended / closed) and the pure verdicts in
//! [`super::policy`]. What it adds is the EXECUTION view of that state:
//! one snapshot type that answers, for every action class, "may this
//! tenant act right now?" — with fail-closed helpers the guard chain
//! consumes directly.

use super::model::{Organization, OrganizationStatus};
use super::policy::{self, TenantAction, TenantDenyReason, TenantVerdict};

/// The execution-relevant view of a tenant's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantStateSnapshot {
    status: OrganizationStatus,
}

impl TenantStateSnapshot {
    /// Snapshot a status.
    pub fn of(status: OrganizationStatus) -> Self {
        TenantStateSnapshot { status }
    }

    /// Snapshot an organization row.
    pub fn of_organization(org: &Organization) -> Self {
        TenantStateSnapshot { status: org.status }
    }

    /// The underlying lifecycle status.
    pub fn status(self) -> OrganizationStatus {
        self.status
    }

    /// Verdict for an arbitrary action class.
    pub fn verdict_for(self, action: TenantAction) -> TenantVerdict {
        policy::check_status(self.status, action)
    }

    /// May the tenant READ its own data?
    pub fn may_read(self) -> bool {
        self.verdict_for(TenantAction::Read).is_allowed()
    }

    /// May the tenant change control-plane settings?
    pub fn may_manage(self) -> bool {
        self.verdict_for(TenantAction::Manage).is_allowed()
    }

    /// May the tenant START NEW trading activity (entries, orders)?
    pub fn may_trade(self) -> bool {
        self.verdict_for(TenantAction::Trade).is_allowed()
    }

    /// May the tenant REDUCE exposure (exits, cancels, flatten)?
    pub fn may_reduce_risk(self) -> bool {
        self.verdict_for(TenantAction::ReduceRisk).is_allowed()
    }

    /// Fail-closed execution gate: `Ok(())` when new trading activity is
    /// allowed, otherwise the machine-readable tenant deny reason.
    pub fn require_trade(self) -> Result<(), TenantDenyReason> {
        match self.verdict_for(TenantAction::Trade) {
            TenantVerdict::Allow => Ok(()),
            TenantVerdict::Deny(reason) => Err(reason),
        }
    }

    /// Fail-closed read gate.
    pub fn require_read(self) -> Result<(), TenantDenyReason> {
        match self.verdict_for(TenantAction::Read) {
            TenantVerdict::Allow => Ok(()),
            TenantVerdict::Deny(reason) => Err(reason),
        }
    }

    /// Fail-closed reduce-risk gate (used by the exit/cancel paths).
    pub fn require_reduce_risk(self) -> Result<(), TenantDenyReason> {
        match self.verdict_for(TenantAction::ReduceRisk) {
            TenantVerdict::Allow => Ok(()),
            TenantVerdict::Deny(reason) => Err(reason),
        }
    }
}

impl From<OrganizationStatus> for TenantStateSnapshot {
    fn from(status: OrganizationStatus) -> Self {
        TenantStateSnapshot::of(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_and_trialing_may_trade() {
        assert!(TenantStateSnapshot::of(OrganizationStatus::Active).may_trade());
        assert!(TenantStateSnapshot::of(OrganizationStatus::Trialing).may_trade());
    }

    #[test]
    fn past_due_blocks_new_trading_but_keeps_reads_and_exits() {
        let s = TenantStateSnapshot::of(OrganizationStatus::PastDue);
        assert!(!s.may_trade());
        assert!(s.may_read());
        assert!(s.may_reduce_risk());
        assert_eq!(s.require_trade(), Err(TenantDenyReason::PastDue));
    }

    #[test]
    fn suspended_blocks_trading_and_management() {
        let s = TenantStateSnapshot::of(OrganizationStatus::Suspended);
        assert!(!s.may_trade());
        assert!(!s.may_manage());
        assert!(s.may_reduce_risk());
        assert_eq!(s.require_trade(), Err(TenantDenyReason::Suspended));
    }

    #[test]
    fn closed_is_fail_closed_for_every_action_class() {
        let s = TenantStateSnapshot::of(OrganizationStatus::Closed);
        assert!(!s.may_trade());
        assert!(!s.may_manage());
        assert!(!s.may_read());
        assert_eq!(s.require_read(), Err(TenantDenyReason::Closed));
        assert_eq!(s.require_trade(), Err(TenantDenyReason::Closed));
    }

    #[test]
    fn snapshot_from_organization_row() {
        let now = chrono::Utc::now();
        let mut org = Organization::new(
            crate::tenant::OrganizationId::new(),
            "acme",
            "Acme",
            None,
            now,
        );
        org.status = OrganizationStatus::Suspended;
        assert_eq!(
            TenantStateSnapshot::of_organization(&org).status(),
            OrganizationStatus::Suspended
        );
    }
}
