//! Tenant module runtime bridge (PROMPT 4/10 §A).
//!
//! The layer that turns a tenant's runtime into RUNNING, FENCED
//! module engines — instead of the process-global, tenant-blind
//! objects the single-operator path uses.
//!
//! | file | concern |
//! |---|---|
//! | `tenant_module_instance.rs` | [`TenantModuleInstance`] — the tenant-specific identity of one module instance (organization, runtime, generation, module, mode, wallet/signer bindings, enabled state), derived from an issued context, never duplicating the core identity types |
//! | `tenant_module_factory.rs` | [`TenantModuleFactory`] — builds Sniper/Copy (and, once wired, Polymarket) engines from an instance, fenced against the live runtime record, with repository-backed sinks; unwired modules are refused explicitly |
//! | `module_handle.rs` | [`ModuleHandle`] — the strong typed handle callers hold; answers fence-currency and execution-permission questions |
//! | `module_registry.rs` | [`TenantModuleRegistry`] — `(organization, module)` → exactly one valid instance, with rotation and generation-checked updates |
//! | `module_lifecycle.rs` | [`ModulePhase`] + the validated transition table; starts require a live fence |
//! | `module_health.rs` | [`ModuleHealth`] — healthy / degraded / fail-closed classification; fail-closed never permits execution |
//!
//! Discipline:
//!
//! * every engine construction passes the instance fence (live record:
//!   same tenant, same runtime, same generation, active, lease live);
//! * an unattached tenant broadcast guard is an automatic fail-closed
//!   verdict — money never moves unguarded;
//! * unwired modules are refused with `module_not_wired`, never
//!   silently replaced by a deployment-global engine;
//! * the single-operator path in `main.rs` is untouched by this layer.

pub mod module_handle;
pub mod module_health;
pub mod module_lifecycle;
pub mod module_registry;
pub mod tenant_module_factory;
pub mod tenant_module_instance;

pub use module_handle::ModuleHandle;
pub use module_health::{HealthObservation, HealthReason, HealthState, ModuleHealth};
pub use module_lifecycle::{
    authorize_start, transition, LifecycleError, LifecycleEvent, ModulePhase,
};
pub use module_registry::{RegistrationError, TenantModuleRegistry};
pub use tenant_module_factory::{
    ModuleEngine, RepoCopySink, RepoExecutionSink, TenantModuleFactory,
};
pub use tenant_module_instance::{InstanceIdentityError, TenantModuleInstance};
