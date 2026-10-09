//! The authorization context (TASK 7A file 19).
//!
//! One value that carries everything a decision needs: who is calling,
//! which tenant they are acting for, what that membership grants, and (for
//! API keys) how the credential narrowed it. Every SaaS handler receives
//! this instead of re-deriving the pieces, which is what keeps the
//! isolation rules in one place.
//!
//! ```text
//! AuthorizationContext
//!  ├── principal   : who (user session | tenant API key | platform admin)
//!  ├── user_id
//!  ├── organization_id   ← the tenant everything resolves to
//!  ├── membership role
//!  └── permissions  = role permissions ∩ credential scopes
//! ```
//!
//! The context is built by the middleware after authentication; it cannot
//! be constructed with a tenant the caller has no membership in, because
//! the constructor takes the membership itself.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::membership::{Membership, MembershipRole, Permission, PermissionSet};
use crate::tenant::{Organization, OrganizationId, OrganizationStatus, UserId};

/// What kind of credential authenticated the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Principal {
    /// A human with a browser session.
    UserSession {
        /// The session row.
        session_id: String,
    },
    /// A tenant-scoped API key.
    ApiKey {
        /// The key row id.
        key_id: String,
        /// The public, non-secret prefix (safe to log).
        key_prefix: String,
    },
    /// The pre-TASK-7A deployment credential, mapped into the SaaS model
    /// for the single-tenant deployment. Kept so existing operators keep
    /// working unchanged.
    LegacyDeploymentKey {
        /// The key label from the configuration.
        label: String,
    },
}

impl Principal {
    /// Stable label for metrics and audit.
    pub fn kind(&self) -> &'static str {
        match self {
            Principal::UserSession { .. } => "user_session",
            Principal::ApiKey { .. } => "api_key",
            Principal::LegacyDeploymentKey { .. } => "legacy_key",
        }
    }

    /// A short, non-secret identifier for audit lines.
    pub fn identifier(&self) -> String {
        match self {
            Principal::UserSession { session_id } => format!("session:{session_id}"),
            Principal::ApiKey { key_prefix, .. } => format!("key:{key_prefix}"),
            Principal::LegacyDeploymentKey { label } => format!("legacy:{label}"),
        }
    }
}

/// Everything one authorization decision needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuthorizationContext {
    /// Which credential authenticated.
    pub principal: Principal,
    /// The human behind it (`None` for a machine key with no creator).
    pub user_id: Option<UserId>,
    /// THE tenant. Every resource check compares against this.
    pub organization_id: OrganizationId,
    /// The tenant's lifecycle state at request time.
    pub organization_status: OrganizationStatus,
    /// The role the membership grants.
    pub role: MembershipRole,
    /// The EFFECTIVE permissions: the role's set, intersected with the
    /// credential's scopes. Scopes can only remove power.
    pub permissions: PermissionSet,
    /// Whether the user record is a platform administrator. Required (in
    /// addition to the role) for any cross-tenant action.
    pub platform_admin: bool,
    /// When the context was built.
    pub issued_at: DateTime<Utc>,
}

impl AuthorizationContext {
    /// Build from a membership and its organization.
    ///
    /// `scopes = None` means "the role's full set"; `Some(set)` narrows it.
    /// A suspended or removed membership yields an empty permission set
    /// (see [`Membership::permissions`]), so it cannot be used to act.
    pub fn from_membership(
        principal: Principal,
        organization: &Organization,
        membership: &Membership,
        scopes: Option<&PermissionSet>,
        platform_admin: bool,
        now: DateTime<Utc>,
    ) -> Self {
        let granted = membership.permissions();
        let permissions = match scopes {
            Some(s) => granted.intersect(s),
            None => granted,
        };
        AuthorizationContext {
            principal,
            user_id: Some(membership.user_id),
            organization_id: organization.id,
            organization_status: organization.status,
            role: membership.role,
            permissions,
            platform_admin,
            issued_at: now,
        }
    }

