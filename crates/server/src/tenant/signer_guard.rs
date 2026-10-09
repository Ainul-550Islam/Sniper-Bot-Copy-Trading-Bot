//! The tenant signer binding guard (tenant-isolation file 25).
//!
//! The signer half of [`super::binding_guard`], wrapped in a
//! context-aware guard: the request's signer reference (provider +
//! key ref — never key material) must resolve to an ACTIVE signer
//! binding of the SAME tenant the context was resolved for, checked
//! immediately before signing. A signer bound to another tenant does
//! not exist in this tenant's slice; an inactive signer is refused.
//!
//! Fails closed on registry errors.

use std::sync::Arc;

use bot_core::tenant::OrganizationId;

use crate::tenant::registry::{SignerBindingRow, TenantBindingRegistry};
use crate::tenant::request::SignerBinding;

use super::binding_guard;
use super::context::TenantContext;
use super::decision::{DenyReason, GuardOutcome};

/// The signer binding guard.
#[derive(Clone)]
pub struct SignerGuard {
    registry: Arc<dyn TenantBindingRegistry>,
}

impl SignerGuard {
    /// Build over the tenant binding registry.
    pub fn new(registry: Arc<dyn TenantBindingRegistry>) -> Self {
        SignerGuard { registry }
    }

    /// Check the request's signer binding for the context's tenant.
    ///
    /// 1. the request must belong to the context's tenant;
    /// 2. the signer reference must resolve to an ACTIVE binding of
    ///    that tenant (delegated to the shared binding guard).
    pub async fn check(
        &self,
        context: &TenantContext,
        signer: &SignerBinding,
        organization_id: OrganizationId,
    ) -> GuardOutcome {
        if !context.is(organization_id) {
            return GuardOutcome::Deny(DenyReason::ContextIssue {
                reason: "signer check requested for a different tenant than the context",
            });
        }
        binding_guard::signer(&self.registry, organization_id, signer).await
    }

    /// Resolve the binding row without a verdict (callers preparing
    /// the signing payload after the guard passed). `Ok(None)` when
    /// the tenant has no such binding; registry errors fail closed.
    pub async fn resolve(
        &self,
        context: &TenantContext,
        signer: &SignerBinding,
    ) -> Result<Option<SignerBindingRow>, DenyReason> {
        let row = self
            .registry
            .signer(context.organization_id(), signer.provider, &signer.key_ref)
            .await
            .map_err(|_| DenyReason::DependencyError {
                source: "binding_registry",
            })?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::registry::{MemoryTenantBindingRegistry, SignerBindingRow};
    use bot_core::tenant::{Organization, OrganizationId, SignerProvider};
    use chrono::Utc;

    use super::super::context::ContextOrigin;

    fn context_for(organization_id: OrganizationId) -> TenantContext {
        let organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
        TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap()
    }

    fn signer_binding() -> SignerBinding {
        SignerBinding {
            provider: SignerProvider::Custody,
            key_ref: "key-7".into(),
        }
    }

    async fn seeded() -> (Arc<MemoryTenantBindingRegistry>, OrganizationId) {
        let registry = Arc::new(MemoryTenantBindingRegistry::new());
        let org = OrganizationId::new();
        registry
            .put_signer(SignerBindingRow {
                organization_id: org,
                provider: SignerProvider::Custody,
                key_ref: "key-7".into(),
                active: true,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        (registry, org)
    }

    #[tokio::test]
    async fn the_own_active_signer_passes() {
        let (registry, org) = seeded().await;
        let guard = SignerGuard::new(registry);
        let outcome = guard.check(&context_for(org), &signer_binding(), org).await;
        assert!(outcome.is_allow(), "{:?}", outcome.deny_reason());
    }

    #[tokio::test]
    async fn another_tenants_signer_is_invisible_here() {
        let (registry, _org) = seeded().await;
        let guard = SignerGuard::new(registry);
        let other = OrganizationId::new();
        let outcome = guard
            .check(&context_for(other), &signer_binding(), other)
            .await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "signer_not_bound");
    }

    #[tokio::test]
    async fn a_check_for_a_foreign_tenant_is_a_context_violation() {
        let (registry, org) = seeded().await;
        let guard = SignerGuard::new(registry);
        let outcome = guard
            .check(&context_for(org), &signer_binding(), OrganizationId::new())
            .await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "context_issue");
    }

    #[tokio::test]
    async fn an_inactive_signer_is_refused() {
        let (registry, org) = seeded().await;
        // Deactivate the binding the tenant owns.
        registry
            .put_signer(SignerBindingRow {
                organization_id: org,
                provider: SignerProvider::Custody,
                key_ref: "key-7".into(),
                active: false,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        let guard = SignerGuard::new(registry);
        let outcome = guard.check(&context_for(org), &signer_binding(), org).await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "signer_inactive");
    }
}
