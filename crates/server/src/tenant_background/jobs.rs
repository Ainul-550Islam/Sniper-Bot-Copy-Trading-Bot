//! Tenant job descriptors (STEP 3 file 50).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bot_core::tenant::{ModuleKind, OrganizationId};

use crate::runtime_registry::FenceToken;

/// Identifies one running job (tenant + module + name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobKey {
    /// The tenant the job belongs to.
    pub organization_id: OrganizationId,
    /// The module the job serves.
    pub module: ModuleKind,
    /// The job's name within the module ("recon", "sweep", …).
    pub name: &'static str,
}

impl JobKey {
    /// Build a key.
    pub fn new(organization_id: OrganizationId, module: ModuleKind, name: &'static str) -> Self {
        JobKey {
            organization_id,
            module,
            name,
        }
    }
}

/// What one job run does. Implementations live in the module engines;
/// the scheduler only orchestrates.
#[async_trait::async_trait]
pub trait TenantJob: Send + Sync {
    /// The job's identity.
    fn key(&self) -> JobKey;

    /// How often the job wants to run.
    fn interval(&self) -> Duration;

    /// One run. `Ok(())` = done until next tick. Errors are logged and
    /// retried on the next tick — a job never takes the worker down.
    async fn run(&self, token: &FenceToken) -> bot_core::error::BotResult<()>;
}

/// A shareable job.
pub type SharedTenantJob = Arc<dyn TenantJob>;

/// The registry of spawned job handles (used by the supervisor to stop
/// jobs by tenant or module).
#[derive(Default)]
pub struct JobHandles {
    handles: tokio::sync::RwLock<HashMap<JobKey, tokio::task::JoinHandle<()>>>,
}

impl JobHandles {
    /// An empty registry.
    pub fn new() -> Self {
        JobHandles::default()
    }

    /// Register a running job's handle.
    pub async fn insert(&self, key: JobKey, handle: tokio::task::JoinHandle<()>) {
        // If a previous incarnation is somehow still present, abort it —
        // one job per key, always.
        if let Some(old) = self.handles.write().await.insert(key.clone(), handle) {
            old.abort();
        }
    }

    /// Abort and remove one job.
    pub async fn remove(&self, key: &JobKey) -> bool {
        self.handles
            .write()
            .await
            .remove(key)
            .map(|h| {
                h.abort();
                true
            })
            .unwrap_or(false)
    }

    /// Abort every job of one tenant. Returns how many were stopped.
    pub async fn remove_tenant(&self, organization_id: OrganizationId) -> usize {
        let mut handles = self.handles.write().await;
        let keys: Vec<JobKey> = handles
            .keys()
            .filter(|k| k.organization_id == organization_id)
            .cloned()
            .collect();
        let stopped = keys.len();
        for key in keys {
            if let Some(handle) = handles.remove(&key) {
                handle.abort();
            }
        }
        stopped
    }

    /// Abort every job of one tenant's module. Returns how many stopped.
    pub async fn remove_tenant_module(
        &self,
        organization_id: OrganizationId,
        module: ModuleKind,
    ) -> usize {
        let mut handles = self.handles.write().await;
        let keys: Vec<JobKey> = handles
            .keys()
            .filter(|k| k.organization_id == organization_id && k.module == module)
            .cloned()
            .collect();
        let stopped = keys.len();
        for key in keys {
            if let Some(handle) = handles.remove(&key) {
                handle.abort();
            }
        }
        stopped
    }

    /// Every running key (observability).
    pub async fn keys(&self) -> Vec<JobKey> {
        self.handles.read().await.keys().cloned().collect()
    }

    /// How many jobs are running.
    pub async fn len(&self) -> usize {
        self.handles.read().await.len()
    }

    /// Empty?
    pub async fn is_empty(&self) -> bool {
        self.handles.read().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::RuntimeGeneration;
    use bot_core::tenant::RuntimeId;

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
        async fn run(&self, _token: &FenceToken) -> bot_core::error::BotResult<()> {
            Ok(())
        }
    }

    fn token(org: OrganizationId) -> FenceToken {
        FenceToken {
            organization_id: org,
            runtime_id: RuntimeId::new(),
            generation: RuntimeGeneration::first(),
        }
    }

    #[tokio::test]
    async fn job_trait_is_implementable_with_a_plain_closure_free_struct() {
        let org = OrganizationId::new();
        let job = NoopJob {
            key: JobKey::new(org, ModuleKind::Copy, "recon"),
        };
        assert_eq!(job.key().name, "recon");
        assert_eq!(job.interval(), Duration::from_secs(60));
        job.run(&token(org)).await.unwrap();
    }

    #[tokio::test]
    async fn handles_track_and_stop_by_tenant_and_module() {
        let handles = JobHandles::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();

        for (org, module, name) in [
            (a, ModuleKind::Copy, "recon"),
            (a, ModuleKind::Copy, "sweep"),
            (a, ModuleKind::Sniper, "recon"),
            (b, ModuleKind::Copy, "recon"),
        ] {
            let handle = tokio::spawn(async {});
            handles.insert(JobKey::new(org, module, name), handle).await;
        }
        assert_eq!(handles.len().await, 4);

        // Stop one tenant's module.
        assert_eq!(handles.remove_tenant_module(a, ModuleKind::Copy).await, 2);
        assert_eq!(handles.len().await, 2);

        // Stop the whole tenant.
        assert_eq!(handles.remove_tenant(a).await, 1);
        assert_eq!(handles.len().await, 1);

        // The other tenant is untouched.
        let keys = handles.keys().await;
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].organization_id, b);
    }

    #[tokio::test]
    async fn inserting_the_same_key_twice_keeps_exactly_one() {
        let handles = JobHandles::new();
        let org = OrganizationId::new();
        let key = JobKey::new(org, ModuleKind::Copy, "recon");
        handles.insert(key.clone(), tokio::spawn(async {})).await;
        handles.insert(key, tokio::spawn(async {})).await;
        assert_eq!(handles.len().await, 1);
    }
}
