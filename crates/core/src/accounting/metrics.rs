//! Global accounting metrics (TASK 5 §8) — the `global_ledger_*` /
//! `global_portfolio_*` families, registered through the process-wide
//! [`crate::obs::metrics`] registry like every other module family.
//!
//! | series | kind | labels | meaning |
//! |---|---|---|---|
//! | `global_ledger_events_total` | counter | `kind`, `outcome` | submissions by kind and `new` / `duplicate` / `rejected` |
//! | `global_ledger_journal_errors_total` | counter | `op` | durable journal writes / reads that failed |
//! | `global_ledger_pending_events` | gauge | — | applied events not yet durably journaled |
//! | `global_accounting_findings_total` | counter | `kind` | accounting reconciliation findings (new ids) |
//! | `global_accounting_recovery_actions_total` | counter | `action` | restart recovery actions |
//! | `global_portfolio_exposure_ref_milli` | gauge | `scope`, `key` | exposure per venue / wallet / strategy / module (milli reference units) |
//! | `global_portfolio_total_exposure_ref_milli` | gauge | — | total exposure |
//! | `global_portfolio_utilization_milli` | gauge | — | exposure / capital base × 1000 |
//! | `global_portfolio_realized_pnl_ref_milli` | gauge | — | gross realized PnL |
//! | `global_portfolio_unrealized_pnl_ref_milli` | gauge | — | unrealized PnL |
//! | `global_portfolio_fees_ref_milli` | gauge | — | fees |
//! | `global_portfolio_net_pnl_ref_milli` | gauge | — | realized + unrealized − fees |
//! | `global_portfolio_realized_today_ref_milli` | gauge | — | net realized for the UTC day |
//! | `global_portfolio_drawdown_ref_milli` | gauge | — | drawdown from the realized peak |
//! | `global_portfolio_open_positions` | gauge | — | open aggregated positions |
//! | `global_portfolio_missing_rates` | gauge | — | quote assets with exposure but no reference rate |
//!
//! The shared `bot_duplicate_execution_prevented_total{where}` family is
//! also fed (`global_ledger`, `global_ledger_journal`). Cardinality is
//! bounded: kinds, outcomes, ops, actions and scopes are closed sets; `key`
//! is the small configured set of venues / wallets / strategies / modules.

use crate::obs::metrics;

use super::view::PortfolioView;

/// Count one submission outcome.
pub(crate) fn count_ledger_event(kind: &str, outcome: &str) {
    metrics::global()
        .counter(
            "global_ledger_events_total",
            "Global ledger submissions by kind and outcome (new / duplicate / rejected).",
            &[("kind", kind), ("outcome", outcome)],
        )
        .inc();
}

/// Count one failed journal operation.
pub(crate) fn count_journal_error(op: &str) {
    metrics::global()
        .counter(
            "global_ledger_journal_errors_total",
            "Global ledger durable journal operations that failed.",
            &[("op", op)],
        )
        .inc();
}

/// Feed the shared duplicate-prevention family.
pub(crate) fn count_duplicate_prevented(where_: &str) {
    metrics::global()
        .counter(
            "bot_duplicate_execution_prevented_total",
            "Duplicate logical executions prevented by idempotency layers.",
            &[("where", where_)],
        )
        .inc();
}

/// Count new reconciliation findings by kind.
pub(crate) fn count_finding(kind: &str) {
    metrics::global()
        .counter(
            "global_accounting_findings_total",
            "Global accounting reconciliation findings by kind.",
            &[("kind", kind)],
        )
        .inc();
}

/// Count recovery actions (`n` may be zero — the series still registers).
pub(crate) fn count_recovery_action(action: &str, n: usize) {
    metrics::global()
        .counter(
            "global_accounting_recovery_actions_total",
            "Global ledger restart-recovery actions.",
            &[("action", action)],
        )
        .inc_by(n as u64);
}

/// Set the pending-events gauge.
pub(crate) fn set_pending(n: usize) {
    metrics::global()
        .gauge(
            "global_ledger_pending_events",
            "Global ledger events applied in memory but not yet durably journaled.",
            &[],
        )
        .set(n as i64);
}

fn milli(v: f64) -> i64 {
    if v.is_finite() {
        (v * 1_000.0).round() as i64
    } else {
        0
    }
}

/// Publish the portfolio gauges from one view.
pub fn publish_portfolio(view: &PortfolioView) {
    let g = metrics::global();
    g.gauge(
        "global_portfolio_total_exposure_ref_milli",
        "Total open exposure across every module in milli reference units.",
        &[],
    )
    .set(milli(view.total_exposure_ref));
    g.gauge(
        "global_portfolio_utilization_milli",
        "Total exposure divided by the configured capital base, times 1000.",
        &[],
    )
    .set(milli(view.utilization));
    g.gauge(
        "global_portfolio_realized_pnl_ref_milli",
        "Gross realized PnL across every module in milli reference units.",
        &[],
    )
    .set(milli(view.realized_ref));
    g.gauge(
        "global_portfolio_unrealized_pnl_ref_milli",
        "Unrealized PnL across every module in milli reference units.",
        &[],
    )
    .set(milli(view.unrealized_ref));
    g.gauge(
        "global_portfolio_fees_ref_milli",
        "Fees paid across every module in milli reference units.",
        &[],
    )
    .set(milli(view.fees_ref));
    g.gauge(
        "global_portfolio_net_pnl_ref_milli",
        "Realized + unrealized - fees across every module in milli reference units.",
        &[],
    )
    .set(milli(view.net_pnl_ref));
    g.gauge(
        "global_portfolio_realized_today_ref_milli",
        "Net realized PnL for the current UTC day in milli reference units.",
        &[],
    )
    .set(milli(view.realized_today_ref));
    g.gauge(
        "global_portfolio_drawdown_ref_milli",
        "Drawdown from the peak cumulative realized PnL (incl. unrealized) in milli reference units.",
        &[],
    )
    .set(milli(view.drawdown_ref()));
    g.gauge(
        "global_portfolio_open_positions",
        "Open aggregated positions across every module.",
        &[],
    )
    .set(view.open_positions as i64);
    g.gauge(
        "global_portfolio_missing_rates",
        "Quote assets that carry open exposure but have no reference rate configured.",
        &[],
    )
    .set(view.missing_rates.len() as i64);
    for (scope, map) in [
        ("venue", &view.by_venue),
        ("wallet", &view.by_wallet),
        ("strategy", &view.by_strategy),
        ("module", &view.by_module),
    ] {
        for (key, slice) in map {
            g.gauge(
                "global_portfolio_exposure_ref_milli",
                "Open exposure per venue / wallet / strategy / module in milli reference units.",
                &[("scope", scope), ("key", key)],
            )
            .set(milli(slice.exposure_ref));
        }
    }
}
