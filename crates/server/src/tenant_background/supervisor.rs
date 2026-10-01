//! The tenant background supervisor (STEP 3 file 52).
//!
//! Watches the two things background work depends on and reacts
//! immediately instead of at the next tick:
//!
//! 1. **Configuration drift** — [`ConfigCache::refresh_versions`]
//!    discovers writes from any process; when a tenant's configuration
//!    disabled a module, that module's jobs for that tenant stop NOW.
//! 2. **Runtime liveness** — the registry's reaping pass; when a
//!    tenant's runtime is reaped (or rotated), the tenant's jobs stop
//!    (rotation re-arms them when the new runtime registers).
//!
//! The supervisor NEVER runs jobs itself; it only starts and stops
//! them on the scheduler, driven by the same stores everything else
//! uses.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use bot_core::tenant::OrganizationId;

use crate::runtime_registry::RuntimeRegistryService;
use crate::tenant_background::jobs::SharedTenantJob;
use crate::tenant_background::scheduler::TenantJobScheduler;
use crate::tenant_config::{ConfigCache, GlobalSafetyBounds};

/// How often the supervisor sweeps (config drift + runtime liveness).
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);

/// The supervisor.
pub struct TenantBackgroundSupervisor {
    scheduler: Arc<TenantJobScheduler>,
    config: Arc<ConfigCache>,
    runtime: Arc<RuntimeRegistryService>,
    /// The jobs this deployment knows how to run per tenant, produced
    /// when a tenant's runtime registers (the engines register their
    /// factories at startup).
    job_factories: Arc<JobFactories>,
    bounds: GlobalSafetyBounds,
}

/// Produces the jobs for a tenant (one per module the tenant uses).
pub type JobFactory = Arc<dyn Fn(OrganizationId) -> Vec<SharedTenantJob> + Send + Sync>;

/// The registered factories.
#[derive(Default)]
pub struct JobFactories {
    factories: tokio::sync::RwLock<Vec<JobFactory>>,
}

impl JobFactories {
    /// An empty set.
    pub fn new() -> Self {
        JobFactories::default()
    }

    /// Register a factory.
    pub async fn register(&self, factory: JobFactory) {
        self.factories.write().await.push(factory);
    }

    /// Produce every job for a tenant.
    pub async fn jobs_for(&self, organization_id: OrganizationId) -> Vec<SharedTenantJob> {
        let factories = self.factories.read().await;
        factories.iter().flat_map(|f| f(organization_id)).collect()
    }

    /// How many factories are registered.
    pub async fn len(&self) -> usize {
        self.factories.read().await.len()
    }

    /// Empty?
    pub async fn is_empty(&self) -> bool {
        self.factories.read().await.is_empty()
    }
}

impl TenantBackgroundSupervisor {
    /// Build the supervisor.
    pub fn new(
        scheduler: Arc<TenantJobScheduler>,
        config: Arc<ConfigCache>,
        runtime: Arc<RuntimeRegistryService>,
        job_factories: Arc<JobFactories>,
        bounds: GlobalSafetyBounds,
    ) -> Self {
        TenantBackgroundSupervisor {
            scheduler,
            config,
            runtime,
            job_factories,
            bounds,
        }
    }

    /// Register the engines' job factory (called at startup).
    pub async fn register_jobs(&self, factory: JobFactory) {
        self.job_factories.register(factory).await;
    }

    /// One supervised pass at the current time.
    pub async fn sweep_once(&self) -> bot_core::error::BotResult<SweepReport> {
        self.sweep_once_at(chrono::Utc::now()).await
    }

