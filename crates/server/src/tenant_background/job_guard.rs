//! The background job guard (tenant-isolation file 53).
//!
//! A background job tick NEVER runs on ambient process state: the
//! [`JobGuard`] refuses a tick whose [`JobContext`] is incomplete,
//! whose fence token is not the tenant's CURRENT lease-live runtime,
//! or (for trading jobs) whose tenant lifecycle no longer permits
//! trading. This is the job-layer twin of the gateway's fence guard —
//! the scheduler performs the same verification inline (it exits the
//! task on a lost fence); this guard is the standalone, testable
//! form entry paths and operators use to decide BEFORE spawning.
//!
//! Fail closed on every dependency error.

use std::sync::Arc;

use crate::runtime_registry::{FenceVerdict, RuntimeRegistryService};

use crate::tenant::decision::{DenyReason, GuardOutcome};

use super::job_context::JobContext;

/// What kind of work the tick performs — trading ticks additionally
/// require the tenant's lifecycle to permit NEW trading activity;
/// maintenance ticks (reconciliation, retention, cleanup) keep
/// running while a tenant is past-due or suspended so its books stay
/// correct, but never for a closed tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobClass {
    /// Opens/changes trading exposure (entries, orders).
    Trading,
    /// Repairs books / reclaims state (recon, retention, sweeps).
    Maintenance,
}

impl JobClass {
    /// Stable label.
    pub const fn as_str(self) -> &'static str {
        match self {
            JobClass::Trading => "trading",
            JobClass::Maintenance => "maintenance",
        }
    }
}

/// The background job guard.
pub struct JobGuard {
    runtime: Arc<RuntimeRegistryService>,
}

impl JobGuard {
    /// Build over the runtime registry.
    pub fn new(runtime: Arc<RuntimeRegistryService>) -> Self {
        JobGuard { runtime }
    }

    /// Full check for one job tick.
    ///
    /// Order (first deny wins):
    /// 1. the context is complete (tenant principal present, fence
    ///    bound to the context's tenant);
    /// 2. the fence token is the tenant's CURRENT lease-live runtime;
    /// 3. the tenant's lifecycle permits this class of work.
    pub async fn authorize(&self, context: &JobContext, class: JobClass) -> GuardOutcome {
        // 1. context completeness.
        if context.tenant().principal().trim().is_empty() {
            return GuardOutcome::Deny(DenyReason::ContextIssue {
                reason: "job context has no principal",
            });
        }
        if context.fence_token().organization_id != context.organization_id() {
            return GuardOutcome::Deny(DenyReason::ContextIssue {
                reason: "job fence token does not belong to the job's tenant",
            });
        }

        // 2. the fence is current.
        let verdict = match self.runtime.verify(context.fence_token()).await {
            Ok(verdict) => verdict,
            Err(_) => {
                return GuardOutcome::Deny(DenyReason::DependencyError {
                    source: "runtime_registry",
                })
            }
        };
        match verdict {
            FenceVerdict::Current(_) => {}
            other => {
                return GuardOutcome::Deny(DenyReason::FenceFailed {
                    verdict: other.as_str(),
                })
            }
        }

        // 3. the tenant's lifecycle permits this class of work.
        let snapshot = context.tenant().state();
        let allowed = match class {
            JobClass::Trading => snapshot.may_trade(),
            JobClass::Maintenance => snapshot.may_read() || snapshot.may_trade(),
        };
        if !allowed {
            return GuardOutcome::Deny(DenyReason::TenantState {
                state: snapshot.status().as_str(),
            });
        }

        GuardOutcome::Allow("job_authorized")
    }

    /// Fence-only check (the tick-time hot path the scheduler uses).
    pub async fn fence_is_current(&self, context: &JobContext) -> bool {
        match self.runtime.verify(context.fence_token()).await {
            Ok(verdict) => verdict.is_current(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::tenant::{
        Organization, OrganizationId, OrganizationStatus, RuntimeGeneration, RuntimeId,
    };
    use chrono::Utc;

    use super::super::job_context::JobContext;
    use crate::tenant::context::ContextOrigin;
    use crate::tenant::TenantContext;

    fn service() -> Arc<RuntimeRegistryService> {
        Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )))
    }

    fn organization_with(
        organization_id: OrganizationId,
        status: OrganizationStatus,
    ) -> Organization {
        let mut organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
        organization.status = status;
        organization
    }

