//! The authorization decision vocabulary (TASK 7A file 20).
//!
//! Every refusal in the control plane is one of these values. A handler
//! cannot invent a reason string, cannot return a bare boolean, and cannot
//! decide "denied but let's continue": the type has no such shape.
//!
//! The vocabulary is ordered by the gate that produced it, which is also
//! the order the gates run in:
//!
//! ```text
//! authenticated? → tenant resolved? → tenant healthy? → role → permission
//!                → resource ownership → entitlement → ALLOW
//! ```
//!
//! Pure data: constructing a [`Decision`] has no side effects, so the
//! caller decides what to log, meter or audit.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Why a request was refused (or that it was allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// The caller may proceed.
    Allow,
    /// No usable credential (missing, unknown, expired, revoked).
    DenyUnauthenticated,
    /// Authenticated, but no tenant context could be established — or the
    /// caller asked for a tenant it has no membership in.
    DenyTenant,
    /// The membership's role is not sufficient.
    DenyRole,
    /// The role is sufficient in principle, but the specific permission is
    /// not granted (or an API key's scopes narrowed it away).
    DenyPermission,
    /// The resource belongs to another tenant.
    DenyResource,
    /// The tenant is suspended / past due / closed for this action.
    DenySuspended,
    /// The plan does not include the feature, or a limit is exhausted.
    DenyEntitlement,
}

impl DecisionKind {
    /// Every kind, in gate order.
    pub const ALL: [DecisionKind; 8] = [
        DecisionKind::Allow,
        DecisionKind::DenyUnauthenticated,
        DecisionKind::DenyTenant,
        DecisionKind::DenyRole,
        DecisionKind::DenyPermission,
        DecisionKind::DenyResource,
        DecisionKind::DenySuspended,
        DecisionKind::DenyEntitlement,
    ];

    /// Stable label (metrics, audit, API error body).
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionKind::Allow => "allow",
            DecisionKind::DenyUnauthenticated => "deny_unauthenticated",
            DecisionKind::DenyTenant => "deny_tenant",
            DecisionKind::DenyRole => "deny_role",
            DecisionKind::DenyPermission => "deny_permission",
            DecisionKind::DenyResource => "deny_resource",
            DecisionKind::DenySuspended => "deny_suspended",
            DecisionKind::DenyEntitlement => "deny_entitlement",
        }
    }

    /// Inverse of [`DecisionKind::as_str`].
    pub fn parse(s: &str) -> Option<DecisionKind> {
        DecisionKind::ALL
            .iter()
            .copied()
            .find(|d| d.as_str() == s.trim())
    }

    /// True only for [`DecisionKind::Allow`].
    pub fn is_allowed(&self) -> bool {
        matches!(self, DecisionKind::Allow)
    }

    /// The HTTP status this refusal maps to.
    ///
    /// Everything except "you are not authenticated" is 403: a 404 would
    /// leak whether a resource exists in another tenant, and a 401 would
    /// invite a client to retry with the same credential.
    pub fn http_status(&self) -> u16 {
        match self {
            DecisionKind::Allow => 200,
            DecisionKind::DenyUnauthenticated => 401,
            DecisionKind::DenyEntitlement => 402,
            _ => 403,
        }
    }

    /// Should this refusal be audited as a security-relevant event?
    /// Cross-tenant attempts and role/permission failures are; a missing
    /// credential on a public probe is routine noise.
    pub fn is_security_relevant(&self) -> bool {
        matches!(
            self,
            DecisionKind::DenyResource
                | DecisionKind::DenyRole
                | DecisionKind::DenyPermission
                | DecisionKind::DenyTenant
        )
    }
}

impl fmt::Display for DecisionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A complete authorization outcome: the kind plus a stable, human reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// What was decided.
    pub kind: DecisionKind,
    /// Why, in words a caller may see. Never contains secret material.
    pub reason: String,
}

impl Decision {
    /// Allow.
    pub fn allow() -> Self {
        Decision {
            kind: DecisionKind::Allow,
            reason: "allowed".into(),
        }
    }

    /// Deny with an explicit kind and reason.
    pub fn deny(kind: DecisionKind, reason: impl Into<String>) -> Self {
        Decision {
            kind,
            reason: reason.into(),
        }
    }

