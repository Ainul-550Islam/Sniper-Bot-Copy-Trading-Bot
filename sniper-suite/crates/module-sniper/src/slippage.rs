//! Slippage engine (TASK 2 §H).
//!
//! One pure function decides the slippage tolerance for a sniper entry:
//!
//! * **fixed** — the configured base tolerance, unchanged (legacy behaviour);
//! * **liquidity-aware** — widens the base tolerance to at least twice the
//!   modelled price impact, so a thin pool does not fail the transaction on
//!   the very move our own buy causes, and never goes below the base;
//! * **price-impact** — the modelled impact plus the base tolerance as a
//!   buffer, i.e. "exactly what this trade needs against this pool".
//!
//! Precedence of the inputs: per-token override → per-protocol override →
//! strategy base. Every mode is bounded by the **hard maximum**
//! (`risk.max_slippage_bps`): the fixed mode passes its value through and
//! lets the risk engine reject an over-limit request (one authoritative
//! decision), while the adaptive modes refuse to *ask* for more than the
//! hard maximum and report `SlippageLimit` themselves — the pool is too thin
//! for the size, and no tolerance the operator allowed would fill it.
//!
//! All arithmetic is in `u128` with saturating conversions: zero liquidity,
//! dust liquidity, `u64::MAX` trades and reserves are all defined inputs.

use serde::{Deserialize, Serialize};

use bot_core::maths::BPS_DENOM;

/// Which rule sets the tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SlippageMode {
    #[default]
    Fixed,
    LiquidityAware,
    PriceImpact,
}

impl SlippageMode {
    /// Parse the config string (already normalised by
    /// `SniperConfig::slippage_mode_normalized`). Unknown → `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "fixed" => Some(SlippageMode::Fixed),
            "liquidity_aware" => Some(SlippageMode::LiquidityAware),
            "price_impact" => Some(SlippageMode::PriceImpact),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SlippageMode::Fixed => "fixed",
            SlippageMode::LiquidityAware => "liquidity_aware",
            SlippageMode::PriceImpact => "price_impact",
        }
    }
}

/// Where the base tolerance came from (observability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlippageSource {
    Strategy,
    Protocol,
    Token,
}

impl SlippageSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SlippageSource::Strategy => "strategy",
            SlippageSource::Protocol => "protocol",
            SlippageSource::Token => "token",
        }
    }
}

/// Everything the engine needs for one decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlippageInputs {
    pub mode: SlippageMode,
    /// `sniper.slippage_pct` in basis points.
    pub strategy_bps: u64,
    /// Per-protocol override (`pumpswap_slippage_pct` / `raydium_slippage_pct`).
    pub protocol_bps: Option<u64>,
    /// Per-token override (`[sniper.slippage_overrides_bps]`).
    pub token_bps: Option<u64>,
    /// `risk.max_slippage_bps` — the hard ceiling for every mode.
    pub hard_max_bps: u64,
    /// Quote-side reserve the trade goes against, in lamports.
    pub quote_reserve_lamports: u64,
    /// Quote amount we intend to spend, in lamports.
    pub trade_lamports: u64,
}

/// The engine's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlippageDecision {
    /// Tolerance to request from the venue, in basis points.
    pub bps: u64,
    /// Modelled price impact of the trade against the given reserve, bps.
    pub price_impact_bps: u64,
    pub mode: SlippageMode,
    pub source: SlippageSource,
    /// True when an adaptive mode wanted more than the hard maximum allowed
    /// and settled exactly on it (the impact itself still fits).
    pub clamped: bool,
}

/// Why no tolerance could be decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlippageError {
    /// The quote reserve is zero: there is nothing to trade against.
    ZeroLiquidity,
    /// The trade's own impact exceeds the hard maximum — no allowed
    /// tolerance can absorb it.
    ImpactExceedsHardMax {
        price_impact_bps: u64,
        hard_max_bps: u64,
    },
    /// The trade size is zero.
    ZeroTrade,
}

impl std::fmt::Display for SlippageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlippageError::ZeroLiquidity => f.write_str("quote reserve is zero"),
            SlippageError::ImpactExceedsHardMax {
                price_impact_bps,
                hard_max_bps,
            } => write!(
                f,
                "modelled price impact {price_impact_bps}bps exceeds the hard slippage maximum {hard_max_bps}bps"
            ),
            SlippageError::ZeroTrade => f.write_str("trade size is zero"),
        }
    }
}

