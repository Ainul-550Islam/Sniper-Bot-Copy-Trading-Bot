//! Server-side provisioning workers (BATCH 2).
//!
//! Lifecycle and retention workers consume pending jobs via a lease-based
//! claim so two replicas cannot process the same job concurrently.

pub mod job_claim;
pub mod lifecycle_worker;
pub mod retention_worker;

pub use job_claim::{release, try_claim, JobClaim, JobKind, CLAIM_SQL};
pub use lifecycle_worker::{
    backoff_for, execute_phase, next_phase, LifecycleWorkOutcome, LifecycleWorkerConfig,
};
pub use retention_worker::{execute_purge, is_eligible, PurgeDecision, RetentionOutcome};