    /// No usable credential.
    pub fn unauthenticated(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenyUnauthenticated, reason)
    }

    /// No tenant context, or no membership in the requested tenant.
    pub fn tenant(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenyTenant, reason)
    }

    /// Role insufficient.
    pub fn role(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenyRole, reason)
    }

    /// Permission not granted.
    pub fn permission(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenyPermission, reason)
    }

    /// Resource owned by another tenant.
    pub fn resource(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenyResource, reason)
    }

    /// Tenant state forbids the action.
    pub fn suspended(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenySuspended, reason)
    }

    /// Plan / limit forbids the action.
    pub fn entitlement(reason: impl Into<String>) -> Self {
        Decision::deny(DecisionKind::DenyEntitlement, reason)
    }

    /// True on allow.
    pub fn is_allowed(&self) -> bool {
        self.kind.is_allowed()
    }

    /// The HTTP status to return.
    pub fn http_status(&self) -> u16 {
        self.kind.http_status()
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!("decision={} reason={}", self.kind, self.reason)
    }
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_round_trips_and_is_complete() {
        for d in DecisionKind::ALL {
            assert_eq!(DecisionKind::parse(d.as_str()), Some(d));
        }
        assert_eq!(DecisionKind::parse("deny_because_i_said_so"), None);
        assert_eq!(DecisionKind::ALL.len(), 8);
        // Exactly one allow.
        assert_eq!(
            DecisionKind::ALL.iter().filter(|d| d.is_allowed()).count(),
            1
        );
    }

    #[test]
    fn constructors_set_the_right_kind() {
        assert!(Decision::allow().is_allowed());
        assert_eq!(
            Decision::unauthenticated("no key").kind,
            DecisionKind::DenyUnauthenticated
        );
        assert_eq!(Decision::tenant("no org").kind, DecisionKind::DenyTenant);
        assert_eq!(Decision::role("too weak").kind, DecisionKind::DenyRole);
        assert_eq!(
            Decision::permission("missing bot.start").kind,
            DecisionKind::DenyPermission
        );
        assert_eq!(
            Decision::resource("other tenant").kind,
            DecisionKind::DenyResource
        );
        assert_eq!(
            Decision::suspended("org suspended").kind,
            DecisionKind::DenySuspended
        );
        assert_eq!(
            Decision::entitlement("plan lacks it").kind,
            DecisionKind::DenyEntitlement
        );
        for d in DecisionKind::ALL {
            if d.is_allowed() {
                continue;
            }
            assert!(!Decision::deny(d, "x").is_allowed(), "{d}");
        }
    }

    #[test]
    fn http_mapping_does_not_leak_existence() {
        assert_eq!(DecisionKind::Allow.http_status(), 200);
        assert_eq!(DecisionKind::DenyUnauthenticated.http_status(), 401);
        assert_eq!(
            DecisionKind::DenyEntitlement.http_status(),
            402,
            "payment required is the honest answer for a plan limit"
        );
        for d in [
            DecisionKind::DenyTenant,
            DecisionKind::DenyRole,
            DecisionKind::DenyPermission,
            DecisionKind::DenyResource,
            DecisionKind::DenySuspended,
        ] {
            assert_eq!(
                d.http_status(),
                403,
                "{d} must not be a 404: that would confirm the resource exists"
            );
        }
    }

    #[test]
    fn security_relevant_refusals_are_marked() {
        assert!(DecisionKind::DenyResource.is_security_relevant());
        assert!(DecisionKind::DenyPermission.is_security_relevant());
        assert!(DecisionKind::DenyRole.is_security_relevant());
        assert!(DecisionKind::DenyTenant.is_security_relevant());
        assert!(!DecisionKind::DenyUnauthenticated.is_security_relevant());
        assert!(!DecisionKind::Allow.is_security_relevant());
    }

    #[test]
    fn summaries_are_single_line() {
        let d = Decision::resource("order belongs to organization X");
        assert!(!d.summary().contains('\n'));
        assert!(d.summary().contains("deny_resource"));
        assert!(d.to_string().contains("order belongs"));
    }
}
