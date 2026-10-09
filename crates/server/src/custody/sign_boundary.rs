//! The custody sign boundary (§F, Batch 8).
//!
//! The single place where a tenant's request to sign a digest meets the
//! custody providers. Order of guards (all fail closed, all audited):
//!
//! 1. **Policy** — organization lifecycle (closed/suspended/past-due),
//!    profile + signer ownership (cross-tenant), signer status,
//!    capability, module entitlement, provider pinning
//!    (`bot_core::custody::check`).
//! 2. **Resolution** — provider health gate + adapter resolution of the
//!    signer (`bot_core::custody::resolve_signer_handle`). A Vault-bound
//!    signer can never be satisfied by a local wallet here: the provider
//!    is resolved from the signer record itself, and there is no local
//!    fallback.
//! 3. **Signing** — the resolved provider handle signs the digest or
//!    fails with a real provider error.
//!
//! Every outcome — including refusal — is written to the audit log. There
//! is no code path that produces a `Signed` response without a provider
//! handle having produced the signature.

use bot_core::custody::{
    check_custody, resolve_signer_handle, CustodyProfile, CustodyProviderError, CustodyRequest,
    ProviderHealth, ResolveRequest, SignerRecord,
};
use bot_core::tenant::OrganizationStatus;
use chrono::{DateTime, Utc};

use crate::custody::audit::{CustodyAuditLog, CustodyAuditRecord};
use crate::custody::provider_registry::{
    health_state_for_error, CustodyDeployment, ProviderAvailability,
};
use crate::custody::sign_request::CustodySignRequest;
use crate::custody::sign_response::{CustodySignResponse, RefusalReason};

/// The custody boundary: deployment posture + provider registry + audit.
pub struct CustodySignBoundary {
    deployment: CustodyDeployment,
    audit: CustodyAuditLog,
}

impl std::fmt::Debug for CustodySignBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustodySignBoundary")
            .field("deployment", &self.deployment.summary())
            .field("audit_records", &self.audit.len())
            .finish()
    }
}

impl CustodySignBoundary {
    /// Build the boundary from the deployment environment.
    pub fn from_env() -> Self {
        Self {
            deployment: CustodyDeployment::resolve_from_env(),
            audit: CustodyAuditLog::new(),
        }
    }

    /// Build the boundary from an explicit posture (tests, ops tooling).
    pub fn new(deployment: CustodyDeployment) -> Self {
        Self {
            deployment,
            audit: CustodyAuditLog::new(),
        }
    }

    pub fn deployment(&self) -> &CustodyDeployment {
        &self.deployment
    }

    pub fn audit_log(&self) -> &CustodyAuditLog {
        &self.audit
    }

    /// Attempt to sign one digest.
    ///
    /// `profile` and `signer` are the caller's view of the custody
    /// resources; the boundary re-checks ownership and status, so a
    /// mismatched or foreign profile/signer is refused, never signed.
    pub async fn sign(
        &self,
        request: &CustodySignRequest,
        organization_status: OrganizationStatus,
        profile: &CustodyProfile,
        signer: &SignerRecord,
        health: &ProviderHealth,
    ) -> CustodySignResponse {
        let now = Utc::now();
        let response = self
            .sign_inner(request, organization_status, profile, signer, health, now)
            .await;
        self.audit_outcome(request, &response, now);
        response
    }

