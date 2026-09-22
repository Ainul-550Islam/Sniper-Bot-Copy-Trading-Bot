//! Copy-engine metrics (TASK 3 §10).
//!
//! Every series is registered through the process-wide
//! [`bot_core::obs::metrics`] registry (the same one `/metrics` renders), so
//! nothing here needs wiring beyond calling the helpers. Names are prefixed
//! `copy_` and mirror the sniper's `sniper_*` families:
//!
//! | series | kind | labels | meaning |
//! |---|---|---|---|
//! | `copy_events_total` | counter | `source`, `side` | events handed over by feeds |
//! | `copy_stage_total` | counter | `stage` | stage transitions |
//! | `copy_rejections_total` | counter | `reason`, `stage` | refused events |
//! | `copy_dedup_total` | counter | `outcome` | `fresh` / `duplicate` / `seeded` |
//! | `copy_ordering_total` | counter | `verdict` | in-order / out-of-order / gap |
//! | `copy_leader_events_total` | counter | `event` | registry transitions |
//! | `copy_leaders` | gauge | `status` | leaders per lifecycle state |
//! | `copy_leader_exposure_sol_milli` | gauge | `leader` | open exposure per leader (milli-SOL) |
//! | `copy_recon_findings_total` | counter | `kind` | reconciliation findings |
//! | `copy_recovery_actions_total` | counter | `action` | restart recovery actions |
//! | `copy_journal_errors_total` | counter | `op` | durable store write failures |
//! | `copy_detection_lag_ms` | histogram | — | chain time → local observation |
//! | `copy_<stage>_latency_ms` | histogram | — | per-stage wall clock |
//! | `copy_total_latency_ms` | histogram | — | observation → terminal stage |
//! | `copy_mirror_size_sol_milli` | histogram | — | approved mirror sizes |
//!
//! Cardinality is bounded: sources, stages, reasons and verdicts are closed
//! enums; the only free label is `leader`, which is the small configured set.

use bot_core::obs::metrics::{self, LATENCY_BUCKETS_MS};
use chrono::{DateTime, Utc};

use crate::event::{CopyStage, EventSource, LeaderTradeEvent, RejectReason};

/// Bucket bounds for mirror sizes, in milli-SOL.
pub const SIZE_BUCKETS_MILLI_SOL: &[u64] = &[1, 5, 10, 25, 50, 100, 250, 500, 1_000, 5_000];

/// Count a feed hand-over.
pub fn count_event(source: EventSource, side: &str) {
    metrics::global()
        .counter(
            "copy_events_total",
            "Leader-trade events handed to the copy pipeline by the feeds.",
            &[("source", source.as_str()), ("side", side)],
        )
        .inc();
}

/// Count a stage transition.
pub fn count_stage(stage: CopyStage) {
    metrics::global()
        .counter(
            "copy_stage_total",
            "Copy pipeline stage transitions.",
            &[("stage", stage.as_str())],
        )
        .inc();
}

/// Count a rejection at a stage.
pub fn count_rejection(reason: RejectReason, stage: CopyStage) {
    metrics::global()
        .counter(
            "copy_rejections_total",
            "Leader-trade events the copy pipeline refused, by reason and stage.",
            &[("reason", reason.as_str()), ("stage", stage.as_str())],
        )
        .inc();
}

/// Count a dedup decision (`fresh`, `duplicate`, `seeded`).
pub fn count_dedup(outcome: &str) {
    metrics::global()
        .counter(
            "copy_dedup_total",
            "Authoritative copy-event dedup decisions.",
            &[("outcome", outcome)],
        )
        .inc();
}

/// Count an ordering verdict (`in_order`, `same_slot`, `out_of_order`, `gap`, `unknown_slot`).
pub fn count_ordering(verdict: &str) {
    metrics::global()
        .counter(
            "copy_ordering_total",
            "Per-leader ordering verdicts for incoming events.",
            &[("verdict", verdict)],
        )
        .inc();
}

/// Count a leader lifecycle transition.
pub fn count_leader_event(event: &str) {
    metrics::global()
        .counter(
            "copy_leader_events_total",
            "Leader registry lifecycle transitions.",
            &[("event", event)],
        )
        .inc();
}

/// Publish how many leaders are in each lifecycle state.
pub fn set_leader_gauge(status: &str, n: usize) {
    metrics::global()
        .gauge(
            "copy_leaders",
            "Leaders in the copy registry, by lifecycle status.",
            &[("status", status)],
        )
        .set(n as i64);
}

/// Publish the open exposure mirrored from one leader (milli-SOL, integer).
pub fn set_leader_exposure(leader: &str, exposure_sol: f64) {
    let milli = if exposure_sol.is_finite() {
        (exposure_sol * 1_000.0).round().max(0.0) as i64
    } else {
        0
    };
    metrics::global()
        .gauge(
            "copy_leader_exposure_sol_milli",
            "Open exposure mirrored from one leader, in milli-SOL.",
            &[("leader", leader)],
        )
        .set(milli);
}

/// Count a reconciliation finding.
pub fn count_recon_finding(kind: &str) {
    metrics::global()
        .counter(
            "copy_recon_findings_total",
            "Leader-vs-follower reconciliation findings, by kind.",
            &[("kind", kind)],
        )
        .inc();
}

/// Count a restart-recovery action.
pub fn count_recovery_action(action: &str) {
    metrics::global()
        .counter(
            "copy_recovery_actions_total",
            "Copy-engine restart recovery actions, by kind.",
            &[("action", action)],
        )
        .inc();
}

/// Count a durable-store write failure (journaling is best effort).
pub fn count_journal_error(op: &str) {
    metrics::global()
        .counter(
            "copy_journal_errors_total",
            "Copy journal (leaders/events/links) writes that failed.",
            &[("op", op)],
        )
        .inc();
}

