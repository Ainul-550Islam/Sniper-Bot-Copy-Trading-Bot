//! Central typed configuration for external billing providers (Batch 3).
//!
//! Supports provider selection and provider-specific configuration references.
//! Never stores secret values in the domain object — only references (env var names, ARNs, handle IDs).
//! Validates configuration before startup/use and distinguishes:
//! not_configured / configured_but_unreachable / configured_and_ready / invalid_configuration.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Which billing provider the deployment has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingProviderKind {
    Manual,
    Stripe,
    Paddle,
    Test,
}

impl BillingProviderKind {
    pub const ALL: [BillingProviderKind; 4] = [
        BillingProviderKind::Manual,
        BillingProviderKind::Stripe,
        BillingProviderKind::Paddle,
        BillingProviderKind::Test,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            BillingProviderKind::Manual => "manual",
            BillingProviderKind::Stripe => "stripe",
            BillingProviderKind::Paddle => "paddle",
            BillingProviderKind::Test => "test",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s.trim().to_ascii_lowercase())
    }

    pub fn is_external(&self) -> bool {
        matches!(
            self,
            BillingProviderKind::Stripe | BillingProviderKind::Paddle
        )
    }
}

impl fmt::Display for BillingProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Readiness classification for a billing provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadiness {
    NotConfigured,
    ConfiguredButUnreachable,
    ConfiguredAndReady,
    InvalidConfiguration,
}

impl ProviderReadiness {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderReadiness::NotConfigured => "not_configured",
            ProviderReadiness::ConfiguredButUnreachable => "configured_but_unreachable",
            ProviderReadiness::ConfiguredAndReady => "configured_and_ready",
            ProviderReadiness::InvalidConfiguration => "invalid_configuration",
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, ProviderReadiness::ConfiguredAndReady)
    }
}

/// Typed configuration reference — never holds the secret value itself.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingProviderConfig {
    pub kind: BillingProviderKind,
    /// Env var name / ARN / handle that resolves to the API key (never the key itself)
    pub api_key_ref: Option<String>,
    /// Env var name for webhook secret
    pub webhook_secret_ref: Option<String>,
    /// Provider environment (e.g., "live" vs "test")
    pub environment: Option<String>,
    /// Optional price-catalogue override reference
    pub price_config_ref: Option<String>,
}

impl BillingProviderConfig {
    pub fn manual() -> Self {
        Self {
            kind: BillingProviderKind::Manual,
            api_key_ref: None,
            webhook_secret_ref: None,
            environment: None,
            price_config_ref: None,
        }
    }

    pub fn for_kind(kind: BillingProviderKind) -> Self {
        Self {
            kind,
            api_key_ref: None,
            webhook_secret_ref: None,
            environment: None,
            price_config_ref: None,
        }
    }

    pub fn with_api_key_ref(mut self, r: impl Into<String>) -> Self {
        self.api_key_ref = Some(r.into());
        self
    }

    pub fn with_webhook_secret_ref(mut self, r: impl Into<String>) -> Self {
        self.webhook_secret_ref = Some(r.into());
        self
    }

    pub fn with_environment(mut self, env: impl Into<String>) -> Self {
        self.environment = Some(env.into());
        self
    }

    /// Validate domain invariants before use.
    /// Returns readiness + optional reason when invalid.
    pub fn validate(&self) -> (ProviderReadiness, Option<String>) {
        match self.kind {
            BillingProviderKind::Manual | BillingProviderKind::Test => {
                // manual/test never requires external secrets
                if self.api_key_ref.is_some()
                    && self
                        .api_key_ref
                        .as_deref()
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(false)
                {
                    return (
                        ProviderReadiness::InvalidConfiguration,
                        Some("api_key_ref must not be empty".into()),
                    );
                }
                (ProviderReadiness::ConfiguredAndReady, None)
            }
            BillingProviderKind::Stripe | BillingProviderKind::Paddle => {
                // external provider requires references, and must look like identifiers not secrets
                let api = self.api_key_ref.as_deref().map(|s| s.trim()).unwrap_or("");
                let wh = self
                    .webhook_secret_ref
                    .as_deref()
                    .map(|s| s.trim())
                    .unwrap_or("");
                if api.is_empty() && wh.is_empty() {
                    return (
                        ProviderReadiness::NotConfigured,
                        Some("no provider credentials configured".into()),
                    );
                }
                if api.is_empty() || wh.is_empty() {
                    return (
                        ProviderReadiness::InvalidConfiguration,
                        Some(
                            "both api_key_ref and webhook_secret_ref required for stripe/paddle"
                                .into(),
                        ),
                    );
                }
                // Must look like env var / reference, not raw secret
                for r in [api, wh] {
                    if is_secret_value(r) {
                        return (
                            ProviderReadiness::InvalidConfiguration,
                            Some(format!(
                                "reference looks like secret value: {}",
                                &r[..r.len().min(20)]
                            )),
                        );
                    }
                    if !is_reference_like(r) {
                        return (
                            ProviderReadiness::InvalidConfiguration,
                            Some(format!("reference must be env var or ARN-like: {}", r)),
                        );
                    }
                }
                // Configured — reachability is runtime concern (network), not domain validation
                (ProviderReadiness::ConfiguredButUnreachable, None)
            }
        }
    }

