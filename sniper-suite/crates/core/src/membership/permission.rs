//! The permission vocabulary (TASK 7A file 07).
//!
//! ONE closed set of permissions for the whole control plane. A handler may
//! only require a [`Permission`] that exists here; it can never invent a
//! string, because there is no way to build a `Permission` from an unknown
//! one ([`Permission::parse`] returns `None`).
//!
//! Permissions are grouped by the resource they guard and follow
//! `resource.verb`. `read` verbs are safe; `manage` / `start` / `stop` /
//! `create` / `revoke` verbs change state and are therefore never granted
//! to read-only roles.
//!
//! This module is pure data: no I/O, no role logic (that is
//! [`super::role`]), no tenant logic (that is [`crate::tenant::policy`]).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Everything the control plane can gate. Closed vocabulary — bounded
/// metric label and a reviewable list of every capability in the product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Read the organization record and its settings.
    TenantRead,
    /// Change organization settings (name, configuration).
    TenantUpdate,
    /// List members.
    UsersRead,
    /// Invite a new member.
    UsersInvite,
    /// Remove a member.
    UsersRemove,
    /// Read wallet metadata and balances.
    WalletRead,
    /// Add / rotate / remove wallets.
    WalletManage,
    /// Read bot/module status.
    BotRead,
    /// Start a module.
    BotStart,
    /// Stop a module.
    BotStop,
    /// Read orders and executions.
    OrderRead,
    /// Place / cancel orders through the control plane.
    OrderManage,
    /// Read risk configuration and decisions.
    RiskRead,
    /// Change risk limits or engage kill switches.
    RiskManage,
    /// Read the accounting ledger and the portfolio.
    LedgerRead,
    /// Read reconciliation findings.
    ReconciliationRead,
    /// Read plan, subscription, invoices and usage.
    BillingRead,
    /// Change plan / payment method / cancel.
    BillingManage,
    /// Create a tenant API key.
    ApiKeyCreate,
    /// Revoke a tenant API key.
    ApiKeyRevoke,
    /// Read the audit trail.
    AuditRead,
    /// Create a data export.
    ExportCreate,
}

impl Permission {
    /// Every permission, stable order (docs, matrices, tests).
    pub const ALL: [Permission; 22] = [
        Permission::TenantRead,
        Permission::TenantUpdate,
        Permission::UsersRead,
        Permission::UsersInvite,
        Permission::UsersRemove,
        Permission::WalletRead,
        Permission::WalletManage,
        Permission::BotRead,
        Permission::BotStart,
        Permission::BotStop,
        Permission::OrderRead,
        Permission::OrderManage,
        Permission::RiskRead,
        Permission::RiskManage,
        Permission::LedgerRead,
        Permission::ReconciliationRead,
        Permission::BillingRead,
        Permission::BillingManage,
        Permission::ApiKeyCreate,
        Permission::ApiKeyRevoke,
        Permission::AuditRead,
        Permission::ExportCreate,
    ];

