//! Pure tenant policy (TASK 7A file 04).
//!
//! "May this tenant do this kind of thing at all?" — decided from the
//! organization's own lifecycle state and nothing else. No database, no
//! HTTP, no billing provider: the functions here take a status (and, for
//! ownership checks, two ids) and return a deterministic verdict, so the
//! whole matrix is unit-testable and every caller gets the same answer.
//!
//! This module deliberately does NOT know about roles, permissions or
//! entitlements. Those are separate gates:
//!
//! ```text
//! request → tenant policy (here: is the tenant allowed to act?)
//!         → RBAC          (membership role → permission)
//!         → entitlement   (does the plan include the feature?)
//!         → handler
//! ```
//!
//! A tenant that fails here can never be rescued by a role or a plan.

use serde::Serialize;

use super::model::{Organization, OrganizationId, OrganizationStatus};

/// What a caller wants to do with a tenant's resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantAction {
    /// Read tenant-owned data (positions, orders, ledger, audit…).
    Read,
    /// Change control-plane settings (members, keys, configuration).
    Manage,
    /// Start new trading activity (new entries, new orders).
    Trade,
    /// Reduce exposure: exits, cancels, flatten. Always allowed while the
    /// tenant exists — refusing it would trap a customer in a position.
    ReduceRisk,
    /// Billing actions (plan change, payment method, invoices).
    Billing,
}

impl TenantAction {
    /// Every action, stable order.
    pub const ALL: [TenantAction; 5] = [
        TenantAction::Read,
        TenantAction::Manage,
        TenantAction::Trade,
        TenantAction::ReduceRisk,
        TenantAction::Billing,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantAction::Read => "read",
            TenantAction::Manage => "manage",
            TenantAction::Trade => "trade",
            TenantAction::ReduceRisk => "reduce_risk",
            TenantAction::Billing => "billing",
        }
    }

    /// Inverse of [`TenantAction::as_str`].
    pub fn parse(s: &str) -> Option<TenantAction> {
        TenantAction::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == s.trim())
    }
}

impl std::fmt::Display for TenantAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a tenant-level check failed. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantDenyReason {
    /// The organization is suspended.
    Suspended,
    /// The organization is closed (terminal).
    Closed,
    /// Payment failed: reads continue, new trading and management stop.
    PastDue,
    /// The resource belongs to a different tenant.
    CrossTenant,
}

impl TenantDenyReason {
    /// Every reason, stable order.
    pub const ALL: [TenantDenyReason; 4] = [
        TenantDenyReason::Suspended,
        TenantDenyReason::Closed,
        TenantDenyReason::PastDue,
        TenantDenyReason::CrossTenant,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantDenyReason::Suspended => "tenant_suspended",
            TenantDenyReason::Closed => "tenant_closed",
            TenantDenyReason::PastDue => "tenant_past_due",
            TenantDenyReason::CrossTenant => "cross_tenant",
        }
    }

    /// Inverse of [`TenantDenyReason::as_str`].
    pub fn parse(s: &str) -> Option<TenantDenyReason> {
        TenantDenyReason::ALL
            .iter()
            .copied()
            .find(|r| r.as_str() == s.trim())
    }

    /// Human explanation (safe to return to the caller).
    pub fn detail(&self) -> &'static str {
        match self {
            TenantDenyReason::Suspended => {
                "the organization is suspended; contact support to restore access"
            }
            TenantDenyReason::Closed => "the organization is closed",
            TenantDenyReason::PastDue => {
                "the subscription is past due; reading and reducing risk stay available"
            }
            TenantDenyReason::CrossTenant => "the resource belongs to a different organization",
        }
    }
}

impl std::fmt::Display for TenantDenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The verdict of a tenant-level check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantVerdict {
    /// The tenant may proceed to the RBAC / entitlement gates.
    Allow,
    /// Refused at the tenant level; no later gate can override it.
    Deny(TenantDenyReason),
}

impl TenantVerdict {
    /// True on allow.
    pub fn is_allowed(&self) -> bool {
        matches!(self, TenantVerdict::Allow)
    }

    /// The reason on deny.
    pub fn reason(&self) -> Option<TenantDenyReason> {
        match self {
            TenantVerdict::Allow => None,
            TenantVerdict::Deny(r) => Some(*r),
        }
    }
}

/// May a tenant in `status` perform `action`?
///
/// The matrix, deliberately explicit:
///
/// | status | read | manage | trade | reduce risk | billing |
/// |---|---|---|---|---|---|
/// | active / trialing | ✓ | ✓ | ✓ | ✓ | ✓ |
/// | past due | ✓ | ✗ | ✗ | ✓ | ✓ (so the customer can pay) |
/// | suspended | ✓ | ✗ | ✗ | ✓ | ✓ |
/// | closed | ✗ | ✗ | ✗ | ✗ | ✗ |
///
/// Two rules are load-bearing: a non-closed tenant may ALWAYS reduce risk
/// (never trap a customer in a position), and a past-due or suspended
/// tenant may always reach billing (so it can fix the cause).
pub fn check_status(status: OrganizationStatus, action: TenantAction) -> TenantVerdict {
    use OrganizationStatus as S;
    use TenantAction as A;
    match status {
        S::Active | S::Trialing => TenantVerdict::Allow,
        S::PastDue => match action {
            A::Read | A::ReduceRisk | A::Billing => TenantVerdict::Allow,
            A::Manage | A::Trade => TenantVerdict::Deny(TenantDenyReason::PastDue),
        },
        S::Suspended => match action {
            A::Read | A::ReduceRisk | A::Billing => TenantVerdict::Allow,
            A::Manage | A::Trade => TenantVerdict::Deny(TenantDenyReason::Suspended),
        },
        S::Closed => TenantVerdict::Deny(TenantDenyReason::Closed),
    }
}

