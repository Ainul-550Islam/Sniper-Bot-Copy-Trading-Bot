//! Canonical remote custody provider configuration (Batch 3).
//!
//! Supports Vault/KMS/HSM configuration references. No secrets in persisted domain values.
//! Validates provider-specific required references. Provides readiness classification.
//! No local fallback when a remote provider is selected.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::model::ProviderType;

/// Readiness for custody provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyReadiness {
    NotConfigured,
    InvalidConfiguration,
    ConfiguredButUnreachable,
    Ready,
}

impl CustodyReadiness {
    pub fn as_str(&self) -> &'static str {
        match self {
            CustodyReadiness::NotConfigured => "not_configured",
            CustodyReadiness::InvalidConfiguration => "invalid_configuration",
            CustodyReadiness::ConfiguredButUnreachable => "configured_but_unreachable",
            CustodyReadiness::Ready => "ready",
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, CustodyReadiness::Ready)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyProviderConfig {
    pub provider_type: ProviderType,
    /// Env var name / ARN / Vault path / HSM slot reference (never secret)
    pub reference: Option<String>,
    /// Optional secondary ref (e.g., Vault transit mount, KMS region, HSM partition)
    pub secondary_ref: Option<String>,
    /// Whether this provider is explicitly allowed for local fallback disabled scenario
    pub allow_local_fallback: bool,
}

impl CustodyProviderConfig {
    pub fn for_provider(provider_type: ProviderType) -> Self {
        Self {
            provider_type,
            reference: None,
            secondary_ref: None,
            allow_local_fallback: false,
        }
    }

    pub fn with_reference(mut self, r: impl Into<String>) -> Self {
        self.reference = Some(r.into());
        self
    }

    pub fn with_secondary(mut self, r: impl Into<String>) -> Self {
        self.secondary_ref = Some(r.into());
        self
    }

    pub fn validate(&self) -> (CustodyReadiness, Option<String>) {
        let r = self.reference.as_deref().map(|s| s.trim()).unwrap_or("");
        match self.provider_type {
            ProviderType::Local => {
                // Local is always ready unless misconfigured (reference must not look like secret)
                if r.is_empty() {
                    return (CustodyReadiness::Ready, None);
                }
                if is_secret_like(r) {
                    return (
                        CustodyReadiness::InvalidConfiguration,
                        Some("local reference looks like secret".into()),
                    );
                }
                (CustodyReadiness::Ready, None)
            }
            ProviderType::Vault => {
                if r.is_empty() {
                    return (
                        CustodyReadiness::NotConfigured,
                        Some("vault reference required (VAULT_ADDR or transit path)".into()),
                    );
                }
                if !is_vault_ref(r) {
                    return (
                        CustodyReadiness::InvalidConfiguration,
                        Some(format!(
                            "vault reference must be env var or vault path: {}",
                            r
                        )),
                    );
                }
                if is_secret_like(r) {
                    return (
                        CustodyReadiness::InvalidConfiguration,
                        Some("vault reference looks like secret".into()),
                    );
                }
                // Configured — reachability is runtime
                (CustodyReadiness::ConfiguredButUnreachable, None)
            }
            ProviderType::Kms => {
                if r.is_empty() {
                    return (
                        CustodyReadiness::NotConfigured,
                        Some("kms key id / ARN required".into()),
                    );
                }
                if !is_kms_ref(r) {
                    return (
                        CustodyReadiness::InvalidConfiguration,
                        Some(format!("kms reference must be ARN or env var: {}", r)),
                    );
                }
                (CustodyReadiness::ConfiguredButUnreachable, None)
            }
            ProviderType::Hsm => {
                if r.is_empty() {
                    return (
                        CustodyReadiness::NotConfigured,
                        Some("hsm slot/partition reference required".into()),
                    );
                }
                if r.len() < 3 {
                    return (
                        CustodyReadiness::InvalidConfiguration,
                        Some("hsm reference too short".into()),
                    );
                }
                (CustodyReadiness::ConfiguredButUnreachable, None)
            }
        }
    }