    /// The stable wire / storage form (`bot.start`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::TenantRead => "tenant.read",
            Permission::TenantUpdate => "tenant.update",
            Permission::UsersRead => "users.read",
            Permission::UsersInvite => "users.invite",
            Permission::UsersRemove => "users.remove",
            Permission::WalletRead => "wallet.read",
            Permission::WalletManage => "wallet.manage",
            Permission::BotRead => "bot.read",
            Permission::BotStart => "bot.start",
            Permission::BotStop => "bot.stop",
            Permission::OrderRead => "order.read",
            Permission::OrderManage => "order.manage",
            Permission::RiskRead => "risk.read",
            Permission::RiskManage => "risk.manage",
            Permission::LedgerRead => "ledger.read",
            Permission::ReconciliationRead => "reconciliation.read",
            Permission::BillingRead => "billing.read",
            Permission::BillingManage => "billing.manage",
            Permission::ApiKeyCreate => "api_key.create",
            Permission::ApiKeyRevoke => "api_key.revoke",
            Permission::AuditRead => "audit.read",
            Permission::ExportCreate => "export.create",
        }
    }

    /// Inverse of [`Permission::as_str`]. Unknown input is `None`, which is
    /// what stops a handler from inventing its own permission string.
    pub fn parse(s: &str) -> Option<Permission> {
        let s = s.trim();
        Permission::ALL.iter().copied().find(|p| p.as_str() == s)
    }

    /// The resource family (`bot`, `order`, `billing`, …) — used for
    /// grouping in the UI and as a bounded metric label.
    pub fn resource(&self) -> &'static str {
        self.as_str().split('.').next().unwrap_or("unknown")
    }

    /// True when the permission only reads. Read permissions are safe to
    /// grant broadly; everything else changes state.
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Permission::TenantRead
                | Permission::UsersRead
                | Permission::WalletRead
                | Permission::BotRead
                | Permission::OrderRead
                | Permission::RiskRead
                | Permission::LedgerRead
                | Permission::ReconciliationRead
                | Permission::BillingRead
                | Permission::AuditRead
        )
    }

    /// True when exercising this permission can move money or change what
    /// the trading engines will do. These are the permissions that also
    /// have to pass the tenant-status and entitlement gates.
    pub fn is_money_affecting(&self) -> bool {
        matches!(
            self,
            Permission::WalletManage
                | Permission::BotStart
                | Permission::OrderManage
                | Permission::RiskManage
        )
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An immutable, de-duplicated permission set.
///
/// Stored sorted so two sets built in different orders compare and
/// serialise identically (deterministic matching, stable audit lines).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionSet(Vec<Permission>);

impl PermissionSet {
    /// An empty set (grants nothing).
    pub fn empty() -> Self {
        PermissionSet(Vec::new())
    }

    /// Every permission (the full-power set).
    pub fn all() -> Self {
        PermissionSet::from_iter(Permission::ALL)
    }

    /// Does the set grant `p`?
    pub fn contains(&self, p: Permission) -> bool {
        self.0.binary_search(&p).is_ok()
    }

    /// Does the set grant every permission in `required`?
    pub fn contains_all(&self, required: &[Permission]) -> bool {
        required.iter().all(|p| self.contains(*p))
    }

    /// The permissions, sorted.
    pub fn as_slice(&self) -> &[Permission] {
        &self.0
    }

    /// How many permissions are granted.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when nothing is granted.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The intersection — used to narrow a role's permissions with an API
    /// key's scopes. A scope can only ever REMOVE power, never add it.
    pub fn intersect(&self, other: &PermissionSet) -> PermissionSet {
        PermissionSet::from_iter(self.0.iter().copied().filter(|p| other.contains(*p)))
    }

    /// Stable string list (audit, API responses).
    pub fn to_strings(&self) -> Vec<String> {
        self.0.iter().map(|p| p.as_str().to_string()).collect()
    }

    /// Parse a list of strings, ignoring unknown entries (an unknown scope
    /// must never silently become a grant).
    pub fn parse_list<S: AsRef<str>>(items: &[S]) -> PermissionSet {
        PermissionSet::from_iter(items.iter().filter_map(|s| Permission::parse(s.as_ref())))
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = Permission>>(items: I) -> Self {
        let mut permissions: Vec<Permission> = items.into_iter().collect();
        permissions.sort();
        permissions.dedup();
        PermissionSet(permissions)
    }
}

impl IntoIterator for PermissionSet {
    type Item = Permission;
    type IntoIter = std::vec::IntoIter<Permission>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_round_trips_and_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.as_str()), Some(p), "{p}");
            assert!(seen.insert(p.as_str()), "duplicate string for {p}");
            assert!(p.as_str().contains('.'), "{p} must be resource.verb");
        }
        assert_eq!(Permission::ALL.len(), 22);
        assert_eq!(Permission::parse("bot.launch_rockets"), None);
        assert_eq!(Permission::parse(""), None);
        assert_eq!(Permission::parse("  bot.read  "), Some(Permission::BotRead));
    }

    #[test]
    fn classification_is_consistent() {
        for p in Permission::ALL {
            if p.is_read_only() {
                assert!(
                    p.as_str().ends_with(".read"),
                    "{p} claims read-only but is not a .read verb"
                );
                assert!(!p.is_money_affecting(), "{p} cannot be both");
            }
        }
        assert!(Permission::BotStart.is_money_affecting());
        assert!(Permission::OrderManage.is_money_affecting());
        assert!(
            !Permission::BotStop.is_money_affecting(),
            "stopping reduces risk"
        );
        assert_eq!(Permission::ApiKeyCreate.resource(), "api_key");
        assert_eq!(Permission::BotRead.resource(), "bot");
    }

    #[test]
    fn sets_are_deterministic_and_deduplicated() {
        let a = PermissionSet::from_iter([
            Permission::BotRead,
            Permission::TenantRead,
            Permission::BotRead,
        ]);
        let b = PermissionSet::from_iter([Permission::TenantRead, Permission::BotRead]);
        assert_eq!(a, b, "order must not matter");
        assert_eq!(a.len(), 2);
        assert!(a.contains(Permission::BotRead));
        assert!(!a.contains(Permission::BotStart));
        assert!(a.contains_all(&[Permission::BotRead, Permission::TenantRead]));
        assert!(!a.contains_all(&[Permission::BotRead, Permission::BotStart]));
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert_eq!(PermissionSet::all().len(), Permission::ALL.len());
        assert!(PermissionSet::empty().is_empty());
    }

    #[test]
    fn intersection_can_only_remove_power() {
        let role = PermissionSet::from_iter([
            Permission::BotRead,
            Permission::BotStart,
            Permission::OrderRead,
        ]);
        let scopes = PermissionSet::from_iter([
            Permission::BotRead,
            // A scope naming something the role does not have must not grant it.
            Permission::RiskManage,
        ]);
        let effective = role.intersect(&scopes);
        assert!(effective.contains(Permission::BotRead));
        assert!(
            !effective.contains(Permission::RiskManage),
            "scopes cannot add power"
        );
        assert!(!effective.contains(Permission::BotStart));
        assert_eq!(effective.len(), 1);
    }

    #[test]
    fn unknown_scope_strings_are_ignored_not_granted() {
        let set = PermissionSet::parse_list(&["bot.read", "not.a.permission", "order.read"]);
        assert_eq!(set.len(), 2);
        assert!(set.contains(Permission::BotRead));
        assert!(set.contains(Permission::OrderRead));
        assert_eq!(
            set.to_strings(),
            vec!["bot.read".to_string(), "order.read".to_string()]
        );
    }
}
