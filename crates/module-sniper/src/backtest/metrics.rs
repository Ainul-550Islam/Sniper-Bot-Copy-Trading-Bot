//! Backtest metrics (GAP-MAP P1 NEW): net PnL, win rate, max drawdown,
//! Sharpe — computed ONLY when the sample is large enough, never faked.
//!
//! Sharpe needs a real distribution: with fewer than
//! [`SHARPE_MIN_TRADES`] closed trades it is `None`, and a report renderer
//! must show "—" for it rather than invent a number from a tiny sample.

use serde::{Deserialize, Serialize};

use crate::backtest::dataset::BacktestScenario;
use crate::backtest::exit_sim::{ExitOutcome, ExitReason};
use crate::backtest::fill_model::FilledEntry;

/// Minimum closed trades before a Sharpe ratio is reported.
pub const SHARPE_MIN_TRADES: usize = 30;

/// One round-trip: entry fill → exit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeRecord {
    /// Scenario name (join key back to the fixture/dataset).
    pub scenario: String,
    /// SOL actually spent on entry (size × filled fraction).
    pub entry_sol: f64,
    pub tokens_bought: f64,
    pub fill_price_sol: f64,
    pub exit_reason: String,
    pub exit_price_sol: f64,
    /// Exit proceeds before costs.
    pub gross_sol: f64,
    /// Exit proceeds after costs — what the wallet received.
    pub net_sol: f64,
    /// `net_sol - entry_sol`.
    pub pnl_sol: f64,
    /// `net_sol / entry_sol` (≥ 0; entry_sol > 0 by construction).
    pub returned_ratio: f64,
}

impl TradeRecord {
    pub fn from_fill_and_exit(
        scenario: &BacktestScenario,
        fill: &FilledEntry,
        exit: &ExitOutcome,
    ) -> Self {
        let pnl_sol = exit.net_sol - fill.cost_sol;
        TradeRecord {
            scenario: scenario.name.clone(),
            entry_sol: fill.cost_sol,
            tokens_bought: fill.tokens_bought,
            fill_price_sol: fill.fill_price_sol,
            exit_reason: exit.reason.as_str().to_string(),
            exit_price_sol: exit.exit_price_sol,
            gross_sol: exit.gross_sol,
            net_sol: exit.net_sol,
            pnl_sol,
            returned_ratio: exit.net_sol / fill.cost_sol,
        }
    }
}

/// Aggregate performance over a set of closed trades.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestMetrics {
    pub trades: u64,
    pub net_pnl_sol: f64,
    /// Fraction of trades with positive PnL (0.0 when no trades).
    pub win_rate: f64,
    /// Largest peak-to-trough loss of the cumulative PnL curve, SOL ≤ 0.
    /// The curve starts at 0 equity, so one losing trade shows its full loss.
    pub max_drawdown_sol: f64,
    pub best_pnl_sol: f64,
    pub worst_pnl_sol: f64,
    /// Mean / population-std of per-trade returns, `None` below
    /// [`SHARPE_MIN_TRADES`] or with zero variance.
    pub sharpe: Option<f64>,
}

impl Default for BacktestMetrics {
    fn default() -> Self {
        BacktestMetrics {
            trades: 0,
            net_pnl_sol: 0.0,
            win_rate: 0.0,
            max_drawdown_sol: 0.0,
            best_pnl_sol: 0.0,
            worst_pnl_sol: 0.0,
            sharpe: None,
        }
    }
}