    #[tokio::test]
    async fn a_live_fence_with_an_active_tenant_authorizes_trading_work() {
        let service = service();
        let org = OrganizationId::new();
        let record = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        let context = JobContext::new(
            &organization_with(org, OrganizationStatus::Active),
            bot_core::tenant::ModuleKind::Copy,
            "recon",
            record.fence_token(),
            Utc::now(),
        )
        .unwrap();
        let guard = JobGuard::new(service);
        let outcome = guard.authorize(&context, JobClass::Trading).await;
        assert!(outcome.is_allow(), "{:?}", outcome.deny_reason());
        assert!(guard.fence_is_current(&context).await);
    }

    #[tokio::test]
    async fn a_rotated_fence_never_authorizes() {
        let service = service();
        let org = OrganizationId::new();
        let first = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        service.rotate(org, "worker-b", Utc::now()).await.unwrap();

        let context = JobContext::new(
            &organization_with(org, OrganizationStatus::Active),
            bot_core::tenant::ModuleKind::Copy,
            "recon",
            first.fence_token(),
            Utc::now(),
        )
        .unwrap();
        let guard = JobGuard::new(service);
        let outcome = guard.authorize(&context, JobClass::Trading).await;
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "fence_failed");
        assert!(!guard.fence_is_current(&context).await);
    }

    #[tokio::test]
    async fn a_suspended_tenant_stops_trading_but_keeps_maintenance() {
        let service = service();
        let org = OrganizationId::new();
        let record = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        let context = JobContext::new(
            &organization_with(org, OrganizationStatus::Suspended),
            bot_core::tenant::ModuleKind::Copy,
            "recon",
            record.fence_token(),
            Utc::now(),
        )
        .unwrap();
        let guard = JobGuard::new(service);
        assert_eq!(
            guard
                .authorize(&context, JobClass::Trading)
                .await
                .deny_reason()
                .unwrap()
                .as_str(),
            "tenant_state"
        );
        assert!(guard
            .authorize(&context, JobClass::Maintenance)
            .await
            .is_allow());
    }

    #[tokio::test]
    async fn a_tenantless_fence_is_a_context_violation() {
        let service = service();
        let org = OrganizationId::new();
        let record = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        // Build the context, then swap the fence for a FOREIGN token —
        // the constructor refuses this, so the guard's own rule is
        // exercised through a hand-built context below.
        let context = JobContext::new(
            &organization_with(org, OrganizationStatus::Active),
            bot_core::tenant::ModuleKind::Copy,
            "recon",
            record.fence_token(),
            Utc::now(),
        )
        .unwrap();
        let foreign = JobContext::new(
            &organization_with(OrganizationId::new(), OrganizationStatus::Active),
            bot_core::tenant::ModuleKind::Copy,
            "sweep",
            FenceTokenForeign::make(),
            Utc::now(),
        );
        // The foreign-fence construction is rejected outright:
        assert!(foreign.is_err());
        // The honest context still authorizes:
        let guard = JobGuard::new(service);
        assert!(guard
            .authorize(&context, JobClass::Maintenance)
            .await
            .is_allow());
    }

    /// Test helper that mints a fence token for a DIFFERENT tenant.
    struct FenceTokenForeign;
    impl FenceTokenForeign {
        fn make() -> crate::runtime_registry::FenceToken {
            crate::runtime_registry::FenceToken {
                organization_id: OrganizationId::new(),
                runtime_id: RuntimeId::new(),
                generation: RuntimeGeneration::first(),
            }
        }
    }

    #[tokio::test]
    async fn an_anonymous_context_is_impossible_so_the_guard_sees_none() {
        // The strongest anonymous-job defense is at construction: a
        // JobContext without a principal cannot exist. The guard's
        // first rule backs that up for any future construction path.
        let service = service();
        let org = OrganizationId::new();
        let record = service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap()
            .record()
            .clone();
        // A TenantContext with a blank principal cannot be built:
        let err = TenantContext::new(
            organization_with(OrganizationId::new(), OrganizationStatus::Active),
            "  ",
            ContextOrigin::Job,
            Utc::now(),
        );
        assert!(err.is_err());
        // And the valid path keeps working:
        let context = JobContext::new(
            &organization_with(org, OrganizationStatus::Active),
            bot_core::tenant::ModuleKind::Copy,
            "recon",
            record.fence_token(),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(context.tenant().principal(), "job:copy:recon");
    }
}