    async fn sign_inner(
        &self,
        request: &CustodySignRequest,
        organization_status: OrganizationStatus,
        profile: &CustodyProfile,
        signer: &SignerRecord,
        health: &ProviderHealth,
        now: DateTime<Utc>,
    ) -> CustodySignResponse {
        // Guard 0 — remote custody must be opted in before anything else.
        // Single-operator local deployments are unaffected: their signing
        // continues through the existing solana module wallet path.
        if self.deployment.active.is_remote() && !self.deployment.live_enabled {
            return CustodySignResponse::refused(RefusalReason::ProviderUnsupported {
                provider: self.deployment.active,
                dependency:
                    "LIVE_CUSTODY=1 (remote custody is not opted in; the boundary refuses to sign)"
                        .to_string(),
            });
        }

        // Guard 1 — policy: tenant lifecycle, ownership, capability,
        // module, provider pinning.
        let policy_request = CustodyRequest::new(
            *request.organization_id(),
            organization_status,
            request.module(),
            request.capability(),
        )
        .with_provider(match request.expected_provider() {
            Some(p) => p,
            None => self.deployment.active,
        });
        let verdict = check_custody(Some(profile), Some(signer), &policy_request);
        if let bot_core::custody::CustodyVerdict::Deny(reason, detail) = verdict {
            return CustodySignResponse::refused(RefusalReason::Policy {
                deny: reason,
                detail,
            });
        }

        // Guard 2 — resolution: health gate + provider adapter.
        let resolve_request = ResolveRequest {
            organization_id: *request.organization_id(),
            organization_status,
            custody_profile_id: *request.custody_profile_id(),
            signer_id: *request.signer_id(),
            required_capability: Some(request.capability().to_string()),
            expected_provider: Some(self.deployment.active),
        };
        let registry = self.deployment.registry();
        let resolved = match resolve_signer_handle(
            resolve_request,
            profile.clone(),
            signer.clone(),
            &registry,
            health,
            now,
        )
        .await
        {
            Ok(resolved) => resolved,
            Err(deny) => {
                // Distinguish "integration absent" (refuse with the exact
                // dependency) from runtime failure.
                return match &deny {
                    bot_core::custody::ResolveDenyReason::ProviderNotConfigured(p) => {
                        let dependency = self
                            .deployment
                            .active_availability()
                            .dependency()
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "provider configuration".to_string());
                        let _ = p;
                        CustodySignResponse::refused(RefusalReason::ProviderUnsupported {
                            provider: self.deployment.active,
                            dependency,
                        })
                    }
                    other => CustodySignResponse::refused(RefusalReason::from_resolve(other)),
                };
            }
        };

        // Guard 3 — signing through the resolved provider handle.
        match resolved.inner.sign_message(&request.digest_bytes()).await {
            Ok(signature) => {
                CustodySignResponse::signed(signature.as_ref().to_vec(), resolved.provider_type)
            }
            Err(err) => {
                CustodySignResponse::refused(provider_failure_reason(err, self.deployment.active))
            }
        }
    }

    fn audit_outcome(
        &self,
        request: &CustodySignRequest,
        response: &CustodySignResponse,
        at: DateTime<Utc>,
    ) {
        let provider_name = response
            .provider()
            .or(Some(self.deployment.active))
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let record = match response {
            CustodySignResponse::Signed { .. } => CustodyAuditRecord::signed(
                at,
                *request.organization_id(),
                &provider_name,
                request.module(),
                request.purpose(),
                request.digest_hex(),
            ),
            CustodySignResponse::Refused { reason } => CustodyAuditRecord::refused(
                at,
                *request.organization_id(),
                &provider_name,
                request.module(),
                request.purpose(),
                request.digest_hex(),
                &reason.code(),
            ),
        };
        self.audit.record(record);
    }
}

/// Map a provider error to the honest refusal story:
/// `NotConfigured`/`UnsupportedProvider` state the exact deployment
/// dependency; anything else is a runtime failure with the provider's own
/// (secret-free) detail.
fn provider_failure_reason(
    err: CustodyProviderError,
    active: bot_core::custody::ProviderType,
) -> RefusalReason {
    match err {
        CustodyProviderError::NotConfigured(_) | CustodyProviderError::UnsupportedProvider(_) => {
            RefusalReason::ProviderUnsupported {
                provider: active,
                dependency: dependency_text_for(active),
            }
        }
        other => RefusalReason::ProviderFailure {
            provider: active,
            detail: other.to_string(),
        },
    }
}

fn dependency_text_for(provider: bot_core::custody::ProviderType) -> String {
    match provider {
        bot_core::custody::ProviderType::Local => {
            "local wallet signing for the multi-tenant custody boundary (single-operator deployments sign via the existing solana module wallet path)".to_string()
        }
        bot_core::custody::ProviderType::Vault => {
            crate::custody::vault::VAULT_TRANSIT_DEPENDENCY.to_string()
        }
        bot_core::custody::ProviderType::Kms => {
            crate::custody::kms::AWS_KMS_DEPENDENCY.to_string()
        }
        bot_core::custody::ProviderType::Hsm => {
            "PKCS#11 module with HSM_SLOT and an HSM_PIN reference (pkcs11 sign integration)".to_string()
        }
    }
}

