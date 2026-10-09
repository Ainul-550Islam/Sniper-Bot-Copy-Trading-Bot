//! Runtime instance metadata domain model (STEP 3 file 38).
//!
//! A [`TenantRuntimeRecord`] is one row of the `tenant_runtimes` table
//! (migration 0025): the runtime INSTANCE currently (or formerly)
//! executing for one tenant. It is deliberately narrow — identity,
//! generation, lifecycle, heartbeat and lease. Everything about HOW work
//! is executed stays where it already lives: per-execution ownership in
//! `bot_core::ownership` (execution_claims, 0009), process-level HA in
//! `bot_core::ha` (0016), the engines in the module crates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

/// Lifecycle of one runtime instance. Closed vocabulary — the database
/// CHECK constraint enforces the same set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    /// Registered, not yet confirmed live.
    Provisioning,
    /// The one live runtime of the tenant.
    Active,
    /// Gracefully handing over; refuses new executions.
    Draining,
    /// Left the live set (stopped, superseded, reaped after stale
    /// heartbeat). Terminal for this row: rotation mints a fresh row.
    Stopped,
    /// Long-retired rows kept for audit queries.
    Retired,
}

impl RuntimeStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [RuntimeStatus; 5] = [
        RuntimeStatus::Provisioning,
        RuntimeStatus::Active,
        RuntimeStatus::Draining,
        RuntimeStatus::Stopped,
        RuntimeStatus::Retired,
    ];

    /// Stable label (database, metrics, audit).
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeStatus::Provisioning => "provisioning",
            RuntimeStatus::Active => "active",
            RuntimeStatus::Draining => "draining",
            RuntimeStatus::Stopped => "stopped",
            RuntimeStatus::Retired => "retired",
        }
    }

    /// Inverse of [`RuntimeStatus::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        RuntimeStatus::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// May a runtime in this status accept new executions?
    pub fn is_live(self) -> bool {
        matches!(self, RuntimeStatus::Provisioning | RuntimeStatus::Active)
    }

    /// Is this the terminal stopped/retired set?
    pub fn is_terminal(self) -> bool {
        matches!(self, RuntimeStatus::Stopped | RuntimeStatus::Retired)
    }
}

impl std::fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One runtime instance row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantRuntimeRecord {
    /// Runtime instance identity.
    pub runtime_id: RuntimeId,
    /// The tenant it executes for.
    pub organization_id: OrganizationId,
    /// Its fencing generation.
    pub generation: RuntimeGeneration,
    /// Its lifecycle status.
    pub status: RuntimeStatus,
    /// The registering process identity (host/pid or operator label).
    pub worker_id: String,
    /// When it registered.
    pub started_at: DateTime<Utc>,
    /// Its last heartbeat.
    pub heartbeat_at: DateTime<Utc>,
    /// Optional hard lease expiry.
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// When it left the live set, if it did.
    pub stopped_at: Option<DateTime<Utc>>,
}

impl TenantRuntimeRecord {
    /// A brand-new active runtime (generation 1 on first registration).
    pub fn new_active(
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
        worker_id: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        TenantRuntimeRecord {
            runtime_id,
            organization_id,
            generation,
            status: RuntimeStatus::Active,
            worker_id: worker_id.into(),
            started_at: now,
            heartbeat_at: now,
            lease_expires_at: None,
            stopped_at: None,
        }
    }

    /// Is the heartbeat fresh enough to consider this runtime live?
    pub fn is_heartbeat_fresh(&self, now: DateTime<Utc>, max_age: chrono::Duration) -> bool {
        now - self.heartbeat_at <= max_age
    }

    /// Is the optional hard lease still live? A runtime without a lease
    /// relies on its heartbeat (see `lease.rs` for the combined verdict).
    pub fn is_lease_live(&self, now: DateTime<Utc>) -> bool {
        self.lease_expires_at
            .map(|expires| now < expires)
            .unwrap_or(true)
    }

    /// The fence token this runtime currently holds.
    pub fn fence_token(&self) -> FenceToken {
        FenceToken {
            organization_id: self.organization_id,
            runtime_id: self.runtime_id,
            generation: self.generation,
        }
    }
}

/// The claim a worker makes about which runtime it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FenceToken {
    /// The tenant.
    pub organization_id: OrganizationId,
    /// The runtime instance the worker believes it is.
    pub runtime_id: RuntimeId,
    /// The generation the worker was issued.
    pub generation: RuntimeGeneration,
}

impl std::fmt::Display for FenceToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "org={} runtime={} {}",
            self.organization_id, self.runtime_id, self.generation
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(now: DateTime<Utc>) -> TenantRuntimeRecord {
        TenantRuntimeRecord::new_active(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker-a",
            now,
        )
    }

    #[test]
    fn status_vocabulary_round_trips_and_matches_the_db_check() {
        for s in RuntimeStatus::ALL {
            assert_eq!(RuntimeStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(RuntimeStatus::parse("active"), Some(RuntimeStatus::Active));
        assert_eq!(RuntimeStatus::parse("nope"), None);
    }

    #[test]
    fn live_and_terminal_sets_are_disjoint() {
        for s in RuntimeStatus::ALL {
            // Disjoint = never both at once. `draining` is deliberately
            // NEITHER (still operating, accepts no new work).
            assert!(
                !(s.is_live() && s.is_terminal()),
                "{s} must not be both live and terminal"
            );
        }
    }

    #[test]
    fn heartbeat_freshness_follows_the_max_age() {
        let now = Utc::now();
        let r = record(now);
        assert!(r.is_heartbeat_fresh(now, chrono::Duration::seconds(90)));
        assert!(!r.is_heartbeat_fresh(
            now + chrono::Duration::seconds(91),
            chrono::Duration::seconds(90)
        ));
    }

    #[test]
    fn lease_is_optional_hardening() {
        let now = Utc::now();
        let mut r = record(now);
        // Without a lease: heartbeat decides.
        assert!(r.is_lease_live(now));
        // With an expired lease: dead even if the heartbeat is fresh.
        r.lease_expires_at = Some(now - chrono::Duration::seconds(1));
        assert!(!r.is_lease_live(now));
        // With a future lease: live.
        r.lease_expires_at = Some(now + chrono::Duration::seconds(60));
        assert!(r.is_lease_live(now));
    }

    #[test]
    fn fence_token_carries_the_runtime_identity() {
        let now = Utc::now();
        let r = record(now);
        let t = r.fence_token();
        assert_eq!(t.organization_id, r.organization_id);
        assert_eq!(t.runtime_id, r.runtime_id);
        assert_eq!(t.generation, r.generation);
        assert!(t.to_string().contains("gen-1"));
    }
}
