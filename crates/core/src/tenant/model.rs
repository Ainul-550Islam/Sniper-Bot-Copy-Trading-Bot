//! Canonical tenant / organization domain model (TASK 7A file 03).
//!
//! Before TASK 7A the suite had exactly one implicit operator: resources
//! were owned by "the deployment". A SaaS control plane needs an explicit,
//! strongly typed owner on every row, so this module defines the identity
//! types and the organization record that every authorization decision and
//! every SaaS-owned table resolves to.
//!
//! Identifiers are newtypes over `Uuid` ([`UserId`], [`OrganizationId`]),
//! not raw strings: a function that takes an `OrganizationId` cannot be
//! handed a user id, a slug or a symbol by accident. Serialization is the
//! plain UUID string, so the wire format and the database column match
//! exactly.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Declare a UUID newtype with a stable string form.
macro_rules! uuid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// A fresh random identifier.
            pub fn new() -> Self {
                $name(Uuid::new_v4())
            }

            /// The inner UUID.
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }

            /// Parse from the canonical string form.
            pub fn parse(s: &str) -> Option<Self> {
                Uuid::parse_str(s.trim()).ok().map($name)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                $name::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = TenantIdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                $name::parse(s).ok_or_else(|| TenantIdError {
                    kind: stringify!($name),
                    value: s.to_string(),
                })
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                $name(u)
            }
        }
    };
}

/// A malformed identifier. Carries what was expected and what was given
/// (the value is an id, never a secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantIdError {
    /// The type that failed to parse (`OrganizationId`, …).
    pub kind: &'static str,
    /// The offending input.
    pub value: String,
}

impl fmt::Display for TenantIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {}: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for TenantIdError {}

uuid_id!(UserId, "A human identity (`users.id`).");
uuid_id!(
    OrganizationId,
    "A TENANT (`organizations.id`). Every SaaS-owned resource carries one."
);
uuid_id!(MembershipId, "One (user, organization) membership row.");

/// `OrganizationId` is the tenant identity; the alias documents intent at
/// call sites that talk about tenancy rather than about the organization
/// record itself.
pub type TenantId = OrganizationId;

/// Lifecycle of a tenant. Closed vocabulary — bounded metric label, stable
/// database `CHECK`, and the input of [`super::policy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationStatus {
    /// Paying (or free-tier) and fully operational.
    Active,
    /// Inside a trial window; operational.
    Trialing,
    /// Payment failed. Read access continues, new trading is refused.
    PastDue,
    /// Administratively suspended: no trading, no control actions.
    Suspended,
    /// Closed by the customer or the platform; terminal.
    Closed,
}

impl OrganizationStatus {
    /// Every status, stable order.
    pub const ALL: [OrganizationStatus; 5] = [
        OrganizationStatus::Active,
        OrganizationStatus::Trialing,
        OrganizationStatus::PastDue,
        OrganizationStatus::Suspended,
        OrganizationStatus::Closed,
    ];

    /// Stable label (database, metrics, audit).
    pub fn as_str(&self) -> &'static str {
        match self {
            OrganizationStatus::Active => "active",
            OrganizationStatus::Trialing => "trialing",
            OrganizationStatus::PastDue => "past_due",
            OrganizationStatus::Suspended => "suspended",
            OrganizationStatus::Closed => "closed",
        }
    }

    /// Inverse of [`OrganizationStatus::as_str`].
    pub fn parse(s: &str) -> Option<OrganizationStatus> {
        OrganizationStatus::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Terminal states cannot come back.
    pub fn is_terminal(&self) -> bool {
        matches!(self, OrganizationStatus::Closed)
    }
}

impl fmt::Display for OrganizationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Organization {
    /// Tenant identity.
    pub id: OrganizationId,
    /// URL-safe unique handle.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Lifecycle state.
    pub status: OrganizationStatus,
    /// Creator, when still known.
    pub created_by: Option<UserId>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
    /// When it was suspended, if it is.
    pub suspended_at: Option<DateTime<Utc>>,
    /// Why it was suspended (operator text; never a secret).
    pub suspend_reason: String,
}

impl Organization {
    /// A fresh active organization.
    pub fn new(
        id: OrganizationId,
        slug: impl Into<String>,
        name: impl Into<String>,
        created_by: Option<UserId>,
        now: DateTime<Utc>,
    ) -> Self {
        Organization {
            id,
            slug: slug.into(),
            name: name.into(),
            status: OrganizationStatus::Active,
            created_by,
            created_at: now,
            updated_at: now,
            suspended_at: None,
            suspend_reason: String::new(),
        }
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "organization={} slug={} status={} created_by={}",
            self.id,
            self.slug,
            self.status,
            self.created_by
                .map(|u| u.to_string())
                .unwrap_or_else(|| "-".into())
        )
    }
}

/// Lifecycle of a human account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    /// Normal.
    Active,
    /// Administratively suspended: cannot authenticate.
    Suspended,
    /// Self-deactivated; terminal unless an admin reactivates.
    Deactivated,
}

