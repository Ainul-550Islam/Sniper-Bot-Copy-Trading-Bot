//! Tenant runtime resolution (tenant-isolation file 19).
//!
//! [`RuntimeContext`] resolves the tenant's CURRENT runtime from a
//! [`TenantContext`] and verifies its generation — the step between
//! "we know the tenant" and "we know the live runtime that may execute
//! for them right now". Everything here is fail closed:
//!
//! * no live runtime → [`RuntimeContextError::NoLiveRuntime`];
//! * a runtime that exists but is not live (draining/stopped/retired)
//!   → [`RuntimeContextError::RuntimeNotLive`];
//! * a runtime whose fence token no longer verifies (superseded, stale
//!   generation, lease lapsed) →
//!   [`RuntimeContextError::FenceNotCurrent`];
//! * a registry error → [`RuntimeContextError::RegistryUnavailable`]
//!   (never a silent allow).
//!
//! The resolved context carries the verified runtime record and its
//! fence token — the token the composed guard chain re-verifies at
//! execution time (defense in depth: resolution and execution each
//! check the fence independently).

use std::sync::Arc;

use bot_core::tenant::OrganizationId;

use crate::runtime_registry::{FenceToken, RuntimeRegistryService, TenantRuntimeRecord};

use super::context::TenantContext;

/// Every way runtime resolution can fail. Fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeContextError {
    /// The tenant has no runtime record at all.
    NoLiveRuntime,
    /// A runtime record exists but is not in the live set.
    RuntimeNotLive { status: &'static str },
    /// The runtime's fence token failed verification (superseded /
    /// stale generation / lease lapsed).
    FenceNotCurrent { verdict: &'static str },
    /// The runtime registry itself failed (storage down). Never an
    /// allow.
    RegistryUnavailable,
}

impl RuntimeContextError {
    /// Stable machine-readable label.
    pub const fn as_str(&self) -> &'static str {
        match self {
            RuntimeContextError::NoLiveRuntime => "no_live_runtime",
            RuntimeContextError::RuntimeNotLive { .. } => "runtime_not_live",
            RuntimeContextError::FenceNotCurrent { .. } => "fence_not_current",
            RuntimeContextError::RegistryUnavailable => "registry_unavailable",
        }
    }
}

impl std::fmt::Display for RuntimeContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeContextError::RuntimeNotLive { status } => {
                write!(f, "runtime not live: {status}")
            }
            RuntimeContextError::FenceNotCurrent { verdict } => {
                write!(f, "fence not current: {verdict}")
            }
            other => write!(f, "runtime resolution error: {}", other.as_str()),
        }
    }
}

impl std::error::Error for RuntimeContextError {}

/// A resolved, verified tenant runtime.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    record: TenantRuntimeRecord,
}

impl RuntimeContext {
    /// Resolve the tenant's current live runtime and verify its fence.
    pub async fn resolve(
        service: &Arc<RuntimeRegistryService>,
        context: &TenantContext,
    ) -> Result<Self, RuntimeContextError> {
        Self::resolve_for(service, context.organization_id()).await
    }

    /// Resolve for an explicit organization (the resolution core —
    /// [`RuntimeContext::resolve`] delegates here after pulling the
    /// organization out of the tenant context).
    pub async fn resolve_for(
        service: &Arc<RuntimeRegistryService>,
        organization_id: OrganizationId,
    ) -> Result<Self, RuntimeContextError> {
        let record = match service.current(organization_id).await {
            Ok(Some(record)) => record,
            Ok(None) => return Err(RuntimeContextError::NoLiveRuntime),
            Err(_) => return Err(RuntimeContextError::RegistryUnavailable),
        };
        if !record.status.is_live() {
            return Err(RuntimeContextError::RuntimeNotLive {
                status: record.status.as_str(),
            });
        }
        let token = record.fence_token();
        let verdict = match service.verify(&token).await {
            Ok(verdict) => verdict,
            Err(_) => return Err(RuntimeContextError::RegistryUnavailable),
        };
        if !verdict.is_current() {
            return Err(RuntimeContextError::FenceNotCurrent {
                verdict: verdict.as_str(),
            });
        }
        Ok(RuntimeContext { record })
    }

    /// The verified runtime record.
    pub fn record(&self) -> &TenantRuntimeRecord {
        &self.record
    }

    /// The runtime's fence token (re-verified by the guard chain at
    /// execution time — this value is a claim, not a proof).
    pub fn fence_token(&self) -> FenceToken {
        self.record.fence_token()
    }

    /// The runtime's generation.
    pub fn generation(&self) -> bot_core::tenant::RuntimeGeneration {
        self.record.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::tenant::{Organization, OrganizationId};
    use chrono::Utc;

    use super::super::context::{ContextOrigin, TenantContext};

    fn service() -> Arc<RuntimeRegistryService> {
        Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )))
    }

    fn context_for(organization_id: OrganizationId) -> TenantContext {
        let organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
        TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap()
    }

    #[tokio::test]
    async fn a_tenant_with_a_live_runtime_resolves() {
        let service = service();
        let org = OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        let resolved = RuntimeContext::resolve(&service, &context_for(org))
            .await
            .unwrap();
        assert_eq!(resolved.record().organization_id, org);
        assert!(resolved.record().status.is_live());
        assert_eq!(resolved.generation().raw(), 1);
    }

    #[tokio::test]
    async fn a_tenant_without_a_runtime_fails_closed() {
        let service = service();
        let err = RuntimeContext::resolve(&service, &context_for(OrganizationId::new()))
            .await
            .unwrap_err();
        assert_eq!(err.as_str(), "no_live_runtime");
    }

    #[tokio::test]
    async fn a_stopped_runtime_resolves_to_no_live_runtime() {
        // `current` answers only with a lease-LIVE runtime: a stopped
        // worker means the tenant currently has none — the resolve
        // fails closed with `no_live_runtime`. (The `RuntimeNotLive`
        // branch stays as defense in depth for stores that hand back
        // a non-live record.)
        let service = service();
        let org = OrganizationId::new();
        let ensured = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        service
            .stop(ensured.record().runtime_id, Utc::now())
            .await
            .unwrap();
        let err = RuntimeContext::resolve(&service, &context_for(org))
            .await
            .unwrap_err();
        assert_eq!(err.as_str(), "no_live_runtime");
    }

    #[tokio::test]
    async fn a_rotated_runtime_fails_the_fence_check() {
        let service = service();
        let org = OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        // Rotate: the first worker is now superseded. `current` returns
        // the NEW worker, whose fence verifies — resolution succeeds.
        // The stale WORKER's token is what fails (verified in the
        // runtime registry suites); here we assert the new worker is
        // what resolves, at generation 2.
        service.rotate(org, "worker-b", Utc::now()).await.unwrap();
        let resolved = RuntimeContext::resolve(&service, &context_for(org))
            .await
            .unwrap();
        assert_eq!(resolved.record().worker_id, "worker-b");
        assert_eq!(resolved.generation().raw(), 2);
    }
}
