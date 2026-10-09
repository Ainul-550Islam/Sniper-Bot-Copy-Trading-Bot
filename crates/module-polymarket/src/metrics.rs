//! Polymarket engine metrics.
//!
//! Every series is registered through the process-wide
//! [`bot_core::obs::metrics`] registry (the same one `/metrics` renders), so
//! nothing here needs wiring beyond calling the helpers. Names are prefixed
//! `poly_` and mirror the sniper's `sniper_*` and the copy engine's `copy_*`
//! families:
//!
//! | series | kind | labels | meaning |
//! |---|---|---|---|
//! | `poly_strategy_skips_total` | counter | `strategy`, `reason` | strategy verdicts that produced no decision |
//! | `poly_stage_total` | counter | `stage` | signals that reached each pipeline stage |
//! | `poly_signals_total` | counter | `strategy`, `outcome` | signals by terminal stage |
//! | `poly_rejections_total` | counter | `reason` | signals rejected / failed by reason |
//! | `poly_pipeline_latency_ms` | histogram | `outcome` | wall time of one signal through the pipeline |
//! | `poly_quote_age_ms` | histogram | — | age of the quote a decision was made on |
//! | `poly_order_transitions_total` | counter | `from`, `to` | venue-order lifecycle transitions |
//! | `poly_orders_total` | counter | `result` | venue orders by terminal result |
//! | `poly_fills_total` | counter | `source` | fills booked (`poll`, `user_ws`, `recon`, `paper`) |
//! | `poly_cancel_total` | counter | `reason` | cancel requests (`ttl`, `expired`, `reprice`, `shutdown`, `recon`, `cancel_all`, …) |
//! | `poly_open_orders` | gauge | — | resting venue orders |
//! | `poly_exposure_usd_milli` | gauge | — | open exposure (positions + resting buys), milli-USDC |
//! | `poly_recon_findings_total` | counter | `kind` | reconciliation findings |
//! | `poly_recovery_actions_total` | counter | `action` | restart-recovery actions |
//! | `poly_journal_errors_total` | counter | `op` | durable journal writes / reads that failed |
//! | `poly_user_ws_events_total` | counter | `type` | user-channel events received (emitted by `ws.rs`) |
//!
//! Two shared `bot_*` families are also fed from here with a module label:
//! `bot_symbol_gated_entries_total{module="polymarket"}` and
//! `bot_duplicate_execution_prevented_total{where="poly_pipeline"|"poly_fill_journal"}`.
//!
//! Cardinality is bounded: stages, reasons, states, sources, kinds and
//! actions are closed enums; `strategy` is the configured name.

use bot_core::models::BotModule;
use bot_core::obs::metrics::{self, LATENCY_BUCKETS_MS};

use crate::PolyBot;

/// Buckets for the quote-age histogram (ms): sub-second websocket ages up to
/// the stale threshold and beyond.
const QUOTE_AGE_BUCKETS_MS: &[u64] = &[250, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000];

/// Count a strategy verdict that did not produce a decision.
pub(crate) fn count_strategy_skip(strategy: &str, reason: &str) {
    metrics::global()
        .counter(
            "poly_strategy_skips_total",
            "Polymarket strategy verdicts that did not produce a decision.",
            &[("strategy", strategy), ("reason", reason)],
        )
        .inc();
}

/// Count a signal reaching a pipeline stage.
pub(crate) fn count_stage(stage: &str) {
    metrics::global()
        .counter(
            "poly_stage_total",
            "Polymarket signals that reached each pipeline stage.",
            &[("stage", stage)],
        )
        .inc();
}

/// Count a signal by strategy and terminal stage.
pub(crate) fn count_signal_outcome(strategy: &str, outcome: &str) {
    metrics::global()
        .counter(
            "poly_signals_total",
            "Polymarket signals by strategy and terminal stage.",
            &[("strategy", strategy), ("outcome", outcome)],
        )
        .inc();
}

/// Count a rejected / failed signal by reason.
pub(crate) fn count_rejection(reason: &str) {
    metrics::global()
        .counter(
            "poly_rejections_total",
            "Polymarket signals rejected/failed by reason.",
            &[("reason", reason)],
        )
        .inc();
}