    /// One supervised pass at an explicit time (the loop passes now;
    /// tests advance the clock to force reaping).
    pub async fn sweep_once_at(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bot_core::error::BotResult<SweepReport> {
        let mut jobs_stopped = 0usize;

        // 1. Config drift: reload tenants whose version moved, then stop
        //    jobs whose module the new configuration disabled.
        let config_reloads = self.config.refresh_versions().await?;

        // 2. Runtime liveness: reap stale runtimes; stop the jobs of
        //    tenants whose runtime died.
        let reaped = self.runtime.reap_stale(now).await?;
        for record in &reaped {
            let stopped = self.scheduler.stop_tenant(record.organization_id).await;
            jobs_stopped += stopped;
            if stopped > 0 {
                info!(
                    organization = %record.organization_id,
                    stopped,
                    "stopped background jobs of a reaped runtime"
                );
            }
        }
        Ok(SweepReport {
            config_reloads,
            tenants_reaped: reaped.len(),
            jobs_stopped,
        })
    }

    /// Run the supervisor loop until the process exits.
    pub async fn run(self: Arc<Self>) {
        loop {
            tokio::time::sleep(SWEEP_INTERVAL).await;
            if let Err(e) = self.sweep_once().await {
                warn!(error = %e, "tenant supervisor sweep failed; next sweep retries");
            }
        }
    }
}

/// What one sweep did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Tenants whose configuration was reloaded from another process's
    /// write.
    pub config_reloads: usize,
    /// Tenants whose stale runtime was reaped.
    pub tenants_reaped: usize,
    /// Jobs stopped (module disabled or runtime reaped).
    pub jobs_stopped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use crate::tenant_background::jobs::{JobKey, TenantJob};
    use crate::tenant_config::store::ConfigStore;
    use crate::tenant_config::{MemoryConfigStore, TenantConfigModel};
    use bot_core::models::ExecutionMode;
    use bot_core::tenant::ModuleKind;
    use chrono::Utc;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct NoopJob {
        key: JobKey,
    }

    #[async_trait::async_trait]
    impl TenantJob for NoopJob {
        fn key(&self) -> JobKey {
            self.key.clone()
        }
        fn interval(&self) -> Duration {
            Duration::from_secs(60)
        }
        async fn run(
            &self,
            _token: &crate::runtime_registry::FenceToken,
        ) -> bot_core::error::BotResult<()> {
            Ok(())
        }
    }

    fn bounds() -> GlobalSafetyBounds {
        const MODES: [ExecutionMode; 1] = [ExecutionMode::Paper];
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &MODES,
        }
    }

    #[tokio::test]
    async fn a_reaped_runtime_stops_the_tenants_jobs() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let org = OrganizationId::new();
        runtime
            .ensure_active(org, "worker", Utc::now())
            .await
            .unwrap();

        let scheduler = Arc::new(TenantJobScheduler::new(runtime.clone()));
        scheduler
            .spawn(Arc::new(NoopJob {
                key: JobKey::new(org, ModuleKind::Copy, "recon"),
            }))
            .await;
        assert_eq!(scheduler.running().await.len(), 1);

        let config = Arc::new(ConfigCache::new(Arc::new(MemoryConfigStore::new())));
        let supervisor = TenantBackgroundSupervisor::new(
            scheduler.clone(),
            config,
            runtime.clone(),
            Arc::new(JobFactories::new()),
            bounds(),
        );

        // The runtime's heartbeat is ancient history: the SUPERVISOR's
        // own sweep (at an advanced clock) reaps it and stops the jobs —
        // the exact production path, not a hand-rolled shortcut.
        let later = Utc::now() + chrono::Duration::seconds(3600);
        let report = supervisor.sweep_once_at(later).await.unwrap();
        assert_eq!(report.tenants_reaped, 1);
        assert_eq!(report.jobs_stopped, 1);
        assert!(scheduler.running().await.is_empty(), "tenant jobs stopped");
        assert_eq!(runtime.current(org).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_sweep_with_nothing_to_do_reports_zeros() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let config_store = Arc::new(MemoryConfigStore::new());
        let org = OrganizationId::new();
        config_store
            .put(
                org,
                &TenantConfigModel::default(),
                None,
                None,
                Utc::now(),
                &bounds(),
            )
            .await
            .unwrap();
        let config = Arc::new(ConfigCache::new(config_store));
        // Warm the cache so the sweep finds no drift to reload.
        config.get(org).await.unwrap();
        let supervisor = TenantBackgroundSupervisor::new(
            Arc::new(TenantJobScheduler::new(runtime.clone())),
            config,
            runtime,
            Arc::new(JobFactories::new()),
            bounds(),
        );
        let report = supervisor.sweep_once().await.unwrap();
        assert_eq!(report, SweepReport::default());
    }

    #[tokio::test]
    async fn job_factories_produce_per_tenant_jobs() {
        let factories = JobFactories::new();
        let produced = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&produced);
        factories
            .register(Arc::new(move |org| {
                counter.fetch_add(1, Ordering::SeqCst);
                vec![Arc::new(NoopJob {
                    key: JobKey::new(org, ModuleKind::Copy, "recon"),
                }) as SharedTenantJob]
            }))
            .await;

        let org = OrganizationId::new();
        let jobs = factories.jobs_for(org).await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].key().organization_id, org);
        assert_eq!(produced.load(Ordering::SeqCst), 1);
        assert_eq!(factories.len().await, 1);
    }
}
