//! Provider-neutral remote signer abstraction (BATCH file 08).
//!
//! Connects tenant custody profiles to the existing `TransactionSigner`.
//! Provides typed interfaces for Vault/KMS/HSM providers.
//! Must NOT silently fall back to local signing. Unsupported provider fails closed.
//!
//! NOTE: To avoid a circular dependency (bot-core <-> solana-kit), this
//! module defines a minimal `CustodySigner` trait that mirrors the shape of
//! `solana_kit::signer::TransactionSigner`. The server crate bridges the two:
//! a `CustodySigner` resolved here is wrapped/adapted into a real
//! `TransactionSigner` without ever falling back to local keys.

use std::sync::Arc;

use async_trait::async_trait;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;

use crate::tenant::OrganizationId;

use super::model::{CustodyProfile, CustodyStatus, ProviderType, SignerId, SignerRecord};

/// Errors for custody provider resolution. No secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodyProviderError {
    UnsupportedProvider(ProviderType),
    NotConfigured(ProviderType),
    SignerNotFound(SignerId),
    ProfileNotFound(String),
    SignerNotActive(SignerId),
    TenantMismatch,
    ProviderMismatch {
        expected: ProviderType,
        actual: ProviderType,
    },
    /// The signer record's pinned public key does not match the key the
    /// remote provider actually holds. Fail closed: a stale or wrong
    /// public key must never be allowed to sign.
    PubkeyMismatch {
        signer: SignerId,
        recorded: String,
        provider_key: String,
    },
    Transport(String),
}

impl std::fmt::Display for CustodyProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CustodyProviderError::UnsupportedProvider(p) => {
                write!(f, "unsupported custody provider: {}", p.as_str())
            }
            CustodyProviderError::NotConfigured(p) => {
                write!(f, "custody provider not configured: {}", p.as_str())
            }
            CustodyProviderError::SignerNotFound(id) => write!(f, "signer not found: {}", id),
            CustodyProviderError::ProfileNotFound(id) => {
                write!(f, "custody profile not found: {}", id)
            }
            CustodyProviderError::SignerNotActive(id) => write!(f, "signer not active: {}", id),
            CustodyProviderError::TenantMismatch => {
                write!(f, "custody signer belongs to another organization")
            }
            CustodyProviderError::ProviderMismatch { expected, actual } => write!(
                f,
                "provider mismatch: expected {} but signer is {}",
                expected.as_str(),
                actual.as_str()
            ),
            CustodyProviderError::PubkeyMismatch {
                signer,
                recorded,
                provider_key,
            } => write!(
                f,
                "signer {} public key {} does not match provider key {} — refusing to sign",
                signer, recorded, provider_key
            ),
            CustodyProviderError::Transport(m) => {
                write!(f, "custody provider transport error: {}", m)
            }
        }
    }
}

impl std::error::Error for CustodyProviderError {}

/// Minimal signing capability — shape-compatible with `solana_kit::signer::TransactionSigner`
/// but defined here to keep `bot-core` free of the `solana-kit` dependency.
#[async_trait]
pub trait CustodySigner: Send + Sync + std::fmt::Debug {
    fn pubkey(&self) -> Pubkey;
    async fn sign_message(&self, message: &[u8]) -> Result<Signature, CustodyProviderError>;
}

/// A resolved signer that can sign. Wraps the CustodySigner trait object plus metadata.
#[derive(Debug)]
pub struct ResolvedSigner {
    pub signer_id: SignerId,
    pub organization_id: OrganizationId,
    pub profile_id: crate::custody::model::CustodyProfileId,
    pub public_address: String,
    pub provider_type: ProviderType,
    pub inner: Arc<dyn CustodySigner>,
}

/// Provider adapter: knows how to resolve a signer identity into a signing handle.
#[async_trait]
pub trait CustodyProvider: Send + Sync {
    fn provider_type(&self) -> ProviderType;

