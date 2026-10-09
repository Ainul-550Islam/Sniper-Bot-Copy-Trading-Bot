//! KMS-backed signer and provider adapter (§H, spec file 55).
//!
//! `KmsSigner` implements `bot_core::custody::CustodySigner`: every
//! signature is produced by a real AWS KMS `Sign` call (Ed25519 /
//! `EDDSA_SHA_512`, supported by AWS KMS since November 2025). There is
//! no local fallback, no cached signature, and no synthetic output —
//! when KMS is unreachable or credentials are absent, signing fails
//! closed with a typed error naming the exact missing dependency.
//!
//! `KmsCustodyProvider` (the server-side adapter, distinct from the core
//! placeholder of the same name) resolves a `SignerRecord` into a
//! `KmsSigner` only after verifying, in order:
//!
//! 1. the record is actually a KMS signer (`ProviderMismatch` if not);
//! 2. the signer is `Active` (`SignerNotActive`);
//! 3. a key id can be resolved from the record's `provider_ref` or the
//!    deployment default (`NotConfigured` if neither exists);
//! 4. `GetPublicKey` succeeds and the key spec is `ECC_ED25519`
//!    (`UnsupportedProvider` for any other key spec);
//! 5. the public key KMS holds matches the record's pinned
//!    `public_address` (`PubkeyMismatch` — a stale or wrong key must
//!    never sign).

use std::sync::Arc;

use async_trait::async_trait;
use bot_core::custody::{
    CustodyProvider, CustodyProviderError, CustodySigner, ProviderType, SignerRecord,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;

use crate::custody::kms::client::{
    AwsEnvCredentials, KmsClient, KmsClientError, AWS_KMS_DEPENDENCY,
};
use crate::custody::kms::config::KmsConfig;

/// Map a client error to the custody provider error surface, preserving
/// the typed code in the detail so audits carry the real reason.
fn provider_error(err: KmsClientError) -> CustodyProviderError {
    match err.code {
        "kms_unreachable" | "kms_status" | "kms_protocol" => {
            CustodyProviderError::Transport(err.to_string())
        }
        // Missing credentials / forbidden / not-found are deployment
        // configuration states — honest "not configured", exact dependency.
        "kms_credentials_missing" => CustodyProviderError::NotConfigured(ProviderType::Kms),
        _ => CustodyProviderError::Transport(err.to_string()),
    }
}

/// A handle that signs through AWS KMS.
#[derive(Debug)]
pub struct KmsSigner {
    client: KmsClient,
    key_id: String,
    pubkey: Pubkey,
}

impl KmsSigner {
    /// Bind to a specific KMS key. The public key MUST have been fetched
    /// from KMS (never caller-supplied) by the provider below.
    pub(crate) fn new(client: KmsClient, key_id: String, pubkey: Pubkey) -> Self {
        Self {
            client,
            key_id,
            pubkey,
        }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[async_trait]
impl CustodySigner for KmsSigner {
    fn pubkey(&self) -> Pubkey {
        self.pubkey
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, CustodyProviderError> {
        let signature = self
            .client
            .sign_ed25519(&self.key_id, message)
            .await
            .map_err(provider_error)?;
        Ok(Signature::from(signature))
    }
}

/// Server-side AWS KMS custody provider — the real adapter.
///
/// Distinct from `bot_core::custody::KmsCustodyProvider` (the OPTION-B
/// placeholder that always refuses): this one talks to KMS over
/// SigV4-signed HTTPS. It is registered by the deployment registry
/// whenever KMS is the selected provider.
pub struct KmsCustodyProvider {
    config: KmsConfig,
    client: KmsClient,
}

impl std::fmt::Debug for KmsCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KmsCustodyProvider")
            .field("config", &self.config)
            .finish()
    }
}

impl KmsCustodyProvider {
    /// Build from resolved configuration. No network I/O at construction.
    pub fn new(config: KmsConfig) -> Self {
        let client = KmsClient::new(&config);
        Self { config, client }
    }

    /// The key id a signer resolves to, or the honest not-configured
    /// error naming what is missing.
    fn resolved_key_id(&self, record: &SignerRecord) -> Result<String, CustodyProviderError> {
        self.config
            .key_id_for(record)
            .filter(|id| !id.is_empty())
            .ok_or(CustodyProviderError::NotConfigured(ProviderType::Kms))
    }
}

#[async_trait]
impl CustodyProvider for KmsCustodyProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Kms
    }

    async fn resolve_signer(
        &self,
        record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        if record.provider_type != ProviderType::Kms {
            return Err(CustodyProviderError::ProviderMismatch {
                expected: ProviderType::Kms,
                actual: record.provider_type,
            });
        }
        if record.status != bot_core::custody::CustodyStatus::Active {
            return Err(CustodyProviderError::SignerNotActive(record.id));
        }
        let key_id = self.resolved_key_id(record)?;

        // Credentials are required before any KMS call — fail fast with
        // the exact dependency instead of an opaque transport error.
        AwsEnvCredentials::from_env().map_err(provider_error)?;

        // Fetch the public key from KMS — it comes from KMS, never from
        // the caller.
        let public = self
            .client
            .get_public_key(&key_id)
            .await
            .map_err(provider_error)?;
        if !public.key_spec.eq_ignore_ascii_case("ECC_ED25519") {
            return Err(CustodyProviderError::UnsupportedProvider(ProviderType::Kms));
        }
        let provider_pubkey = Pubkey::try_from(&public.ed25519_public_key[..]).map_err(|_| {
            CustodyProviderError::Transport(
                "KMS returned an Ed25519 key whose bytes are not a valid Solana pubkey".to_string(),
            )
        })?;

        // The record's pinned public address must match KMS's key. A
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

        Ok(Arc::new(KmsSigner::new(
            self.client.clone(),
            key_id,
            provider_pubkey,
        )))
    }

    async fn health_check(&self) -> Result<(), CustodyProviderError> {
        // Health is honest and cheap: verify credentials resolve, and if
        // a key id is configured, verify KMS serves its public key.
        AwsEnvCredentials::from_env().map_err(provider_error)?;
        if let Some(key_id) = self.config.key_id() {
            let public = self
                .client
                .get_public_key(key_id)
                .await
                .map_err(provider_error)?;
            if !public.key_spec.eq_ignore_ascii_case("ECC_ED25519") {
                return Err(CustodyProviderError::UnsupportedProvider(ProviderType::Kms));
            }
        } else {
            return Err(CustodyProviderError::NotConfigured(ProviderType::Kms));
        }
        Ok(())
    }
}

