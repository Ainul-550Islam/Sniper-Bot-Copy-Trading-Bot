//! AWS KMS readiness / permission checks (§H, spec file 56).
//!
//! Ordered, real probes — no fabricated "healthy":
//!
//! 1. **References** — a key id (deployment default or signer
//!    `provider_ref`) and AWS credentials from the standard environment
//!    chain are present;
//! 2. **Permission + readiness** — `GetPublicKey` on the resolved key
//!    succeeds (proves endpoint reachability, SigV4 credentials, and the
//!    `kms:GetPublicKey` grant in one authenticated call);
//! 3. **Key type** — the key spec is `ECC_ED25519` (Ed25519), which is
//!    what Solana signing requires and what KMS supports since Nov 2025.
//!
//! The result is a secret-free `KmsReadiness` mapping onto the core
//! `ProviderHealth` used by the boundary and health endpoints.

use bot_core::custody::{HealthState, ProviderHealth, ProviderType, SignerRecord};
use chrono::Utc;

use crate::custody::kms::client::{
    AwsEnvCredentials, KmsClient, KmsClientError, AWS_KMS_DEPENDENCY,
};
use crate::custody::kms::config::KmsConfig;

/// The full, ordered result of a KMS readiness probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmsReadiness {
    /// Key id and AWS credentials are present.
    pub references_present: bool,
    /// An authenticated `GetPublicKey` call succeeded.
    pub reachable_and_authorized: bool,
    /// The key is an Ed25519 (`ECC_ED25519`) key.
    pub key_is_ed25519: bool,
    /// Secret-free detail for the failed stage (or the success summary).
    pub detail: String,
}

impl KmsReadiness {
    /// True only when every probe passed.
    pub fn is_ready(&self) -> bool {
        self.references_present && self.reachable_and_authorized && self.key_is_ed25519
    }

    /// Honest health-state mapping:
    /// * ready → `Reachable`;
    /// * authenticated but wrong key type → `Degraded`;
    /// * references missing or KMS unreachable → `Unavailable`.
    pub fn health_state(&self) -> HealthState {
        if self.is_ready() {
            HealthState::Reachable
        } else if self.reachable_and_authorized {
            HealthState::Degraded
        } else {
            HealthState::Unavailable
        }
    }

    /// Render as the core `ProviderHealth`.
    pub fn to_provider_health(&self) -> ProviderHealth {
        ProviderHealth::new(
            ProviderType::Kms,
            self.health_state(),
            self.detail.clone(),
            Utc::now(),
        )
    }
}

/// Run the full ordered probe for a specific signer's key (or the
/// deployment default). Never panics, never logs secrets.
pub async fn check_kms_readiness(
    config: &KmsConfig,
    signer: Option<&SignerRecord>,
) -> KmsReadiness {
    let key_id = match signer {
        Some(record) => config.key_id_for(record),
        None => config.key_id().map(|s| s.to_string()),
    }
    .filter(|id| !id.is_empty());

    // Stage 1 — references.
    let credentials = AwsEnvCredentials::from_env();
    if key_id.is_none() || credentials.is_err() {
        let mut missing = Vec::new();
        if key_id.is_none() {
            missing.push("KMS_KEY_ID (or signer provider_ref)");
        }
        if credentials.is_err() {
            missing.push(
                "AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY (standard AWS env credential chain)",
            );
        }
        return KmsReadiness {
            references_present: false,
            reachable_and_authorized: false,
            key_is_ed25519: false,
            detail: format!(
                "kms references missing: {}; requires {AWS_KMS_DEPENDENCY}",
                missing.join(", ")
            ),
        };
    }
    let key_id = key_id.expect("checked above");

    // Stage 2 + 3 — authenticated GetPublicKey + key type.
    let client = KmsClient::new(config);
    match client.get_public_key(&key_id).await {
        Ok(public) if public.key_spec.eq_ignore_ascii_case("ECC_ED25519") => KmsReadiness {
            references_present: true,
            reachable_and_authorized: true,
            key_is_ed25519: true,
            detail: format!(
                "kms reachable and authorized, key '{key_id}' is ECC_ED25519 and ready for ed25519 signing"
            ),
        },
        Ok(public) => KmsReadiness {
            references_present: true,
            reachable_and_authorized: true,
            key_is_ed25519: false,
            detail: format!(
                "kms key '{key_id}' has KeySpec {} but this boundary signs Solana ed25519 messages (ECC_ED25519 required); requires {AWS_KMS_DEPENDENCY}",
                public.key_spec
            ),
        },
        Err(err) => {
            let state = health_state_for_client_error(&err);
            KmsReadiness {
                references_present: true,
                reachable_and_authorized: false,
                key_is_ed25519: false,
                detail: format!(
                    "kms probe failed ({}): {err}; requires {AWS_KMS_DEPENDENCY}",
                    state.as_str()
                ),
            }
        }
    }
}

/// Deployment-level probe (default key, no specific signer).
pub async fn deployment_readiness(config: &KmsConfig) -> KmsReadiness {
    check_kms_readiness(config, None).await
}

/// Map a client error to an honest health state.
pub fn health_state_for_client_error(err: &KmsClientError) -> HealthState {
    match err.code {
        "kms_unreachable" => HealthState::Unavailable,
        "kms_credentials_missing" => HealthState::Unavailable,
        _ => HealthState::Degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_without_configuration_is_unavailable_not_fake_healthy() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KMS_KEY_ID");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        let (config, state) = KmsConfig::from_env();
        assert!(!state.ready());
        drop(_lock); // release the env lock before awaiting
        let readiness = deployment_readiness(&config).await;
        assert!(!readiness.is_ready());
        assert_eq!(readiness.health_state(), HealthState::Unavailable);
        assert!(readiness.detail.contains("KMS_KEY_ID"));
        let health = readiness.to_provider_health();
        assert!(!health.is_signing_allowed());
    }

    #[tokio::test]
    async fn probe_of_unreachable_kms_is_unavailable_with_dependency_named() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KMS_KEY_ID", "1234abcd-12ab-34cd-56ef-1234567890ab");
        std::env::set_var("KMS_REGION", "us-east-1");
        std::env::set_var("KMS_ENDPOINT", "http://127.0.0.1:1");
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
        std::env::set_var(
            "AWS_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        let (config, state) = KmsConfig::from_env();
        assert!(state.ready());
        drop(_lock); // release the env lock before awaiting
        let readiness = deployment_readiness(&config).await;
        assert!(!readiness.is_ready());
        assert_eq!(readiness.health_state(), HealthState::Unavailable);
        assert!(readiness.detail.contains("kms probe failed"));
        // Secret never leaks through readiness detail.
        assert!(!readiness.detail.contains("wJalrXUtnFEMI"));
        std::env::remove_var("KMS_KEY_ID");
        std::env::remove_var("KMS_REGION");
        std::env::remove_var("KMS_ENDPOINT");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
    }
}
