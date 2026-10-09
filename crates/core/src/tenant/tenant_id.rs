//! Strong typed tenant identity (STEP 3 file 01 — tenant runtime identity
//! foundation).
//!
//! The canonical tenant identifier is [`OrganizationId`] from
//! [`super::model`]; this module is the *identity* layer on top of it: the
//! same strong type, plus a validated `(id, slug)` pair —
//! [`TenantIdentity`] — that execution, guard and observability code passes
//! around so a tenant is never reduced to a raw string.
//!
//! No duplicate UUID type is introduced here (program rule: one canonical
//! tenant id). Everything in this file is a re-export, a validation or a
//! narrow composite of [`OrganizationId`].

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::model::{Organization, OrganizationId, TenantIdError};
use crate::error::{BotError, BotResult};

pub use super::model::TenantIdError as Error;

/// Reject the nil UUID: a tenant id must identify a real organization row.
pub fn is_valid_tenant_id(id: OrganizationId) -> bool {
    id.as_uuid() != Uuid::nil()
}

/// Validate a tenant id for use in a security-sensitive path.
///
/// Fail closed: the nil UUID (the value an uninitialized column or a
/// deserialization bug produces) is rejected.
pub fn require_valid_tenant_id(id: OrganizationId) -> BotResult<OrganizationId> {
    if is_valid_tenant_id(id) {
        Ok(id)
    } else {
        Err(BotError::Config(
            "tenant identity rejected: the nil organization id is not a tenant".into(),
        ))
    }
}

/// A validated tenant handle: `(OrganizationId, slug)`.
///
/// The slug is carried alongside the id because logs, audit events and
/// operator tooling identify tenants by handle; keeping the pair together
/// removes the "id without a name" ambiguity at call sites. The slug is
/// public data (it appears in URLs and the SaaS console) — never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantIdentity {
    id: OrganizationId,
    slug: String,
}

impl TenantIdentity {
    /// Validate and build. The slug must be non-empty, at most 64 bytes,
    /// and use the URL-safe subset the SaaS console already produces.
    pub fn new(id: OrganizationId, slug: impl Into<String>) -> BotResult<Self> {
        let id = require_valid_tenant_id(id)?;
        let slug = slug.into();
        let trimmed = slug.trim();
        if trimmed.is_empty() || trimmed.len() > 64 {
            return Err(BotError::Config(format!(
                "tenant slug rejected: {:?} (must be 1..=64 bytes)",
                slug
            )));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(BotError::Config(format!(
                "tenant slug rejected: {:?} (allowed: a-z, 0-9, '-')",
                slug
            )));
        }
        Ok(TenantIdentity {
            id,
            slug: trimmed.to_string(),
        })
    }

    /// From an organization row (its own id + slug, validated).
    pub fn from_organization(org: &Organization) -> BotResult<Self> {
        TenantIdentity::new(org.id, org.slug.as_str())
    }

    /// The canonical tenant id.
    pub fn id(&self) -> OrganizationId {
        self.id
    }

    /// The public handle.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Does this identity denote the given tenant?
    pub fn is(&self, other: OrganizationId) -> bool {
        self.id == other
    }
}

impl fmt::Display for TenantIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.id, self.slug)
    }
}

/// Parse a tenant id from its canonical string form, rejecting the nil UUID.
pub fn parse_tenant_id(s: &str) -> BotResult<OrganizationId> {
    let id: OrganizationId = s
        .trim()
        .parse()
        .map_err(|e: TenantIdError| BotError::Config(format!("tenant id rejected: {e}")))?;
    require_valid_tenant_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::TenantId;

    fn org() -> Organization {
        let now = chrono::Utc::now();
        Organization::new(
            OrganizationId::new(),
            "acme-trading",
            "Acme Trading",
            None,
            now,
        )
    }

    #[test]
    fn nil_uuid_is_not_a_tenant() {
        let nil = OrganizationId::from(Uuid::nil());
        assert!(!is_valid_tenant_id(nil));
        assert!(require_valid_tenant_id(nil).is_err());
        assert!(TenantIdentity::new(nil, "acme").is_err());
        assert!(parse_tenant_id(&Uuid::nil().to_string()).is_err());
    }

    #[test]
    fn identity_validates_slug_charset_and_length() {
        let o = org();
        assert!(TenantIdentity::from_organization(&o).is_ok());
        assert!(TenantIdentity::new(o.id, "").is_err());
        assert!(TenantIdentity::new(o.id, "Acme").is_err()); // uppercase
        assert!(TenantIdentity::new(o.id, "has space").is_err());
        assert!(TenantIdentity::new(o.id, "x".repeat(65)).is_err());
        assert!(TenantIdentity::new(o.id, "acme-2").is_ok());
    }

    #[test]
    fn identity_round_trips_display_and_fields() {
        let o = org();
        let id = TenantIdentity::from_organization(&o).unwrap();
        assert_eq!(id.id(), o.id);
        assert_eq!(id.slug(), "acme-trading");
        assert!(id.is(o.id));
        assert!(id.to_string().contains("acme-trading"));
        assert!(id.to_string().contains(&o.id.to_string()));
    }

    #[test]
    fn canonical_alias_is_organization_id() {
        // TenantId remains the documented alias of OrganizationId.
        let t: TenantId = org().id;
        let o2: OrganizationId = t;
        assert_eq!(t, o2);
    }
}
