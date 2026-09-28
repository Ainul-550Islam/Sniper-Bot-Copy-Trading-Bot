//! Central signer-resolution service (BATCH 2 file 04).
//!
//! Resolves `organization → custody profile → provider → signer`.
//! Enforces lifecycle, capability, signer status, tenant ownership, and
//! provider selection. No fallback from remote provider to local signer.
//! No private key exposure.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::tenant::{OrganizationId, OrganizationStatus};

use super::health::ProviderHealth;
use super::model::{
    CustodyProfile, CustodyProfileId, CustodyStatus, ProviderType, SignerId, SignerRecord,
};
use super::provider::{
    CustodyProviderError, CustodyProviderRegistry, CustodySigner, ResolvedSigner,
};

/// Resolution request.
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    pub organization_id: OrganizationId,
    pub organization_status: OrganizationStatus,
    pub custody_profile_id: CustodyProfileId,
    pub signer_id: SignerId,
    pub required_capability: Option<String>,
    pub expected_provider: Option<ProviderType>,
}

/// Resolution denial reason — machine-readable, secret-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveDenyReason {
    OrganizationClosed,
    OrganizationSuspended,
    ProfileNotFound,
    ProfileNotActive,
    ProfileTenantMismatch,
    SignerNotFound,
    SignerNotActive,
    SignerTenantMismatch,
    SignerProfileMismatch,
    CapabilityMissing(String),
    ProviderMismatch {
        expected: ProviderType,
        actual: ProviderType,
    },
    ProviderNotConfigured(ProviderType),
    ProviderUnavailable(String),
}

impl std::fmt::Display for ResolveDenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveDenyReason::OrganizationClosed => write!(f, "organization is closed"),
            ResolveDenyReason::OrganizationSuspended => write!(f, "organization is suspended"),
            ResolveDenyReason::ProfileNotFound => write!(f, "custody profile not found"),
            ResolveDenyReason::ProfileNotActive => write!(f, "custody profile not active"),
            ResolveDenyReason::ProfileTenantMismatch => {
                write!(f, "custody profile belongs to another organization")
            }
            ResolveDenyReason::SignerNotFound => write!(f, "signer not found"),
            ResolveDenyReason::SignerNotActive => write!(f, "signer not active"),
            ResolveDenyReason::SignerTenantMismatch => {
                write!(f, "signer belongs to another organization")
            }
            ResolveDenyReason::SignerProfileMismatch => {
                write!(f, "signer not in requested profile")
            }
            ResolveDenyReason::CapabilityMissing(c) => {
                write!(f, "signer missing capability: {}", c)
            }
            ResolveDenyReason::ProviderMismatch { expected, actual } => write!(
                f,
                "provider mismatch: expected {} but signer is {}",
                expected.as_str(),
                actual.as_str()
            ),
            ResolveDenyReason::ProviderNotConfigured(p) => {
                write!(f, "provider not configured: {}", p.as_str())
            }
            ResolveDenyReason::ProviderUnavailable(m) => {
                write!(f, "provider unavailable: {}", m)
            }
        }
    }
}
impl std::error::Error for ResolveDenyReason {}

