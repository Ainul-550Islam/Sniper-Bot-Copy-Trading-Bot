//! Entry fill model (GAP-MAP P1 NEW): latency, congestion/landing,
//! reserve-based price impact, partial fills. Pure and deterministic —
//! the same (scenario, config) always yields the same outcome.
//!
//! What this model deliberately does NOT claim to capture: queue position
//! inside a block, leader schedules, Jito bundle ordering, competing
//! snipers. It answers "given the launch path, what would a price-taker
//! entry at `entry_latency_ms` have paid and landed?" — see
//! `BacktestReport::model_caveats`.

use serde::{Deserialize, Serialize};

use crate::backtest::dataset::BacktestScenario;

/// Tip lamports a fully competitive bundle pays at congestion 1.0; the
/// required tip scales linearly with the scenario's congestion.
pub const CONGESTION_FULL_TIP_LAMPORTS: f64 = 5_000_000.0;

/// Impact is capped: beyond 50% of the reserve the constant-product math
/// stops being a useful approximation for a single taker swap.
pub const MAX_IMPACT_BPS: f64 = 5_000.0;

/// Fraction of worst-case slippage the model charges on entry (a quarter:
/// the average realized slip of a bounded move, not the bound itself).
pub const SLIPPAGE_COST_FACTOR: f64 = 0.25;

/// The entry can consume at most this fraction of the quote reserve in one
/// swap; anything larger fills only partially (the rest is left on the
/// table, matching how real size limits behave).
pub const MAX_RESERVE_FRACTION: f64 = 0.05;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FillModelConfig {
    /// Event-to-transaction latency budget, milliseconds.
    pub entry_latency_ms: u64,
    /// Configured slippage tolerance (percent) — charged at 25% of worst case.
    pub slippage_pct: f64,
    /// Jito tip the operator pays (drives the landing threshold).
    pub tip_lamports: u64,
}

impl Default for FillModelConfig {
    fn default() -> Self {
        FillModelConfig {
            entry_latency_ms: 250,
            slippage_pct: 15.0,
            tip_lamports: 1_000_000,
        }
    }
}

/// Why an entry did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillReject {
    /// The scenario failed its own invariants (`defects()` non-empty).
    InvalidScenario,
    /// The price path ends before `entry_t_ms + entry_latency_ms`: the bot
    /// was too slow for this launch.
    NoTickAfterLatency,
    /// The configured tip could not beat the scenario's congestion.
    OutbidInCongestion,
}

impl FillReject {
    pub fn as_str(&self) -> &'static str {
        match self {
            FillReject::InvalidScenario => "invalid_scenario",
            FillReject::NoTickAfterLatency => "no_tick_after_latency",
            FillReject::OutbidInCongestion => "outbid_in_congestion",
        }
    }
}

/// A successful fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilledEntry {
    /// The tick the fill executed against.
    pub fill_t_ms: u64,
    /// Effective fill price in SOL per whole token (post impact+slippage).
    pub fill_price_sol: f64,
    /// SOL actually spent (entry size × filled fraction).
    pub cost_sol: f64,
    /// Tokens received.
    pub tokens_bought: f64,
    /// 0..=1 — below 1 when the size exceeded `MAX_RESERVE_FRACTION`.
    pub filled_fraction: f64,
    /// Reserve-based impact charged, in basis points.
    pub impact_bps: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FillOutcome {
    Filled(FilledEntry),
    Rejected(FillReject),
}