/// Pure aggregation. Input order matters for the drawdown walk; the caller
/// (`run_backtest`) sorts records by scenario name first so the result is
/// independent of the order scenarios were fed in.
pub fn compute_metrics(records: &[TradeRecord]) -> BacktestMetrics {
    if records.is_empty() {
        return BacktestMetrics::default();
    }

    let n = records.len();
    let mut net_pnl_sol = 0.0;
    let mut wins = 0u64;
    let mut best = f64::MIN;
    let mut worst = f64::MAX;

    // Cumulative-PnL drawdown (equity curve starts at 0).
    let mut cum = 0.0_f64;
    let mut peak = 0.0_f64;
    let mut max_dd = 0.0_f64;

    let mut returns: Vec<f64> = Vec::with_capacity(n);

    for r in records {
        net_pnl_sol += r.pnl_sol;
        if r.pnl_sol > 0.0 {
            wins += 1;
        }
        if r.pnl_sol > best {
            best = r.pnl_sol;
        }
        if r.pnl_sol < worst {
            worst = r.pnl_sol;
        }
        cum += r.pnl_sol;
        if cum > peak {
            peak = cum;
        }
        let dd = cum - peak;
        if dd < max_dd {
            max_dd = dd;
        }
        returns.push(r.pnl_sol / r.entry_sol);
    }

    let sharpe = if n >= SHARPE_MIN_TRADES {
        let mean = returns.iter().sum::<f64>() / n as f64;
        let var = returns.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64;
        let std = var.sqrt();
        // Relative tolerance: identical returns do not average back to the
        // exact same f64, so their deviation is rounding noise, not variance.
        // Treat that as zero variance (no Sharpe) rather than a huge ratio.
        let noise_floor = 1e-12 * mean.abs().max(1.0);
        if std > noise_floor {
            Some(mean / std)
        } else {
            None
        }
    } else {
        None
    };

    BacktestMetrics {
        trades: n as u64,
        net_pnl_sol,
        win_rate: wins as f64 / n as f64,
        max_drawdown_sol: max_dd,
        best_pnl_sol: best,
        worst_pnl_sol: worst,
        sharpe,
    }
}

/// Convenience: exit reasons that count as the strategy working
/// (`EndOfData` means the path ran out before any rule triggered).
pub fn is_planned_exit(reason: &str) -> bool {
    reason == ExitReason::TakeProfit.as_str()
        || reason == ExitReason::StopLoss.as_str()
        || reason == ExitReason::MaxHold.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, entry: f64, pnl: f64) -> TradeRecord {
        TradeRecord {
            scenario: name.into(),
            entry_sol: entry,
            tokens_bought: 1.0,
            fill_price_sol: entry,
            exit_reason: "end_of_data".into(),
            exit_price_sol: entry,
            gross_sol: entry + pnl,
            net_sol: entry + pnl,
            pnl_sol: pnl,
            returned_ratio: (entry + pnl) / entry,
        }
    }

    #[test]
    fn empty_input_gives_default_metrics() {
        let m = compute_metrics(&[]);
        assert_eq!(m, BacktestMetrics::default());
        assert!(m.sharpe.is_none());
    }

    #[test]
    fn single_losing_trade_shows_full_drawdown() {
        let m = compute_metrics(&[record("a", 1.0, -0.5)]);
        assert_eq!(m.trades, 1);
        assert!((m.net_pnl_sol + 0.5).abs() < 1e-12);
        assert!((m.win_rate - 0.0).abs() < 1e-12);
        // Equity curve: 0 → -0.5; peak stays 0, dd = -0.5.
        assert!((m.max_drawdown_sol + 0.5).abs() < 1e-12);
        assert!(m.sharpe.is_none(), "one trade cannot have a Sharpe");
    }

    #[test]
    fn drawdown_walks_peak_to_trough() {
        // +1, -2, +0.5 → curve 0,1,-1,-0.5; worst dd = -1 - 1 = -2.
        let m = compute_metrics(&[
            record("a", 1.0, 1.0),
            record("b", 1.0, -2.0),
            record("c", 1.0, 0.5),
        ]);
        assert!((m.max_drawdown_sol + 2.0).abs() < 1e-12);
        assert!((m.win_rate - 2.0 / 3.0).abs() < 1e-12);
        assert!((m.best_pnl_sol - 1.0).abs() < 1e-12);
        assert!((m.worst_pnl_sol + 2.0).abs() < 1e-12);
    }

    #[test]
    fn sharpe_needs_enough_points() {
        let records: Vec<TradeRecord> = (0..SHARPE_MIN_TRADES - 1)
            .map(|i| record(&format!("t{i}"), 1.0, if i % 2 == 0 { 0.1 } else { -0.05 }))
            .collect();
        assert!(compute_metrics(&records).sharpe.is_none());

        let mut enough = records.clone();
        enough.push(record("final", 1.0, 0.2));
        let m = compute_metrics(&enough);
        assert!(m.sharpe.is_some());
        assert!(m.sharpe.unwrap().is_finite());
    }

    #[test]
    fn zero_variance_returns_no_sharpe() {
        let records: Vec<TradeRecord> = (0..SHARPE_MIN_TRADES)
            .map(|i| record(&format!("t{i}"), 1.0, 0.1))
            .collect();
        assert!(compute_metrics(&records).sharpe.is_none());
    }
}
