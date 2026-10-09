//! Global risk metrics (TASK 5 §8) — the `global_risk_*` family.
//!
//! | series | kind | labels | meaning |
//! |---|---|---|---|
//! | `global_risk_decisions_total` | counter | `verdict`, `reason` | every global decision (`accept` carries `reason="-"`) |
//! | `global_risk_rejections_total` | counter | `reason` | rejections by reason |
//! | `global_risk_kill_switch_changes_total` | counter | `scope`, `action` | runtime engage / release |
//! | `global_risk_kill_switches_active` | gauge | `scope` | active venue / strategy switches |
//! | `global_risk_journal_errors_total` | counter | `op` | durable journal writes / reads that failed |
//!
//! Cardinality is bounded: verdicts, reasons, scopes, actions and ops are
//! closed sets.

use super::kill_switch::KillSwitchState;
use crate::obs::metrics;

/// Count one decision.
pub(crate) fn count_decision(verdict: &str, reason: &str) {
    metrics::global()
        .counter(
            "global_risk_decisions_total",
            "Global risk decisions by verdict and reason.",
            &[("verdict", verdict), ("reason", reason)],
        )
        .inc();
}

/// Count one rejection by reason.
pub(crate) fn count_rejection(reason: &str) {
    metrics::global()
        .counter(
            "global_risk_rejections_total",
            "Global risk rejections by reason.",
            &[("reason", reason)],
        )
        .inc();
}

/// Count one runtime kill-switch change.
pub(crate) fn count_kill_switch_change(scope: &str, action: &str) {
    metrics::global()
        .counter(
            "global_risk_kill_switch_changes_total",
            "Runtime kill-switch engage / release actions by scope.",
            &[("scope", scope), ("action", action)],
        )
        .inc();
}

/// Publish the active-switch gauges (both scopes always registered).
pub(crate) fn set_kill_switch_count(active: &[KillSwitchState]) {
    let venues = active.iter().filter(|s| s.scope.kind() == "venue").count();
    let strategies = active
        .iter()
        .filter(|s| s.scope.kind() == "strategy")
        .count();
    for (scope, n) in [("venue", venues), ("strategy", strategies)] {
        metrics::global()
            .gauge(
                "global_risk_kill_switches_active",
                "Active venue / strategy kill switches (configured or operator-engaged).",
                &[("scope", scope)],
            )
            .set(n as i64);
    }
}

/// Count one failed journal operation.
pub(crate) fn count_journal_error(op: &str) {
    metrics::global()
        .counter(
            "global_risk_journal_errors_total",
            "Global risk durable journal operations that failed.",
            &[("op", op)],
        )
        .inc();
}
