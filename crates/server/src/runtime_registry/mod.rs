//! Per-tenant runtime registry (STEP 3 files 38–44).
//!
//! | file | concern |
//! |---|---|
//! | `model.rs` | [`TenantRuntimeRecord`], [`RuntimeStatus`], [`FenceToken`] |
//! | `store.rs` | durable contract over `tenant_runtimes` (migration 0025) + in-memory implementation |
//! | `service.rs` | registration / resolution / rotation / reaping ([`RuntimeRegistryService`]) |
//! | `lease.rs` | heartbeat + optional hard-lease liveness policy ([`LeasePolicy`], [`LeaseVerdict`]) |
//! | `heartbeat.rs` | the tenant-aware heartbeat loop |
//! | `fencing.rs` | the fence check every guard shares ([`FenceVerdict`]) |
//! | `reaper.rs` | the periodic stale-runtime recovery pass ([`spawn_reaper`], [`ReaperReport`]) |
//!
//! Scope discipline (what this registry is NOT):
//!
//! * NOT a scheduler — the module engines keep their own loops.
//! * NOT per-execution ownership — that is `bot_core::ownership`
//!   (`execution_claims`, migration 0009), untouched.
//! * NOT the process-wide HA layer — that is `bot_core::ha` (0016),
//!   untouched. The registry is the TENANT-scoped "which runtime may
//!   execute for this organization right now" answer that the HA layer
//!   has never needed (it predates tenancy) and that tenant execution
//!   cannot exist without.

pub mod fencing;
pub mod heartbeat;
pub mod lease;
pub mod model;
pub mod reaper;
pub mod service;
pub mod store;

pub use fencing::{verify, verify_with_policy, FenceVerdict};
pub use heartbeat::{record as record_heartbeat, spawn_heartbeat_loop};
pub use lease::{evaluate as evaluate_lease, LeasePolicy, LeaseVerdict};
pub use model::{FenceToken, RuntimeStatus, TenantRuntimeRecord};
pub use reaper::{
    reap_once, reap_once_at, spawn_default_reaper, spawn_reaper, ReaperReport,
    DEFAULT_REAP_INTERVAL,
};
pub use service::{EnsureOutcome, RuntimeRegistryService};
pub use store::{log_reap_failures, MemoryRuntimeStore, PgRuntimeStore, RuntimeStore};