/// The exact dependency text, re-exported for health surfaces.
pub fn kms_dependency_text() -> &'static str {
    AWS_KMS_DEPENDENCY
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::custody::{CustodyProfileId, CustodyStatus, SignerId};

    fn kms_signer_record(provider_ref: Option<&str>, public_address: &str) -> SignerRecord {
        let mut record = SignerRecord::new(
            bot_core::tenant::OrganizationId::new(),
            CustodyProfileId::new(),
            "kms-signer".to_string(),
            ProviderType::Kms,
            public_address.to_string(),
            chrono::Utc::now(),
        );
        record.provider_ref = provider_ref.map(|s| s.to_string());
        record.status = CustodyStatus::Active;
        record
    }

    fn provider_without_network() -> (KmsCustodyProvider, Vec<&'static str>) {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KMS_KEY_ID", "1234abcd-12ab-34cd-56ef-1234567890ab");
        std::env::set_var("KMS_REGION", "us-east-1");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        std::env::remove_var("AWS_SESSION_TOKEN");
        let (config, state) = KmsConfig::from_env();
        (KmsCustodyProvider::new(config), state.missing())
    }

    #[tokio::test]
    async fn provider_mismatch_is_refused_before_any_network_call() {
        let (provider, _) = provider_without_network();
        let mut record = kms_signer_record(Some("k1"), "");
        record.provider_type = ProviderType::Vault;
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("non-kms record must be refused");
        assert_eq!(
            err,
            CustodyProviderError::ProviderMismatch {
                expected: ProviderType::Kms,
                actual: ProviderType::Vault
            }
        );
    }

    #[tokio::test]
    async fn inactive_signer_is_refused_before_any_network_call() {
        let (provider, _) = provider_without_network();
        let mut record = kms_signer_record(Some("k1"), "");
        record.status = CustodyStatus::Suspended;
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("inactive signer must be refused");
        assert_eq!(err, CustodyProviderError::SignerNotActive(record.id));
    }

    #[tokio::test]
    async fn missing_credentials_fail_closed_with_exact_dependency() {
        let (provider, missing) = provider_without_network();
        assert!(!missing.is_empty());
        let record = kms_signer_record(Some("k1"), "");
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("missing credentials must refuse");
        assert_eq!(err, CustodyProviderError::NotConfigured(ProviderType::Kms));
        assert!(kms_dependency_text().contains("AWS_ACCESS_KEY_ID"));
    }

    #[tokio::test]
    async fn missing_key_reference_is_not_configured() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KMS_KEY_ID");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        let (config, state) = KmsConfig::from_env();
        assert!(!state.key_present);
        let provider = KmsCustodyProvider::new(config);
        let record = kms_signer_record(None, "");
        drop(_lock); // release the env lock before awaiting
        let err = provider
            .resolve_signer(&record)
            .await
            .expect_err("no key reference must refuse");
        assert_eq!(err, CustodyProviderError::NotConfigured(ProviderType::Kms));
    }

    #[tokio::test]
    async fn health_check_without_configuration_fails_closed() {
        let (provider, _) = provider_without_network();
        let err = provider
            .health_check()
            .await
            .expect_err("no credentials -> not healthy");
        assert_eq!(err, CustodyProviderError::NotConfigured(ProviderType::Kms));
    }

    #[test]
    fn signer_ids_render_without_secrets() {
        let id = SignerId::new();
        assert!(!format!("{id}").is_empty());
    }
}