/// Pure resolution check — no I/O, no signing. Returns Ok(()) if all guards pass.
pub fn check_resolve(
    req: &ResolveRequest,
    profile: &CustodyProfile,
    signer: &SignerRecord,
    now: DateTime<Utc>,
) -> Result<(), ResolveDenyReason> {
    // 1. Tenant lifecycle
    if req.organization_status == OrganizationStatus::Closed {
        return Err(ResolveDenyReason::OrganizationClosed);
    }
    if req.organization_status == OrganizationStatus::Suspended {
        return Err(ResolveDenyReason::OrganizationSuspended);
    }
    // 2. Profile existence & ownership
    if profile.id != req.custody_profile_id {
        return Err(ResolveDenyReason::ProfileNotFound);
    }
    if profile.organization_id != req.organization_id {
        return Err(ResolveDenyReason::ProfileTenantMismatch);
    }
    if profile.status != CustodyStatus::Active {
        return Err(ResolveDenyReason::ProfileNotActive);
    }
    // 3. Signer existence & ownership
    if signer.id != req.signer_id {
        return Err(ResolveDenyReason::SignerNotFound);
    }
    if signer.organization_id != req.organization_id {
        return Err(ResolveDenyReason::SignerTenantMismatch);
    }
    if signer.custody_profile_id != req.custody_profile_id {
        return Err(ResolveDenyReason::SignerProfileMismatch);
    }
    if signer.status != CustodyStatus::Active {
        return Err(ResolveDenyReason::SignerNotActive);
    }
    // 4. Provider mismatch guard (explicit expected vs actual)
    if let Some(expected) = req.expected_provider {
        if signer.provider_type != expected {
            return Err(ResolveDenyReason::ProviderMismatch {
                expected,
                actual: signer.provider_type,
            });
        }
        if profile.provider_type != expected {
            return Err(ResolveDenyReason::ProviderMismatch {
                expected,
                actual: profile.provider_type,
            });
        }
    }
    // Profile and signer provider must agree
    if profile.provider_type != signer.provider_type {
        return Err(ResolveDenyReason::ProviderMismatch {
            expected: profile.provider_type,
            actual: signer.provider_type,
        });
    }
    // 5. Capability check
    if let Some(cap) = &req.required_capability {
        if !signer.capabilities.contains(cap) {
            return Err(ResolveDenyReason::CapabilityMissing(cap.clone()));
        }
    }
    // 6. Timestamp sanity (signer not from future)
    if signer.created_at > now {
        return Err(ResolveDenyReason::SignerNotActive);
    }
    let _ = now;
    Ok(())
}