    /// Resolve a signer record into a signing handle. Must verify org ownership and active status.
    async fn resolve_signer(
        &self,
        record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError>;

    /// Health check: is this provider configured and reachable?
    async fn health_check(&self) -> Result<(), CustodyProviderError>;
}

/// Local provider adapter — wraps an existing local wallet/signer.
pub struct LocalCustodyProvider {
    wallet_pubkey: Option<Pubkey>,
}

impl Default for LocalCustodyProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalCustodyProvider {
    pub fn new() -> Self {
        Self {
            wallet_pubkey: None,
        }
    }
    pub fn with_pubkey(pubkey: Pubkey) -> Self {
        Self {
            wallet_pubkey: Some(pubkey),
        }
    }
}

impl std::fmt::Debug for LocalCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalCustodyProvider")
            .field("has_pubkey", &self.wallet_pubkey.is_some())
            .finish()
    }
}

#[async_trait]
impl CustodyProvider for LocalCustodyProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Local
    }
    async fn resolve_signer(
        &self,
        record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        if record.provider_type != ProviderType::Local {
            return Err(CustodyProviderError::ProviderMismatch {
                expected: ProviderType::Local,
                actual: record.provider_type,
            });
        }
        if record.status != CustodyStatus::Active {
            return Err(CustodyProviderError::SignerNotActive(record.id));
        }
        Err(CustodyProviderError::NotConfigured(ProviderType::Local))
    }
    async fn health_check(&self) -> Result<(), CustodyProviderError> {
        Ok(())
    }
}

/// Stub for Vault — intentionally unsupported until real backend.
pub struct VaultCustodyProvider;
impl std::fmt::Debug for VaultCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultCustodyProvider")
    }
}
#[async_trait]
impl CustodyProvider for VaultCustodyProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Vault
    }
    async fn resolve_signer(
        &self,
        _record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        Err(CustodyProviderError::UnsupportedProvider(
            ProviderType::Vault,
        ))
    }
    async fn health_check(&self) -> Result<(), CustodyProviderError> {
        Err(CustodyProviderError::NotConfigured(ProviderType::Vault))
    }
}

/// Stub for KMS — intentionally unsupported.
pub struct KmsCustodyProvider;
impl std::fmt::Debug for KmsCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KmsCustodyProvider")
    }
}
#[async_trait]
impl CustodyProvider for KmsCustodyProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Kms
    }
    async fn resolve_signer(
        &self,
        _record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        Err(CustodyProviderError::UnsupportedProvider(ProviderType::Kms))
    }
    async fn health_check(&self) -> Result<(), CustodyProviderError> {
        Err(CustodyProviderError::NotConfigured(ProviderType::Kms))
    }
}

/// Stub for HSM — intentionally unsupported.
pub struct HsmCustodyProvider;
impl std::fmt::Debug for HsmCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HsmCustodyProvider")
    }
}
#[async_trait]
impl CustodyProvider for HsmCustodyProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Hsm
    }
    async fn resolve_signer(
        &self,
        _record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        Err(CustodyProviderError::UnsupportedProvider(ProviderType::Hsm))
    }
    async fn health_check(&self) -> Result<(), CustodyProviderError> {
        Err(CustodyProviderError::NotConfigured(ProviderType::Hsm))
    }
}

/// Registry of custody providers. Fail-closed: missing provider is error, never local fallback.
#[derive(Default)]
pub struct CustodyProviderRegistry {
    providers: std::collections::HashMap<ProviderType, Arc<dyn CustodyProvider>>,
}

impl CustodyProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
        }
    }

    pub fn with_local() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(LocalCustodyProvider::new()));
        r
    }

    pub fn register(&mut self, provider: Arc<dyn CustodyProvider>) {
        self.providers.insert(provider.provider_type(), provider);
    }

    pub fn provider(&self, t: ProviderType) -> Option<Arc<dyn CustodyProvider>> {
        self.providers.get(&t).cloned()
    }

    /// Resolve a signer record via its declared provider. Fails closed if provider missing or unsupported.
    pub async fn resolve(
        &self,
        record: &SignerRecord,
    ) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
        let provider = self
            .provider(record.provider_type)
            .ok_or(CustodyProviderError::NotConfigured(record.provider_type))?;
        provider.resolve_signer(record).await
    }
}