/// Price impact of buying with `trade_in` against a constant-product pool
/// whose input-side reserve is `reserve_in`, in basis points of the spot
/// price: `trade_in / (reserve_in + trade_in)`. This is exactly the relative
/// gap between the spot price and the average execution price for x·y=k
/// (fees excluded — they are charged on top and do not move the price).
///
/// Total on hostile inputs: zero reserve → 10 000 bps (100%), zero trade →
/// 0 bps, `u64::MAX` on both sides → 5 000 bps without overflow.
pub fn price_impact_bps(trade_in: u64, reserve_in: u64) -> u64 {
    if trade_in == 0 {
        return 0;
    }
    if reserve_in == 0 {
        return BPS_DENOM;
    }
    let trade = trade_in as u128;
    let denom = reserve_in as u128 + trade;
    let bps = trade * BPS_DENOM as u128 / denom;
    u64::try_from(bps).unwrap_or(BPS_DENOM).min(BPS_DENOM)
}

/// Percent → basis points, rounded, saturating at 100%.
pub fn pct_to_bps(pct: f64) -> u64 {
    if !pct.is_finite() || pct <= 0.0 {
        return 0;
    }
    ((pct * 100.0).round() as u64).min(BPS_DENOM)
}

/// Basis points → percent (for the venue builders that take percent).
pub fn bps_to_pct(bps: u64) -> f64 {
    bps.min(BPS_DENOM) as f64 / 100.0
}

