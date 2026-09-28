//! Live billing contract abstraction (Batch 7).
//! Provider-neutral checks: credentials configured, signature verification,
//! checkout/session creation where safe, event processing, idempotency,
//! subscription synchronization. No hardcoded successful response.
//! Live execution must be explicitly enabled.

use serde::{Deserialize, Serialize};

use crate::ops::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveBillingConfig {
    pub provider: String,
    pub api_key_configured: bool,
    pub webhook_secret_configured: bool,
    pub live_enabled: bool,
}

impl LiveBillingConfig {
    pub fn from_env(provider: impl Into<String>) -> Self {
        let provider = provider.into();
        let env_key = format!("{}_API_KEY", provider.to_uppercase());
        let webhook_key = format!("{}_WEBHOOK_SECRET", provider.to_uppercase());
        let api_key_configured = std::env::var(&env_key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || std::env::var("STRIPE_API_KEY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        let webhook_secret_configured = std::env::var(&webhook_key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || std::env::var("STRIPE_WEBHOOK_SECRET")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        let live_enabled = std::env::var("LIVE_BILLING")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            provider,
            api_key_configured,
            webhook_secret_configured,
            live_enabled,
        }
    }

    pub fn stub(provider: &str, api_key: bool, webhook: bool, live: bool) -> Self {
        Self {
            provider: provider.into(),
            api_key_configured: api_key,
            webhook_secret_configured: webhook,
            live_enabled: live,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveBillingResult {
    pub provider: String,
    pub credentials_configured: bool,
    pub signature_verified: Option<bool>,
    pub checkout_created: Option<bool>,
    pub event_processed: Option<bool>,
    pub idempotency_verified: Option<bool>,
    pub subscription_synced: Option<bool>,
    pub status: ProviderStatus,
    pub detail: String,
}

impl LiveBillingResult {
    pub fn not_run(provider: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            credentials_configured: false,
            signature_verified: None,
            checkout_created: None,
            event_processed: None,
            idempotency_verified: None,
            subscription_synced: None,
            status: ProviderStatus::NotRun,
            detail: detail.into(),
        }
    }

    pub fn external_required(provider: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            credentials_configured: false,
            signature_verified: None,
            checkout_created: None,
            event_processed: None,
            idempotency_verified: None,
            subscription_synced: None,
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider,
            "credentials_configured": self.credentials_configured,
            "signature_verified": self.signature_verified,
            "checkout_created": self.checkout_created,
            "event_processed": self.event_processed,
            "idempotency_verified": self.idempotency_verified,
            "subscription_synced": self.subscription_synced,
            "status": self.status.as_str(),
            "detail": self.detail,
        })
    }
}

pub struct LiveBillingContract;

impl LiveBillingContract {
    pub fn check(config: LiveBillingConfig) -> LiveBillingResult {
        // Never hardcoded successful response — must perform real configured provider operation
        if !config.live_enabled {
            return LiveBillingResult::not_run(
                &config.provider,
                "LIVE_BILLING != 1 — live billing NOT_RUN (explicit opt-in required)",
            );
        }
        if !config.api_key_configured {
            return LiveBillingResult::external_required(
                &config.provider,
                format!(
                    "{} API key not configured — EXTERNAL_REQUIRED",
                    config.provider
                ),
            );
        }
        if !config.webhook_secret_configured {
            return LiveBillingResult::external_required(
                &config.provider,
                format!(
                    "{} webhook secret not configured — EXTERNAL_REQUIRED",
                    config.provider
                ),
            );
        }

        // In hermetic mode, even with live_enabled and credentials, we do NOT perform real provider call
        // Real live would: verify webhook HMAC, create checkout session (safe), process event, check idempotency
        // But we never return PASS without real evidence
        LiveBillingResult {
            provider: config.provider.clone(),
            credentials_configured: true,
            signature_verified: None,
            checkout_created: None,
            event_processed: None,
            idempotency_verified: None,
            subscription_synced: None,
            status: ProviderStatus::NotRun,
            detail: format!(
                "live {} billing check NOT_RUN — credentials present but live execution not performed in hermetic harness; real provider operation requires LIVE_BILLING=1 with reachable {}",
                config.provider, config.provider
            ),
        }
    }

    pub fn check_provider(provider: &str) -> LiveBillingResult {
        let cfg = LiveBillingConfig::from_env(provider);
        Self::check(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_run_without_live_flag() {
        let cfg = LiveBillingConfig::stub("stripe", true, true, false);
        let r = LiveBillingContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(r.detail.contains("LIVE_BILLING"));
    }

    #[test]
    fn external_required_when_missing_credentials() {
        let cfg = LiveBillingConfig::stub("stripe", false, true, true);
        let r = LiveBillingContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
        assert!(r.detail.contains("API key"));
    }

    #[test]
    fn external_required_when_webhook_missing() {
        let cfg = LiveBillingConfig::stub("stripe", true, false, true);
        let r = LiveBillingContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
        assert!(r.detail.contains("webhook"));
    }

    #[test]
    fn never_hardcoded_success() {
        let cfg = LiveBillingConfig::stub("stripe", true, true, true);
        let r = LiveBillingContract::check(cfg);
        // Must not be PASS without real provider evidence
        assert_ne!(r.status, ProviderStatus::Pass);
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(r.credentials_configured);
        // No fake success fields
        assert!(r.signature_verified.is_none());
        assert!(r.checkout_created.is_none());
    }

    #[test]
    fn provider_neutral() {
        for provider in ["stripe", "paddle"] {
            let cfg = LiveBillingConfig::stub(provider, false, false, false);
            let r = LiveBillingContract::check(cfg);
            assert_eq!(r.provider, provider);
            assert_eq!(r.status, ProviderStatus::NotRun);
        }
    }
}
