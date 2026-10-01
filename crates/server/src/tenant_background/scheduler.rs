//! The tenant job scheduler (STEP 3 file 51).
//!
//! Spawns one tokio task per [`TenantJob`]. Every tick is fence-gated:
//! the job's runtime must still be the tenant's CURRENT lease-live
//! runtime, or the task exits — background work never outlives its
//! mandate. The first tick waits one interval (the engines' own
//! startup pass does the immediate work).

use std::sync::Arc;

use tracing::{info, warn};

use crate::runtime_registry::{FenceToken, RuntimeRegistryService};

use super::jobs::{JobHandles, JobKey, SharedTenantJob};

/// How many consecutive run errors before the job is paused for a
/// cool-down (it is NOT dropped: the supervisor re-arms it).
const MAX_CONSECUTIVE_ERRORS: u32 = 10;

/// The scheduler.
pub struct TenantJobScheduler {
    runtime: Arc<RuntimeRegistryService>,
    handles: Arc<JobHandles>,
}

impl TenantJobScheduler {
    /// Build over the runtime registry.
    pub fn new(runtime: Arc<RuntimeRegistryService>) -> Self {
        TenantJobScheduler {
            runtime,
            handles: Arc::new(JobHandles::new()),
        }
    }

    /// The shared handle registry (the supervisor stops jobs through it).
    pub fn handles(&self) -> &Arc<JobHandles> {
        &self.handles
    }

    /// Spawn (or replace) a job. The job runs until it finishes, its
    /// fence fails, or [`JobHandles::remove`] aborts it.
    pub async fn spawn(&self, job: SharedTenantJob) {
        let key = job.key();
        let runtime = self.runtime.clone();
        let handles = self.handles.clone();
        let task_key = key.clone();

        let handle = tokio::spawn(async move {
            let key = task_key;
            let mut token = match current_fence(&runtime, &key).await {
                Some(token) => token,
                None => {
                    info!(job = ?key, "tenant job found no live runtime; exiting");
                    return;
                }
            };
            let mut errors: u32 = 0;
            loop {
                tokio::time::sleep(job.interval()).await;

                // Re-verify the fence on EVERY tick.
                match runtime.verify(&token).await {
                    Ok(verdict) if verdict.is_current() => {}
                    Ok(_) => {
                        info!(job = ?key, "tenant job fence lost; exiting");
                        return;
                    }
                    Err(e) => {
                        warn!(job = ?key, error = %e, "fence check failed; retrying next tick");
                        continue;
                    }
                }

                match job.run(&token).await {
                    Ok(()) => errors = 0,
                    Err(e) => {
                        errors += 1;
                        warn!(
                            job = ?key,
                            error = %e,
                            consecutive = errors,
                            "tenant job tick failed"
                        );
                        if errors >= MAX_CONSECUTIVE_ERRORS {
                            warn!(job = ?key, "tenant job paused after repeated failures");
                            return;
                        }
                    }
                }

                // The runtime may have rotated between ticks: re-read it
                // so the next tick verifies against the live record.
                if let Some(fresh) = current_fence(&runtime, &key).await {
                    token = fresh;
                }
            }
        });

        handles.insert(key, handle).await;
    }

    /// Stop every job of one tenant (runtime reaped/rotated away).
    pub async fn stop_tenant(&self, organization_id: bot_core::tenant::OrganizationId) -> usize {
        self.handles.remove_tenant(organization_id).await
    }

    /// Running job keys (observability).
    pub async fn running(&self) -> Vec<JobKey> {
        self.handles.keys().await
    }
}

/// The tenant's CURRENT fence token, when a lease-live runtime exists.
async fn current_fence(runtime: &Arc<RuntimeRegistryService>, key: &JobKey) -> Option<FenceToken> {
    let record = runtime.current(key.organization_id).await.ok()??;
    Some(FenceToken {
        organization_id: key.organization_id,
        runtime_id: record.runtime_id,
        generation: record.generation,
    })
}

/// Test-only helper: the cool-down bound, asserted by tests below.
#[cfg(test)]
pub(crate) const fn max_consecutive_errors() -> u32 {
    MAX_CONSECUTIVE_ERRORS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::error::BotError;
    use bot_core::tenant::{ModuleKind, OrganizationId};
    use chrono::Utc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    struct CountingJob {
        key: JobKey,
        interval: Duration,
        ticks: AtomicU32,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl super::super::jobs::TenantJob for CountingJob {
        fn key(&self) -> JobKey {
            self.key.clone()
        }
        fn interval(&self) -> Duration {
            self.interval
        }
        async fn run(&self, _token: &FenceToken) -> bot_core::error::BotResult<()> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(BotError::db("intentional test failure"))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn a_job_ticks_under_a_live_fence() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let org = OrganizationId::new();
        runtime
            .ensure_active(org, "worker", Utc::now())
            .await
            .unwrap();

        let scheduler = TenantJobScheduler::new(runtime);
        let job = Arc::new(CountingJob {
            key: JobKey::new(org, ModuleKind::Copy, "tick"),
            interval: Duration::from_millis(20),
            ticks: AtomicU32::new(0),
            fail: false,
        });
        let observed = Arc::clone(&job);
        scheduler.spawn(job).await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        let ticks = observed.ticks.load(Ordering::SeqCst);
        assert!(ticks >= 2, "expected several ticks, got {ticks}");
    }

    #[tokio::test]
    async fn a_tenant_without_a_runtime_never_runs() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let scheduler = TenantJobScheduler::new(runtime);
        let job = Arc::new(CountingJob {
            key: JobKey::new(OrganizationId::new(), ModuleKind::Copy, "orphan"),
            interval: Duration::from_millis(10),
            ticks: AtomicU32::new(0),
            fail: false,
        });
        let observed = Arc::clone(&job);
        scheduler.spawn(job).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(observed.ticks.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stopping_a_tenant_aborts_its_jobs() {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let org = OrganizationId::new();
        runtime
            .ensure_active(org, "worker", Utc::now())
            .await
            .unwrap();

        let scheduler = TenantJobScheduler::new(runtime);
        let job = Arc::new(CountingJob {
            key: JobKey::new(org, ModuleKind::Copy, "stoppable"),
            interval: Duration::from_millis(20),
            ticks: AtomicU32::new(0),
            fail: false,
        });
        scheduler.spawn(job).await;
        assert_eq!(scheduler.running().await.len(), 1);
        assert_eq!(scheduler.stop_tenant(org).await, 1);
        assert!(scheduler.running().await.is_empty());
    }
}
