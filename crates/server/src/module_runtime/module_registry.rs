//! Tenant module registry (PROMPT 4/10 §A file 4).
//!
//! Maps `(organization, module)` to EXACTLY ONE valid tenant module
//! instance. Invariants:
//!
//! * one tenant never has two live instances of the same module — a
//!   duplicate registration is refused unless it is a legitimate
//!   ROTATION (a strictly newer generation superseding the old
//!   instance, which the caller must then drain);
//! * removal is generation-checked — a stale remover cannot clobber a
//!   newer instance;
//! * lookups are tenant-scoped by construction: the caller presents
//!   the organization, and the registry only ever answers with that
//!   tenant's handles.
//!
//! The registry stores HANDLES (identity + phase), never engines —
//! engines are owned by their run tasks.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};

use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;

use crate::module_runtime::module_handle::ModuleHandle;
use crate::module_runtime::module_lifecycle::ModulePhase;

/// Why a registration was refused. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    /// A live instance of the same module already exists for the
    /// tenant at the SAME generation (a duplicate, not a rotation).
    DuplicateLiveInstance {
        organization_id: OrganizationId,
        module: BotModule,
        generation: bot_core::tenant::RuntimeGeneration,
    },
    /// A registration may only supersede a live instance with a
    /// strictly NEWER generation (same-generation re-registration and
    /// downgrades are refused).
    NotARotation,
    /// Only a terminal handle may be replaced at the same generation.
    InstanceStillLive,
}

