//! Guard 7/7: runtime fence (STEP 3 file 23).
//!
//! The LAST guard before context issuance: the claiming runtime must
//! still be the tenant's current LIVE runtime (right instance, right
//! generation, lease-live). A stale worker from before a rotation, a
//! superseded replica or a runtime whose heartbeat lapsed is denied —
//! this is the mechanism that keeps a rotated worker from executing one
//! more order "just because it was already running".

use std::sync::Arc;

use bot_core::tenant::OrganizationId;

use crate::runtime_registry::fencing::FenceVerdict;
use crate::runtime_registry::{FenceToken, RuntimeRegistryService};

use super::decision::{DenyReason, GuardOutcome};

/// Check the claiming runtime's fence token.
pub async fn check(service: &Arc<RuntimeRegistryService>, token: &FenceToken) -> GuardOutcome {
    let verdict = match service.verify(token).await {
        Ok(verdict) => verdict,
        Err(_) => {
            return GuardOutcome::Deny(DenyReason::DependencyError {
                source: "runtime_registry",
            })
        }
    };
    match verdict {
        FenceVerdict::Current(_) => GuardOutcome::Allow("fence_current"),
        other => GuardOutcome::Deny(DenyReason::FenceFailed {
            verdict: other.as_str(),
        }),
    }
}

/// Convenience for callers that only know the tenant (no token): the
/// tenant's current lease-live runtime, when there is one. The gateway
/// uses this to mint the fence token for a request that arrived
/// without one (HTTP entry paths).
pub async fn current_runtime(
    service: &Arc<RuntimeRegistryService>,
    organization_id: OrganizationId,
) -> Option<crate::runtime_registry::TenantRuntimeRecord> {
    service.current(organization_id).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::tenant::RuntimeGeneration;
    use bot_core::tenant::RuntimeId;
    use chrono::Utc;

    fn service() -> Arc<RuntimeRegistryService> {
        Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )))
    }

    #[tokio::test]
    async fn the_live_runtimes_fence_verifies() {
        let service = service();
        let org = bot_core::tenant::OrganizationId::new();
        let record = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        assert!(check(&service, &record.fence_token()).await.is_allow());
    }

    #[tokio::test]
    async fn a_rotated_workers_fence_fails() {
        let service = service();
        let org = bot_core::tenant::OrganizationId::new();
        let first = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        service.rotate(org, "worker-b", Utc::now()).await.unwrap();

        let outcome = check(&service, &first.fence_token()).await;
        let reason = outcome.deny_reason().unwrap();
        assert_eq!(reason.as_str(), "fence_failed");
        assert!(matches!(
            reason,
            DenyReason::FenceFailed {
                verdict: "superseded"
            }
        ));
    }

    #[tokio::test]
    async fn a_tenant_without_a_runtime_has_no_fence() {
        let service = service();
        let token = FenceToken {
            organization_id: bot_core::tenant::OrganizationId::new(),
            runtime_id: RuntimeId::new(),
            generation: RuntimeGeneration::first(),
        };
        assert_eq!(
            check(&service, &token)
                .await
                .deny_reason()
                .unwrap()
                .as_str(),
            "fence_failed"
        );
    }
}
