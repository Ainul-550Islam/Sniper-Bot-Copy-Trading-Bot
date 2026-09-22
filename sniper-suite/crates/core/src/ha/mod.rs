//! High availability, crash recovery and distributed reliability (TASK 6).
//!
//! ```text
//!                     ┌─────────────────────┐
//!                     │    Control plane    │  (server: API, probes, workers)
//!                     └──────────┬──────────┘
//!                                │
//!                   ┌────────────┴────────────┐
//!              Worker A                  Worker B          HaRuntime per process
//!                   └────────────┬────────────┘
//!                                │
//!                      Durable state (HaStore, migration 0016)
//!                                │
//!              ┌─────────────────┼──────────────────┐
//!           workers            leases            cursors
//!         heartbeats     fencing generations   gaps / replay
//!                                │
//!                       recovery records
//! ```
//!
//! | file | concern |
//! |---|---|
//! | `worker.rs` | worker identity, registration, heartbeat, the 9-state machine, HA modes |
//! | `lease.rs` | singleton role leases, fencing tokens, [`lease::FenceError`] |
//! | `cursor.rs` | durable feed cursors, duplicate suppression, gap detection, replay |
//! | `recovery_plan.rs` | the §5/§6 crash-boundary and order-recovery matrices (pure) |
//! | `store.rs` | [`store::HaStore`] durable contract + [`store::MemoryHaStore`] |
//! | `runtime.rs` | [`runtime::HaRuntime`]: the process-wide facade (leases, cursors, readiness, graceful shutdown) |
//! | `metrics.rs` / `audit.rs` | `ha_*` series, `ha.*` audit actions |
//!
//! What this layer does NOT do: it never decides trades, never books money
//! and never replaces the TASK 1–5 idempotency mechanisms. Per-execution
//! ownership stays with [`crate::ownership`] (one claim per intent), order
//! identity stays with [`crate::oms`] (`idempotency_key`), financial
//! identity stays with [`crate::accounting`] (`event_id`). TASK 6 adds the
//! substrate that makes those safe across workers and restarts: who is
//! alive, who owns each singleton role, where each feed got to, and what a
//! restart must do about every unfinished order.

pub mod audit;
pub mod cursor;
pub mod lease;
pub mod metrics;
pub mod recovery_plan;
pub mod runtime;
pub mod store;
pub mod worker;

pub use audit::AUDIT_ACTOR;
pub use cursor::{CursorAdvance, FeedCursor, FeedGap, FeedId, GapStatus};
pub use lease::{
    next_generation, renew_interval, FenceError, Lease, LeaseDecision, LeaseRequest, LeaseRole,
};
pub use recovery_plan::{
    plan_order_recovery, CrashBoundary, LocalOrderEvidence, OrderRecoveryAction, OrderRecoveryPlan,
    VenueOrderEvidence,
};
pub use runtime::{HaRuntime, HaSettings, LeaseGuard, NotReadyReason, Readiness};
pub use store::{HaStore, MemoryHaStore, RecoveryRecord};
pub use worker::{HaMode, WorkerHealth, WorkerRegistration, WorkerState, WorkerTransitionError};
