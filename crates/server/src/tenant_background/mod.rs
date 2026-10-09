//! Tenant-scoped background work (STEP 3 files 49–52 + the job-context
//! layer).
//!
//! The existing recovery/reconciliation workers are process-wide; this
//! module adds the tenant-scoped layer: per-tenant recurring jobs that
//! run ONLY while the tenant's runtime is lease-live, and a supervisor
//! that reacts to configuration changes (a module disabled for a tenant
//! stops that module's jobs for it, immediately, not at the next tick).
//!
//! | file | concern |
//! |---|---|
//! | `jobs.rs` | [`jobs::TenantJob`] — the job descriptor + registry of handles |
//! | `job_context.rs` | [`job_context::JobContext`] — the explicit tenant context every tick carries |
//! | `job_guard.rs` | [`job_guard::JobGuard`] — no tick without valid tenant/runtime/fence context |
//! | `job_identity.rs` | [`job_identity::JobIdentity`] — stable tenant-aware identity for dedup/recovery |
//! | `scheduler.rs` | [`scheduler::TenantJobScheduler`] — spawn/tick/stop with fence checks |
//! | `supervisor.rs` | [`supervisor::TenantBackgroundSupervisor`] — config-diff + runtime reactions |
//!
//! Discipline:
//!
//! * A job tick first verifies the fence; a rotated/stale runtime stops
//!   its jobs — the background layer never outlives its mandate.
//! * No job may run for a tenant whose module was disabled in config.
//! * No anonymous tenant job exists: every tick carries a
//!   [`JobContext`] with an explicit `job:<module>:<name>` principal.
//! * Errors are logged and the job continues (bounded backoff is the
//!   job's own concern); the scheduler never panics a worker.

pub mod job_context;
pub mod job_guard;
pub mod job_identity;
pub mod jobs;
pub mod scheduler;
pub mod supervisor;

pub use job_context::{JobContext, JobContextError};
pub use job_guard::{JobClass, JobGuard};
pub use job_identity::JobIdentity;
pub use jobs::{JobKey, TenantJob};
pub use scheduler::TenantJobScheduler;
pub use supervisor::TenantBackgroundSupervisor;