impl UserStatus {
    /// Every status, stable order.
    pub const ALL: [UserStatus; 3] = [
        UserStatus::Active,
        UserStatus::Suspended,
        UserStatus::Deactivated,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            UserStatus::Active => "active",
            UserStatus::Suspended => "suspended",
            UserStatus::Deactivated => "deactivated",
        }
    }

    /// Inverse of [`UserStatus::as_str`].
    pub fn parse(s: &str) -> Option<UserStatus> {
        UserStatus::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }
}

impl fmt::Display for UserStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One human identity.
///
/// `password_hash` is the encoded PBKDF2 string
/// ([`crate::session::token::hash_password`]) — never a plaintext password.
/// The API layer serialises [`UserProfile`] instead of this record, so a
/// hash cannot leak through a handler by accident.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// Identity.
    pub id: UserId,
    /// Login address (stored lowercased).
    pub email: String,
    /// Whether the address was verified.
    pub email_verified: bool,
    /// Display name.
    pub display_name: String,
    /// Encoded PBKDF2 hash. NEVER a plaintext password.
    pub password_hash: String,
    /// Lifecycle.
    pub status: UserStatus,
    /// Cross-tenant platform administrator.
    pub platform_admin: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
    /// Last successful login.
    pub last_login_at: Option<DateTime<Utc>>,
}

impl User {
    /// Normalise an email for storage and lookup.
    pub fn normalize_email(email: &str) -> String {
        email.trim().to_ascii_lowercase()
    }

    /// May this account authenticate at all?
    pub fn can_authenticate(&self) -> bool {
        self.status == UserStatus::Active
    }

    /// The safe, serialisable view (no hash, no secrets).
    pub fn profile(&self) -> UserProfile {
        UserProfile {
            id: self.id,
            email: self.email.clone(),
            email_verified: self.email_verified,
            display_name: self.display_name.clone(),
            status: self.status,
            platform_admin: self.platform_admin,
            created_at: self.created_at,
            last_login_at: self.last_login_at,
        }
    }
}

/// The user view that may cross an API boundary: everything except the
/// password hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserProfile {
    /// Identity.
    pub id: UserId,
    /// Login address.
    pub email: String,
    /// Whether the address was verified.
    pub email_verified: bool,
    /// Display name.
    pub display_name: String,
    /// Lifecycle.
    pub status: UserStatus,
    /// Cross-tenant platform administrator.
    pub platform_admin: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last successful login.
    pub last_login_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_typed_and_round_trip() {
        let org = OrganizationId::new();
        let parsed = OrganizationId::parse(&org.to_string()).expect("round trip");
        assert_eq!(org, parsed);
        assert_eq!(org.as_uuid(), parsed.as_uuid());
        assert!(OrganizationId::parse("not-a-uuid").is_none());
        let err = "nope".parse::<OrganizationId>().unwrap_err();
        assert_eq!(err.kind, "OrganizationId");
        assert!(err.to_string().contains("nope"));
        // Distinct types cannot be confused even with the same bytes.
        let raw = Uuid::new_v4();
        let u = UserId::from(raw);
        let o = OrganizationId::from(raw);
        assert_eq!(u.as_uuid(), o.as_uuid());
        assert_eq!(
            serde_json::to_string(&u).unwrap(),
            serde_json::to_string(&o).unwrap(),
            "the wire form is the plain uuid"
        );
    }

    #[test]
    fn status_vocabularies_round_trip() {
        for s in OrganizationStatus::ALL {
            assert_eq!(OrganizationStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(OrganizationStatus::parse("nope"), None);
        assert!(OrganizationStatus::Closed.is_terminal());
        assert!(!OrganizationStatus::Suspended.is_terminal());
        for s in UserStatus::ALL {
            assert_eq!(UserStatus::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn user_profile_never_carries_the_hash() {
        let now = Utc::now();
        let u = User {
            id: UserId::new(),
            email: "Person@Example.COM".into(),
            email_verified: false,
            display_name: "Person".into(),
            password_hash: "pbkdf2-sha256$600000$c2FsdA$aGFzaA".into(),
            status: UserStatus::Active,
            platform_admin: false,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        };
        let json = serde_json::to_string(&u.profile()).unwrap();
        assert!(!json.contains("pbkdf2"), "{json}");
        assert!(!json.contains("password"), "{json}");
        assert!(json.contains("Person@Example.COM"));
        assert_eq!(
            User::normalize_email(" Person@Example.COM "),
            "person@example.com"
        );
        assert!(u.can_authenticate());
    }

    #[test]
    fn organization_summary_is_single_line() {
        let now = Utc::now();
        let o = Organization::new(
            OrganizationId::new(),
            "acme-capital",
            "Acme Capital",
            Some(UserId::new()),
            now,
        );
        assert_eq!(o.status, OrganizationStatus::Active);
        let s = o.summary();
        assert!(!s.contains('\n'));
        assert!(s.contains("slug=acme-capital"));
    }
}
