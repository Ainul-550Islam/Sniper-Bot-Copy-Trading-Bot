//! Organization membership — the SaaS RBAC domain (TASK 7A file 05).
//!
//! A membership is the link `(user, organization) → role`. It is what turns
//! "a logged-in human" into "a caller with permissions inside one tenant".
//! A user may belong to several organizations with a different role in
//! each; the membership row, never the user record, decides what they may
//! do in a given tenant.
//!
//! | file | concern |
//! |---|---|
//! | `role.rs` | the 8 canonical roles and the ONE role → permission matrix |
//! | `permission.rs` | the closed permission vocabulary and [`PermissionSet`] |
//!
//! The pre-TASK-7A [`crate::auth::Role`] (owner / operator / readonly) is
//! untouched and still governs deployment API keys; the bridge between the
//! two vocabularies is explicit
//! ([`MembershipRole::from_legacy`] / [`MembershipRole::to_legacy`]).

pub mod permission;
pub mod role;

pub use permission::{Permission, PermissionSet};
pub use role::MembershipRole;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::BotResult;
use crate::tenant::{MembershipId, OrganizationId, UserId};

/// Lifecycle of a membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStatus {
    /// Normal.
    Active,
    /// Temporarily disabled; the row stays for audit.
    Suspended,
    /// Removed from the organization; terminal.
    Removed,
}

impl MembershipStatus {
    /// Every status, stable order.
    pub const ALL: [MembershipStatus; 3] = [
        MembershipStatus::Active,
        MembershipStatus::Suspended,
        MembershipStatus::Removed,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            MembershipStatus::Active => "active",
            MembershipStatus::Suspended => "suspended",
            MembershipStatus::Removed => "removed",
        }
    }

    /// Inverse of [`MembershipStatus::as_str`].
    pub fn parse(s: &str) -> Option<MembershipStatus> {
        MembershipStatus::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Only an active membership grants anything.
    pub fn is_active(&self) -> bool {
        matches!(self, MembershipStatus::Active)
    }
}

impl std::fmt::Display for MembershipStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `(user, organization) → role` link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Membership {
    /// Row identity.
    pub id: MembershipId,
    /// The tenant.
    pub organization_id: OrganizationId,
    /// The human.
    pub user_id: UserId,
    /// What they may do inside this tenant.
    pub role: MembershipRole,
    /// Lifecycle.
    pub status: MembershipStatus,
    /// Who invited them, when known.
    pub invited_by: Option<UserId>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
}

impl Membership {
    /// A fresh active membership.
    pub fn new(
        organization_id: OrganizationId,
        user_id: UserId,
        role: MembershipRole,
        invited_by: Option<UserId>,
        now: DateTime<Utc>,
    ) -> Self {
        Membership {
            id: MembershipId::new(),
            organization_id,
            user_id,
            role,
            status: MembershipStatus::Active,
            invited_by,
            created_at: now,
            updated_at: now,
        }
    }

    /// The permissions this membership grants — empty unless it is active,
    /// so a suspended member is powerless without deleting the row.
    pub fn permissions(&self) -> PermissionSet {
        if self.status.is_active() {
            self.role.permissions()
        } else {
            PermissionSet::empty()
        }
    }

    /// Does this membership grant `p`?
    pub fn grants(&self, p: Permission) -> bool {
        self.permissions().contains(p)
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "membership={} organization={} user={} role={} status={}",
            self.id, self.organization_id, self.user_id, self.role, self.status
        )
    }
}

/// Durable membership storage.
///
/// Every method is tenant-scoped by construction: there is no "list all
/// memberships" call, so a handler cannot accidentally enumerate another
/// tenant's people.
#[async_trait]
pub trait MembershipStore: Send + Sync {
    /// Insert a membership. Fails when the pair already exists.
    async fn create_membership(&self, m: &Membership) -> BotResult<()>;

    /// The membership of one user in one organization.
    async fn membership(
        &self,
        organization_id: OrganizationId,
        user_id: UserId,
    ) -> BotResult<Option<Membership>>;

    /// Every membership of one organization.
    async fn members(&self, organization_id: OrganizationId) -> BotResult<Vec<Membership>>;

    /// Every organization one user belongs to.
    async fn memberships_of_user(&self, user_id: UserId) -> BotResult<Vec<Membership>>;

    /// Persist a changed membership (role change, suspension, removal).
    async fn update_membership(&self, m: &Membership) -> BotResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn membership(role: MembershipRole, status: MembershipStatus) -> Membership {
        let mut m = Membership::new(OrganizationId::new(), UserId::new(), role, None, Utc::now());
        m.status = status;
        m
    }

    #[test]
    fn status_round_trips() {
        for s in MembershipStatus::ALL {
            assert_eq!(MembershipStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(MembershipStatus::parse("nope"), None);
        assert!(MembershipStatus::Active.is_active());
        assert!(!MembershipStatus::Suspended.is_active());
    }

    #[test]
    fn only_an_active_membership_grants_permissions() {
        let active = membership(MembershipRole::Trader, MembershipStatus::Active);
        assert!(active.grants(Permission::BotStart));
        assert!(!active.permissions().is_empty());

        for s in [MembershipStatus::Suspended, MembershipStatus::Removed] {
            let m = membership(MembershipRole::OrgOwner, s);
            assert!(m.permissions().is_empty(), "a {s} owner must hold nothing");
            assert!(!m.grants(Permission::TenantRead));
        }
    }

    #[test]
    fn summary_is_single_line_and_names_the_tenant() {
        let m = membership(MembershipRole::Auditor, MembershipStatus::Active);
        let s = m.summary();
        assert!(!s.contains('\n'));
        assert!(s.contains(&m.organization_id.to_string()));
        assert!(s.contains("role=auditor"));
    }
}
