//! The tenant wallet binding guard (tenant-isolation file 24).
//!
//! A focused, context-aware wrapper over the wallet half of
//! [`super::binding_guard`]: the request's wallet label must resolve
//! to an ACTIVE wallet binding of the SAME tenant the context was
//! resolved for. This is the cross-tenant firewall at the wallet
//! boundary — another tenant's wallet label is not an error, it
//! simply does not exist in this tenant's binding slice.
//!
//! The guard runs BEFORE any signer/risk work (cheapest lookup, most
//! common misconfiguration) and fails closed on registry errors.

use std::sync::Arc;

use bot_core::tenant::OrganizationId;

use crate::tenant::registry::{TenantBindingRegistry, WalletBinding};

use super::binding_guard;
use super::context::TenantContext;
use super::decision::{DenyReason, GuardOutcome};

/// The wallet binding guard.
#[derive(Clone)]
pub struct WalletGuard {
    registry: Arc<dyn TenantBindingRegistry>,
}

impl WalletGuard {
    /// Build over the tenant binding registry.
    pub fn new(registry: Arc<dyn TenantBindingRegistry>) -> Self {
        WalletGuard { registry }
    }

    /// Check the request's wallet binding for the context's tenant.
    ///
    /// 1. the request must belong to the context's tenant (a request
    ///    naming another organization is a context violation, not a
    ///    binding miss);
    /// 2. the wallet label must resolve to an ACTIVE binding of that
    ///    tenant (delegated to the shared binding guard).
    pub async fn check(
        &self,
        context: &TenantContext,
        label: &str,
        organization_id: OrganizationId,
    ) -> GuardOutcome {
        if !context.is(organization_id) {
            return GuardOutcome::Deny(DenyReason::ContextIssue {
                reason: "wallet check requested for a different tenant than the context",
            });
        }
        binding_guard::wallet(&self.registry, organization_id, label).await
    }

    /// Resolve the binding without a verdict (callers that need the
    /// wallet reference after the guard passed). `Ok(None)` when the
    /// tenant has no such binding; registry errors fail closed.
    pub async fn resolve(
        &self,
        context: &TenantContext,
        label: &str,
    ) -> Result<Option<WalletBinding>, DenyReason> {
        let binding = self
            .registry
            .wallet(context.organization_id(), label)
            .await
            .map_err(|_| DenyReason::DependencyError {
                source: "binding_registry",
            })?;
        Ok(binding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::registry::{MemoryTenantBindingRegistry, WalletBinding};
    use bot_core::tenant::{Organization, OrganizationId};
    use chrono::Utc;

    use super::super::context::ContextOrigin;

    fn context_for(organization_id: OrganizationId) -> TenantContext {
        let organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
        TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap()
    }

    async fn seeded() -> (Arc<MemoryTenantBindingRegistry>, OrganizationId) {
        let registry = Arc::new(MemoryTenantBindingRegistry::new());
        let org = OrganizationId::new();
        registry
            .put_wallet(WalletBinding {
                organization_id: org,
                label: "main".into(),
                address: "WalletMain1111111111111111111111111111111111111".into(),
                active: true,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        (registry, org)
    }

    #[tokio::test]
    async fn the_own_active_wallet_passes() {
        let (registry, org) = seeded().await;
        let guard = WalletGuard::new(registry);
        let outcome = guard.check(&context_for(org), "main", org).await;
        assert!(outcome.is_allow(), "{:?}", outcome.deny_reason());
    }

    #[tokio::test]
    async fn another_tenants_wallet_label_does_not_exist_here() {
        let (registry, _org) = seeded().await;
        let guard = WalletGuard::new(registry);
        let other = OrganizationId::new();
        // A label bound by ANOTHER tenant is invisible in this slice.
        let outcome = guard.check(&context_for(other), "main", other).await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "wallet_not_bound");
    }

    #[tokio::test]
    async fn a_check_for_a_foreign_tenant_is_a_context_violation() {
        let (registry, org) = seeded().await;
        let guard = WalletGuard::new(registry);
        let outcome = guard
            .check(&context_for(org), "main", OrganizationId::new())
            .await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "context_issue");
    }

    #[tokio::test]
    async fn an_unknown_label_is_not_bound() {
        let (registry, org) = seeded().await;
        let guard = WalletGuard::new(registry);
        let outcome = guard.check(&context_for(org), "ghost", org).await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "wallet_not_bound");
    }
}
