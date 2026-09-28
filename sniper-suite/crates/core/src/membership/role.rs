//! Canonical SaaS roles and the role → permission matrix (TASK 7A file 06).
//!
//! The pre-TASK-7A roles ([`crate::auth::Role`]: owner / operator /
//! readonly) describe a DEPLOYMENT credential and stay exactly as they are
//! for the existing non-SaaS API keys. They are not enough for a
//! multi-user tenant, where billing, security and audit responsibilities
//! belong to different people.
//!
//! This module defines the eight SaaS roles and the ONE place that says
//! which permissions each of them grants. No handler may re-derive this
//! matrix; it asks [`MembershipRole::permissions`].
//!
//! Bridging to the legacy vocabulary is explicit
//! ([`MembershipRole::from_legacy`] / [`MembershipRole::to_legacy`]) so the
//! existing deployment keys keep working with the existing role gate while
//! tenant credentials use the richer set.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::permission::{Permission, PermissionSet};

/// The canonical SaaS roles, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    /// Platform staff: cross-tenant. The ONLY role that may act outside its
    /// own organization, and only when the user record is also flagged
    /// `platform_admin`.
    PlatformAdmin,
    /// The customer's owner: everything inside the tenant, including
    /// billing and deleting the organization.
    OrgOwner,
    /// Full operational control inside the tenant, minus billing.
    OrgAdmin,
    /// Runs the trading: start/stop modules, manage orders and risk.
    Trader,
    /// Security: API keys, members, audit. No trading.
    SecurityAdmin,
    /// Billing only.
    BillingAdmin,
    /// Read everything, including audit. Changes nothing.
    Auditor,
    /// Read the operational surface. No audit, no billing.
    Viewer,
}

impl MembershipRole {
    /// Every role, strongest first.
    pub const ALL: [MembershipRole; 8] = [
        MembershipRole::PlatformAdmin,
        MembershipRole::OrgOwner,
        MembershipRole::OrgAdmin,
        MembershipRole::Trader,
        MembershipRole::SecurityAdmin,
        MembershipRole::BillingAdmin,
        MembershipRole::Auditor,
        MembershipRole::Viewer,
    ];

