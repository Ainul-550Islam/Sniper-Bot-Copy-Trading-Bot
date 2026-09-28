//! HA metrics (TASK 6 §14) — the `ha_*` family, registered through the
//! process-wide [`crate::obs::metrics`] registry like every other module.
//!
//! | series | kind | labels | meaning |
//! |---|---|---|---|
//! | `ha_worker_state` | gauge | `state` | 1 on the worker's current state, 0 on the others |
//! | `ha_worker_state_changes_total` | counter | `from`, `to` | worker state transitions |
//! | `ha_worker_heartbeats_total` | counter | `outcome` | `ok` / `stale` / `error` |
//! | `ha_workers_seen` | gauge | `health` | workers in the registry by `live` / `stale` / `stopped` |
//! | `ha_lease_operations_total` | counter | `role`, `op`, `outcome` | acquire / renew / release / verify × ok / rejected / lost / error |
//! | `ha_leases_held` | gauge | `role` | 1 while this worker holds the role |
//! | `ha_lease_takeovers_total` | counter | `role` | leases taken over from another worker |
//! | `ha_fenced_mutations_total` | counter | `role`, `reason` | refused fenced mutations (`fenced` / `expired` / `store_unavailable`) |
//! | `ha_recovery_actions_total` | counter | `scope`, `action` | deterministic recovery actions taken |
//! | `ha_recovery_failures_total` | counter | `scope`, `reason` | recovery steps that could not complete |
//! | `ha_replay_events_total` | counter | `feed`, `outcome` | cursor offers by `advanced` / `duplicate` / `gap` |
//! | `ha_feed_gaps_total` | counter | `feed` | detected feed gaps |
//! | `ha_cursor_lag_secs` | gauge | `feed` | seconds between the newest processed event and now |
//! | `ha_cursor_position` | gauge | `feed` | last durable sequence (sequenced feeds) |
//! | `ha_readiness` | gauge | — | 1 when the worker reports READY, else 0 |
//! | `ha_readiness_changes_total` | counter | `ready` | readiness transitions (`true` / `false`) |
//!
//! Duplicate prevention feeds the existing shared family
//! `bot_duplicate_execution_prevented_total{where="ha_cursor"|"ha_lease"}`,
//! so TASK 1–5 dashboards keep working unchanged. Cardinality is bounded:
//! states, outcomes, roles (`LeaseRole::kind`), feeds and reasons are all
//! closed sets.

use super::cursor::FeedId;
use super::lease::LeaseRole;
use super::worker::{WorkerHealth, WorkerState};
use crate::obs::metrics;

/// Publish the worker-state gauge family (1 on the current state).
pub fn set_worker_state(current: WorkerState) {
    for s in WorkerState::ALL {
        metrics::global()
            .gauge(
                "ha_worker_state",
                "Current worker state (1 on the active state, 0 on the others).",
                &[("state", s.as_str())],
            )
            .set(i64::from(s == current));
    }
}

/// Count one worker state transition.
pub fn count_state_change(from: WorkerState, to: WorkerState) {
    metrics::global()
        .counter(
            "ha_worker_state_changes_total",
            "Worker state transitions.",
            &[("from", from.as_str()), ("to", to.as_str())],
        )
        .inc();
}

/// Count one heartbeat attempt (`ok` / `stale` / `error`).
pub fn count_heartbeat(outcome: &str) {
    metrics::global()
        .counter(
            "ha_worker_heartbeats_total",
            "Worker heartbeat attempts by outcome.",
            &[("outcome", outcome)],
        )
        .inc();
}

/// Publish the registry census.
pub fn set_workers_seen(live: usize, stale: usize, stopped: usize) {
    for (health, n) in [
        (WorkerHealth::Live, live),
        (WorkerHealth::Stale, stale),
        (WorkerHealth::Stopped, stopped),
    ] {
        metrics::global()
            .gauge(
                "ha_workers_seen",
                "Workers in the registry by health.",
                &[("health", health.as_str())],
            )
            .set(n as i64);
    }
}