    /// Runtime readiness given env resolution result.
    /// `env_present` indicates whether the referenced env vars actually resolve in the current process.
    /// `reachable` indicates network probe success.
    pub fn runtime_readiness(&self, env_present: bool, reachable: bool) -> ProviderReadiness {
        let (base, _) = self.validate();
        match base {
            ProviderReadiness::InvalidConfiguration | ProviderReadiness::NotConfigured => base,
            ProviderReadiness::ConfiguredAndReady => ProviderReadiness::ConfiguredAndReady,
            ProviderReadiness::ConfiguredButUnreachable => {
                if !env_present {
                    ProviderReadiness::ConfiguredButUnreachable
                } else if reachable {
                    ProviderReadiness::ConfiguredAndReady
                } else {
                    ProviderReadiness::ConfiguredButUnreachable
                }
            }
        }
    }
}

fn is_reference_like(s: &str) -> bool {
    // env var name, ARN, or path-like
    if s.contains("://") {
        return true;
    }
    if s.starts_with("arn:") {
        return true;
    }
    if s.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && s.len() <= 64
    {
        return true;
    }
    if s.contains('/') || s.contains(':') {
        return true;
    }
    // also allow STRIPE_API_KEY style
    s.len() <= 128 && !s.contains(' ')
}

fn is_secret_value(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    if lower.contains("sk_live") || lower.contains("sk_test") {
        return true;
    }
    if s.len() > 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || c == '_' || c == '-')
    {
        return true;
    }
    if s.starts_with("whsec_") && s.len() > 32 {
        return true;
    }
    false
}

impl fmt::Debug for BillingProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillingProviderConfig")
            .field("kind", &self.kind)
            .field("has_api_key_ref", &self.api_key_ref.is_some())
            .field("has_webhook_secret_ref", &self.webhook_secret_ref.is_some())
            .field("environment", &self.environment)
            .finish()
    }
}

impl fmt::Display for BillingProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "billing_provider={} readiness={}",
            self.kind,
            self.validate().0.as_str()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_is_ready_without_credentials() {
        let c = BillingProviderConfig::manual();
        assert_eq!(c.validate().0, ProviderReadiness::ConfiguredAndReady);
    }

    #[test]
    fn stripe_not_configured_when_empty() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Stripe);
        assert_eq!(c.validate().0, ProviderReadiness::NotConfigured);
    }

    #[test]
    fn stripe_invalid_when_partial() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Stripe)
            .with_api_key_ref("STRIPE_API_KEY");
        assert_eq!(c.validate().0, ProviderReadiness::InvalidConfiguration);
    }

    #[test]
    fn stripe_configured_but_unreachable_when_refs_present() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Stripe)
            .with_api_key_ref("STRIPE_API_KEY")
            .with_webhook_secret_ref("STRIPE_WEBHOOK_SECRET");
        assert_eq!(c.validate().0, ProviderReadiness::ConfiguredButUnreachable);
    }

    #[test]
    fn stripe_invalid_when_raw_secret() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Stripe)
            .with_api_key_ref(
                "sk_live_51Hxxx_secret_value_long_enough_to_be_secret_xxxxxxxxxxxxxxxx",
            )
            .with_webhook_secret_ref("STRIPE_WEBHOOK_SECRET");
        assert_eq!(c.validate().0, ProviderReadiness::InvalidConfiguration);
    }

    #[test]
    fn runtime_ready_when_env_and_reachable() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Stripe)
            .with_api_key_ref("STRIPE_API_KEY")
            .with_webhook_secret_ref("STRIPE_WEBHOOK_SECRET");
        assert_eq!(
            c.runtime_readiness(true, true),
            ProviderReadiness::ConfiguredAndReady
        );
        assert_eq!(
            c.runtime_readiness(true, false),
            ProviderReadiness::ConfiguredButUnreachable
        );
        assert_eq!(
            c.runtime_readiness(false, false),
            ProviderReadiness::ConfiguredButUnreachable
        );
    }

    #[test]
    fn debug_never_exposes_secret() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Stripe)
            .with_api_key_ref("STRIPE_API_KEY")
            .with_webhook_secret_ref("STRIPE_WEBHOOK_SECRET");
        let dbg = format!("{:?}", c);
        assert!(!dbg.contains("sk_live"));
        assert!(dbg.contains("has_api_key_ref"));
    }

    #[test]
    fn kind_parse_roundtrip() {
        for k in BillingProviderKind::ALL {
            assert_eq!(BillingProviderKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(BillingProviderKind::parse("unknown"), None);
    }

    #[test]
    fn test_provider_ready() {
        let c = BillingProviderConfig::for_kind(BillingProviderKind::Test);
        assert_eq!(c.validate().0, ProviderReadiness::ConfiguredAndReady);
    }
}