    /// Stable label (database `CHECK`, wire form, metrics).
    pub fn as_str(&self) -> &'static str {
        match self {
            MembershipRole::PlatformAdmin => "platform_admin",
            MembershipRole::OrgOwner => "org_owner",
            MembershipRole::OrgAdmin => "org_admin",
            MembershipRole::Trader => "trader",
            MembershipRole::SecurityAdmin => "security_admin",
            MembershipRole::BillingAdmin => "billing_admin",
            MembershipRole::Auditor => "auditor",
            MembershipRole::Viewer => "viewer",
        }
    }

    /// Inverse of [`MembershipRole::as_str`]. Also accepts the SCREAMING
    /// form used in the specification (`ORG_OWNER`).
    pub fn parse(s: &str) -> Option<MembershipRole> {
        let lower = s.trim().to_ascii_lowercase();
        MembershipRole::ALL
            .iter()
            .copied()
            .find(|r| r.as_str() == lower)
    }

    /// The permissions this role grants. The single source of the RBAC
    /// matrix.
    pub fn permissions(&self) -> PermissionSet {
        use Permission as P;
        match self {
            // Platform staff and the organization owner hold everything.
            MembershipRole::PlatformAdmin | MembershipRole::OrgOwner => PermissionSet::all(),
            // Everything operational; billing is the owner's / billing
            // admin's business.
            MembershipRole::OrgAdmin => PermissionSet::from_iter([
                P::TenantRead,
                P::TenantUpdate,
                P::UsersRead,
                P::UsersInvite,
                P::UsersRemove,
                P::WalletRead,
                P::WalletManage,
                P::BotRead,
                P::BotStart,
                P::BotStop,
                P::OrderRead,
                P::OrderManage,
                P::RiskRead,
                P::RiskManage,
                P::LedgerRead,
                P::ReconciliationRead,
                P::BillingRead,
                P::ApiKeyCreate,
                P::ApiKeyRevoke,
                P::AuditRead,
                P::ExportCreate,
            ]),
            // Runs the trading. No members, no keys, no billing.
            MembershipRole::Trader => PermissionSet::from_iter([
                P::TenantRead,
                P::WalletRead,
                P::BotRead,
                P::BotStart,
                P::BotStop,
                P::OrderRead,
                P::OrderManage,
                P::RiskRead,
                P::RiskManage,
                P::LedgerRead,
                P::ReconciliationRead,
                P::ExportCreate,
            ]),
            // Guards access, not money: keys, members, audit — and the
            // ability to STOP a module in an incident (stopping only
            // reduces risk).
            MembershipRole::SecurityAdmin => PermissionSet::from_iter([
                P::TenantRead,
                P::UsersRead,
                P::UsersInvite,
                P::UsersRemove,
                P::WalletRead,
                P::BotRead,
                P::BotStop,
                P::OrderRead,
                P::RiskRead,
                P::LedgerRead,
                P::ReconciliationRead,
                P::ApiKeyCreate,
                P::ApiKeyRevoke,
                P::AuditRead,
                P::ExportCreate,
            ]),
            // Billing only, plus the tenant record it pays for.
            MembershipRole::BillingAdmin => PermissionSet::from_iter([
                P::TenantRead,
                P::BillingRead,
                P::BillingManage,
                P::UsersRead,
            ]),
            // Reads everything, including the audit trail.
            MembershipRole::Auditor => PermissionSet::from_iter([
                P::TenantRead,
                P::UsersRead,
                P::WalletRead,
                P::BotRead,
                P::OrderRead,
                P::RiskRead,
                P::LedgerRead,
                P::ReconciliationRead,
                P::BillingRead,
                P::AuditRead,
                P::ExportCreate,
            ]),
            // The operational read surface.
            MembershipRole::Viewer => PermissionSet::from_iter([
                P::TenantRead,
                P::BotRead,
                P::OrderRead,
                P::RiskRead,
                P::LedgerRead,
                P::ReconciliationRead,
            ]),
        }
    }

    /// Does this role grant `p`?
    pub fn grants(&self, p: Permission) -> bool {
        self.permissions().contains(p)
    }

    /// May this role act on tenants other than its own? Only platform
    /// staff, and the caller must ALSO check `User::platform_admin` — a
    /// membership row alone must never confer cross-tenant power.
    pub fn is_platform_scope(&self) -> bool {
        matches!(self, MembershipRole::PlatformAdmin)
    }

    /// May this role change who has access (members, API keys)?
    pub fn can_administer_access(&self) -> bool {
        self.grants(Permission::UsersInvite) || self.grants(Permission::ApiKeyCreate)
    }

    /// The closest legacy deployment role, for the shared `Role` gate the
    /// existing API already uses. Deliberately conservative: anything that
    /// cannot start trading maps to `Readonly`.
    pub fn to_legacy(&self) -> crate::auth::Role {
        use crate::auth::Role as L;
        match self {
            MembershipRole::PlatformAdmin | MembershipRole::OrgOwner => L::Owner,
            MembershipRole::OrgAdmin | MembershipRole::Trader => L::Operator,
            MembershipRole::SecurityAdmin => L::Operator,
            MembershipRole::BillingAdmin | MembershipRole::Auditor | MembershipRole::Viewer => {
                L::Readonly
            }
        }
    }

    /// The SaaS role a legacy deployment credential corresponds to.
    pub fn from_legacy(role: crate::auth::Role) -> MembershipRole {
        use crate::auth::Role as L;
        match role {
            L::Owner => MembershipRole::OrgOwner,
            L::Operator => MembershipRole::Trader,
            L::Readonly => MembershipRole::Viewer,
        }
    }
}

