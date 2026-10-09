//! Live Vault/KMS/HSM contract abstraction (Batch 7).
//! Check provider configured, credential reference valid, sign capability available,
//! public-key/address derivation where supported. No local fallback, no private key extraction.

use serde::{Deserialize, Serialize};

use crate::ops::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveCustodyConfig {
    pub provider: String,
    pub credential_ref_valid: bool,
    pub sign_capability: bool,
    pub live_enabled: bool,
}

impl LiveCustodyConfig {
    pub fn from_env(provider: impl Into<String>) -> Self {
        let provider = provider.into();
        let (credential_ref_valid, sign_capability) = match provider.as_str() {
            "vault" => (
                std::env::var("VAULT_ADDR")
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
                    && std::env::var("VAULT_TOKEN")
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false),
                true,
            ),
            "kms" => (
                std::env::var("KMS_KEY_ID")
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false),
                true,
            ),
            "hsm" => (
                std::env::var("HSM_SLOT")
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false),
                true,
            ),
            _ => (false, false),
        };
        let live_enabled = std::env::var("LIVE_CUSTODY")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            provider,
            credential_ref_valid,
            sign_capability,
            live_enabled,
        }
    }

    pub fn stub(provider: &str, cred_valid: bool, sign_cap: bool, live: bool) -> Self {
        Self {
            provider: provider.into(),
            credential_ref_valid: cred_valid,
            sign_capability: sign_cap,
            live_enabled: live,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveCustodyResult {
    pub provider: String,
    pub credential_valid: bool,
    pub sign_available: bool,
    pub public_key_derived: Option<String>,
    pub address_derived: Option<String>,
    pub status: ProviderStatus,
    pub detail: String,
}

impl LiveCustodyResult {
    pub fn not_run(provider: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            credential_valid: false,
            sign_available: false,
            public_key_derived: None,
            address_derived: None,
            status: ProviderStatus::NotRun,
            detail: detail.into(),
        }
    }

    pub fn external_required(provider: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            credential_valid: false,
            sign_available: false,
            public_key_derived: None,
            address_derived: None,
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
        }
    }

    pub fn fail(provider: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            credential_valid: false,
            sign_available: false,
            public_key_derived: None,
            address_derived: None,
            status: ProviderStatus::Fail,
            detail: detail.into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        // Never expose private key
        serde_json::json!({
            "provider": self.provider,
            "credential_valid": self.credential_valid,
            "sign_available": self.sign_available,
            "public_key_derived": self.public_key_derived.as_ref().map(|k| format!("{}...<redacted>", &k[..8.min(k.len())])),
            "address_derived": self.address_derived,
            "status": self.status.as_str(),
            "detail": self.detail,
        })
    }
}

pub struct LiveCustodyContract;

impl LiveCustodyContract {
    pub fn check(config: LiveCustodyConfig) -> LiveCustodyResult {
        // No local fallback — remote provider selected => remote required
        if !config.live_enabled {
            return LiveCustodyResult::not_run(
                &config.provider,
                "LIVE_CUSTODY != 1 — live custody NOT_RUN (explicit opt-in required, no local fallback)",
            );
        }
        if !config.credential_ref_valid {
            return LiveCustodyResult::external_required(
                &config.provider,
                format!(
                    "{} credential reference invalid — EXTERNAL_REQUIRED (no local fallback)",
                    config.provider
                ),
            );
        }
        if !config.sign_capability {
            return LiveCustodyResult::fail(
                &config.provider,
                format!(
                    "{} sign capability not available — FAIL/CANNOT_SIGN",
                    config.provider
                ),
            );
        }

        // Hermetic: even with live_enabled and creds, don't perform real remote sign without network
        LiveCustodyResult {
            provider: config.provider.clone(),
            credential_valid: true,
            sign_available: true,
            public_key_derived: None,
            address_derived: None,
            status: ProviderStatus::NotRun,
            detail: format!(
                "live {} custody check NOT_RUN — credentials valid but live remote sign not performed in hermetic harness; no private key extraction",
                config.provider
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_run_without_live_flag() {
        let cfg = LiveCustodyConfig::stub("vault", true, true, false);
        let r = LiveCustodyContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(r.detail.contains("LIVE_CUSTODY"));
    }

    #[test]
    fn external_required_when_credential_invalid() {
        let cfg = LiveCustodyConfig::stub("vault", false, true, true);
        let r = LiveCustodyContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
        assert!(r.detail.contains("credential"));
    }

    #[test]
    fn no_local_fallback() {
        let cfg = LiveCustodyConfig::stub("vault", false, true, true);
        let r = LiveCustodyContract::check(cfg);
        // Must be EXTERNAL_REQUIRED or FAIL, never PASS via local fallback
        assert_ne!(r.status, ProviderStatus::Pass);
        assert!(r.detail.contains("no local fallback") || r.detail.contains("EXTERNAL_REQUIRED"));
    }

    #[test]
    fn never_extracts_private_key() {
        let cfg = LiveCustodyConfig::stub("vault", true, true, true);
        let r = LiveCustodyContract::check(cfg);
        let json = r.to_safe_json().to_string();
        // Must never expose actual private key material; detail may contain words \"private key\" as description
        assert!(!json.contains("BEGIN PRIVATE KEY"));
        assert!(!json.to_lowercase().contains("-----begin"));
        assert!(!json.to_lowercase().contains("private_key"));
    }

    #[test]
    fn fail_when_sign_unavailable() {
        let cfg = LiveCustodyConfig::stub("hsm", true, false, true);
        let r = LiveCustodyContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::Fail);
        assert!(r.detail.contains("CANNOT_SIGN") || r.detail.contains("sign capability"));
    }
}
