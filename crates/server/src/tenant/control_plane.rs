//! Boot-time construction of the tenant execution control plane.
//!
//! Before this module existed, `RuntimeRegistryService` and
//! `TenantExecutionGateway` were only ever built inside `#[cfg(test)]` blocks:
//! `tenant_runtimes` (migration 0025) never received a row for the running
//! process, the lease/reaper machinery never ran, and the "which runtime may
//! execute for this tenant right now" question could not be answered by the
//! deployment at all. Everything compiled and every unit test passed; the
//! control plane simply was not assembled.
//!
//! [`spawn`] is that assembly, called once from `main` after the database and
//! the deployment organization exist:
//!
//! 1. `PgRuntimeStore` (durable, migration 0025) → `RuntimeRegistryService`;
//! 2. `ensure_active(deployment_org, worker_id)` — the process registers as
//!    that tenant's runtime, at the next generation, refusing to create a
//!    split brain against a healthy peer;
//! 3. the heartbeat loop keeps the row fresh, the reaper stops runtimes whose
//!    heartbeat went stale (and their fence tokens with them);
//! 4. `PgConfigStore` → `ConfigCache`, `PgTenantBindingRegistry`, and a
//!    SaaS-backed `EntitlementProvider` complete the
//!    [`ExecutionInputs`](crate::tenant::ExecutionInputs) the gateway needs.
//!
//! Nothing here invents a value: every field comes from PostgreSQL or the
//! platform defaults, and a failed registration is a startup error, not a
//! warning that hides a dead control plane.

use std::sync::Arc;

use chrono::Utc;
use tracing::{info, warn};

use bot_core::db::Database;
use bot_core::error::BotResult;
use bot_core::tenant::{OrganizationId, TenantEntitlementView};

use crate::runtime_registry::{
    lease::LeasePolicy, spawn_default_reaper, spawn_heartbeat_loop, EnsureOutcome, PgRuntimeStore,
    RuntimeRegistryService,
};
use crate::saas::SaasStore;
use crate::tenant::gateway::{EntitlementProvider, ExecutionInputs, TenantExecutionGateway};
use crate::tenant::registry::PgTenantBindingRegistry;
use crate::tenant_config::{ConfigCache, GlobalSafetyBounds, PgConfigStore};

/// Entitlements resolved from the SaaS store (plans, subscriptions, overrides)
/// on every admission — never cached in this process, so a plan downgrade or a
/// suspension written by another replica is observed immediately.
pub struct SaasEntitlementProvider {
    saas: Arc<SaasStore>,
}

impl SaasEntitlementProvider {
    /// Bind to the deployment's SaaS store.
    pub fn new(saas: Arc<SaasStore>) -> Self {
        SaasEntitlementProvider { saas }
    }
}

