//! HA audit trail (TASK 6 §15).
//!
//! Every ownership, recovery and lifecycle event is published as an
//! [`AppEvent::Audit`] on the shared bus — the same bus the server journals
//! to disk / the hash-chained `audit_events` table — with
//! `actor = "ha"` ([`AUDIT_ACTOR`]) and a dotted action:
//!
//! | action | when |
//! |---|---|
//! | `ha.worker.registered` | a worker life registered (target = `worker:generation`) |
//! | `ha.worker.state` | worker state transition |
//! | `ha.worker.heartbeat_failed` | a heartbeat could not be written, or the row was taken over |
//! | `ha.worker.stale_detected` | another worker missed its heartbeat deadline |
//! | `ha.lease.acquired` / `ha.lease.renewed` / `ha.lease.lost` / `ha.lease.released` | lease lifecycle |
//! | `ha.lease.takeover` | a lease was taken over from another worker |
//! | `ha.lease.fenced` | a fenced mutation was refused |
//! | `ha.recovery.started` / `ha.recovery.action` / `ha.recovery.completed` / `ha.recovery.failed` | recovery lifecycle |
//! | `ha.feed.gap` / `ha.feed.replay` | a cursor gap was detected / a deliberate replay was performed |
//! | `ha.readiness` | readiness changed |
//! | `ha.shutdown` | graceful shutdown phases |
//!
//! Nothing here is a source of truth — the durable HA tables are.

use chrono::Utc;

use super::cursor::FeedGap;
use super::lease::{FenceError, Lease, LeaseRole};
use super::store::RecoveryRecord;
use super::worker::{WorkerRegistration, WorkerState};
use crate::events::{AppEvent, EventBus};

/// Actor recorded on every HA audit event.
pub const AUDIT_ACTOR: &str = "ha";

fn publish(bus: &EventBus, action: String, target: String, outcome: String) {
    bus.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action,
        target: Some(target),
        outcome: crate::accounting::audit::sanitize(&outcome),
    });
}

/// A worker life registered.
pub fn worker_registered(bus: &EventBus, reg: &WorkerRegistration) {
    publish(
        bus,
        "ha.worker.registered".into(),
        format!("{}:{}", reg.worker_id, reg.generation),
        reg.summary(),
    );
}

/// A worker state transition.
pub fn worker_state(
    bus: &EventBus,
    worker_id: &str,
    from: WorkerState,
    to: WorkerState,
    detail: &str,
) {
    publish(
        bus,
        "ha.worker.state".into(),
        worker_id.to_string(),
        format!("from={from} to={to} detail={detail}"),
    );
}

/// A heartbeat could not be written (store error, or this life was taken
/// over by a newer generation).
pub fn heartbeat_failed(bus: &EventBus, worker_id: &str, generation: i64, detail: &str) {
    publish(
        bus,
        "ha.worker.heartbeat_failed".into(),
        format!("{worker_id}:{generation}"),
        detail.to_string(),
    );
}

/// Another worker missed its heartbeat deadline. Detection only — nothing
/// is taken over here (§1).
pub fn stale_worker(bus: &EventBus, reg: &WorkerRegistration, age_secs: i64) {
    publish(
        bus,
        "ha.worker.stale_detected".into(),
        reg.worker_id.clone(),
        format!("age_secs={age_secs} {}", reg.summary()),
    );
}

/// A lease was acquired (fresh or takeover).
pub fn lease_acquired(bus: &EventBus, lease: &Lease, takeover: bool) {
    publish(
        bus,
        if takeover {
            "ha.lease.takeover".into()
        } else {
            "ha.lease.acquired".into()
        },
        lease.role.as_string(),
        lease.summary(),
    );
}

/// A lease was renewed (logged at a low rate by the caller).
pub fn lease_renewed(bus: &EventBus, lease: &Lease) {
    publish(
        bus,
        "ha.lease.renewed".into(),
        lease.role.as_string(),
        lease.summary(),
    );
}

/// Ownership was lost (renewal CAS failed, expiry, takeover).
pub fn lease_lost(bus: &EventBus, role: &LeaseRole, holder: &str, generation: i64, detail: &str) {
    publish(
        bus,
        "ha.lease.lost".into(),
        role.as_string(),
        format!("holder={holder} generation={generation} detail={detail}"),
    );
}

/// A lease was released cleanly.
pub fn lease_released(bus: &EventBus, role: &LeaseRole, holder: &str, generation: i64) {
    publish(
        bus,
        "ha.lease.released".into(),
        role.as_string(),
        format!("holder={holder} generation={generation}"),
    );
}

/// A fenced mutation was refused.
pub fn fenced(bus: &EventBus, err: &FenceError) {
    publish(
        bus,
        "ha.lease.fenced".into(),
        err.role().to_string(),
        format!("reason={} detail={}", err.reason(), err),
    );
}

/// Recovery started.
pub fn recovery_started(bus: &EventBus, worker_id: &str, trigger: &str) {
    publish(
        bus,
        "ha.recovery.started".into(),
        worker_id.to_string(),
        format!("trigger={trigger}"),
    );
}

/// One deterministic recovery action.
pub fn recovery_action(bus: &EventBus, record: &RecoveryRecord) {
    publish(
        bus,
        "ha.recovery.action".into(),
        if record.subject.is_empty() {
            record.scope.clone()
        } else {
            record.subject.clone()
        },
        record.summary(),
    );
}

/// Recovery completed.
pub fn recovery_completed(bus: &EventBus, worker_id: &str, summary: &str) {
    publish(
        bus,
        "ha.recovery.completed".into(),
        worker_id.to_string(),
        summary.to_string(),
    );
}

/// Recovery failed; the worker must report RECOVERY_REQUIRED.
pub fn recovery_failed(bus: &EventBus, worker_id: &str, scope: &str, detail: &str) {
    publish(
        bus,
        "ha.recovery.failed".into(),
        worker_id.to_string(),
        format!("scope={scope} detail={detail}"),
    );
}

/// A feed gap was detected.
pub fn feed_gap(bus: &EventBus, gap: &FeedGap) {
    publish(
        bus,
        "ha.feed.gap".into(),
        gap.feed.as_str().to_string(),
        gap.summary(),
    );
}

/// A deliberate replay / backfill was performed.
pub fn feed_replay(bus: &EventBus, feed: &str, detail: &str) {
    publish(
        bus,
        "ha.feed.replay".into(),
        feed.to_string(),
        detail.to_string(),
    );
}

/// Readiness changed.
pub fn readiness(bus: &EventBus, worker_id: &str, ready: bool, detail: &str) {
    publish(
        bus,
        "ha.readiness".into(),
        worker_id.to_string(),
        format!("ready={ready} detail={detail}"),
    );
}

/// A graceful-shutdown phase.
pub fn shutdown(bus: &EventBus, worker_id: &str, phase: &str, detail: &str) {
    publish(
        bus,
        "ha.shutdown".into(),
        worker_id.to_string(),
        format!("phase={phase} detail={detail}"),
    );
}
