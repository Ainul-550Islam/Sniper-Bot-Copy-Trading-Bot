//! Background worker tenant propagation and denial tests
//! (tenant-isolation file 70).
//!
//! Positive: a job context carries an explicit `job:<module>:<name>`
//! principal for the right tenant; a live fence with an active tenant
//! authorizes trading-class work; maintenance work keeps running for
//! a suspended tenant.
//!
//! Negative: a rotated (superseded) fence never authorizes; a foreign
//! tenant's fence token is rejected at construction; an empty job
//! name is rejected; job identities of different tenants never
//! collide (dedup/recovery keys are tenant-qualified).

use std::sync::Arc;

use chrono::Utc;

use sniper_suite::runtime_registry::store::MemoryRuntimeStore;
use sniper_suite::runtime_registry::{FenceToken, RuntimeRegistryService};
use sniper_suite::tenant::context::ContextOrigin;
use sniper_suite::tenant_background::job_context::{JobContext, JobContextError};
use sniper_suite::tenant_background::job_guard::{JobClass, JobGuard};
use sniper_suite::tenant_background::job_identity::JobIdentity;

use bot_core::tenant::{
    ModuleKind, Organization, OrganizationId, OrganizationStatus, RuntimeGeneration, RuntimeId,
};

fn service() -> Arc<RuntimeRegistryService> {
    Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )))
}

fn organization_with(organization_id: OrganizationId, status: OrganizationStatus) -> Organization {
    let mut organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
    organization.status = status;
    organization
}

fn foreign_fence() -> FenceToken {
    FenceToken {
        organization_id: OrganizationId::new(),
        runtime_id: RuntimeId::new(),
        generation: RuntimeGeneration::first(),
    }
}

#[tokio::test]
async fn a_job_tick_carries_an_explicit_tenant_principal() {
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
        ModuleKind::Copy,
        "recon",
        record.fence_token(),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(context.organization_id(), org);
    assert_eq!(context.tenant().principal(), "job:copy:recon");
    assert_eq!(context.tenant().origin(), ContextOrigin::Job);
    assert_eq!(
        context.identity().dedup_key(),
        JobIdentity::new(org, ModuleKind::Copy, "recon").dedup_key()
    );
}

#[tokio::test]
async fn a_live_fence_with_an_active_tenant_authorizes_work() {
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
        ModuleKind::Copy,
        "recon",
        record.fence_token(),
        Utc::now(),
    )
    .unwrap();
    let guard = JobGuard::new(service);
    assert!(guard
        .authorize(&context, JobClass::Trading)
        .await
        .is_allow());
    assert!(guard
        .authorize(&context, JobClass::Maintenance)
        .await
        .is_allow());
    assert!(guard.fence_is_current(&context).await);
}

#[tokio::test]
async fn a_rotated_fence_never_authorizes_a_job_tick() {
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
        ModuleKind::Copy,
        "recon",
        first.fence_token(),
        Utc::now(),
    )
    .unwrap();
    let guard = JobGuard::new(service);
    let outcome = guard.authorize(&context, JobClass::Maintenance).await;
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "fence_failed");
    assert!(!guard.fence_is_current(&context).await);
}

#[tokio::test]
async fn a_foreign_fence_token_is_rejected_at_construction() {
    let organization = organization_with(OrganizationId::new(), OrganizationStatus::Active);
    let error = JobContext::new(
        &organization,
        ModuleKind::Copy,
        "recon",
        foreign_fence(),
        Utc::now(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        JobContextError::InvalidTenantContext("fence token belongs to a different tenant")
    );
}

#[test]
fn an_empty_job_name_is_rejected() {
    let organization = organization_with(OrganizationId::new(), OrganizationStatus::Active);
    let error = JobContext::new(
        &organization,
        ModuleKind::Copy,
        "",
        FenceToken {
            organization_id: organization.id,
            runtime_id: RuntimeId::new(),
            generation: RuntimeGeneration::first(),
        },
        Utc::now(),
    )
    .unwrap_err();
    assert_eq!(error, JobContextError::MissingJobName);
    assert_eq!(error.as_str(), "missing_job_name");
}

#[tokio::test]
async fn a_suspended_tenants_trading_jobs_stop_but_maintenance_continues() {
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
        ModuleKind::Copy,
        "recon",
        record.fence_token(),
        Utc::now(),
    )
    .unwrap();
    let guard = JobGuard::new(service);
    let trading = guard.authorize(&context, JobClass::Trading).await;
    assert_eq!(trading.deny_reason().unwrap().as_str(), "tenant_state");
    let maintenance = guard.authorize(&context, JobClass::Maintenance).await;
    assert!(maintenance.is_allow(), "{:?}", maintenance.deny_reason());
}

#[test]
fn job_identities_never_collide_across_tenants() {
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let a_recon = JobIdentity::new(a, ModuleKind::Copy, "recon");
    let b_recon = JobIdentity::new(b, ModuleKind::Copy, "recon");
    assert_ne!(a_recon.identity_string(), b_recon.identity_string());
    assert_ne!(a_recon.dedup_key(), b_recon.dedup_key());
    assert!(!a_recon.belongs_to(b));
    assert!(a_recon.belongs_to(a));
    // Restart-stable for the same tenant+job.
    assert_eq!(
        a_recon.dedup_key(),
        JobIdentity::new(a, ModuleKind::Copy, "recon").dedup_key()
    );
    // Different modules of one tenant differ too.
    let a_sweep = JobIdentity::new(a, ModuleKind::Sniper, "sweep");
    assert_ne!(a_recon.dedup_key(), a_sweep.dedup_key());
}

#[tokio::test]
async fn tenant_a_can_never_run_its_job_under_tenant_bs_fence() {
    // The registry has live runtimes for BOTH tenants; tenant A's job
    // context is built with tenant B's fence token — refused.
    let service = service();
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let a_record = service
        .ensure_active(a, "worker-a", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();
    let b_record = service
        .ensure_active(b, "worker-b", Utc::now())
        .await
        .unwrap()
        .record()
        .clone();

    // A's organization with B's fence: construction-level rejection.
    let error = JobContext::new(
        &organization_with(OrganizationId::new(), OrganizationStatus::Active),
        ModuleKind::Copy,
        "recon",
        b_record.fence_token(),
        Utc::now(),
    )
    .unwrap_err();
    assert!(matches!(error, JobContextError::InvalidTenantContext(_)));

    // And the honest A context authorizes only under A's own token.
    let context = JobContext::new(
        &organization_with(a, OrganizationStatus::Active),
        ModuleKind::Copy,
        "recon",
        a_record.fence_token(),
        Utc::now(),
    )
    .unwrap();
    let guard = JobGuard::new(service);
    assert!(guard
        .authorize(&context, JobClass::Maintenance)
        .await
        .is_allow());
}