/// Human-readable state for a provider error, reusing the registry's
/// mapping so health endpoints and refusals agree (rule #35).
pub fn provider_error_health_state(err: &CustodyProviderError) -> bot_core::custody::HealthState {
    health_state_for_error(err)
}

/// Availability of the boundary's active provider, for ops surfaces.
pub fn active_availability(boundary: &CustodySignBoundary) -> &ProviderAvailability {
    boundary.deployment().active_availability()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::custody::{CustodyStatus, HealthState, ProviderType};
    use bot_core::tenant::OrganizationId;

    fn org() -> OrganizationId {
        OrganizationId::new()
    }

    fn profile(org: OrganizationId, provider: ProviderType) -> CustodyProfile {
        let now = Utc::now();
        let mut p = CustodyProfile::new(org, "primary", provider, now);
        p.status = CustodyStatus::Active;
        p.activated_at = Some(now);
        p
    }

    fn signer(
        org: OrganizationId,
        profile: &CustodyProfile,
        provider: ProviderType,
    ) -> SignerRecord {
        let mut s = SignerRecord::new(
            org,
            profile.id,
            "primary-signer",
            provider,
            "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            Utc::now(),
        );
        s.status = CustodyStatus::Active;
        // Core policy requires BOTH the capability and (when distinct)
        // the module the engine wants to use.
        s.capabilities = vec!["solana:sign".to_string(), "module-sniper".to_string()];
        s
    }

    fn request(
        org: OrganizationId,
        profile: &CustodyProfile,
        signer: &SignerRecord,
    ) -> CustodySignRequest {
        CustodySignRequest::new(
            org,
            profile.id,
            signer.id,
            "ab".repeat(32),
            "module-sniper",
            "solana:sign",
            None,
            "order-signing",
        )
        .expect("valid request")
    }

    fn local_deployment() -> CustodyDeployment {
        let _guard = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_provider = std::env::var("CUSTODY_PROVIDER").ok();
        let prev_live = std::env::var("LIVE_CUSTODY").ok();
        std::env::remove_var("CUSTODY_PROVIDER");
        std::env::remove_var("LIVE_CUSTODY");
        let d = CustodyDeployment::resolve_from_env();
        match prev_provider {
            Some(v) => std::env::set_var("CUSTODY_PROVIDER", v),
            None => std::env::remove_var("CUSTODY_PROVIDER"),
        }
        match prev_live {
            Some(v) => std::env::set_var("LIVE_CUSTODY", v),
            None => std::env::remove_var("LIVE_CUSTODY"),
        }
        d
    }

    fn healthy(provider: ProviderType) -> ProviderHealth {
        ProviderHealth::reachable(provider, Utc::now())
    }

    #[tokio::test]
    async fn local_boundary_refuses_and_names_the_dependency() {
        let deployment = local_deployment();
        assert_eq!(deployment.active, ProviderType::Local);
        let boundary = CustodySignBoundary::new(deployment);
        let o = org();
        let profile = profile(o, ProviderType::Local);
        let signer = signer(o, &profile, ProviderType::Local);
        let request = request(o, &profile, &signer);
        let response = boundary
            .sign(
                &request,
                OrganizationStatus::Active,
                &profile,
                &signer,
                &healthy(ProviderType::Local),
            )
            .await;
        // OPTION B: local provider refuses honestly — no fabricated
        // signature. The refusal names the dependency.
        assert!(!response.is_success());
        let reason = response.refusal().unwrap();
        assert!(matches!(reason, RefusalReason::ProviderUnsupported { .. }));
        assert!(reason.detail().contains("solana module wallet path"));
        // The refusal is audited.
        let audit = boundary.audit_log().recent(1);
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].code, reason.code());
        assert_eq!(
            audit[0].outcome,
            crate::custody::audit::CustodyAuditOutcome::Refused
        );
    }

    #[tokio::test]
    async fn suspended_tenant_is_refused_by_policy() {
        let boundary = CustodySignBoundary::new(local_deployment());
        let o = org();
        let profile = profile(o, ProviderType::Local);
        let signer = signer(o, &profile, ProviderType::Local);
        let request = request(o, &profile, &signer);
        let response = boundary
            .sign(
                &request,
                OrganizationStatus::Suspended,
                &profile,
                &signer,
                &healthy(ProviderType::Local),
            )
            .await;
        let reason = response.refusal().unwrap();
        assert_eq!(
            reason.code(),
            "policy.tenant_suspended",
            "policy runs before the provider is touched"
        );
        let audit = boundary.audit_log().recent(1);
        assert_eq!(audit[0].code, "policy.tenant_suspended");
    }

    #[tokio::test]
    async fn cross_tenant_signer_is_refused() {
        let boundary = CustodySignBoundary::new(local_deployment());
        let owner = org();
        let attacker = org();
        let profile = profile(owner, ProviderType::Local);
        let signer = signer(owner, &profile, ProviderType::Local);
        // attacker requests signing with the owner's signer
        let request = request(attacker, &profile, &signer);
        let response = boundary
            .sign(
                &request,
                OrganizationStatus::Active,
                &profile,
                &signer,
                &healthy(ProviderType::Local),
            )
            .await;
        assert!(!response.is_success());
        assert_eq!(response.refusal().unwrap().code(), "policy.cross_tenant");
        // Audit row belongs to the requester org (attacker), recording the refusal.
        let audit = boundary.audit_log().recent(1);
        assert_eq!(audit[0].organization_id, attacker);
    }

    #[tokio::test]
    async fn unhealthy_provider_is_refused_before_signing() {
        let boundary = CustodySignBoundary::new(local_deployment());
        let o = org();
        let profile = profile(o, ProviderType::Local);
        let signer = signer(o, &profile, ProviderType::Local);
        let request = request(o, &profile, &signer);
        let response = boundary
            .sign(
                &request,
                OrganizationStatus::Active,
                &profile,
                &signer,
                &ProviderHealth::new(
                    ProviderType::Local,
                    HealthState::Degraded,
                    "wallet backend latency",
                    Utc::now(),
                ),
            )
            .await;
        assert!(!response.is_success());
        let reason = response.refusal().unwrap();
        assert!(reason.code().starts_with("resolve."));
        assert!(reason.detail().contains("Degraded"));
    }

    #[test]
    fn remote_not_opted_in_refuses_before_policy() {
        // vault posture without LIVE_CUSTODY
        let _guard = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_provider = std::env::var("CUSTODY_PROVIDER").ok();
        let prev_live = std::env::var("LIVE_CUSTODY").ok();
        std::env::set_var("CUSTODY_PROVIDER", "vault");
        std::env::remove_var("LIVE_CUSTODY");
        let deployment = CustodyDeployment::resolve_from_env();
        match prev_provider {
            Some(v) => std::env::set_var("CUSTODY_PROVIDER", v),
            None => std::env::remove_var("CUSTODY_PROVIDER"),
        }
        match prev_live {
            Some(v) => std::env::set_var("LIVE_CUSTODY", v),
            None => std::env::remove_var("LIVE_CUSTODY"),
        }
        let boundary = CustodySignBoundary::new(deployment);
        let o = org();
        let profile = profile(o, ProviderType::Vault);
        let signer = signer(o, &profile, ProviderType::Vault);
        let request = request(o, &profile, &signer);
        let response = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(boundary.sign(
                &request,
                OrganizationStatus::Active,
                &profile,
                &signer,
                &healthy(ProviderType::Vault),
            ));
        let reason = response.refusal().unwrap();
        assert_eq!(reason.code(), "provider_unsupported.vault");
        assert!(reason.detail().contains("LIVE_CUSTODY"));
    }

    #[tokio::test]
    async fn missing_capability_is_refused() {
        let boundary = CustodySignBoundary::new(local_deployment());
        let o = org();
        let profile = profile(o, ProviderType::Local);
        let mut signer = SignerRecord::new(
            o,
            profile.id,
            "view-only-signer",
            ProviderType::Local,
            "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            Utc::now(),
        );
        signer.status = CustodyStatus::Active;
        signer.capabilities = vec!["solana:view".to_string()]; // cannot sign
        let request = request(o, &profile, &signer);
        let response = boundary
            .sign(
                &request,
                OrganizationStatus::Active,
                &profile,
                &signer,
                &healthy(ProviderType::Local),
            )
            .await;
        // resolve guard reports the missing capability before the provider.
        let reason = response.refusal().unwrap();
        assert_eq!(reason.code(), "policy.missing_capability");
        // The dynamic policy detail names the missing capability.
        assert!(reason.detail().contains("solana:sign"));
    }
}