/// Observe the wall time of one signal through the pipeline.
pub(crate) fn observe_pipeline_latency(outcome: &str, ms: u64) {
    metrics::global()
        .histogram(
            "poly_pipeline_latency_ms",
            "Wall time of one Polymarket signal through the pipeline.",
            &[("outcome", outcome)],
            LATENCY_BUCKETS_MS,
        )
        .observe(ms);
}

/// Observe the age of the quote a decision was made on.
pub(crate) fn observe_quote_age(ms: u64) {
    metrics::global()
        .histogram(
            "poly_quote_age_ms",
            "Age of the quote a Polymarket decision was made on.",
            &[],
            QUOTE_AGE_BUCKETS_MS,
        )
        .observe(ms);
}

/// Count a venue-order lifecycle transition.
pub(crate) fn count_order_transition(from: &str, to: &str) {
    metrics::global()
        .counter(
            "poly_order_transitions_total",
            "Polymarket venue-order lifecycle transitions.",
            &[("from", from), ("to", to)],
        )
        .inc();
}

/// Count a venue order reaching a terminal result.
pub(crate) fn count_order_terminal(result: &str) {
    metrics::global()
        .counter(
            "poly_orders_total",
            "Polymarket venue orders by terminal result.",
            &[("result", result)],
        )
        .inc();
}

/// Count a booked fill by source.
pub(crate) fn count_fill(source: &str) {
    metrics::global()
        .counter(
            "poly_fills_total",
            "Polymarket fills booked, by source.",
            &[("source", source)],
        )
        .inc();
}

/// Count a cancel request by reason.
pub(crate) fn count_cancel(reason: &str) {
    metrics::global()
        .counter(
            "poly_cancel_total",
            "Polymarket cancel requests by reason.",
            &[("reason", reason)],
        )
        .inc();
}

/// Count a reconciliation finding by kind.
pub(crate) fn count_recon_finding(kind: &str) {
    metrics::global()
        .counter(
            "poly_recon_findings_total",
            "Polymarket reconciliation findings by kind.",
            &[("kind", kind)],
        )
        .inc();
}

/// Count a restart-recovery action.
pub(crate) fn count_recovery_action(action: &str) {
    metrics::global()
        .counter(
            "poly_recovery_actions_total",
            "Polymarket restart-recovery actions.",
            &[("action", action)],
        )
        .inc();
}

/// Count a durable-journal write / read that failed.
pub(crate) fn count_journal_error(op: &str) {
    metrics::global()
        .counter(
            "poly_journal_errors_total",
            "Polymarket journal writes/reads that failed.",
            &[("op", op)],
        )
        .inc();
}

/// Count an entry refused because the symbol is gated by unresolved
/// reconciliation (shared `bot_*` family, module label).
pub(crate) fn count_symbol_gated() {
    metrics::global()
        .counter(
            "bot_symbol_gated_entries_total",
            "Entries refused because the symbol is gated by unresolved reconciliation.",
            &[("module", "polymarket")],
        )
        .inc();
}

/// Count a duplicate logical execution prevented by an idempotency layer
/// (`where` = `poly_pipeline` | `poly_fill_journal`).
pub(crate) fn count_duplicate_prevented(where_: &str) {
    metrics::global()
        .counter(
            "bot_duplicate_execution_prevented_total",
            "Duplicate logical executions prevented by idempotency layers.",
            &[("where", where_)],
        )
        .inc();
}

/// Set the resting-venue-orders gauge.
pub(crate) fn set_open_orders(open: usize) {
    metrics::global()
        .gauge("poly_open_orders", "Polymarket resting venue orders.", &[])
        .set(open as i64);
}

/// Set the open-exposure gauge (milli-USDC).
pub(crate) fn set_exposure_usd(usd: f64) {
    metrics::global()
        .gauge(
            "poly_exposure_usd_milli",
            "Polymarket open exposure (positions + resting buys) in milli-USDC.",
            &[],
        )
        .set((usd * 1_000.0) as i64);
}

impl PolyBot {
    /// Publish the two gauges after every poll: resting venue orders and
    /// open exposure (positions + resting buys).
    pub(crate) async fn publish_gauges(&self) {
        let (open, resting) = {
            let tracked = self.tracked.read().await;
            let open = tracked.values().filter(|t| !t.state.is_terminal()).count();
            let resting: f64 = tracked.values().map(|t| t.resting_quote()).sum();
            (open, resting)
        };
        let positions = self.state.open_exposure(BotModule::Polymarket).await;
        set_open_orders(open);
        set_exposure_usd(positions + resting);
    }
}
