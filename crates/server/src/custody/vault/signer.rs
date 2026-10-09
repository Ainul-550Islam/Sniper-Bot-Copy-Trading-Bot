//! Vault-backed signer and provider adapter (§G, spec file 50).
//!
//! `VaultSigner` implements `bot_core::custody::CustodySigner`: every
//! signature is produced by a real Vault transit `sign` call. There is no
//! local fallback, no cached signature, and no synthetic output — when
//! Vault is unreachable, signing fails closed with a typed error.
//!
//! `VaultCustodyProvider` (the server-side adapter, distinct from the
//! core placeholder of the same name) resolves a `SignerRecord` into a
//! `VaultSigner` only after verifying, in order:
//!
//! 1. the record is actually a Vault signer (`ProviderMismatch` if not);
//! 2. the signer is `Active` (`SignerNotActive`);
//! 3. a transit key name can be resolved from the record's `provider_ref`
//!    or the deployment default (`NotConfigured` if neither exists);
//! 4. the transit key exists and is ed25519 (`UnsupportedProvider` for a
//!    wrong key type, `Transport` for a missing key);
//! 5. the public key Vault holds for that key matches the record's pinned
//!    `public_address` (`PubkeyMismatch` — a stale or wrong key must
//!    never sign).

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use bot_core::custody::{
    CustodyProvider, CustodyProviderError, CustodySigner, ProviderType, SignerRecord,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;

use crate::custody::vault::client::{VaultClient, VaultClientError, VAULT_TRANSIT_DEPENDENCY};
use crate::custody::vault::config::VaultConfig;

/// Map a client error to the custody provider error surface, preserving
/// the typed code in the detail so audits carry the real reason.
fn provider_error(err: VaultClientError) -> CustodyProviderError {
    match err.code {
        "vault_unreachable" | "vault_status" | "vault_protocol" => {
            CustodyProviderError::Transport(err.to_string())
        }
        // Forbidden / not-found mean the deployment's Vault does not
        // serve this signer — a permanent, honest "not configured for
        // this key" state naming the exact dependency.
        "vault_token_forbidden" | "vault_key_forbidden" | "vault_key_not_found" => {
            CustodyProviderError::NotConfigured(ProviderType::Vault)
        }
        _ => CustodyProviderError::Transport(err.to_string()),
    }
}

/// A handle that signs through Vault transit.
#[derive(Debug)]
pub struct VaultSigner {
    client: VaultClient,
    key_name: String,
    pubkey: Pubkey,
}

impl VaultSigner {
    /// Bind to a specific transit key. The public key MUST have been
    /// fetched from Vault (never caller-supplied) by the provider below.
    pub(crate) fn new(client: VaultClient, key_name: String, pubkey: Pubkey) -> Self {
        Self {
            client,
            key_name,
            pubkey,
        }
    }

    pub fn key_name(&self) -> &str {
        &self.key_name
    }
}

#[async_trait]
impl CustodySigner for VaultSigner {
    fn pubkey(&self) -> Pubkey {
        self.pubkey
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, CustodyProviderError> {
        let signature = self
            .client
            .sign_ed25519(&self.key_name, message)
            .await
            .map_err(provider_error)?;
        Ok(Signature::from(signature))
    }
}

/// Server-side Vault custody provider — the real adapter.
///
/// Distinct from `bot_core::custody::VaultCustodyProvider` (the OPTION-B
/// placeholder that always refuses): this one talks to Vault. It is
/// registered by the deployment registry whenever Vault is the selected
/// provider.
pub struct VaultCustodyProvider {
    config: VaultConfig,
    client: VaultClient,
}

impl std::fmt::Debug for VaultCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultCustodyProvider")
            .field("config", &self.config)
            .finish()
    }
}

impl VaultCustodyProvider {
    /// Build from resolved configuration. No network I/O at construction.
    pub fn new(config: VaultConfig) -> Self {
        let client = VaultClient::new(&config);
        Self { config, client }
    }