/// Record an approved mirror size.
pub fn observe_mirror_size(sol: f64) {
    if !sol.is_finite() || sol < 0.0 {
        return;
    }
    metrics::global()
        .histogram(
            "copy_mirror_size_sol_milli",
            "Mirror sizes approved by the risk engine, in milli-SOL.",
            &[],
            SIZE_BUCKETS_MILLI_SOL,
        )
        .observe((sol * 1_000.0).round() as u64);
}

/// Record the feed's detection lag (chain time → local observation) when the
/// event carries chain time.
pub fn observe_detection_lag(event: &LeaderTradeEvent) {
    if let Some(ms) = event.detection_lag_ms() {
        metrics::global()
            .histogram(
                "copy_detection_lag_ms",
                "Milliseconds between the leader trade's chain time and local observation.",
                &[],
                LATENCY_BUCKETS_MS,
            )
            .observe(ms.max(0) as u64);
    }
}

/// Wall-clock timeline of one event through the pipeline. Stages are
/// stamped as they complete; [`LatencyTimeline::record`] emits one histogram
/// sample per stage interval plus the end-to-end total.
#[derive(Debug, Clone)]
pub struct LatencyTimeline {
    /// `observed_at` of the event — the timeline origin.
    pub observed_at: DateTime<Utc>,
    marks: Vec<(&'static str, DateTime<Utc>)>,
}

impl LatencyTimeline {
    /// Start a timeline at the event's observation time.
    pub fn start(observed_at: DateTime<Utc>) -> Self {
        LatencyTimeline {
            observed_at,
            marks: Vec::with_capacity(8),
        }
    }

    /// Stamp the completion of `stage` at `now`. Non-monotonic stamps are
    /// clamped when recorded, never rejected.
    pub fn mark(&mut self, stage: &'static str) {
        self.marks.push((stage, Utc::now()));
    }

    /// Stamp with an explicit time (tests, replay).
    pub fn mark_at(&mut self, stage: &'static str, at: DateTime<Utc>) {
        self.marks.push((stage, at));
    }

    /// Milliseconds from the origin to the last stamp (0 when none).
    pub fn total_ms(&self) -> u64 {
        self.marks
            .last()
            .map(|(_, at)| {
                at.signed_duration_since(self.observed_at)
                    .num_milliseconds()
            })
            .unwrap_or(0)
            .max(0) as u64
    }

    /// Milliseconds spent in each stamped stage, in order.
    pub fn intervals_ms(&self) -> Vec<(&'static str, u64)> {
        let mut prev = self.observed_at;
        self.marks
            .iter()
            .map(|(stage, at)| {
                let ms = at.signed_duration_since(prev).num_milliseconds().max(0) as u64;
                prev = *at;
                (*stage, ms)
            })
            .collect()
    }

    /// Emit `copy_<stage>_latency_ms` per interval and `copy_total_latency_ms`.
    pub fn record(&self) {
        let reg = metrics::global();
        for (stage, ms) in self.intervals_ms() {
            let name = format!("copy_{stage}_latency_ms");
            reg.histogram(
                &name,
                "Milliseconds the copy pipeline spent in this stage.",
                &[],
                LATENCY_BUCKETS_MS,
            )
            .observe(ms);
        }
        reg.histogram(
            "copy_total_latency_ms",
            "Milliseconds from observing a leader trade to the terminal pipeline stage.",
            &[],
            LATENCY_BUCKETS_MS,
        )
        .observe(self.total_ms());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn timeline_intervals_are_monotone_and_total_matches() {
        let t0 = Utc::now();
        let mut tl = LatencyTimeline::start(t0);
        tl.mark_at("validated", t0 + Duration::milliseconds(5));
        tl.mark_at("policy", t0 + Duration::milliseconds(12));
        tl.mark_at("submitted", t0 + Duration::milliseconds(10)); // clock went backwards
        tl.mark_at("filled", t0 + Duration::milliseconds(40));
        let iv = tl.intervals_ms();
        assert_eq!(iv[0], ("validated", 5));
        assert_eq!(iv[1], ("policy", 7));
        assert_eq!(iv[2], ("submitted", 0), "negative intervals clamp to zero");
        assert_eq!(iv[3], ("filled", 30));
        assert_eq!(tl.total_ms(), 40);
        tl.record();
        assert!(
            metrics::global()
                .histogram("copy_total_latency_ms", "", &[], LATENCY_BUCKETS_MS)
                .count()
                >= 1
        );
    }

    #[test]
    fn counters_register_under_copy_prefix() {
        count_event(EventSource::PumpPortal, "buy");
        count_stage(CopyStage::Received);
        count_rejection(RejectReason::StaleEvent, CopyStage::PolicyPassed);
        count_dedup("fresh");
        count_ordering("in_order");
        count_leader_event("followed");
        set_leader_gauge("active", 2);
        set_leader_exposure("whale", 1.2345);
        set_leader_exposure("whale-nan", f64::NAN);
        count_recon_finding("leader_exited_we_hold");
        count_recovery_action("hold_ambiguous");
        count_journal_error("record_event");
        observe_mirror_size(0.05);
        observe_mirror_size(f64::NAN);
        let reg = metrics::global();
        assert!(
            reg.counter(
                "copy_rejections_total",
                "",
                &[("reason", "STALE_EVENT"), ("stage", "POLICY_PASSED")]
            )
            .get()
                >= 1
        );
        assert_eq!(
            reg.gauge("copy_leader_exposure_sol_milli", "", &[("leader", "whale")])
                .get(),
            1235
        );
        assert_eq!(
            reg.gauge(
                "copy_leader_exposure_sol_milli",
                "",
                &[("leader", "whale-nan")]
            )
            .get(),
            0
        );
    }
}