/// [`check_status`] for a loaded organization.
pub fn check(org: &Organization, action: TenantAction) -> TenantVerdict {
    check_status(org.status, action)
}

/// Ownership: the resource's tenant must be the acting tenant.
///
/// This is the rule that makes two organizations with identical-looking
/// resource ids unable to read each other: the comparison is on the tenant
/// id, never on the resource id.
pub fn check_ownership(acting: OrganizationId, resource_owner: OrganizationId) -> TenantVerdict {
    if acting == resource_owner {
        TenantVerdict::Allow
    } else {
        TenantVerdict::Deny(TenantDenyReason::CrossTenant)
    }
}

/// Ownership plus status in one call: the resource must belong to the
/// acting tenant AND the tenant must be allowed to perform the action.
pub fn check_resource(
    org: &Organization,
    resource_owner: OrganizationId,
    action: TenantAction,
) -> TenantVerdict {
    match check_ownership(org.id, resource_owner) {
        TenantVerdict::Allow => check(org, action),
        deny => deny,
    }
}

/// May this tenant still authenticate (log in, use an API key)? A closed
/// organization cannot; everything else can, and the action-level checks
/// above decide what it may then do.
pub fn can_authenticate(status: OrganizationStatus) -> bool {
    !matches!(status, OrganizationStatus::Closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::model::{Organization, OrganizationId, UserId};
    use chrono::Utc;

    fn org(status: OrganizationStatus) -> Organization {
        let mut o = Organization::new(
            OrganizationId::new(),
            "acme",
            "Acme",
            Some(UserId::new()),
            Utc::now(),
        );
        o.status = status;
        o
    }

    #[test]
    fn active_and_trialing_tenants_may_do_everything() {
        for s in [OrganizationStatus::Active, OrganizationStatus::Trialing] {
            for a in TenantAction::ALL {
                assert!(check_status(s, a).is_allowed(), "{s} / {a}");
            }
        }
    }

    #[test]
    fn suspended_tenant_cannot_trade_or_manage_but_can_read_and_reduce() {
        let s = OrganizationStatus::Suspended;
        assert_eq!(
            check_status(s, TenantAction::Trade).reason(),
            Some(TenantDenyReason::Suspended)
        );
        assert_eq!(
            check_status(s, TenantAction::Manage).reason(),
            Some(TenantDenyReason::Suspended)
        );
        assert!(check_status(s, TenantAction::Read).is_allowed());
        assert!(
            check_status(s, TenantAction::ReduceRisk).is_allowed(),
            "a customer must always be able to exit a position"
        );
        assert!(
            check_status(s, TenantAction::Billing).is_allowed(),
            "a suspended customer must be able to reach billing"
        );
    }

    #[test]
    fn past_due_mirrors_suspension_for_new_activity_only() {
        let s = OrganizationStatus::PastDue;
        assert_eq!(
            check_status(s, TenantAction::Trade).reason(),
            Some(TenantDenyReason::PastDue)
        );
        assert!(check_status(s, TenantAction::Read).is_allowed());
        assert!(check_status(s, TenantAction::ReduceRisk).is_allowed());
        assert!(check_status(s, TenantAction::Billing).is_allowed());
    }

    #[test]
    fn closed_tenant_is_denied_everything_including_reads() {
        for a in TenantAction::ALL {
            assert_eq!(
                check_status(OrganizationStatus::Closed, a).reason(),
                Some(TenantDenyReason::Closed),
                "{a}"
            );
        }
        assert!(!can_authenticate(OrganizationStatus::Closed));
        assert!(can_authenticate(OrganizationStatus::Suspended));
    }

    #[test]
    fn ownership_compares_tenants_never_resource_ids() {
        let a = org(OrganizationStatus::Active);
        let b = org(OrganizationStatus::Active);
        assert!(check_ownership(a.id, a.id).is_allowed());
        assert_eq!(
            check_ownership(a.id, b.id).reason(),
            Some(TenantDenyReason::CrossTenant)
        );
        // Cross-tenant beats everything, even for a healthy tenant and a
        // harmless action.
        assert_eq!(
            check_resource(&a, b.id, TenantAction::Read).reason(),
            Some(TenantDenyReason::CrossTenant)
        );
        assert!(check_resource(&a, a.id, TenantAction::Read).is_allowed());
        // Own resource, but the tenant is suspended: the status rule applies.
        let s = {
            let mut o = a.clone();
            o.status = OrganizationStatus::Suspended;
            o
        };
        assert_eq!(
            check_resource(&s, s.id, TenantAction::Trade).reason(),
            Some(TenantDenyReason::Suspended)
        );
    }

    #[test]
    fn vocabularies_round_trip_and_carry_detail() {
        for a in TenantAction::ALL {
            assert_eq!(TenantAction::parse(a.as_str()), Some(a));
        }
        for r in TenantDenyReason::ALL {
            assert_eq!(TenantDenyReason::parse(r.as_str()), Some(r));
            assert!(!r.detail().is_empty());
        }
        assert_eq!(TenantAction::parse("nope"), None);
        assert_eq!(TenantDenyReason::parse("nope"), None);
    }
}
