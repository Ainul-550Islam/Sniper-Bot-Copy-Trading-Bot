//! STEP 3 integration: the tenant background layer — fence-gated ticks
//! and the supervisor's reactions.

use std::sync::Arc;
use std::time::Duration;

use bot_core::models::ExecutionMode;
use bot_core::tenant::{ModuleKind, OrganizationId};
use chrono::Utc;
use sniper_suite::runtime_registry::{MemoryRuntimeStore, RuntimeRegistryService};
use sniper_suite::tenant_background::jobs::{JobHandles, JobKey, TenantJob};
use sniper_suite::tenant_background::scheduler::TenantJobScheduler;
use sniper_suite::tenant_background::supervisor::{JobFactories, TenantBackgroundSupervisor};
use sniper_suite::tenant_config::store::ConfigStore;
use sniper_suite::tenant_config::{
    ConfigCache, GlobalSafetyBounds, MemoryConfigStore, TenantConfigModel,
};
use std::sync::atomic::{AtomicU32, Ordering};

const MODES: [ExecutionMode; 1] = [ExecutionMode::Paper];

fn bounds() -> GlobalSafetyBounds {
    GlobalSafetyBounds {
        max_position_usd: 10_000.0,
        daily_loss_usd_cap: 1_000.0,
        max_slippage_bps: 500,
        allowed_modes: &MODES,
    }
}

struct TickJob {
    key: JobKey,
    ticks: AtomicU32,
}

#[async_trait::async_trait]
impl TenantJob for TickJob {
    fn key(&self) -> JobKey {
        self.key.clone()
    }
    fn interval(&self) -> Duration {
        Duration::from_millis(25)
    }
    async fn run(
        &self,
        _token: &sniper_suite::runtime_registry::FenceToken,
    ) -> bot_core::error::BotResult<()> {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn jobs_run_only_while_the_fence_is_current() {
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    let org = OrganizationId::new();
    runtime
        .ensure_active(org, "worker", Utc::now())
        .await
        .unwrap();

    let scheduler = Arc::new(TenantJobScheduler::new(runtime.clone()));
    let job = Arc::new(TickJob {
        key: JobKey::new(org, ModuleKind::Copy, "tick"),
        ticks: AtomicU32::new(0),
    });
    let observed = Arc::clone(&job);
    scheduler.spawn(job).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    let ran = observed.ticks.load(Ordering::SeqCst);
    assert!(ran >= 2, "expected ticks, got {ran}");

    // Rotating the runtime invalidates the job's fence: it exits by itself.
    runtime.rotate(org, "worker-2", Utc::now()).await.unwrap();
    let settled = observed.ticks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let after = observed.ticks.load(Ordering::SeqCst);
    assert_eq!(after, settled, "the job must stop ticking after rotation");
}

#[tokio::test]
async fn the_supervisor_sweep_reaps_and_reports() {
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
        .spawn(Arc::new(TickJob {
            key: JobKey::new(org, ModuleKind::Copy, "tick"),
            ticks: AtomicU32::new(0),
        }))
        .await;

    let config_store = Arc::new(MemoryConfigStore::new());
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
    config.get(org).await.unwrap(); // warm: no drift

    let supervisor = TenantBackgroundSupervisor::new(
        scheduler.clone(),
        config,
        runtime.clone(),
        Arc::new(JobFactories::new()),
        bounds(),
    );

    // A sweep at the current time: nothing to do.
    let report = supervisor.sweep_once().await.unwrap();
    assert_eq!(report.tenants_reaped, 0);

    // A sweep with an ancient runtime on the clock: reaped + jobs stopped.
    let later = Utc::now() + chrono::Duration::seconds(3_600);
    let report = supervisor.sweep_once_at(later).await.unwrap();
    assert_eq!(report.tenants_reaped, 1);
    assert_eq!(report.jobs_stopped, 1);
    assert!(scheduler.running().await.is_empty());
}

#[tokio::test]
async fn handle_stops_are_scoped_by_tenant_and_module() {
    let handles = JobHandles::new();
    let a = OrganizationId::new();
    handles
        .insert(
            JobKey::new(a, ModuleKind::Copy, "one"),
            tokio::spawn(async {}),
        )
        .await;
    handles
        .insert(
            JobKey::new(a, ModuleKind::Sniper, "two"),
            tokio::spawn(async {}),
        )
        .await;
    handles
        .insert(
            JobKey::new(OrganizationId::new(), ModuleKind::Copy, "three"),
            tokio::spawn(async {}),
        )
        .await;

    assert_eq!(handles.remove_tenant_module(a, ModuleKind::Copy).await, 1);
    assert_eq!(handles.len().await, 2);
    assert_eq!(handles.remove_tenant(a).await, 1);
    assert_eq!(handles.len().await, 1);
}