impl fmt::Display for MembershipRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_round_trip_including_the_screaming_form() {
        for r in MembershipRole::ALL {
            assert_eq!(MembershipRole::parse(r.as_str()), Some(r));
            assert_eq!(
                MembershipRole::parse(&r.as_str().to_ascii_uppercase()),
                Some(r),
                "the spec writes ORG_OWNER"
            );
        }
        assert_eq!(MembershipRole::parse("superuser"), None);
        assert_eq!(MembershipRole::ALL.len(), 8);
    }

    #[test]
    fn owner_and_platform_admin_hold_everything() {
        for r in [MembershipRole::PlatformAdmin, MembershipRole::OrgOwner] {
            assert_eq!(r.permissions().len(), Permission::ALL.len(), "{r}");
        }
        assert!(MembershipRole::PlatformAdmin.is_platform_scope());
        assert!(!MembershipRole::OrgOwner.is_platform_scope());
    }

    #[test]
    fn separation_of_duties_holds() {
        // A trader trades but cannot touch access or billing.
        let t = MembershipRole::Trader;
        assert!(t.grants(Permission::BotStart));
        assert!(t.grants(Permission::OrderManage));
        assert!(t.grants(Permission::RiskManage));
        assert!(!t.grants(Permission::ApiKeyCreate));
        assert!(!t.grants(Permission::UsersInvite));
        assert!(!t.grants(Permission::BillingManage));
        assert!(!t.grants(Permission::AuditRead));

        // Security administers access but never starts trading or moves money.
        let s = MembershipRole::SecurityAdmin;
        assert!(s.grants(Permission::ApiKeyCreate));
        assert!(s.grants(Permission::ApiKeyRevoke));
        assert!(s.grants(Permission::UsersRemove));
        assert!(s.grants(Permission::AuditRead));
        assert!(!s.grants(Permission::BotStart), "security must not trade");
        assert!(!s.grants(Permission::OrderManage));
        assert!(!s.grants(Permission::RiskManage));
        assert!(!s.grants(Permission::WalletManage));
        assert!(
            s.grants(Permission::BotStop),
            "stopping a module in an incident only reduces risk"
        );

        // Billing sees nothing operational.
        let b = MembershipRole::BillingAdmin;
        assert!(b.grants(Permission::BillingManage));
        assert!(!b.grants(Permission::OrderRead));
        assert!(!b.grants(Permission::LedgerRead));
        assert!(!b.grants(Permission::BotRead));

        // Auditor reads widely but writes nothing.
        let a = MembershipRole::Auditor;
        assert!(a.grants(Permission::AuditRead));
        assert!(a.grants(Permission::LedgerRead));
        for p in a.permissions() {
            assert!(
                p.is_read_only() || p == Permission::ExportCreate,
                "auditor must not hold {p}"
            );
        }

        // Viewer is the narrowest.
        let v = MembershipRole::Viewer;
        for p in v.permissions() {
            assert!(p.is_read_only(), "viewer must not hold {p}");
        }
        assert!(
            !v.grants(Permission::AuditRead),
            "audit is not a plain read"
        );
        assert!(!v.grants(Permission::BillingRead));
    }

    #[test]
    fn no_role_except_owner_and_platform_can_do_everything() {
        for r in MembershipRole::ALL {
            if matches!(r, MembershipRole::PlatformAdmin | MembershipRole::OrgOwner) {
                continue;
            }
            assert!(
                r.permissions().len() < Permission::ALL.len(),
                "{r} must not hold every permission"
            );
        }
        // Only owner/platform/org-admin may change the tenant record.
        for r in MembershipRole::ALL {
            let expected = matches!(
                r,
                MembershipRole::PlatformAdmin | MembershipRole::OrgOwner | MembershipRole::OrgAdmin
            );
            assert_eq!(r.grants(Permission::TenantUpdate), expected, "{r}");
        }
        // Only owner/platform may change billing… plus the billing admin.
        for r in MembershipRole::ALL {
            let expected = matches!(
                r,
                MembershipRole::PlatformAdmin
                    | MembershipRole::OrgOwner
                    | MembershipRole::BillingAdmin
            );
            assert_eq!(r.grants(Permission::BillingManage), expected, "{r}");
        }
    }

    #[test]
    fn access_administration_flag_matches_the_matrix() {
        for r in MembershipRole::ALL {
            let expected = r.grants(Permission::UsersInvite) || r.grants(Permission::ApiKeyCreate);
            assert_eq!(r.can_administer_access(), expected, "{r}");
        }
        assert!(MembershipRole::SecurityAdmin.can_administer_access());
        assert!(!MembershipRole::Trader.can_administer_access());
    }

    #[test]
    fn legacy_bridge_is_conservative_and_total() {
        use crate::auth::Role as L;
        assert_eq!(MembershipRole::OrgOwner.to_legacy(), L::Owner);
        assert_eq!(MembershipRole::Trader.to_legacy(), L::Operator);
        assert_eq!(MembershipRole::Viewer.to_legacy(), L::Readonly);
        assert_eq!(MembershipRole::Auditor.to_legacy(), L::Readonly);
        assert_eq!(MembershipRole::BillingAdmin.to_legacy(), L::Readonly);
        for r in MembershipRole::ALL {
            // A role that cannot start a module must never map to an
            // operator-or-stronger legacy credential.
            if !r.grants(Permission::BotStart) && r != MembershipRole::SecurityAdmin {
                assert_eq!(r.to_legacy(), L::Readonly, "{r}");
            }
        }
        assert_eq!(
            MembershipRole::from_legacy(L::Owner),
            MembershipRole::OrgOwner
        );
        assert_eq!(
            MembershipRole::from_legacy(L::Operator),
            MembershipRole::Trader
        );
        assert_eq!(
            MembershipRole::from_legacy(L::Readonly),
            MembershipRole::Viewer
        );
    }
}