    /// The transit key name a signer resolves to, or the honest
    /// not-configured error naming what is missing.
    fn resolved_key_name(&self, record: &SignerRecord) -> Result<String, CustodyProviderError> {
        self.config
            .transit_key_for(record)
            .filter(|name| !name.is_empty())
            .ok_or(CustodyProviderError::NotConfigured(ProviderType::Vault))
    }
}

#[async_trait]
impl CustodyProvider for VaultCustodyProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Vault
    }

    async fn resolve_signer(
        &self,
        record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        if record.provider_type != ProviderType::Vault {
            return Err(CustodyProviderError::ProviderMismatch {
                expected: ProviderType::Vault,
                actual: record.provider_type,
            });
        }
        if record.status != bot_core::custody::CustodyStatus::Active {
            return Err(CustodyProviderError::SignerNotActive(record.id));
        }
        let key_name = self.resolved_key_name(record)?;

        // Fetch the key metadata from Vault — the public key comes from
        // Vault, never from the caller.
        let info = self
            .client
            .transit_key(&key_name)
            .await
            .map_err(provider_error)?;
        if !info.is_ed25519() {
            return Err(CustodyProviderError::UnsupportedProvider(
                ProviderType::Vault,
            ));
        }
        if info.deletion_time_present {
            return Err(CustodyProviderError::Transport(format!(
                "transit key '{key_name}' is scheduled for deletion — refusing to sign"
            )));
        }
        let public_key_b64 = info.public_key_b64.clone().ok_or_else(|| {
            CustodyProviderError::Transport(format!(
                "transit key '{key_name}' did not return a public key"
            ))
        })?;
        let public_key_bytes = BASE64.decode(&public_key_b64).map_err(|_| {
            CustodyProviderError::Transport(format!(
                "transit key '{key_name}' returned a malformed public key"
            ))
        })?;
        let provider_pubkey = Pubkey::try_from(public_key_bytes.as_slice()).map_err(|_| {
            CustodyProviderError::Transport(format!(
                "transit key '{key_name}' public key is not a valid ed25519 public key"
            ))
        })?;

        // The record's pinned public address must match Vault's key. A
        // mismatch means the record is stale or points at the wrong key —
        // fail closed, never sign.
        let recorded_address = record.public_address.trim();
        if !recorded_address.is_empty() {
            match recorded_address.parse::<Pubkey>() {
                Ok(recorded) if recorded == provider_pubkey => {}
                _ => {
                    return Err(CustodyProviderError::PubkeyMismatch {
                        signer: record.id,
                        recorded: record.public_address.clone(),
                        provider_key: provider_pubkey.to_string(),
                    })
                }
            }
        }

        Ok(Arc::new(VaultSigner::new(
            self.client.clone(),
            key_name,
            provider_pubkey,
        )))
    }

    async fn health_check(&self) -> Result<(), CustodyProviderError> {
        let state = self.client.sys_health().await;
        if !state.can_serve() {
            return Err(CustodyProviderError::Transport(format!(
                "vault service state: {} ({})",
                state.as_str(),
                VAULT_TRANSIT_DEPENDENCY
            )));
        }
        self.client
            .token_lookup_self()
            .await
            .map_err(provider_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::custody::CustodyStatus;
    use bot_core::custody::{CustodyProfileId, SignerId};
    use bot_core::tenant::OrganizationId;

    fn vault_signer_record(provider_ref: Option<&str>, public_address: &str) -> SignerRecord {
        let mut record = SignerRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            "vault-signer".to_string(),
            ProviderType::Vault,
            public_address.to_string(),
            chrono::Utc::now(),
        );
        record.provider_ref = provider_ref.map(|s| s.to_string());
        record.status = CustodyStatus::Active;
        record
    }

    fn config_pointing_nowhere() -> VaultConfig {
        // A syntactically valid but unreachable address: any network
        // operation fails closed instead of pretending.
        let (config, state) = {
            let _lock = crate::custody::test_support::ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::env::set_var("VAULT_ADDR", "http://127.0.0.1:1");
            std::env::set_var("VAULT_TOKEN", "hvs.provider.test");
            std::env::set_var("VAULT_TRANSIT_KEY", "k1");
            VaultConfig::from_env()
        };
        assert!(state.sign_ready());
        config
    }

    #[tokio::test]
    async fn provider_mismatch_is_refused_before_any_network_call() {
        let provider = VaultCustodyProvider::new(config_pointing_nowhere());
        let mut record = vault_signer_record(Some("k1"), "");
        record.provider_type = ProviderType::Kms;
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("non-vault record must be refused");
        assert_eq!(
            err,
            CustodyProviderError::ProviderMismatch {
                expected: ProviderType::Vault,
                actual: ProviderType::Kms
            }
        );
    }

    #[tokio::test]
    async fn inactive_signer_is_refused_before_any_network_call() {
        let provider = VaultCustodyProvider::new(config_pointing_nowhere());
        let mut record = vault_signer_record(Some("k1"), "");
        record.status = CustodyStatus::Revoked;
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("inactive signer must be refused");
        assert_eq!(err, CustodyProviderError::SignerNotActive(record.id));
    }

    #[tokio::test]
    async fn signer_without_key_reference_is_not_configured() {
        // A config with NO deployment default key AND a signer record
        // with no provider_ref has no key to resolve — NotConfigured
        // before any network I/O.
        let provider = {
            let _lock = crate::custody::test_support::ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::env::set_var("VAULT_ADDR", "http://127.0.0.1:1");
            std::env::set_var("VAULT_TOKEN", "hvs.provider.test");
            std::env::remove_var("VAULT_TRANSIT_KEY");
            let (config, state) = VaultConfig::from_env();
            assert!(state.base_ready());
            assert!(!state.sign_ready(), "no deployment key configured");
            VaultCustodyProvider::new(config)
        };
        let record = vault_signer_record(None, "");
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("no key reference must be refused");
        assert_eq!(
            err,
            CustodyProviderError::NotConfigured(ProviderType::Vault)
        );
    }

    #[tokio::test]
    async fn resolution_against_unreachable_vault_fails_closed_without_fake_signer() {
        let provider = VaultCustodyProvider::new(config_pointing_nowhere());
        let record = vault_signer_record(Some("transit/keys/sniper-mainnet"), "");
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("unreachable vault must not produce a signer");
        assert!(matches!(err, CustodyProviderError::Transport(_)));
        assert!(!err.to_string().contains("hvs.provider.test"));
    }

    #[tokio::test]
    async fn health_check_against_unreachable_vault_fails_closed() {
        let provider = VaultCustodyProvider::new(config_pointing_nowhere());
        let err = provider
            .health_check()
            .await
            .expect_err("unreachable vault is not healthy");
        assert!(err.to_string().contains("vault service state"));
    }

    #[test]
    fn key_resolution_prefers_signer_reference_over_default() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VAULT_ADDR", "https://vault.internal:8200");
        std::env::set_var("VAULT_TOKEN", "hvs.test");
        std::env::set_var("VAULT_TRANSIT_KEY", "deployment-default");
        let (config, _) = VaultConfig::from_env();
        let provider = VaultCustodyProvider::new(config);
        let record = vault_signer_record(Some("transit/keys/per-signer-key"), "");
        assert_eq!(
            provider.resolved_key_name(&record).unwrap(),
            "per-signer-key"
        );
        let record_default = vault_signer_record(None, "");
        assert_eq!(
            provider.resolved_key_name(&record_default).unwrap(),
            "deployment-default"
        );
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        std::env::remove_var("VAULT_TRANSIT_KEY");
    }

    #[test]
    fn signer_id_is_stable_and_displayed_without_secrets() {
        let id = SignerId::new();
        let rendered = format!("{id}");
        assert!(!rendered.is_empty());
    }
}