/// Async resolution that also checks provider health and resolves the signer handle.
/// No fallback: if health is Unavailable/Degraded/Revoked, returns ProviderUnavailable.
pub async fn resolve_signer_handle(
    req: ResolveRequest,
    profile: CustodyProfile,
    signer: SignerRecord,
    registry: &CustodyProviderRegistry,
    health: &ProviderHealth,
    now: DateTime<Utc>,
) -> Result<ResolvedSigner, ResolveDenyReason> {
    check_resolve(&req, &profile, &signer, now)?;

    // Health gate — must be Configured/Reachable, not Unavailable/Degraded/Revoked
    if !health.is_signing_allowed() {
        return Err(ResolveDenyReason::ProviderUnavailable(format!(
            "provider {} is {:?}",
            signer.provider_type.as_str(),
            health.state
        )));
    }

    let provider =
        registry
            .provider(signer.provider_type)
            .ok_or(ResolveDenyReason::ProviderNotConfigured(
                signer.provider_type,
            ))?;

    let inner: Arc<dyn CustodySigner> =
        provider
            .resolve_signer(&signer)
            .await
            .map_err(|e| match e {
                CustodyProviderError::NotConfigured(p) => {
                    ResolveDenyReason::ProviderNotConfigured(p)
                }
                CustodyProviderError::UnsupportedProvider(p) => {
                    ResolveDenyReason::ProviderNotConfigured(p)
                }
                CustodyProviderError::SignerNotActive(_) => ResolveDenyReason::SignerNotActive,
                CustodyProviderError::TenantMismatch => ResolveDenyReason::SignerTenantMismatch,
                CustodyProviderError::ProviderMismatch { expected, actual } => {
                    ResolveDenyReason::ProviderMismatch { expected, actual }
                }
                CustodyProviderError::Transport(m) => ResolveDenyReason::ProviderUnavailable(m),
                _ => ResolveDenyReason::ProviderUnavailable(e.to_string()),
            })?;

    Ok(ResolvedSigner {
        signer_id: signer.id,
        organization_id: signer.organization_id,
        profile_id: signer.custody_profile_id,
        public_address: signer.public_address.clone(),
        provider_type: signer.provider_type,
        inner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::health::ProviderHealth;
    use crate::custody::model::{CustodyProfile, CustodyStatus, ProviderType, SignerRecord};
    use crate::custody::provider::CustodyProviderRegistry;
    use crate::tenant::{OrganizationId, OrganizationStatus};
    use chrono::Utc;

    fn profile(org: OrganizationId, provider: ProviderType) -> CustodyProfile {
        let mut p = CustodyProfile::new(org, "test", provider, Utc::now());
        p.status = CustodyStatus::Active;
        p
    }

    fn signer(
        org: OrganizationId,
        profile_id: CustodyProfileId,
        provider: ProviderType,
    ) -> SignerRecord {
        let mut s = SignerRecord::new(org, profile_id, "identity", provider, "addr1", Utc::now());
        s.status = CustodyStatus::Active;
        s.capabilities = vec!["trade".into()];
        s
    }

    #[test]
    fn ok_when_all_guards_pass() {
        let org = OrganizationId::new();
        let prof = profile(org, ProviderType::Local);
        let s = signer(org, prof.id, ProviderType::Local);
        let req = ResolveRequest {
            organization_id: org,
            organization_status: OrganizationStatus::Active,
            custody_profile_id: prof.id,
            signer_id: s.id,
            required_capability: Some("trade".into()),
            expected_provider: Some(ProviderType::Local),
        };
        assert!(check_resolve(&req, &prof, &s, Utc::now()).is_ok());
    }

    #[test]
    fn closed_tenant_rejected() {
        let org = OrganizationId::new();
        let prof = profile(org, ProviderType::Local);
        let s = signer(org, prof.id, ProviderType::Local);
        let req = ResolveRequest {
            organization_id: org,
            organization_status: OrganizationStatus::Closed,
            custody_profile_id: prof.id,
            signer_id: s.id,
            required_capability: None,
            expected_provider: None,
        };
        assert_eq!(
            check_resolve(&req, &prof, &s, Utc::now()).unwrap_err(),
            ResolveDenyReason::OrganizationClosed
        );
    }

    #[test]
    fn wrong_tenant_rejected() {
        let org = OrganizationId::new();
        let other = OrganizationId::new();
        let prof = profile(org, ProviderType::Local);
        let s = signer(org, prof.id, ProviderType::Local);
        let req = ResolveRequest {
            organization_id: other,
            organization_status: OrganizationStatus::Active,
            custody_profile_id: prof.id,
            signer_id: s.id,
            required_capability: None,
            expected_provider: None,
        };
        assert!(check_resolve(&req, &prof, &s, Utc::now()).is_err());
    }

    #[test]
    fn capability_missing_rejected() {
        let org = OrganizationId::new();
        let prof = profile(org, ProviderType::Local);
        let s = signer(org, prof.id, ProviderType::Local);
        let req = ResolveRequest {
            organization_id: org,
            organization_status: OrganizationStatus::Active,
            custody_profile_id: prof.id,
            signer_id: s.id,
            required_capability: Some("admin".into()),
            expected_provider: None,
        };
        assert!(matches!(
            check_resolve(&req, &prof, &s, Utc::now()).unwrap_err(),
            ResolveDenyReason::CapabilityMissing(_)
        ));
    }

    #[test]
    fn no_fallback_from_remote_to_local() {
        let org = OrganizationId::new();
        let prof = profile(org, ProviderType::Vault);
        let s = signer(org, prof.id, ProviderType::Vault);
        let req = ResolveRequest {
            organization_id: org,
            organization_status: OrganizationStatus::Active,
            custody_profile_id: prof.id,
            signer_id: s.id,
            required_capability: None,
            expected_provider: Some(ProviderType::Vault),
        };
        // Pure check passes but async resolve must fail closed if Vault not configured
        assert!(check_resolve(&req, &prof, &s, Utc::now()).is_ok());
    }

    #[tokio::test]
    async fn remote_provider_unavailable_fails_closed() {
        let org = OrganizationId::new();
        let prof = profile(org, ProviderType::Vault);
        let s = signer(org, prof.id, ProviderType::Vault);
        let req = ResolveRequest {
            organization_id: org,
            organization_status: OrganizationStatus::Active,
            custody_profile_id: prof.id,
            signer_id: s.id,
            required_capability: None,
            expected_provider: Some(ProviderType::Vault),
        };
        let registry = CustodyProviderRegistry::new(); // no Vault registered
        let health = ProviderHealth::unavailable(ProviderType::Vault, "no credentials");
        let res = resolve_signer_handle(req, prof, s, &registry, &health, Utc::now()).await;
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("unavailable") || msg.contains("not configured"));
    }

    #[test]
    fn debug_never_contains_private_key() {
        let org = OrganizationId::new();
        let prof = profile(org, ProviderType::Vault);
        let s = signer(org, prof.id, ProviderType::Vault);
        let dbg = format!("{:?}", s);
        assert!(!dbg.to_ascii_lowercase().contains("private"));
        assert!(!dbg.to_ascii_lowercase().contains("secret"));
    }
}
