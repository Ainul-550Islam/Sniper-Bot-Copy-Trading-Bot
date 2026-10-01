//! Runtime lease model (STEP 3 file 41).
//!
//! The HA layer (`bot_core::ha`) owns process-wide singleton leases and
//! their fencing tokens. This module is the TENANT-RUNTIME lease policy:
//! when is a runtime row considered live, combining both liveness
//! signals the registry records — heartbeat freshness and (optional)
//! hard lease expiry — into one deterministic verdict.
//!
//! Policy, not storage: the verdict is a pure function of the record
//! and the clock; the service layer applies it.

use chrono::{DateTime, Duration, Utc};

use super::model::{RuntimeStatus, TenantRuntimeRecord};

/// How often a live runtime heartbeats.
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: i64 = 15;
/// How stale a heartbeat may be before the runtime is considered dead
/// (must comfortably exceed the interval; heartbeats share the process
/// with trading work).
pub const DEFAULT_STALE_AFTER_SECS: i64 = 90;
/// The optional hard lease length, when a runtime takes one.
pub const DEFAULT_LEASE_SECS: i64 = 120;

/// The liveness policy of the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeasePolicy {
    /// Heartbeat cadence.
    pub heartbeat_interval: Duration,
    /// Heartbeat staleness threshold.
    pub stale_after: Duration,
    /// Default hard-lease length (a runtime may set its own expiry).
    pub lease_length: Duration,
}

impl Default for LeasePolicy {
    fn default() -> Self {
        LeasePolicy {
            heartbeat_interval: Duration::seconds(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            stale_after: Duration::seconds(DEFAULT_STALE_AFTER_SECS),
            lease_length: Duration::seconds(DEFAULT_LEASE_SECS),
        }
    }
}

/// The combined liveness verdict for a runtime record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseVerdict {
    /// Live: correct status, fresh heartbeat, live lease.
    Live,
    /// The row is not in a live status (stopped/retired/drain-only).
    NotLiveStatus(RuntimeStatus),
    /// The heartbeat went stale.
    HeartbeatStale,
    /// The hard lease expired.
    LeaseExpired,
}

impl LeaseVerdict {
    /// May the runtime accept new executions?
    pub fn is_live(self) -> bool {
        matches!(self, LeaseVerdict::Live)
    }

    /// Stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseVerdict::Live => "live",
            LeaseVerdict::NotLiveStatus(_) => "not_live_status",
            LeaseVerdict::HeartbeatStale => "heartbeat_stale",
            LeaseVerdict::LeaseExpired => "lease_expired",
        }
    }
}

/// Evaluate a record against the policy at `now`.
pub fn evaluate(
    record: &TenantRuntimeRecord,
    policy: &LeasePolicy,
    now: DateTime<Utc>,
) -> LeaseVerdict {
    if !record.status.is_live() {
        return LeaseVerdict::NotLiveStatus(record.status);
    }
    if !record.is_heartbeat_fresh(now, policy.stale_after) {
        return LeaseVerdict::HeartbeatStale;
    }
    if !record.is_lease_live(now) {
        return LeaseVerdict::LeaseExpired;
    }
    LeaseVerdict::Live
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

    fn record(
        status: RuntimeStatus,
        heartbeat_age_secs: i64,
        lease: Option<DateTime<Utc>>,
    ) -> TenantRuntimeRecord {
        let now = Utc::now();
        let mut r = TenantRuntimeRecord::new_active(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker",
            now - Duration::seconds(heartbeat_age_secs),
        );
        r.status = status;
        r.heartbeat_at = now - Duration::seconds(heartbeat_age_secs);
        r.lease_expires_at = lease;
        r
    }

    #[test]
    fn live_runtime_passes_all_three_gates() {
        let now = Utc::now();
        let r = record(RuntimeStatus::Active, 5, None);
        assert!(evaluate(&r, &LeasePolicy::default(), now).is_live());
    }

    #[test]
    fn dead_statuses_are_not_live() {
        let now = Utc::now();
        for status in [
            RuntimeStatus::Draining,
            RuntimeStatus::Stopped,
            RuntimeStatus::Retired,
        ] {
            let r = record(status, 0, None);
            let v = evaluate(&r, &LeasePolicy::default(), now);
            assert!(!v.is_live());
            assert_eq!(v, LeaseVerdict::NotLiveStatus(status));
        }
    }

    #[test]
    fn stale_heartbeat_and_expired_lease_fail_closed() {
        let now = Utc::now();
        let policy = LeasePolicy::default();
        let stale = record(
            RuntimeStatus::Active,
            policy.stale_after.num_seconds() + 1,
            None,
        );
        assert_eq!(evaluate(&stale, &policy, now), LeaseVerdict::HeartbeatStale);
        let expired = record(RuntimeStatus::Active, 0, Some(now - Duration::seconds(1)));
        assert_eq!(evaluate(&expired, &policy, now), LeaseVerdict::LeaseExpired);
    }

    #[test]
    fn statuses_parse_round_trip_for_the_db_check() {
        for s in RuntimeStatus::ALL {
            assert_eq!(RuntimeStatus::parse(s.as_str()), Some(s));
        }
    }
}
