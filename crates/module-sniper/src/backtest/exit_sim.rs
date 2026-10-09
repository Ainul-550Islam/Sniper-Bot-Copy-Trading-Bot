//! Exit simulator (GAP-MAP P1 NEW): walks the scenario's price path after
//! the fill and closes the position at the first rule that triggers —
//! stop-loss first, then take-profit, then max hold, then end of data.
//! Pure and deterministic.
//!
//! The exit is charged `exit_cost_bps` (venue fee + a quarter-slippage
//! allowance, same convention as the entry model) against gross proceeds.

use serde::{Deserialize, Serialize};

use crate::backtest::dataset::BacktestScenario;
use crate::backtest::fill_model::FilledEntry;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitConfig {
    /// Close when net proceeds reach `cost × (1 + take_profit_pct)`.
    pub take_profit_pct: f64,
    /// Close when net proceeds fall to `cost × (1 - stop_loss_pct)`.
    pub stop_loss_pct: f64,
    /// Close unconditionally this many ms after entry.
    pub max_hold_ms: u64,
    /// Total exit cost in basis points of gross proceeds.
    pub exit_cost_bps: f64,
}

impl Default for ExitConfig {
    fn default() -> Self {
        ExitConfig {
            take_profit_pct: 1.0,
            stop_loss_pct: 0.3,
            max_hold_ms: 60_000,
            exit_cost_bps: 100.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    StopLoss,
    TakeProfit,
    MaxHold,
    EndOfData,
}

impl ExitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitReason::StopLoss => "stop_loss",
            ExitReason::TakeProfit => "take_profit",
            ExitReason::MaxHold => "max_hold",
            ExitReason::EndOfData => "end_of_data",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitOutcome {
    pub reason: ExitReason,
    pub exit_t_ms: u64,
    pub exit_price_sol: f64,
    /// Tokens × price, before exit costs.
    pub gross_sol: f64,
    /// Gross minus `exit_cost_bps` — what the wallet receives.
    pub net_sol: f64,
}

/// Value a position marks at one tick, after exit costs.
fn mark(tokens: f64, price: f64, exit_cost_bps: f64) -> (f64, f64) {
    let gross = tokens * price;
    let net = gross * (1.0 - exit_cost_bps / 10_000.0);
    (gross, net)
}

/// Simulate the exit policy against the path. `fill` must come from
/// [`crate::backtest::fill_model::simulate_fill`] on the same scenario.
///
/// Rule order per tick (deliberate: risk rules outrank profit rules):
/// stop-loss → take-profit → max-hold. When no tick triggers, the final
/// tick of the path closes the position as `EndOfData`.
pub fn simulate_exit(scenario: &BacktestScenario, cfg: &ExitConfig, fill: &FilledEntry) -> ExitOutcome {
    let cost = fill.cost_sol;
    // Ticks at or after the fill landed. simulate_fill guarantees at least
    // one exists (it filled against one), but stay defensive: fall back to
    // the whole path rather than divide by nothing.
    let path: Vec<_> = scenario
        .ticks
        .iter()
        .filter(|t| t.t_ms >= fill.fill_t_ms)
        .collect();
    let path = if path.is_empty() {
        scenario.ticks.iter().collect()
    } else {
        path
    };

    let stop_at = cost * (1.0 - cfg.stop_loss_pct.max(0.0));
    let target_at = cost * (1.0 + cfg.take_profit_pct.max(0.0));

    for tick in &path {
        let (gross, net) = mark(fill.tokens_bought, tick.price_sol, cfg.exit_cost_bps);
        if net <= stop_at {
            return ExitOutcome {
                reason: ExitReason::StopLoss,
                exit_t_ms: tick.t_ms,
                exit_price_sol: tick.price_sol,
                gross_sol: gross,
                net_sol: net,
            };
        }
        if net >= target_at {
            return ExitOutcome {
                reason: ExitReason::TakeProfit,
                exit_t_ms: tick.t_ms,
                exit_price_sol: tick.price_sol,
                gross_sol: gross,
                net_sol: net,
            };
        }
        if tick.t_ms.saturating_sub(scenario.entry_t_ms) >= cfg.max_hold_ms {
            return ExitOutcome {
                reason: ExitReason::MaxHold,
                exit_t_ms: tick.t_ms,
                exit_price_sol: tick.price_sol,
                gross_sol: gross,
                net_sol: net,
            };
        }
    }

    let last = path.last().expect("scenario validated non-empty tick path");
    let (gross, net) = mark(fill.tokens_bought, last.price_sol, cfg.exit_cost_bps);
    ExitOutcome {
        reason: ExitReason::EndOfData,
        exit_t_ms: last.t_ms,
        exit_price_sol: last.price_sol,
        gross_sol: gross,
        net_sol: net,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::dataset::{DatasetLabel, Tick};
    use crate::backtest::fill_model::{simulate_fill, FillModelConfig};

    fn crafted(prices: &[f64]) -> BacktestScenario {
        BacktestScenario {
            label: DatasetLabel::Synthetic,
            name: "crafted".into(),
            seed: 0,
            entry_t_ms: 0,
            entry_quote_lamports: 1_000_000, // 0.001 SOL
            initial_reserve_sol: 100.0, // big reserve → tiny impact/fraction effects
            congestion: 0.0,
            ticks: prices
                .iter()
                .enumerate()
                .map(|(i, p)| Tick {
                    t_ms: (i as u64) * 1_000,
                    price_sol: *p,
                    quote_reserve_lamports: 100_000_000_000,
                })
                .collect(),
        }
    }

    fn filled(s: &BacktestScenario) -> FilledEntry {
        let mut cfg = FillModelConfig::default();
        cfg.entry_latency_ms = 0;
        match simulate_fill(s, &cfg) {
            crate::backtest::fill_model::FillOutcome::Filled(f) => f,
            other => panic!("expected fill: {other:?}"),
        }
    }

    #[test]
    fn stop_loss_fires_before_take_profit_on_the_same_tick() {
        // cost ~0.001 SOL; a crash to 1/1000th crosses the stop.
        let s = crafted(&[1e-6, 1e-9]);
        let fill = filled(&s);
        let out = simulate_exit(&s, &ExitConfig::default(), &fill);
        assert_eq!(out.reason, ExitReason::StopLoss);
        assert_eq!(out.exit_t_ms, 1_000);
    }

    #[test]
    fn take_profit_fires_when_target_is_reached() {
        let s = crafted(&[1e-6, 1e-6, 10e-6]); // 10x on tick 2
        let fill = filled(&s);
        let cfg = ExitConfig {
            exit_cost_bps: 0.0,
            ..ExitConfig::default()
        };
        let out = simulate_exit(&s, &cfg, &fill);
        assert_eq!(out.reason, ExitReason::TakeProfit);
        assert_eq!(out.exit_t_ms, 2_000);
    }

    #[test]
    fn max_hold_closes_a_flat_market() {
        let s = crafted(&[1e-6; 400]); // 400 s of nothing
        let fill = filled(&s);
        let cfg = ExitConfig {
            max_hold_ms: 60_000,
            ..ExitConfig::default()
        };
        let out = simulate_exit(&s, &cfg, &fill);
        assert_eq!(out.reason, ExitReason::MaxHold);
        assert!(out.exit_t_ms >= 60_000);
    }

    #[test]
    fn end_of_data_closes_before_max_hold_when_the_path_is_short() {
        let s = crafted(&[1e-6, 1.1e-6]);
        let fill = filled(&s);
        let cfg = ExitConfig {
            max_hold_ms: 60_000,
            exit_cost_bps: 0.0,
            ..ExitConfig::default()
        };
        let out = simulate_exit(&s, &cfg, &fill);
        // +10% is below the +100% take-profit: the path simply ends.
        assert_eq!(out.reason, ExitReason::EndOfData);
        assert_eq!(out.exit_t_ms, 1_000);
        assert!(out.net_sol > fill.cost_sol);
    }

    #[test]
    fn exit_costs_reduce_net_proceeds() {
        let s = crafted(&[1e-6, 10e-6]);
        let fill = filled(&s);
        let free = simulate_exit(&s, &ExitConfig { exit_cost_bps: 0.0, ..ExitConfig::default() }, &fill);
        let paid = simulate_exit(&s, &ExitConfig { exit_cost_bps: 200.0, ..ExitConfig::default() }, &fill);
        assert!((paid.net_sol - free.net_sol * (1.0 - 200.0 / 10_000.0)).abs() < 1e-12);
    }
}