/// High-level resolver that ties profile + signer + organization checks to provider resolution.
pub async fn resolve_active_signer(
    registry: &CustodyProviderRegistry,
    profile: &CustodyProfile,
    signer: &SignerRecord,
    organization_id: OrganizationId,
) -> Result<Arc<dyn CustodySigner>, CustodyProviderError> {
    if profile.organization_id != organization_id || signer.organization_id != organization_id {
        return Err(CustodyProviderError::TenantMismatch);
    }
    if signer.custody_profile_id != profile.id {
        return Err(CustodyProviderError::TenantMismatch);
    }
    if profile.status != CustodyStatus::Active {
        return Err(CustodyProviderError::NotConfigured(profile.provider_type));
    }
    if signer.status != CustodyStatus::Active {
        return Err(CustodyProviderError::SignerNotActive(signer.id));
    }
    if profile.provider_type != signer.provider_type {
        return Err(CustodyProviderError::ProviderMismatch {
            expected: profile.provider_type,
            actual: signer.provider_type,
        });
    }
    registry.resolve(signer).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::model::CustodyStatus;
    use crate::tenant::OrganizationId;
    use chrono::Utc;

    fn profile(org: OrganizationId, pt: ProviderType) -> CustodyProfile {
        let mut p = CustodyProfile::new(org, "main", pt, Utc::now());
        p.status = CustodyStatus::Active;
        p
    }

    fn signer(
        org: OrganizationId,
        profile: &CustodyProfile,
        pt: ProviderType,
        status: CustodyStatus,
    ) -> SignerRecord {
        let mut s = SignerRecord::new(org, profile.id, "sniper", pt, "Addr1", Utc::now());
        s.status = status;
        s
    }

    #[tokio::test]
    async fn unsupported_provider_fails_closed() {
        let registry = {
            let mut r = CustodyProviderRegistry::new();
            r.register(Arc::new(VaultCustodyProvider));
            r
        };
        let org = OrganizationId::new();
        let p = profile(org, ProviderType::Vault);
        let s = signer(org, &p, ProviderType::Vault, CustodyStatus::Active);
        let err = registry.resolve(&s).await.unwrap_err();
        assert_eq!(
            err,
            CustodyProviderError::UnsupportedProvider(ProviderType::Vault)
        );
    }

    #[tokio::test]
    async fn missing_provider_fails_closed_no_fallback() {
        let registry = CustodyProviderRegistry::new(); // empty, no local fallback
        let org = OrganizationId::new();
        let p = profile(org, ProviderType::Kms);
        let s = signer(org, &p, ProviderType::Kms, CustodyStatus::Active);
        let err = resolve_active_signer(&registry, &p, &s, org)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            CustodyProviderError::NotConfigured(ProviderType::Kms)
        ));
    }

    #[tokio::test]
    async fn tenant_mismatch_denied() {
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        let p = profile(org_a, ProviderType::Local);
        let s = signer(org_a, &p, ProviderType::Local, CustodyStatus::Active);
        let registry = CustodyProviderRegistry::with_local();
        let err = resolve_active_signer(&registry, &p, &s, org_b)
            .await
            .unwrap_err();
        assert_eq!(err, CustodyProviderError::TenantMismatch);
    }

    #[tokio::test]
    async fn revoked_signer_cannot_be_resolved() {
        let org = OrganizationId::new();
        let p = profile(org, ProviderType::Local);
        let s = signer(org, &p, ProviderType::Local, CustodyStatus::Revoked);
        let registry = CustodyProviderRegistry::with_local();
        let err = resolve_active_signer(&registry, &p, &s, org)
            .await
            .unwrap_err();
        assert_eq!(err, CustodyProviderError::SignerNotActive(s.id));
    }

    #[tokio::test]
    async fn provider_mismatch_denied() {
        let org = OrganizationId::new();
        let p = profile(org, ProviderType::Local);
        let mut s = signer(org, &p, ProviderType::Vault, CustodyStatus::Active);
        s.provider_type = ProviderType::Vault;
        let registry = CustodyProviderRegistry::with_local();
        let err = resolve_active_signer(&registry, &p, &s, org)
            .await
            .unwrap_err();
        assert!(matches!(err, CustodyProviderError::ProviderMismatch { .. }));
    }

    #[test]
    fn registry_respects_provider_type() {
        let mut r = CustodyProviderRegistry::new();
        r.register(Arc::new(KmsCustodyProvider));
        assert!(r.provider(ProviderType::Kms).is_some());
        assert!(r.provider(ProviderType::Local).is_none());
    }
}
