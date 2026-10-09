//! Provider registry with real API boundaries — selects Stripe/Paddle/Manual adapters
//! with typed configuration errors (no silent fallback to Manual).
//!
//! Secrets are never logged.

use crate::billing::{paddle_adapter::PaddleAdapter, stripe_adapter::StripeAdapter};

#[derive(Debug, Clone)]
pub enum RegistryError {
    NotConfigured(String),
    Transport(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NotConfigured(m) => write!(f, "provider not configured: {m}"),
            RegistryError::Transport(m) => write!(f, "provider transport: {m}"),
        }
    }
}
impl std::error::Error for RegistryError {}

/// Resolve an adapter for `provider`, checking that its required environment
/// configuration is present. Returns typed NotConfigured error when the
/// provider was explicitly requested but not configured — never silent fallback.
pub fn registry_for(
    provider: bot_core::billing::provider::BillingProviderKind,
) -> Result<RegistryAdapter, RegistryError> {
    match provider {
        bot_core::billing::provider::BillingProviderKind::Manual => Ok(RegistryAdapter::Manual),
        bot_core::billing::provider::BillingProviderKind::Stripe => StripeAdapter::from_env()
            .map(RegistryAdapter::Stripe)
            .map_err(|e| match e {
                crate::billing::stripe_adapter::StripeError::NotConfigured(msg) => {
                    RegistryError::NotConfigured(msg)
                }
                crate::billing::stripe_adapter::StripeError::Transport(msg) => {
                    RegistryError::Transport(msg)
                }
                crate::billing::stripe_adapter::StripeError::Verification(msg) => {
                    RegistryError::Transport(msg)
                }
            }),
        bot_core::billing::provider::BillingProviderKind::Paddle => PaddleAdapter::from_env()
            .map(RegistryAdapter::Paddle)
            .map_err(|e| match e {
                crate::billing::paddle_adapter::PaddleError::NotConfigured(msg) => {
                    RegistryError::NotConfigured(msg)
                }
                crate::billing::paddle_adapter::PaddleError::Transport(msg) => {
                    RegistryError::Transport(msg)
                }
                crate::billing::paddle_adapter::PaddleError::Verification(msg) => {
                    RegistryError::Transport(msg)
                }
            }),
    }
}

pub enum RegistryAdapter {
    Manual,
    Stripe(StripeAdapter),
    Paddle(PaddleAdapter),
}

impl RegistryAdapter {
    pub fn provider_name(&self) -> &'static str {
        match self {
            RegistryAdapter::Manual => "manual",
            RegistryAdapter::Stripe(_) => "stripe",
            RegistryAdapter::Paddle(_) => "paddle",
        }
    }
    pub fn is_manual(&self) -> bool {
        matches!(self, RegistryAdapter::Manual)
    }
}

/// Build a registry view that reports which providers are configured.
/// Useful for callers that want to check `is_some()` without typed error.
pub fn configured_providers() -> Vec<&'static str> {
    let mut v = vec!["manual"];
    if StripeAdapter::from_env().is_ok() {
        v.push("stripe");
    }
    if PaddleAdapter::from_env().is_ok() {
        v.push("paddle");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::billing::provider::BillingProviderKind;

    #[test]
    fn stripe_request_without_config_is_typed_error_not_manual_fallback() {
        let prev = std::env::var("STRIPE_API_KEY").ok();
        std::env::remove_var("STRIPE_API_KEY");
        let err = registry_for(BillingProviderKind::Stripe);
        assert!(matches!(err, Err(RegistryError::NotConfigured(_))));
        if let Err(RegistryError::NotConfigured(msg)) = &err {
            assert!(msg.contains("STRIPE_API_KEY"), "{msg}");
        }
        assert!(err.is_err());
        if let Some(v) = prev {
            std::env::set_var("STRIPE_API_KEY", v);
        }
    }

    #[test]
    fn paddle_request_without_config_is_typed_error_not_manual_fallback() {
        let prev = std::env::var("PADDLE_API_KEY").ok();
        std::env::remove_var("PADDLE_API_KEY");
        let err = registry_for(BillingProviderKind::Paddle);
        assert!(matches!(err, Err(RegistryError::NotConfigured(_))));
        if let Some(v) = prev {
            std::env::set_var("PADDLE_API_KEY", v);
        }
    }

    #[test]
    fn manual_always_succeeds() {
        let r = registry_for(BillingProviderKind::Manual).expect("manual must always be available");
        assert!(r.is_manual());
        assert_eq!(r.provider_name(), "manual");
    }

    #[test]
    fn configured_providers_reflects_env() {
        let prev_s = std::env::var("STRIPE_API_KEY").ok();
        let prev_p = std::env::var("PADDLE_API_KEY").ok();
        std::env::remove_var("STRIPE_API_KEY");
        std::env::remove_var("PADDLE_API_KEY");
        let v = configured_providers();
        assert!(v.contains(&"manual"));
        assert!(!v.contains(&"stripe"));
        assert!(!v.contains(&"paddle"));
        if let Some(v) = prev_s {
            std::env::set_var("STRIPE_API_KEY", v);
        }
        if let Some(v) = prev_p {
            std::env::set_var("PADDLE_API_KEY", v);
        }
    }

    #[test]
    #[ignore]
    fn live_stripe_requires_real_keys() {
        let live = std::env::var("LIVE_BILLING").unwrap_or_default() == "1"
            && std::env::var("STRIPE_API_KEY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        assert!(
            live,
            "live registry test requires LIVE_BILLING=1 and STRIPE_API_KEY"
        );
        let adapter =
            registry_for(BillingProviderKind::Stripe).expect("live stripe must be configured");
        assert_eq!(adapter.provider_name(), "stripe");
    }
}