    pub fn runtime_readiness(&self, env_present: bool, reachable: bool) -> CustodyReadiness {
        let (base, _) = self.validate();
        match base {
            CustodyReadiness::NotConfigured | CustodyReadiness::InvalidConfiguration => base,
            CustodyReadiness::ConfiguredButUnreachable => {
                if env_present && reachable {
                    CustodyReadiness::Ready
                } else {
                    CustodyReadiness::ConfiguredButUnreachable
                }
            }
            CustodyReadiness::Ready => CustodyReadiness::Ready,
        }
    }

    /// Whether local fallback is allowed — must be explicitly disabled for remote providers.
    pub fn local_fallback_allowed(&self) -> bool {
        if self.provider_type.is_remote() {
            self.allow_local_fallback
        } else {
            true
        }
    }
}

fn is_vault_ref(s: &str) -> bool {
    // env var name or vault path
    if s.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return true;
    }
    if s.starts_with("vault/") || s.contains('/') {
        return true;
    }
    if s.contains(':') {
        return true;
    }
    s.len() <= 128 && !s.contains(' ')
}

fn is_kms_ref(s: &str) -> bool {
    if s.starts_with("arn:") {
        return true;
    }
    if s.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return true;
    }
    s.contains(':') || s.contains('/')
}

fn is_secret_like(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    if lower.contains("sk-") || lower.contains("private") {
        return true;
    }
    if s.len() > 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

impl fmt::Debug for CustodyProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustodyProviderConfig")
            .field("provider_type", &self.provider_type)
            .field("has_reference", &self.reference.is_some())
            .field("has_secondary_ref", &self.secondary_ref.is_some())
            .field("allow_local_fallback", &self.allow_local_fallback)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::model::ProviderType;

    #[test]
    fn local_ready_without_ref() {
        let c = CustodyProviderConfig::for_provider(ProviderType::Local);
        assert_eq!(c.validate().0, CustodyReadiness::Ready);
        assert!(c.local_fallback_allowed());
    }

    #[test]
    fn vault_not_configured_when_empty() {
        let c = CustodyProviderConfig::for_provider(ProviderType::Vault);
        assert_eq!(c.validate().0, CustodyReadiness::NotConfigured);
    }

    #[test]
    fn vault_requires_vault_like_ref() {
        let c =
            CustodyProviderConfig::for_provider(ProviderType::Vault).with_reference("VAULT_ADDR");
        assert_eq!(c.validate().0, CustodyReadiness::ConfiguredButUnreachable);
        // remote should not allow fallback unless explicit
        assert!(!c.local_fallback_allowed());
    }

    #[test]
    fn kms_requires_arn_or_env() {
        let c = CustodyProviderConfig::for_provider(ProviderType::Kms)
            .with_reference("arn:aws:kms:us-east-1:123:key/abcd");
        assert_eq!(c.validate().0, CustodyReadiness::ConfiguredButUnreachable);
    }

    #[test]
    fn hsm_requires_nonempty() {
        let c = CustodyProviderConfig::for_provider(ProviderType::Hsm).with_reference("slot-0");
        assert_eq!(c.validate().0, CustodyReadiness::ConfiguredButUnreachable);
        let c2 = CustodyProviderConfig::for_provider(ProviderType::Hsm).with_reference("ab");
        assert_eq!(c2.validate().0, CustodyReadiness::InvalidConfiguration);
    }

    #[test]
    fn no_local_fallback_for_remote() {
        let c =
            CustodyProviderConfig::for_provider(ProviderType::Vault).with_reference("VAULT_ADDR");
        assert!(!c.local_fallback_allowed());
        let mut c2 = c.clone();
        c2.allow_local_fallback = true;
        assert!(c2.local_fallback_allowed());
    }

    #[test]
    fn runtime_ready() {
        let c = CustodyProviderConfig::for_provider(ProviderType::Kms).with_reference("KMS_KEY_ID");
        assert_eq!(c.runtime_readiness(true, true), CustodyReadiness::Ready);
        assert_eq!(
            c.runtime_readiness(true, false),
            CustodyReadiness::ConfiguredButUnreachable
        );
    }

    #[test]
    fn debug_redacts() {
        let c =
            CustodyProviderConfig::for_provider(ProviderType::Vault).with_reference("VAULT_ADDR");
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("Vault"));
        assert!(!dbg.contains("sk-"));
    }
}
