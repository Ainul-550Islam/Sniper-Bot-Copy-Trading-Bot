//! Tenant-scoped wallet reference (STEP 3 file 08).
//!
//! A [`TenantWalletRef`] IDENTIFIES a wallet that belongs to a tenant: the
//! public chain address plus the owning organization. It never contains —
//! and structurally cannot contain — private material: there is no field
//! for a key, seed or mnemonic, and the validation rejects anything that
//! is not a plain public address.
//!
//! This is the twin of the server-side `WalletBinding`
//! (`crates/server/src/saas/wallet_access.rs`), which owns the durable
//! binding rows and their revocation state. The reference is what travels
//! through the execution context; the binding is what the wallet guard
//! verifies it against.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::model::OrganizationId;
use crate::error::{BotError, BotResult};

/// Maximum accepted address length (covers Solana base58, EVM hex and
/// venue-specific account ids).
pub const MAX_ADDRESS_LEN: usize = 100;
/// Minimum accepted address length.
pub const MIN_ADDRESS_LEN: usize = 8;

/// Validate a public address shape: printable, no whitespace, bounded
/// length. Chain-agnostic on purpose — the same rules the SaaS wallet
/// registry applies (`wallet_access::valid_public_address`).
pub fn valid_public_address(value: &str) -> bool {
    let v = value.trim();
    (MIN_ADDRESS_LEN..=MAX_ADDRESS_LEN).contains(&v.len())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

/// A tenant-scoped reference to one public wallet address.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantWalletRef {
    /// The tenant that owns the wallet.
    organization_id: OrganizationId,
    /// The PUBLIC chain address (base58 / hex / venue account id).
    address: String,
    /// Optional operator label (display only; never used for matching).
    label: Option<String>,
}

impl TenantWalletRef {
    /// Build and validate. Fail closed on a malformed address.
    pub fn new(organization_id: OrganizationId, address: impl Into<String>) -> BotResult<Self> {
        let address = address.into();
        if !valid_public_address(&address) {
            return Err(BotError::Config(format!(
                "wallet address rejected: {:?} (must be {MIN_ADDRESS_LEN}..={MAX_ADDRESS_LEN} chars, no whitespace)",
                address
            )));
        }
        Ok(TenantWalletRef {
            organization_id,
            address,
            label: None,
        })
    }

    /// Attach a display label (bounded, non-secret).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        let label = label.into();
        if !label.trim().is_empty() && label.len() <= 64 {
            self.label = Some(label);
        }
        self
    }

    /// The owning tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The public address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The display label, if any.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Does this wallet belong to the given tenant?
    pub fn belongs_to(&self, org: OrganizationId) -> bool {
        self.organization_id == org
    }

    /// Fail-closed ownership check.
    pub fn require_belongs_to(&self, org: OrganizationId) -> BotResult<()> {
        if self.belongs_to(org) {
            Ok(())
        } else {
            Err(BotError::Config(
                "wallet does not belong to the acting organization".into(),
            ))
        }
    }
}

impl fmt::Display for TenantWalletRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.label {
            Some(l) => write!(f, "{}:{}", l, self.address),
            None => write!(f, "{}", self.address),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";

    #[test]
    fn valid_address_round_trips() {
        let org = OrganizationId::new();
        let w = TenantWalletRef::new(org, ADDR).unwrap();
        assert_eq!(w.address(), ADDR);
        assert_eq!(w.organization_id(), org);
        assert!(w.belongs_to(org));
        assert!(w.require_belongs_to(org).is_ok());
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        let org = OrganizationId::new();
        assert!(TenantWalletRef::new(org, "short").is_err());
        assert!(TenantWalletRef::new(org, "").is_err());
        assert!(TenantWalletRef::new(org, "has whitespace inside").is_err());
        assert!(TenantWalletRef::new(org, "bad!chars$").is_err());
        assert!(TenantWalletRef::new(org, "x".repeat(101)).is_err());
    }

    #[test]
    fn cross_tenant_ownership_fails_closed() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let w = TenantWalletRef::new(a, ADDR).unwrap();
        assert!(!w.belongs_to(b));
        assert!(w.require_belongs_to(b).is_err());
    }

    #[test]
    fn label_is_optional_and_bounded() {
        let org = OrganizationId::new();
        let w = TenantWalletRef::new(org, ADDR).unwrap();
        assert_eq!(w.label(), None);
        let labeled = w.clone().with_label("hot-wallet-1");
        assert_eq!(labeled.label(), Some("hot-wallet-1"));
        // Over-long labels are ignored, not truncated silently.
        let over = w.with_label("y".repeat(65));
        assert_eq!(over.label(), None);
        assert!(labeled.to_string().starts_with("hot-wallet-1:"));
    }
}
