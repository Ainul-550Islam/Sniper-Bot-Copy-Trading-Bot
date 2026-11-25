//! Builder fee estimates and observed-fee comparison for confirmed fills.
//!
//! Polymarket describes builder fees as matched notional times the
//! profile's maker/taker rate, separately from venue/platform fees. The
//! current documentation does not specify integer micro-unit rounding;
//! this module preserves the exact rational value and reports a range.
//! It does not query the venue, verify settlement, post a ledger entry, or
//! prove that the operator has been paid.
//!
//! Source, accessed 2026-11-09: https://docs.polymarket.com/programs/builders/fees

use crate::error::{PolyError, PolyResult};

const BPS_DENOMINATOR: i128 = 10_000;

/// Which side of the match earned the builder fee. Maker and taker rates
/// are configured independently and must not be guessed from buy/sell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuilderLiquidityRole {
    /// Resting maker order.
    Maker,
    /// Liquidity-taking order.
    Taker,
}

/// The configured maker/taker rates, in basis points, at fill time.
/// These are explicit caller-supplied venue facts, not tenant plan rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuilderFeeRates {
    /// Maker rate at the fill.
    pub maker_bps: u16,
    /// Taker rate at the fill.
    pub taker_bps: u16,
}

impl BuilderFeeRates {
    /// Reject rates beyond the documented builder-program limits.
    pub fn new(maker_bps: u16, taker_bps: u16) -> PolyResult<Self> {
        if maker_bps > 50 || taker_bps > 100 {
            return Err(PolyError::invalid(
                "builder fee rate exceeds documented limit",
            ));
        }
        Ok(Self {
            maker_bps,
            taker_bps,
        })
    }

    fn rate(self, role: BuilderLiquidityRole) -> u16 {
        match role {
            BuilderLiquidityRole::Maker => self.maker_bps,
            BuilderLiquidityRole::Taker => self.taker_bps,
        }
    }
}

/// Expected fee represented as an exact fraction of one pUSD micro-unit.
/// A range is not a claim about the venue's rounding method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuilderFeeEstimate {
    /// Matched notional in pUSD micro-units.
    pub notional_micros: u64,
    /// Applicable builder rate.
    pub rate_bps: u16,
    /// Fee in micro-units multiplied by 10,000.
    pub numerator: u128,
    /// Rounded-down fee, without asserting a venue rounding rule.
    pub lower_micros: u64,
    /// Rounded-up fee, without asserting a venue rounding rule.
    pub upper_micros: u64,
}

/// Difference between the exact estimate and an independently reported fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuilderFeeComparison {
    /// No fee amount was supplied; reconciliation remains unresolved.
    MissingObservation,
    /// The observed integer amount equals the exact rational estimate.
    Exact,
    /// Within floor/ceiling bounds; the venue's rounding rule is not proven.
    WithinUnverifiedRoundingBounds,
    /// The reported fee lies outside the exact floor/ceiling bounds.
    Mismatch,
}

impl BuilderFeeEstimate {
    /// Compare to the venue-reported per-fill fee, never a daily payout.
    pub fn compare(&self, observed_micros: Option<u64>) -> BuilderFeeComparison {
        let Some(amount) = observed_micros else {
            return BuilderFeeComparison::MissingObservation;
        };
        if u128::from(amount) * BPS_DENOMINATOR as u128 == self.numerator {
            BuilderFeeComparison::Exact
        } else if amount >= self.lower_micros && amount <= self.upper_micros {
            BuilderFeeComparison::WithinUnverifiedRoundingBounds
        } else {
            BuilderFeeComparison::Mismatch
        }
    }
}

/// Estimate a builder fee on an already identified matched notional in pUSD
/// micros. This does not establish that the fill was confirmed, attributed,
/// charged, paid, or that the given role/rates were actually in effect.
pub fn estimate_builder_fee(
    notional_micros: u64,
    role: BuilderLiquidityRole,
    rates: BuilderFeeRates,
) -> PolyResult<BuilderFeeEstimate> {
    // Public fields permit construction without `new`.
    let rates = BuilderFeeRates::new(rates.maker_bps, rates.taker_bps)?;
    let rate_bps = rates.rate(role);
    let numerator = u128::from(notional_micros) * u128::from(rate_bps);
    let denominator = BPS_DENOMINATOR as u128;
    let lower_micros = (numerator / denominator) as u64;
    let upper_micros = numerator.div_ceil(denominator) as u64;
    Ok(BuilderFeeEstimate {
        notional_micros,
        rate_bps,
        numerator,
        lower_micros,
        upper_micros,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_differ_by_liquidity_role_not_trade_side() {
        let rates = BuilderFeeRates::new(50, 100).unwrap();
        let maker =
            estimate_builder_fee(1_000_000_000, BuilderLiquidityRole::Maker, rates).unwrap();
        let taker =
            estimate_builder_fee(1_000_000_000, BuilderLiquidityRole::Taker, rates).unwrap();
        assert_eq!(maker.lower_micros, 5_000_000);
        assert_eq!(taker.lower_micros, 10_000_000);
        assert_eq!(taker.compare(Some(10_000_000)), BuilderFeeComparison::Exact);
        assert_eq!(
            taker.compare(None),
            BuilderFeeComparison::MissingObservation
        );
        assert_eq!(
            taker.compare(Some(9_999_999)),
            BuilderFeeComparison::Mismatch
        );
    }

    #[test]
    fn exact_fraction_never_asserts_an_unknown_rounding_rule() {
        let rates = BuilderFeeRates::new(1, 1).unwrap();
        let estimate = estimate_builder_fee(5_001, BuilderLiquidityRole::Taker, rates).unwrap();
        assert_eq!((estimate.lower_micros, estimate.upper_micros), (0, 1));
        assert_eq!(
            estimate.compare(Some(0)),
            BuilderFeeComparison::WithinUnverifiedRoundingBounds
        );
        assert_eq!(
            estimate.compare(Some(1)),
            BuilderFeeComparison::WithinUnverifiedRoundingBounds
        );
        assert_eq!(estimate.compare(Some(2)), BuilderFeeComparison::Mismatch);
    }

    #[test]
    fn zero_maximum_and_invalid_rates_are_safe() {
        let zero = estimate_builder_fee(
            u64::MAX,
            BuilderLiquidityRole::Maker,
            BuilderFeeRates::new(0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(zero.lower_micros, 0);
        assert_eq!(zero.compare(Some(0)), BuilderFeeComparison::Exact);
        let max = estimate_builder_fee(
            u64::MAX,
            BuilderLiquidityRole::Taker,
            BuilderFeeRates::new(50, 100).unwrap(),
        )
        .unwrap();
        assert!(max.upper_micros <= u64::MAX / 100 + 1);
        assert!(BuilderFeeRates::new(51, 0).is_err());
        assert!(BuilderFeeRates::new(0, 101).is_err());
        assert!(estimate_builder_fee(
            1,
            BuilderLiquidityRole::Maker,
            BuilderFeeRates {
                maker_bps: 51,
                taker_bps: 0
            }
        )
        .is_err());
    }
}
