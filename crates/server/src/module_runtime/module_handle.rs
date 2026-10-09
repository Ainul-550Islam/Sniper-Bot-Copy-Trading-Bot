//! Typed handle to a tenant module instance (PROMPT 4/10 §A file 3).
//!
//! A [`ModuleHandle`] is the ONLY thing callers may hold after a
//! module instance is registered. It carries the instance identity
//! plus the lifecycle phase, and answers the two questions every
//! caller must ask before touching the engine:
//!
//! * is this handle still CURRENT for the tenant's runtime (fence
//!   check against a freshly-read [`TenantRuntimeRecord`])?
//! * does the handle permit execution in its current phase?
//!
//! The handle deliberately does NOT hold the engine itself — engines
//! are owned by their run tasks. Holding an identity handle means a
//! rotated runtime fences the holder out even if the task is slow to
//! notice.

use chrono::{DateTime, Utc};

use bot_core::models::BotModule;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

use crate::module_runtime::module_lifecycle::ModulePhase;
use crate::module_runtime::tenant_module_instance::{InstanceIdentityError, TenantModuleInstance};
use crate::runtime_registry::model::TenantRuntimeRecord;

/// A strong typed handle to one registered tenant module instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHandle {
    instance: TenantModuleInstance,
    phase: ModulePhase,
    /// When the phase last changed (audit trail).
    since: DateTime<Utc>,
}

impl ModuleHandle {
    /// A fresh handle for an instance entering the lifecycle.
    pub fn new(instance: TenantModuleInstance, phase: ModulePhase, since: DateTime<Utc>) -> Self {
        ModuleHandle {
            instance,
            phase,
            since,
        }
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.instance.organization_id()
    }

    /// The module this handle runs.
    pub fn module(&self) -> BotModule {
        self.instance.module()
    }

    /// The runtime instance identity.
    pub fn runtime_id(&self) -> RuntimeId {
        self.instance.runtime_id()
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.instance.generation()
    }

    /// The current lifecycle phase.
    pub fn phase(&self) -> ModulePhase {
        self.phase
    }

    /// When the phase last changed.
    pub fn since(&self) -> DateTime<Utc> {
        self.since
    }

    /// The underlying instance identity.
    pub fn instance(&self) -> &TenantModuleInstance {
        &self.instance
    }

    /// Is this handle still CURRENT? A handle is current when the
    /// freshly-read record still names the same tenant, runtime and
    /// generation, the record is live (active + lease), and the handle
    /// has not entered a terminal phase. Anything else means the holder
    /// is fenced out and must stop.
    pub fn is_current(&self, record: &TenantRuntimeRecord, now: DateTime<Utc>) -> bool {
        if self.phase.is_terminal() {
            return false;
        }
        self.instance.verify_record(record, now).is_ok()
    }

    /// The fence verdict with the reason (for observability).
    pub fn fence_error(
        &self,
        record: &TenantRuntimeRecord,
        now: DateTime<Utc>,
    ) -> Option<InstanceIdentityError> {
        if self.phase.is_terminal() {
            return Some(InstanceIdentityError::RuntimeNotActive);
        }
        self.instance.verify_record(record, now).err()
    }

    /// Does this handle permit execution right now? Only non-terminal,
    /// non-draining phases do — a draining module finishes in-flight
    /// work but starts nothing new.
    pub fn permits_execution(&self) -> bool {
        self.phase.permits_execution()
    }

    /// Does this handle belong to the given tenant and module?
    pub fn belongs_to(&self, organization_id: OrganizationId, module: BotModule) -> bool {
        self.instance.organization_id() == organization_id && self.instance.module() == module
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_runtime::tenant_module_instance::TenantModuleInstance;
    use bot_core::execution::TenantExecutionContext;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::{SignerProvider, TenantSignerRef, TenantWalletRef};

    fn instance(module: BotModule) -> TenantModuleInstance {
        let org = OrganizationId::new();
        let runtime = RuntimeId::new();
        let generation = RuntimeGeneration::first();
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
        )
        .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        use solana_sdk::signer::Signer;
        let address = solana_sdk::signature::Keypair::new().pubkey().to_string();
        let wallet = TenantWalletRef::new(org, &address).unwrap();
        let signer = TenantSignerRef::new(org, SignerProvider::Local, "k").unwrap();
        let context = TenantExecutionContext::issue(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap();
        TenantModuleInstance::from_context(&context, true)
    }

    fn live_record(handle: &ModuleHandle, now: DateTime<Utc>) -> TenantRuntimeRecord {
        TenantRuntimeRecord::new_active(
            handle.organization_id(),
            handle.runtime_id(),
            handle.generation(),
            "test-worker",
            now,
        )
    }

    #[test]
    fn a_running_handle_is_current_and_permits_execution() {
        let now = Utc::now();
        let handle = ModuleHandle::new(instance(BotModule::Sniper), ModulePhase::Running, now);
        assert!(handle.is_current(&live_record(&handle, now), now));
        assert!(handle.permits_execution());
        assert!(handle.belongs_to(handle.organization_id(), BotModule::Sniper));
        assert!(!handle.belongs_to(handle.organization_id(), BotModule::Copy));
    }

    #[test]
    fn rotation_and_termination_fence_the_holder_out() {
        let now = Utc::now();
        let handle = ModuleHandle::new(instance(BotModule::Copy), ModulePhase::Running, now);
        // Rotated runtime record: fenced out.
        let mut rotated = live_record(&handle, now);
        rotated.generation = RuntimeGeneration::first().next().unwrap();
        assert!(!handle.is_current(&rotated, now));
        assert_eq!(
            handle.fence_error(&rotated, now).unwrap().as_str(),
            "generation_mismatch"
        );
        // Terminal phase: fenced out regardless of the record.
        let stopped = ModuleHandle::new(handle.instance().clone(), ModulePhase::Stopped, now);
        assert!(!stopped.is_current(&live_record(&stopped, now), now));
        assert!(!stopped.permits_execution());
    }

    #[test]
    fn draining_finishes_inflight_work_but_starts_nothing_new() {
        let now = Utc::now();
        let draining = ModuleHandle::new(instance(BotModule::Sniper), ModulePhase::Draining, now);
        assert!(!draining.permits_execution());
        // Draining is not terminal: the record still matches, the
        // in-flight work may complete.
        assert!(draining.is_current(&live_record(&draining, now), now));
    }
}