/// Count one lease operation.
pub fn count_lease_op(role: &LeaseRole, op: &str, outcome: &str) {
    metrics::global()
        .counter(
            "ha_lease_operations_total",
            "Lease operations by role, operation and outcome.",
            &[("role", role.kind()), ("op", op), ("outcome", outcome)],
        )
        .inc();
}

/// Set the held/not-held gauge for one role.
pub fn set_lease_held(role: &LeaseRole, held: bool) {
    metrics::global()
        .gauge(
            "ha_leases_held",
            "1 while this worker holds the role's lease.",
            &[("role", role.kind())],
        )
        .set(i64::from(held));
}

/// Count one takeover.
pub fn count_takeover(role: &LeaseRole) {
    metrics::global()
        .counter(
            "ha_lease_takeovers_total",
            "Leases taken over from another worker.",
            &[("role", role.kind())],
        )
        .inc();
}

/// Count one refused fenced mutation.
pub fn count_fenced(role: &str, reason: &str) {
    metrics::global()
        .counter(
            "ha_fenced_mutations_total",
            "Mutations refused because the worker no longer owns the lease.",
            &[("role", role), ("reason", reason)],
        )
        .inc();
    metrics::global()
        .counter(
            "bot_duplicate_execution_prevented_total",
            "Duplicate logical executions prevented by idempotency layers.",
            &[("where", "ha_lease")],
        )
        .inc();
}

/// Count one recovery action.
pub fn count_recovery_action(scope: &str, action: &str) {
    metrics::global()
        .counter(
            "ha_recovery_actions_total",
            "Deterministic recovery actions by scope and action.",
            &[("scope", scope), ("action", action)],
        )
        .inc();
}

/// Count one recovery failure.
pub fn count_recovery_failure(scope: &str, reason: &str) {
    metrics::global()
        .counter(
            "ha_recovery_failures_total",
            "Recovery steps that could not complete.",
            &[("scope", scope), ("reason", reason)],
        )
        .inc();
}

/// Count one cursor offer outcome (`advanced` / `duplicate` / `gap`).
pub fn count_replay_event(feed: FeedId, outcome: &str) {
    metrics::global()
        .counter(
            "ha_replay_events_total",
            "Feed items offered to a durable cursor, by outcome.",
            &[("feed", feed.as_str()), ("outcome", outcome)],
        )
        .inc();
    if outcome == "duplicate" {
        metrics::global()
            .counter(
                "bot_duplicate_execution_prevented_total",
                "Duplicate logical executions prevented by idempotency layers.",
                &[("where", "ha_cursor")],
            )
            .inc();
    }
}

/// Count one detected feed gap.
pub fn count_feed_gap(feed: FeedId) {
    metrics::global()
        .counter(
            "ha_feed_gaps_total",
            "Detected discontinuities in a sequenced feed.",
            &[("feed", feed.as_str())],
        )
        .inc();
}

/// Publish cursor lag and position.
pub fn set_cursor(feed: FeedId, lag_secs: Option<i64>, position: Option<u64>) {
    if let Some(lag) = lag_secs {
        metrics::global()
            .gauge(
                "ha_cursor_lag_secs",
                "Seconds between the newest processed feed event and now.",
                &[("feed", feed.as_str())],
            )
            .set(lag);
    }
    if let Some(p) = position {
        metrics::global()
            .gauge(
                "ha_cursor_position",
                "Last durable sequence of a sequenced feed cursor.",
                &[("feed", feed.as_str())],
            )
            .set(p.min(i64::MAX as u64) as i64);
    }
}

/// Publish the readiness gauge and count the transition.
pub fn set_readiness(ready: bool, changed: bool) {
    metrics::global()
        .gauge(
            "ha_readiness",
            "1 when the worker reports READY on its readiness probe.",
            &[],
        )
        .set(i64::from(ready));
    if changed {
        metrics::global()
            .counter(
                "ha_readiness_changes_total",
                "Readiness transitions.",
                &[("ready", if ready { "true" } else { "false" })],
            )
            .inc();
    }
}