#[async_trait::async_trait]
impl EntitlementProvider for SaasEntitlementProvider {
    async fn entitlements(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<TenantEntitlementView> {
        let set = self
            .saas
            .entitlements_of(organization_id, Utc::now())
            .await?;
        Ok(TenantEntitlementView::new(set))
    }
}

/// The assembled control plane, kept alive for the process lifetime.
pub struct TenantControlPlane {
    /// The runtime registry (fencing, rotation, reaping).
    pub runtime: Arc<RuntimeRegistryService>,
    /// The gateway every tenant execution passes through.
    pub gateway: Arc<TenantExecutionGateway>,
    /// This process's registered runtime id (in `tenant_runtimes`).
    pub runtime_id: bot_core::tenant::RuntimeId,
    /// Its generation for the deployment tenant.
    pub generation: u64,
}

/// Assemble the tenant control plane and register this process as the
/// deployment tenant's live runtime.
///
/// `worker_id` should be stable for a process identity (host + pid) so a
/// restart of the same identity rotates its own row rather than being refused
/// as a second live runtime.
pub async fn spawn(
    db: Arc<Database>,
    saas: Arc<SaasStore>,
    deployment_org: OrganizationId,
    worker_id: &str,
) -> BotResult<TenantControlPlane> {
    let store = Arc::new(PgRuntimeStore::new(db.clone()));
    let runtime = Arc::new(RuntimeRegistryService::new(store));

    // Register BEFORE any tenant execution is admitted: the fence guard
    // requires a live runtime row for the tenant.
    //
    // `ensure_active` deliberately refuses to create a split brain: if a peer
    // holds the tenant with a FRESH heartbeat, registration is refused. After
    // an ungraceful restart of this very process (crash, OOM kill, container
    // stop) its row is still "healthy" until `stale_after` elapses, so boot
    // would fail for up to that long. A supervisor restarting a dead worker is
    // the normal case, so we WAIT for the peer's heartbeat to go stale (bounded
    // by the lease policy) and then rotate — never take over while a peer is
    // demonstrably alive.
    let policy = runtime.policy();
    let wait_budget = std::time::Duration::from_secs(
        u64::try_from(policy.stale_after.num_seconds().max(0)).unwrap_or(90) + 30,
    );
    let deadline = tokio::time::Instant::now() + wait_budget;
    let outcome = loop {
        match runtime
            .ensure_active(deployment_org, worker_id, Utc::now())
            .await
        {
            Ok(outcome) => break outcome,
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                warn!(
                    worker = worker_id,
                    error = %error,
                    "tenant runtime registry: tenant held by another live runtime — waiting for its heartbeat to go stale"
                );
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    };
    let (record, action) = match outcome {
        EnsureOutcome::Registered(record) => (record, "registered"),
        EnsureOutcome::Rotated(record) => (record, "rotated"),
    };
    info!(
        worker = worker_id,
        runtime = %record.runtime_id,
        generation = record.generation.raw(),
        action,
        "tenant runtime registry: deployment runtime is live"
    );

    // The heartbeat keeps the row fresh (and the fence valid); the reaper
    // stops runtimes whose heartbeat stopped. Both are real loops holding the
    // same service the gateway checks against.
    let _heartbeat = spawn_heartbeat_loop(
        (*runtime.store()).clone(),
        record.fence_token(),
        LeasePolicy::default(),
    );
    let _reaper = spawn_default_reaper(runtime.clone());

    let config = Arc::new(ConfigCache::new(Arc::new(PgConfigStore::new(db.clone()))));
    let bindings = Arc::new(PgTenantBindingRegistry::new(db.clone()));
    let entitlements = Arc::new(SaasEntitlementProvider::new(saas));
    let inputs = ExecutionInputs::new(
        runtime.clone(),
        config,
        bindings,
        entitlements,
        GlobalSafetyBounds::platform_defaults(),
    );
    let gateway = Arc::new(TenantExecutionGateway::new(Arc::new(inputs)));

    // One read-only sanity check that the freshly registered runtime is what
    // the registry reports — a registration that cannot be read back would
    // make every fence check deny, and we would rather say so at boot.
    match runtime.current(deployment_org).await {
        Ok(Some(live)) if live.runtime_id == record.runtime_id => {
            info!(
                runtime = %live.runtime_id,
                "tenant runtime registry: read-back confirmed"
            );
        }
        Ok(other) => warn!(
            found = ?other.map(|r| r.runtime_id.to_string()),
            expected = %record.runtime_id,
            "tenant runtime registry: read-back mismatch — tenant admissions will deny until resolved"
        ),
        Err(e) => warn!(error = %e, "tenant runtime registry: read-back failed"),
    }
    Ok(TenantControlPlane {
        runtime,
        gateway,
        runtime_id: record.runtime_id,
        generation: record.generation.raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entitlement_provider_is_constructed_from_the_saas_store() {
        // Type-level: the provider must be object-safe behind the trait the
        // gateway consumes, otherwise the control plane cannot be assembled.
        fn assert_provider<T: EntitlementProvider>() {}
        assert_provider::<SaasEntitlementProvider>();
    }
}