    /// Build a genuine human platform-administrator context for a target
    /// organization. The caller must verify the persisted user flag before
    /// using this constructor; unlike an API key, this context may cross
    /// tenant boundaries.
    pub fn from_platform_admin(
        principal: Principal,
        organization: &Organization,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> Self {
        AuthorizationContext {
            principal,
            user_id: Some(user_id),
            organization_id: organization.id,
            organization_status: organization.status,
            role: MembershipRole::PlatformAdmin,
            permissions: MembershipRole::PlatformAdmin.permissions(),
            platform_admin: true,
            issued_at: now,
        }
    }

    /// Build for a machine credential that has no membership row: the
    /// tenant API key carries its own role, which the key's owner chose.
    pub fn from_api_key(
        principal: Principal,
        organization: &Organization,
        role: MembershipRole,
        scopes: Option<&PermissionSet>,
        created_by: Option<UserId>,
        now: DateTime<Utc>,
    ) -> Self {
        let granted = role.permissions();
        let permissions = match scopes {
            Some(s) => granted.intersect(s),
            None => granted,
        };
        AuthorizationContext {
            principal,
            user_id: created_by,
            organization_id: organization.id,
            organization_status: organization.status,
            role,
            permissions,
            // A key never confers platform scope, even if its role says so:
            // cross-tenant power requires a human platform admin.
            platform_admin: false,
            issued_at: now,
        }
    }

    /// Does the caller hold `p`?
    pub fn has(&self, p: Permission) -> bool {
        self.permissions.contains(p)
    }

    /// Does the caller hold every permission in `required`?
    pub fn has_all(&self, required: &[Permission]) -> bool {
        self.permissions.contains_all(required)
    }

    /// May this caller act on `resource_owner`?
    ///
    /// The tenant must match — unless the caller is a genuine platform
    /// administrator (role AND user flag), which is how support reaches a
    /// customer's data with an audit trail.
    pub fn owns(&self, resource_owner: OrganizationId) -> bool {
        self.organization_id == resource_owner || self.is_platform_scope()
    }

    /// True only when BOTH the role and the user record say platform admin.
    pub fn is_platform_scope(&self) -> bool {
        self.platform_admin && self.role.is_platform_scope()
    }

    /// Single-line audit text — identifiers only, never secrets.
    pub fn summary(&self) -> String {
        format!(
            "principal={} actor={} organization={} role={} status={} permissions={} platform={}",
            self.principal.kind(),
            self.principal.identifier(),
            self.organization_id,
            self.role,
            self.organization_status,
            self.permissions.len(),
            self.is_platform_scope()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::MembershipStatus;
    use crate::tenant::UserId;

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

    fn membership(org_id: OrganizationId, role: MembershipRole) -> Membership {
        Membership::new(org_id, UserId::new(), role, None, Utc::now())
    }

    fn session() -> Principal {
        Principal::UserSession {
            session_id: "s-1".into(),
        }
    }

    #[test]
    fn context_carries_the_role_permissions() {
        let o = org(OrganizationStatus::Active);
        let m = membership(o.id, MembershipRole::Trader);
        let ctx = AuthorizationContext::from_membership(session(), &o, &m, None, false, Utc::now());
        assert_eq!(ctx.organization_id, o.id);
        assert_eq!(ctx.role, MembershipRole::Trader);
        assert!(ctx.has(Permission::BotStart));
        assert!(!ctx.has(Permission::ApiKeyCreate));
        assert!(ctx.has_all(&[Permission::BotRead, Permission::OrderRead]));
        assert!(!ctx.has_all(&[Permission::BotRead, Permission::BillingManage]));
        assert!(ctx.owns(o.id));
        assert!(!ctx.owns(OrganizationId::new()), "cross-tenant is refused");
    }

    #[test]
    fn scopes_only_narrow_never_widen() {
        let o = org(OrganizationStatus::Active);
        let m = membership(o.id, MembershipRole::Trader);
        let scopes = PermissionSet::from_iter([
            Permission::BotRead,
            // The role does not grant this; the scope must not add it.
            Permission::BillingManage,
        ]);
        let ctx = AuthorizationContext::from_membership(
            session(),
            &o,
            &m,
            Some(&scopes),
            false,
            Utc::now(),
        );
        assert!(ctx.has(Permission::BotRead));
        assert!(!ctx.has(Permission::BillingManage));
        assert!(!ctx.has(Permission::BotStart), "narrowed away");
        assert_eq!(ctx.permissions.len(), 1);
    }

    #[test]
    fn a_suspended_membership_grants_nothing() {
        let o = org(OrganizationStatus::Active);
        let mut m = membership(o.id, MembershipRole::OrgOwner);
        m.status = MembershipStatus::Suspended;
        let ctx = AuthorizationContext::from_membership(session(), &o, &m, None, true, Utc::now());
        assert!(ctx.permissions.is_empty());
        assert!(!ctx.has(Permission::TenantRead));
    }

    #[test]
    fn platform_scope_requires_both_role_and_user_flag() {
        let o = org(OrganizationStatus::Active);
        let other = OrganizationId::new();

        // Role says platform admin but the user flag is false.
        let m = membership(o.id, MembershipRole::PlatformAdmin);
        let ctx = AuthorizationContext::from_membership(session(), &o, &m, None, false, Utc::now());
        assert!(!ctx.is_platform_scope());
        assert!(!ctx.owns(other), "a role alone must not cross tenants");

        // Both true.
        let ctx = AuthorizationContext::from_membership(session(), &o, &m, None, true, Utc::now());
        assert!(ctx.is_platform_scope());
        assert!(ctx.owns(other));

        // User flag true but an ordinary role.
        let m = membership(o.id, MembershipRole::OrgOwner);
        let ctx = AuthorizationContext::from_membership(session(), &o, &m, None, true, Utc::now());
        assert!(!ctx.is_platform_scope());
        assert!(!ctx.owns(other));
    }

    #[test]
    fn an_api_key_never_confers_platform_scope() {
        let o = org(OrganizationStatus::Active);
        let ctx = AuthorizationContext::from_api_key(
            Principal::ApiKey {
                key_id: "k-1".into(),
                key_prefix: "sk_ab12cd34".into(),
            },
            &o,
            MembershipRole::PlatformAdmin,
            None,
            None,
            Utc::now(),
        );
        assert!(!ctx.platform_admin);
        assert!(!ctx.is_platform_scope());
        assert!(!ctx.owns(OrganizationId::new()));
        assert!(ctx.owns(o.id));
        assert_eq!(ctx.principal.kind(), "api_key");
        assert_eq!(ctx.principal.identifier(), "key:sk_ab12cd34");
    }

    #[test]
    fn summary_is_single_line_and_secret_free() {
        let o = org(OrganizationStatus::Suspended);
        let m = membership(o.id, MembershipRole::Auditor);
        let ctx = AuthorizationContext::from_membership(session(), &o, &m, None, false, Utc::now());
        let s = ctx.summary();
        assert!(!s.contains('\n'));
        assert!(s.contains("role=auditor"));
        assert!(s.contains("status=suspended"));
        assert!(s.contains(&o.id.to_string()));
    }
}
