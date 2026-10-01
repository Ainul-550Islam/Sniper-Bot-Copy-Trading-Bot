//! Per-tenant health roll-up (STEP 3 file 56).
//!
//! The dashboard's tenant card: is the runtime lease-live, what did the
//! gateway decide recently, is anything stuck. PURE aggregation over
//! the same stores everything else uses — no independent probing, no
//! second source of truth.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use bot_core::tenant::OrganizationId;

use crate::runtime_registry::RuntimeRegistryService;
use crate::tenant_observability::metrics::TenantMetrics;

/// One tenant's rolled-up health.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantHealth {
    /// The tenant.
    pub organization_id: OrganizationId,
    /// The rolled-up status.
    pub status: HealthStatus,
    /// The live runtime's generation, when there is one.
    pub runtime_generation: Option<u64>,
    /// The live runtime's worker id, when there is one.
    pub runtime_worker: Option<String>,
    /// Total gateway allows observed.
    pub allows: u64,
    /// Total gateway denies observed.
    pub denies: u64,
    /// Fence failures observed.
    pub fence_failures: u64,
    /// When the roll-up was taken.
    pub at: DateTime<Utc>,
}

/// The roll-up status. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Lease-live runtime, decisions flowing (or none needed yet).
    Healthy,
    /// No live runtime: nothing can execute for this tenant.
    NoRuntime,
    /// Runtime is live but fence failures dominate — possible
    /// split-brain aftermath or worker churn.
    Degraded,
}

impl HealthStatus {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Healthy => "healthy",
            HealthStatus::NoRuntime => "no_runtime",
            HealthStatus::Degraded => "degraded",
        }
    }
}

/// The roll-up engine.
pub struct TenantHealthRollup {
    runtime: Arc<RuntimeRegistryService>,
    metrics: Arc<TenantMetrics>,
}

impl TenantHealthRollup {
    /// Build over the shared stores.
    pub fn new(runtime: Arc<RuntimeRegistryService>, metrics: Arc<TenantMetrics>) -> Self {
        TenantHealthRollup { runtime, metrics }
    }

    /// Roll one tenant up.
    pub async fn tenant(
        &self,
        organization_id: OrganizationId,
    ) -> bot_core::error::BotResult<TenantHealth> {
        let record = self.runtime.current(organization_id).await?;
        let counters = self.metrics.snapshot(organization_id).await;

        let status = match (&record, counters.fence_failures) {
            (None, _) => HealthStatus::NoRuntime,
            (Some(_), failures) if failures > counters.allows && failures > 0 => {
                HealthStatus::Degraded
            }
            (Some(_), _) => HealthStatus::Healthy,
        };

        Ok(TenantHealth {
            organization_id,
            status,
            runtime_generation: record.as_ref().map(|r| r.generation.raw()),
            runtime_worker: record.map(|r| r.worker_id),
            allows: counters.allows,
            denies: counters.total_denies(),
            fence_failures: counters.fence_failures,
            at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use chrono::Utc;

    #[tokio::test]
    async fn a_tenant_without_a_runtime_is_no_runtime() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let metrics = Arc::new(TenantMetrics::new());
        let rollup = TenantHealthRollup::new(runtime, metrics);

        let health = rollup.tenant(OrganizationId::new()).await.unwrap();
        assert_eq!(health.status, HealthStatus::NoRuntime);
        assert_eq!(health.status.as_str(), "no_runtime");
        assert_eq!(health.runtime_generation, None);
    }

    #[tokio::test]
    async fn a_live_runtime_with_flowing_decisions_is_healthy() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let org = OrganizationId::new();
        runtime
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        let metrics = Arc::new(TenantMetrics::new());
        metrics.record_decision(org, "allow").await;
        metrics.record_decision(org, "allow").await;

        let rollup = TenantHealthRollup::new(runtime, metrics);
        let health = rollup.tenant(org).await.unwrap();
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.runtime_generation, Some(1));
        assert_eq!(health.runtime_worker.as_deref(), Some("worker-a"));
        assert_eq!(health.allows, 2);
        assert_eq!(health.denies, 0);
    }

    #[tokio::test]
    async fn fence_failures_dominate_marks_degraded() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let org = OrganizationId::new();
        runtime
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        let metrics = Arc::new(TenantMetrics::new());
        metrics.record_fence_failure(org).await;
        metrics.record_fence_failure(org).await;
        metrics.record_fence_failure(org).await;

        let rollup = TenantHealthRollup::new(runtime, metrics);
        let health = rollup.tenant(org).await.unwrap();
        assert_eq!(health.status, HealthStatus::Degraded);
        assert_eq!(health.fence_failures, 3);
    }
}
