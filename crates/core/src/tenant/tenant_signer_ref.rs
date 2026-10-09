//! Tenant-scoped signer reference (STEP 3 file 09).
//!
//! A [`TenantSignerRef`] IDENTIFIES the signing authority a tenant's
//! execution may use: which custody provider holds the key and the
//! provider-side key handle. Like [`super::tenant_wallet_ref::TenantWalletRef`],
//! it structurally cannot carry secrets — `key_ref` is a provider-side
//! IDENTIFIER (a custody key id, a vault key name), never the key itself.
//!
//! The display form redacts the handle so logs and audit events can carry
//! a signer identity without disclosing even the provider handle shape.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::model::OrganizationId;
use crate::error::{BotError, BotResult};

/// Where the key material for this signer lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignerProvider {
    /// The platform custody service (TASK 7 custody model) holds the key.
    Custody,
    /// A deployment-local key file (single-operator legacy mode). The
    /// reference names the configured slot, never a path with secrets.
    Local,
    /// An external signer the tenant operates (e.g. its own HSM); the
    /// platform only ever sees signatures.
    External,
}

impl SignerProvider {
    /// Every provider, stable order.
    pub const ALL: [SignerProvider; 3] = [
        SignerProvider::Custody,
        SignerProvider::Local,
        SignerProvider::External,
    ];

    /// Stable label.
    pub fn as_str(self) -> &'static str {
        match self {
            SignerProvider::Custody => "custody",
            SignerProvider::Local => "local",
            SignerProvider::External => "external",
        }
    }

    /// Inverse of [`SignerProvider::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        SignerProvider::ALL
            .iter()
            .copied()
            .find(|p| p.as_str() == s.trim())
    }
}

impl fmt::Display for SignerProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Maximum key-reference length (provider handles are short ids).
pub const MAX_KEY_REF_LEN: usize = 128;

/// A tenant-scoped signer reference (identifier only — no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantSignerRef {
    /// The tenant that owns the signer binding.
    organization_id: OrganizationId,
    /// Which provider holds the key.
    provider: SignerProvider,
    /// The provider-side key identifier. NEVER a key, seed or mnemonic.
    key_ref: String,
    /// Revoked bindings never sign.
    active: bool,
}

impl TenantSignerRef {
    /// Build and validate. Fail closed on a malformed handle.
    pub fn new(
        organization_id: OrganizationId,
        provider: SignerProvider,
        key_ref: impl Into<String>,
    ) -> BotResult<Self> {
        let key_ref = key_ref.into();
        let trimmed = key_ref.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_KEY_REF_LEN {
            return Err(BotError::Config(format!(
                "signer key reference rejected: {:?} (must be 1..={MAX_KEY_REF_LEN} chars)",
                key_ref
            )));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.' | '/'))
        {
            return Err(BotError::Config(format!(
                "signer key reference rejected: {:?} (allowed: alphanumerics and : - _ . /)",
                key_ref
            )));
        }
        Ok(TenantSignerRef {
            organization_id,
            provider,
            key_ref: trimmed.to_string(),
            active: true,
        })
    }

    /// The owning tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The provider kind.
    pub fn provider(&self) -> SignerProvider {
        self.provider
    }

    /// The provider-side key identifier (public metadata).
    pub fn key_ref(&self) -> &str {
        &self.key_ref
    }

    /// Is the binding active (not revoked)?
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Revoke the binding (idempotent).
    pub fn revoke(&mut self) {
        self.active = false;
    }

    /// Does this signer belong to the given tenant?
    pub fn belongs_to(&self, org: OrganizationId) -> bool {
        self.organization_id == org
    }

    /// Fail-closed ownership check.
    pub fn require_belongs_to(&self, org: OrganizationId) -> BotResult<()> {
        if self.belongs_to(org) {
            Ok(())
        } else {
            Err(BotError::Config(
                "signer does not belong to the acting organization".into(),
            ))
        }
    }

    /// A log-safe identity: provider + redacted handle. The handle is
    /// shortened to its first four characters so two different signers
    /// remain distinguishable in logs without exposing the full handle.
    pub fn redacted(&self) -> String {
        let head: String = self.key_ref.chars().take(4).collect();
        format!("{}:{}", self.provider.as_str(), head)
    }
}

impl fmt::Display for TenantSignerRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.redacted())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_reference_round_trips() {
        let org = OrganizationId::new();
        let s = TenantSignerRef::new(org, SignerProvider::Custody, "key-2024-main").unwrap();
        assert_eq!(s.provider(), SignerProvider::Custody);
        assert_eq!(s.key_ref(), "key-2024-main");
        assert!(s.is_active());
        assert!(s.belongs_to(org));
        assert!(s.require_belongs_to(org).is_ok());
    }

    #[test]
    fn malformed_handles_are_rejected() {
        let org = OrganizationId::new();
        assert!(TenantSignerRef::new(org, SignerProvider::Custody, "").is_err());
        assert!(TenantSignerRef::new(org, SignerProvider::Custody, "  ").is_err());
        assert!(TenantSignerRef::new(org, SignerProvider::Custody, "bad handle").is_err());
        assert!(TenantSignerRef::new(org, SignerProvider::Custody, "x".repeat(129)).is_err());
        assert!(TenantSignerRef::new(org, SignerProvider::Custody, "vault/keys/sol-1").is_ok());
    }

    #[test]
    fn cross_tenant_and_revoked_fail_closed() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let mut s = TenantSignerRef::new(a, SignerProvider::Local, "deploy-slot-0").unwrap();
        assert!(s.require_belongs_to(b).is_err());
        s.revoke();
        assert!(!s.is_active());
        s.revoke(); // idempotent
        assert!(!s.is_active());
    }

    #[test]
    fn display_is_redacted() {
        let org = OrganizationId::new();
        let s = TenantSignerRef::new(org, SignerProvider::Custody, "key-2024-main").unwrap();
        let d = s.to_string();
        assert!(d.starts_with("custody:key-"));
        assert!(!d.contains("main"), "full handle must not leak: {d}");
        assert_eq!(s.redacted(), "custody:key-");
    }

    #[test]
    fn provider_labels_round_trip() {
        for p in SignerProvider::ALL {
            assert_eq!(SignerProvider::parse(p.as_str()), Some(p));
        }
        assert_eq!(
            SignerProvider::parse("custody"),
            Some(SignerProvider::Custody)
        );
        assert_eq!(SignerProvider::parse("nope"), None);
    }
}