/// The pure fill model. Order of decisions: validity → landing → price.
pub fn simulate_fill(scenario: &BacktestScenario, cfg: &FillModelConfig) -> FillOutcome {
    if !scenario.defects().is_empty() {
        return FillOutcome::Rejected(FillReject::InvalidScenario);
    }

    // Landing: a deterministic congestion threshold. At congestion c the
    // market requires c × 5 SOL-in-lamports of tip; pay less and a better
    // bundle takes the slot.
    let required_tip = scenario.congestion * CONGESTION_FULL_TIP_LAMPORTS;
    if (cfg.tip_lamports as f64) < required_tip {
        return FillOutcome::Rejected(FillReject::OutbidInCongestion);
    }

    // The first tick at or after the transaction would land.
    let land_t = scenario.entry_t_ms.saturating_add(cfg.entry_latency_ms);
    let fill_tick = match scenario.ticks.iter().find(|t| t.t_ms >= land_t) {
        Some(t) => t,
        None => return FillOutcome::Rejected(FillReject::NoTickAfterLatency),
    };

    let entry_sol = scenario.entry_quote_lamports as f64 / 1e9;

    // Impact: linear in entry size over reserve, capped.
    let impact_bps = ((entry_sol / scenario.initial_reserve_sol) * 10_000.0).min(MAX_IMPACT_BPS);

    // Slippage cost: a quarter of the configured worst case.
    let slippage_cost_bps = cfg.slippage_pct.max(0.0) * 100.0 * SLIPPAGE_COST_FACTOR;

    let fill_price_sol = fill_tick.price_sol * (1.0 + (impact_bps + slippage_cost_bps) / 10_000.0);

    // Partial fills: the swap may consume at most 5% of the reserve.
    let filled_fraction =
        1.0_f64.min((scenario.initial_reserve_sol * MAX_RESERVE_FRACTION) / entry_sol);
    let cost_sol = entry_sol * filled_fraction;
    let tokens_bought = cost_sol / fill_price_sol;

    FillOutcome::Filled(FilledEntry {
        fill_t_ms: fill_tick.t_ms,
        fill_price_sol,
        cost_sol,
        tokens_bought,
        filled_fraction,
        impact_bps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::dataset::synthetic_scenarios;

    fn scenario() -> BacktestScenario {
        synthetic_scenarios(1, 1).pop().unwrap()
    }

    #[test]
    fn zero_congestion_always_lands() {
        let mut s = scenario();
        s.congestion = 0.0;
        let mut cfg = FillModelConfig::default();
        cfg.tip_lamports = 0;
        assert!(matches!(simulate_fill(&s, &cfg), FillOutcome::Filled(_)));
    }

    #[test]
    fn congestion_requires_a_matching_tip() {
        let mut s = scenario();
        s.congestion = 1.0; // needs the full 0.005 SOL tip
        let mut cfg = FillModelConfig::default();
        cfg.tip_lamports = 4_999_999;
        assert_eq!(
            simulate_fill(&s, &cfg),
            FillOutcome::Rejected(FillReject::OutbidInCongestion)
        );
        cfg.tip_lamports = 5_000_000;
        assert!(matches!(simulate_fill(&s, &cfg), FillOutcome::Filled(_)));
    }

    #[test]
    fn latency_beyond_the_path_is_a_miss() {
        let mut s = scenario();
        let mut cfg = FillModelConfig::default();
        cfg.entry_latency_ms = s.ticks.last().unwrap().t_ms + 1;
        assert_eq!(
            simulate_fill(&s, &cfg),
            FillOutcome::Rejected(FillReject::NoTickAfterLatency)
        );
    }

    #[test]
    fn large_entries_fill_partially() {
        let mut s = scenario();
        s.congestion = 0.0;
        // 100 SOL into a ~30-50 SOL reserve is way over 5%.
        s.entry_quote_lamports = 100_000_000_000;
        let fill = match simulate_fill(&s, &FillModelConfig::default()) {
            FillOutcome::Filled(f) => f,
            other => panic!("expected fill, got {other:?}"),
        };
        assert!(fill.filled_fraction < 1.0);
        let want = (s.initial_reserve_sol * MAX_RESERVE_FRACTION) / 100.0;
        assert!((fill.filled_fraction - want).abs() < 1e-12);
        assert!((fill.cost_sol - s.initial_reserve_sol * MAX_RESERVE_FRACTION).abs() < 1e-9);
    }

    #[test]
    fn fill_price_includes_impact_and_quarter_slippage() {
        let mut s = scenario();
        s.congestion = 0.0;
        s.initial_reserve_sol = 40.0;
        s.entry_quote_lamports = 10_000_000; // 0.01 SOL
        let cfg = FillModelConfig {
            entry_latency_ms: 0,
            slippage_pct: 20.0,
            tip_lamports: 0,
        };
        let fill = match simulate_fill(&s, &cfg) {
            FillOutcome::Filled(f) => f,
            other => panic!("expected fill, got {other:?}"),
        };
        let tick = s.ticks.iter().find(|t| t.t_ms >= s.entry_t_ms).unwrap();
        let impact_bps = (0.01 / 40.0) * 10_000.0; // 2.5 bps
        assert!((fill.impact_bps - impact_bps).abs() < 1e-12);
        let slip_bps = 20.0 * 100.0 * SLIPPAGE_COST_FACTOR; // 500 bps
        let want_price = tick.price_sol * (1.0 + (impact_bps + slip_bps) / 10_000.0);
        assert!((fill.fill_price_sol - want_price).abs() < 1e-18);
    }
}