/// Decide the tolerance. Pure; see the module docs for the rules.
pub fn decide(inputs: &SlippageInputs) -> Result<SlippageDecision, SlippageError> {
    let hard_max = inputs.hard_max_bps.min(BPS_DENOM);
    let (base, source) = match (inputs.token_bps, inputs.protocol_bps) {
        (Some(t), _) => (t, SlippageSource::Token),
        (None, Some(p)) => (p, SlippageSource::Protocol),
        (None, None) => (inputs.strategy_bps, SlippageSource::Strategy),
    };
    let base = base.min(BPS_DENOM);
    let impact = price_impact_bps(inputs.trade_lamports, inputs.quote_reserve_lamports);

    match inputs.mode {
        SlippageMode::Fixed => Ok(SlippageDecision {
            // Passed through unclamped on purpose: the risk engine owns the
            // over-limit rejection so there is exactly one place that says
            // "slippage too high".
            bps: base,
            price_impact_bps: impact,
            mode: inputs.mode,
            source,
            clamped: false,
        }),
        SlippageMode::LiquidityAware | SlippageMode::PriceImpact => {
            if inputs.trade_lamports == 0 {
                return Err(SlippageError::ZeroTrade);
            }
            if inputs.quote_reserve_lamports == 0 {
                return Err(SlippageError::ZeroLiquidity);
            }
            if impact > hard_max {
                return Err(SlippageError::ImpactExceedsHardMax {
                    price_impact_bps: impact,
                    hard_max_bps: hard_max,
                });
            }
            let wanted = match inputs.mode {
                SlippageMode::LiquidityAware => base.max(impact.saturating_mul(2)),
                _ => impact.saturating_add(base),
            };
            let clamped = wanted > hard_max;
            Ok(SlippageDecision {
                bps: wanted.min(hard_max),
                price_impact_bps: impact,
                mode: inputs.mode,
                source,
                clamped,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: u64 = 1_000_000_000;

    fn inputs(mode: SlippageMode, reserve: u64, trade: u64) -> SlippageInputs {
        SlippageInputs {
            mode,
            strategy_bps: 1_500,
            protocol_bps: None,
            token_bps: None,
            hard_max_bps: 3_000,
            quote_reserve_lamports: reserve,
            trade_lamports: trade,
        }
    }

    #[test]
    fn price_impact_matches_constant_product_math() {
        // 1 SOL into a 30 SOL reserve: 1/31 = 322.58 bps → 322.
        assert_eq!(price_impact_bps(SOL, 30 * SOL), 322);
        // Equal trade and reserve: 50%.
        assert_eq!(price_impact_bps(SOL, SOL), 5_000);
        // Dust trade: rounds to zero.
        assert_eq!(price_impact_bps(1, 30 * SOL), 0);
        assert_eq!(price_impact_bps(0, 30 * SOL), 0);
    }

    #[test]
    fn price_impact_is_total_on_zero_tiny_and_overflowing_inputs() {
        assert_eq!(price_impact_bps(SOL, 0), 10_000, "zero liquidity = 100%");
        assert_eq!(price_impact_bps(SOL, 1), 9_999, "one lamport of liquidity");
        assert_eq!(price_impact_bps(u64::MAX, u64::MAX), 5_000, "no overflow");
        assert_eq!(price_impact_bps(u64::MAX, 1), 9_999);
        assert_eq!(price_impact_bps(1, u64::MAX), 0);
        assert_eq!(price_impact_bps(0, 0), 0, "nothing traded = no impact");
    }

    #[test]
    fn fixed_mode_passes_the_base_through_and_reports_impact() {
        let d = decide(&inputs(SlippageMode::Fixed, 30 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 1_500);
        assert_eq!(d.price_impact_bps, 322);
        assert_eq!(d.source, SlippageSource::Strategy);
        assert!(!d.clamped);
        // Fixed does not need liquidity: zero reserve is still a decision
        // (the liquidity gate, not the slippage engine, rejects that pool).
        let d = decide(&inputs(SlippageMode::Fixed, 0, SOL)).unwrap();
        assert_eq!(d.bps, 1_500);
        assert_eq!(d.price_impact_bps, 10_000);
        // Above the hard max is passed through for the risk engine to refuse.
        let mut i = inputs(SlippageMode::Fixed, 30 * SOL, SOL);
        i.strategy_bps = 9_000;
        assert_eq!(decide(&i).unwrap().bps, 9_000);
        // ...but never above 100%.
        i.strategy_bps = 50_000;
        assert_eq!(decide(&i).unwrap().bps, 10_000);
    }

    #[test]
    fn override_precedence_is_token_then_protocol_then_strategy() {
        let mut i = inputs(SlippageMode::Fixed, 30 * SOL, SOL);
        i.protocol_bps = Some(2_000);
        let d = decide(&i).unwrap();
        assert_eq!((d.bps, d.source), (2_000, SlippageSource::Protocol));
        i.token_bps = Some(800);
        let d = decide(&i).unwrap();
        assert_eq!((d.bps, d.source), (800, SlippageSource::Token));
        i.token_bps = None;
        i.protocol_bps = None;
        let d = decide(&i).unwrap();
        assert_eq!((d.bps, d.source), (1_500, SlippageSource::Strategy));
    }

    #[test]
    fn liquidity_aware_widens_for_thin_pools_and_keeps_the_base_for_deep_ones() {
        // Deep pool: impact 322 bps, 2× = 644 < base 1500 → base.
        let d = decide(&inputs(SlippageMode::LiquidityAware, 30 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 1_500);
        assert!(!d.clamped);
        // Thin pool: 1 SOL into 1 SOL → impact 5000 > hard max 3000 → error.
        let err = decide(&inputs(SlippageMode::LiquidityAware, SOL, SOL)).unwrap_err();
        assert_eq!(
            err,
            SlippageError::ImpactExceedsHardMax {
                price_impact_bps: 5_000,
                hard_max_bps: 3_000
            }
        );
        // Medium pool: 1 SOL into 9 SOL → impact 1000, 2× = 2000 → 2000.
        let d = decide(&inputs(SlippageMode::LiquidityAware, 9 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 2_000);
        assert_eq!(d.price_impact_bps, 1_000);
        // 1 SOL into 4 SOL → impact 2000, 2× = 4000 > hard max → clamped to 3000.
        let d = decide(&inputs(SlippageMode::LiquidityAware, 4 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 3_000);
        assert!(d.clamped);
    }

    #[test]
    fn price_impact_mode_adds_the_base_as_a_buffer() {
        let d = decide(&inputs(SlippageMode::PriceImpact, 30 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 322 + 1_500);
        assert!(!d.clamped);
        // 1 SOL into 9 SOL → 1000 + 1500 = 2500.
        let d = decide(&inputs(SlippageMode::PriceImpact, 9 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 2_500);
        // 1 SOL into 4 SOL → 2000 + 1500 = 3500 → clamped to 3000.
        let d = decide(&inputs(SlippageMode::PriceImpact, 4 * SOL, SOL)).unwrap();
        assert_eq!(d.bps, 3_000);
        assert!(d.clamped);
    }

    #[test]
    fn adaptive_modes_refuse_zero_and_dust_liquidity() {
        for mode in [SlippageMode::LiquidityAware, SlippageMode::PriceImpact] {
            assert_eq!(
                decide(&inputs(mode, 0, SOL)).unwrap_err(),
                SlippageError::ZeroLiquidity
            );
            assert_eq!(
                decide(&inputs(mode, 30 * SOL, 0)).unwrap_err(),
                SlippageError::ZeroTrade
            );
            // One lamport of liquidity: impact 9999 bps > hard max.
            assert!(matches!(
                decide(&inputs(mode, 1, SOL)).unwrap_err(),
                SlippageError::ImpactExceedsHardMax {
                    price_impact_bps: 9_999,
                    ..
                }
            ));
        }
    }

    #[test]
    fn hard_max_boundaries_are_inclusive() {
        // Impact exactly at the hard max is allowed (adaptive), one above is not.
        // impact = trade/(reserve+trade); for 3000 bps: trade = 3, reserve = 7.
        let mut i = inputs(SlippageMode::PriceImpact, 7, 3);
        i.strategy_bps = 0;
        let d = decide(&i).unwrap();
        assert_eq!(d.price_impact_bps, 3_000);
        assert_eq!(d.bps, 3_000);
        assert!(!d.clamped);
        // 3001 bps: trade = 3001, reserve = 6999.
        let i = inputs(SlippageMode::PriceImpact, 6_999, 3_001);
        assert!(matches!(
            decide(&i).unwrap_err(),
            SlippageError::ImpactExceedsHardMax {
                price_impact_bps: 3_001,
                ..
            }
        ));
        // Hard max above 100% is treated as 100%: 2 × 5000 = exactly the cap
        // (not clamped), 2 × 5005 wants more than 100% (clamped).
        let mut i = inputs(SlippageMode::LiquidityAware, SOL, SOL);
        i.hard_max_bps = 50_000;
        let d = decide(&i).unwrap();
        assert_eq!(d.bps, 10_000);
        assert!(!d.clamped, "2 × 5000 is exactly the 100% cap");
        let mut i = inputs(SlippageMode::LiquidityAware, 999, 1_001);
        i.hard_max_bps = 50_000;
        let d = decide(&i).unwrap();
        assert_eq!(d.price_impact_bps, 5_005);
        assert_eq!(d.bps, 10_000);
        assert!(d.clamped, "2 × 5005 wanted, capped at 100%");
    }

    #[test]
    fn overflow_inputs_never_panic_in_any_mode() {
        for mode in [
            SlippageMode::Fixed,
            SlippageMode::LiquidityAware,
            SlippageMode::PriceImpact,
        ] {
            let mut i = inputs(mode, u64::MAX, u64::MAX);
            i.strategy_bps = u64::MAX;
            i.hard_max_bps = u64::MAX;
            i.token_bps = Some(u64::MAX);
            let d = decide(&i).unwrap();
            assert!(d.bps <= 10_000);
            assert_eq!(d.price_impact_bps, 5_000);
        }
    }

    #[test]
    fn property_decisions_are_bounded_and_monotone_in_trade_size() {
        // Deterministic LCG so the property run is reproducible.
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2_000 {
            let reserve = next() % (1_000 * SOL) + 1;
            let trade = next() % (10 * SOL) + 1;
            let hard = next() % 10_001;
            let base = next() % 10_001;
            for mode in [
                SlippageMode::Fixed,
                SlippageMode::LiquidityAware,
                SlippageMode::PriceImpact,
            ] {
                let i = SlippageInputs {
                    mode,
                    strategy_bps: base,
                    protocol_bps: None,
                    token_bps: None,
                    hard_max_bps: hard,
                    quote_reserve_lamports: reserve,
                    trade_lamports: trade,
                };
                match decide(&i) {
                    Ok(d) => {
                        assert!(d.bps <= 10_000);
                        assert!(d.price_impact_bps <= 10_000);
                        if mode != SlippageMode::Fixed {
                            assert!(d.bps <= hard.min(10_000));
                            assert!(d.price_impact_bps <= hard);
                            assert!(d.bps >= d.price_impact_bps.min(hard));
                        }
                    }
                    Err(SlippageError::ImpactExceedsHardMax {
                        price_impact_bps,
                        hard_max_bps,
                    }) => {
                        assert!(price_impact_bps > hard_max_bps);
                        assert_ne!(mode, SlippageMode::Fixed);
                    }
                    Err(other) => panic!("unexpected error {other:?}"),
                }
                // Monotone: a bigger trade never has less impact.
                let bigger = price_impact_bps(trade.saturating_mul(2), reserve);
                assert!(bigger >= price_impact_bps(trade, reserve));
            }
        }
    }

    #[test]
    fn unit_conversions_round_trip() {
        assert_eq!(pct_to_bps(15.0), 1_500);
        assert_eq!(pct_to_bps(0.0), 0);
        assert_eq!(pct_to_bps(-3.0), 0);
        assert_eq!(pct_to_bps(f64::NAN), 0);
        assert_eq!(pct_to_bps(250.0), 10_000);
        assert_eq!(bps_to_pct(1_500), 15.0);
        assert_eq!(bps_to_pct(20_000), 100.0);
        assert_eq!(
            SlippageMode::parse("Liquidity-Aware"),
            Some(SlippageMode::LiquidityAware)
        );
        assert_eq!(
            SlippageMode::parse("price_impact"),
            Some(SlippageMode::PriceImpact)
        );
        assert_eq!(SlippageMode::parse("fixed"), Some(SlippageMode::Fixed));
        assert_eq!(SlippageMode::parse("x"), None);
        assert_eq!(SlippageMode::PriceImpact.as_str(), "price_impact");
        assert_eq!(SlippageSource::Token.as_str(), "token");
    }
}