impl RegistrationError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RegistrationError::DuplicateLiveInstance { .. } => "duplicate_live_instance",
            RegistrationError::NotARotation => "not_a_rotation",
            RegistrationError::InstanceStillLive => "instance_still_live",
        }
    }
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrationError::DuplicateLiveInstance {
                organization_id,
                module,
                generation,
            } => write!(
                f,
                "module registry: tenant {organization_id} already has a live {} instance at generation {generation}",
                module.as_str()
            ),
            RegistrationError::NotARotation => {
                write!(f, "module registry: replacement must use a newer generation")
            }
            RegistrationError::InstanceStillLive => {
                write!(f, "module registry: the existing instance is still live")
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// The registry: `(organization, module)` → exactly one handle.
#[derive(Default)]
pub struct TenantModuleRegistry {
    handles: RwLock<HashMap<(OrganizationId, BotModule), ModuleHandle>>,
}

impl TenantModuleRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        TenantModuleRegistry {
            handles: RwLock::new(HashMap::new()),
        }
    }

    /// Register a handle. Returns the superseded handle when this is a
    /// legitimate ROTATION (the caller MUST drain it); `None` when the
    /// slot was free or held by a terminal handle.
    pub fn register(
        &self,
        handle: ModuleHandle,
    ) -> Result<Option<ModuleHandle>, RegistrationError> {
        let key = (handle.organization_id(), handle.module());
        let mut handles = self.handles.write().expect("module registry lock poisoned");
        match handles.get(&key) {
            None => {
                handles.insert(key, handle);
                Ok(None)
            }
            Some(existing) => {
                if !existing.phase().is_terminal() {
                    // A live instance exists: only a strictly newer
                    // generation may supersede it (runtime rotation).
                    if handle.generation() <= existing.generation() {
                        return Err(RegistrationError::DuplicateLiveInstance {
                            organization_id: key.0,
                            module: key.1,
                            generation: existing.generation(),
                        });
                    }
                    let superseded = handles.insert(key, handle);
                    Ok(superseded)
                } else {
                    // Terminal handle: the slot is free for a fresh
                    // instance of any generation (normally newer).
                    if handle.generation() <= existing.generation() {
                        return Err(RegistrationError::NotARotation);
                    }
                    let superseded = handles.insert(key, handle);
                    Ok(superseded)
                }
            }
        }
    }

    /// The tenant's handle for one module, if registered.
    pub fn get(&self, organization_id: OrganizationId, module: BotModule) -> Option<ModuleHandle> {
        self.handles
            .read()
            .expect("module registry lock poisoned")
            .get(&(organization_id, module))
            .cloned()
    }

    /// All handles of ONE tenant (never another tenant's).
    pub fn modules_for(&self, organization_id: OrganizationId) -> Vec<ModuleHandle> {
        self.handles
            .read()
            .expect("module registry lock poisoned")
            .iter()
            .filter(|((org, _), _)| *org == organization_id)
            .map(|(_, handle)| handle.clone())
            .collect()
    }

    /// Remove the tenant's handle for a module, but ONLY if it is the
    /// presented generation — a stale remover can never clobber a
    /// newer instance. Returns whether the removal happened.
    pub fn remove(
        &self,
        organization_id: OrganizationId,
        module: BotModule,
        generation: bot_core::tenant::RuntimeGeneration,
    ) -> bool {
        let mut handles = self.handles.write().expect("module registry lock poisoned");
        match handles.get(&(organization_id, module)) {
            Some(existing) if existing.generation() == generation => {
                handles.remove(&(organization_id, module));
                true
            }
            _ => false,
        }
    }

    /// Apply a phase update to the tenant's handle for a module, but
    /// ONLY if it is the presented generation (a stale updater can
    /// never move a newer instance). Returns the updated handle.
    pub fn update_phase(
        &self,
        organization_id: OrganizationId,
        module: BotModule,
        generation: bot_core::tenant::RuntimeGeneration,
        phase: ModulePhase,
        at: DateTime<Utc>,
    ) -> Option<ModuleHandle> {
        let mut handles = self.handles.write().expect("module registry lock poisoned");
        let key = (organization_id, module);
        if let Some(existing) = handles.get(&key) {
            if existing.generation() == generation {
                let updated = ModuleHandle::new(existing.instance().clone(), phase, at);
                handles.insert(key, updated.clone());
                return Some(updated);
            }
        }
        None
    }

    /// Total registered handles (diagnostics).
    pub fn len(&self) -> usize {
        self.handles
            .read()
            .expect("module registry lock poisoned")
            .len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_runtime::tenant_module_instance::TenantModuleInstance;
    use bot_core::execution::TenantExecutionContext;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::ExecutionMode;
    use bot_core::tenant::{
        RuntimeGeneration, RuntimeId, SignerProvider, TenantSignerRef, TenantWalletRef,
    };

    fn instance_for(
        org: OrganizationId,
        runtime: RuntimeId,
        generation: RuntimeGeneration,
        module: BotModule,
    ) -> TenantModuleInstance {
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
        )
        .unwrap();
        let now = Utc::now();
        let mut checklist = AuthorityChecklist::new();
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

    fn running_handle(
        org: OrganizationId,
        runtime: RuntimeId,
        generation: RuntimeGeneration,
        module: BotModule,
    ) -> ModuleHandle {
        ModuleHandle::new(
            instance_for(org, runtime, generation, module),
            ModulePhase::Running,
            Utc::now(),
        )
    }

    #[test]
    fn one_tenant_one_module_one_live_instance() {
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let gen = RuntimeGeneration::first();
        let registry = TenantModuleRegistry::new();
        assert!(registry
            .register(running_handle(org, runtime, gen, BotModule::Sniper))
            .is_ok());
        // A duplicate at the SAME generation is refused.
        let err = registry
            .register(running_handle(org, runtime, gen, BotModule::Sniper))
            .unwrap_err();
        assert_eq!(err.as_str(), "duplicate_live_instance");
        // But the same tenant may hold OTHER modules.
        assert!(registry
            .register(running_handle(org, runtime, gen, BotModule::Copy))
            .is_ok());
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn rotation_supersedes_and_returns_the_old_handle_for_draining() {
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let gen1 = RuntimeGeneration::first();
        let gen2 = gen1.next().unwrap();
        let registry = TenantModuleRegistry::new();
        registry
            .register(running_handle(org, runtime, gen1, BotModule::Copy))
            .expect("initial registration");
        // A NEWER generation supersedes the live instance and hands
        // the old handle back for draining.
        let superseded = registry
            .register(running_handle(org, runtime, gen2, BotModule::Copy))
            .expect("rotation")
            .expect("superseded handle");
        assert_eq!(superseded.generation(), gen1);
        // The registry now serves the new generation.
        assert_eq!(
            registry.get(org, BotModule::Copy).unwrap().generation(),
            gen2
        );
    }

    #[test]
    fn a_terminal_slot_only_accepts_a_newer_generation() {
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let gen1 = RuntimeGeneration::first();
        let registry = TenantModuleRegistry::new();
        let stopped = ModuleHandle::new(
            instance_for(org, runtime, gen1, BotModule::Sniper),
            ModulePhase::Stopped,
            Utc::now(),
        );
        registry.register(stopped).expect("register stopped");
        // Same-or-older generation cannot take the slot.
        let err = registry
            .register(running_handle(org, runtime, gen1, BotModule::Sniper))
            .unwrap_err();
        assert_eq!(err.as_str(), "not_a_rotation");
        // A newer generation can.
        assert!(registry
            .register(running_handle(
                org,
                runtime,
                gen1.next().unwrap(),
                BotModule::Sniper
            ))
            .is_ok());
    }

    #[test]
    fn lookups_and_removals_are_tenant_and_generation_scoped() {
        let (org_a, org_b) = (OrganizationId::new(), OrganizationId::new());
        let (runtime_a, runtime_b) = (RuntimeId::new(), RuntimeId::new());
        let gen = RuntimeGeneration::first();
        let registry = TenantModuleRegistry::new();
        registry
            .register(running_handle(org_a, runtime_a, gen, BotModule::Sniper))
            .expect("register A");
        registry
            .register(running_handle(org_b, runtime_b, gen, BotModule::Sniper))
            .expect("register B");
        // Tenant A never sees tenant B's handles.
        assert_eq!(registry.modules_for(org_a).len(), 1);
        assert_eq!(registry.modules_for(org_a)[0].organization_id(), org_a);
        assert!(registry.get(org_b, BotModule::Copy).is_none());
        // A stale (wrong-generation) removal does nothing.
        assert!(!registry.remove(org_a, BotModule::Sniper, gen.next().unwrap()));
        assert!(registry.get(org_a, BotModule::Sniper).is_some());
        // The matching generation removes.
        assert!(registry.remove(org_a, BotModule::Sniper, gen));
        assert!(registry.get(org_a, BotModule::Sniper).is_none());
        // Tenant B is untouched.
        assert!(registry.get(org_b, BotModule::Sniper).is_some());
    }

    #[test]
    fn phase_updates_are_generation_checked() {
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let gen1 = RuntimeGeneration::first();
        let gen2 = gen1.next().unwrap();
        let registry = TenantModuleRegistry::new();
        registry
            .register(running_handle(org, runtime, gen1, BotModule::Copy))
            .expect("register");
        // Rotate to gen2, then a stale gen1 drain update must NOT move
        // the new instance.
        registry
            .register(running_handle(org, runtime, gen2, BotModule::Copy))
            .expect("rotate");
        assert!(registry
            .update_phase(
                org,
                BotModule::Copy,
                gen1,
                ModulePhase::Draining,
                Utc::now()
            )
            .is_none());
        assert_eq!(
            registry.get(org, BotModule::Copy).unwrap().phase(),
            ModulePhase::Running
        );
        // The current generation updates.
        let updated = registry
            .update_phase(
                org,
                BotModule::Copy,
                gen2,
                ModulePhase::Draining,
                Utc::now(),
            )
            .expect("update");
        assert_eq!(updated.phase(), ModulePhase::Draining);
        assert_eq!(
            registry.get(org, BotModule::Copy).unwrap().phase(),
            ModulePhase::Draining
        );
    }
}
