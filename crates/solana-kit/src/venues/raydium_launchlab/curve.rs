//! Exact port of the Raydium LaunchLab bonding-curve maths.
//!
//! # Provenance
//!
//! The LaunchLab Anchor program source is not published, so the maths is ported
//! from Raydium's **official SDK** (`raydium-io/raydium-sdk-V2`, commit
//! `f8549145afe75d4c486e30c43331c2c4da32b1eb`):
//!
//! | Rust here | SDK |
//! |---|---|
//! | [`ConstantProductCurve`] | `src/raydium/launchpad/curve/constantProductCurve.ts` |
//! | [`FixedPriceCurve`] | `src/raydium/launchpad/curve/fixedPriceCurve.ts` |
//! | [`LinearPriceCurve`] | `src/raydium/launchpad/curve/linearPriceCurve.ts` |
//! | [`total_fee_rate`] | `src/raydium/launchpad/curve/curve.ts` → `Curve.totalFeeRate` |
//! | [`calculate_fee`] | `Curve.calculateFee` |
//! | [`calculate_pre_fee`] | `Curve.calculatePreFee` |
//! | [`split_fee`] | `Curve.splitFee` |
//! | [`migration_liquidity`] | `src/raydium/launchpad/curve/func.ts` → `checkPoolToAmm` |
//!
//! # Exactness rules
//!
//! * All arithmetic is integer (`u128` intermediates). No floats.
//! * The rounding direction is the SDK's:
//!   - the total fee is **ceiled** (`ceilDiv`),
//!   - the pre-fee gross-up is **ceiled**,
//!   - `getAmountIn` (exact output) is **ceiled**,
//!   - `getAmountOut` (exact input) is **floored**,
//!   - the linear curve's square roots are **floored**,
//!   - the fee split is **floored** (the protocol keeps the remainder).
//! * Every intermediate is checked; an overflow is an error, never a wrap.
//! * The fee denominator is `1_000_000` (hundredths of a bip, 10^-6).

use serde::{Deserialize, Serialize};

use bot_core::error::{BotError, BotResult};

use crate::consts::{RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR, RAYDIUM_LAUNCHLAB_Q64};

/// The three LaunchLab bonding-curve types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurveType {
    /// `CurveType = 0` — constant product.
    ConstantProduct,
    /// `CurveType = 1` — fixed price.
    FixedPrice,
    /// `CurveType = 2` — linear price.
    LinearPrice,
}

impl CurveType {
    /// Decode the raw `u8` curve type. An unknown value is an error.
    pub fn from_u8(value: u8) -> BotResult<Self> {
        match value {
            crate::consts::RAYDIUM_LAUNCHLAB_CURVE_CONSTANT => Ok(CurveType::ConstantProduct),
            crate::consts::RAYDIUM_LAUNCHLAB_CURVE_FIXED => Ok(CurveType::FixedPrice),
            crate::consts::RAYDIUM_LAUNCHLAB_CURVE_LINEAR => Ok(CurveType::LinearPrice),
            other => Err(BotError::invalid(format!(
                "unknown LaunchLab curve type {other}"
            ))),
        }
    }

    /// Encode back to the raw `u8`.
    pub fn to_u8(self) -> u8 {
        match self {
            CurveType::ConstantProduct => crate::consts::RAYDIUM_LAUNCHLAB_CURVE_CONSTANT,
            CurveType::FixedPrice => crate::consts::RAYDIUM_LAUNCHLAB_CURVE_FIXED,
            CurveType::LinearPrice => crate::consts::RAYDIUM_LAUNCHLAB_CURVE_LINEAR,
        }
    }
}

/// The pool state the curve maths reads.
///
/// This is the subset of the `PoolState` account the curves use. The
/// SDK's `PoolBaseAmount` is `virtualA`, `virtualB`, `realA`, `realB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurveState {
    /// The virtual base parameter (`a`).
    pub virtual_base: u64,
    /// The virtual quote parameter (`b`).
    pub virtual_quote: u64,
    /// The real base amount in the pool.
    pub real_base: u64,
    /// The real quote amount in the pool.
    pub real_quote: u64,
    /// The total base amount to be sold.
    pub total_base_sell: u64,
    /// The total quote to be raised.
    pub total_quote_fund_raising: u64,
}

/// Ceiling division: `ceil(numerator / denominator)`.
///
/// Source: `src/common/transfer.ts` → `ceilDivBN`.
fn ceil_div(numerator: u128, denominator: u128) -> BotResult<u128> {
    if denominator == 0 {
        return Err(BotError::solana("LaunchLab division denominator is zero"));
    }
    if numerator == 0 {
        return Ok(0);
    }
    let quotient = numerator / denominator;
    if quotient == 0 {
        return Ok(1);
    }
    let remainder = numerator % denominator;
    if remainder > 0 {
        quotient
            .checked_add(1)
            .ok_or_else(|| BotError::solana("LaunchLab ceiling division overflow"))
    } else {
        Ok(quotient)
    }
}

/// The integer square root, floored.
///
/// Source: the SDK uses `new Decimal(x).sqrt().toFixed(0, Decimal.ROUND_FLOOR)`.
/// This is the Newton iteration on integers, which is exact for the whole
/// `u128` range.
pub fn isqrt(value: u128) -> u128 {
    if value == 0 {
        return 0;
    }
    // Start from a power of two above the value.
    let mut x: u128 = 1u128 << (128 - value.leading_zeros()).div_ceil(2);
    loop {
        // x_{n+1} = (x_n + value / x_n) / 2
        let next = (x + value / x) / 2;
        if next >= x {
            break;
        }
        x = next;
    }
    // The loop stops when x no longer decreases; x is then the floor root.
    while x > 0 && x.saturating_mul(x) > value {
        x -= 1;
    }
    x
}

/// The combined fee rate of the four fee components.
///
/// Source: `Curve.totalFeeRate`. The sum may not exceed the denominator.
pub fn total_fee_rate(
    protocol_fee_rate: u64,
    platform_fee_rate: u64,
    share_fee_rate: u64,
    creator_fee_rate: u64,
) -> BotResult<u64> {
    let total = protocol_fee_rate
        .checked_add(platform_fee_rate)
        .and_then(|v| v.checked_add(share_fee_rate))
        .and_then(|v| v.checked_add(creator_fee_rate))
        .ok_or_else(|| BotError::solana("LaunchLab fee rate overflow"))?;
    if total > RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR {
        return Err(BotError::invalid(
            "LaunchLab total fee rate exceeds the fee denominator",
        ));
    }
    Ok(total)
}

/// The fee charged on `amount` at `fee_rate`.
///
/// Ceiled: `ceil(amount * fee_rate / 1_000_000)`.
///
/// Source: `Curve.calculateFee`.
pub fn calculate_fee(amount: u128, fee_rate: u64) -> BotResult<u128> {
    let numerator = amount
        .checked_mul(u128::from(fee_rate))
        .ok_or_else(|| BotError::solana("LaunchLab fee multiplication overflow"))?;
    let denominator = u128::from(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR);
    let value = numerator
        .checked_add(denominator)
        .and_then(|v| v.checked_sub(1))
        .ok_or_else(|| BotError::solana("LaunchLab fee addition overflow"))?
        / denominator;
    Ok(value)
}

/// The gross-up: how much must be paid *before* the fee to net
/// `post_fee_amount`.
///
/// Ceiled: `ceil(post_fee_amount * 1_000_000 / (1_000_000 - fee_rate))`.
///
/// Source: `Curve.calculatePreFee`.
pub fn calculate_pre_fee(post_fee_amount: u128, fee_rate: u64) -> BotResult<u128> {
    if fee_rate == 0 {
        return Ok(post_fee_amount);
    }
    if fee_rate >= RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR {
        return Err(BotError::invalid(
            "LaunchLab fee rate must be below the fee denominator",
        ));
    }
    let numerator = post_fee_amount
        .checked_mul(u128::from(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR))
        .ok_or_else(|| BotError::solana("LaunchLab pre-fee multiplication overflow"))?;
    let denominator = u128::from(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR - fee_rate);
    let value = numerator
        .checked_add(denominator)
        .and_then(|v| v.checked_sub(1))
        .ok_or_else(|| BotError::solana("LaunchLab pre-fee addition overflow"))?
        / denominator;
    Ok(value)
}

/// The split of a total fee into its four components.
///
/// Each component is **floored**; the protocol fee takes the remainder, so
/// the parts always add up to the total.
///
/// Source: `Curve.splitFee`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeSplit {
    /// The platform's share.
    pub platform_fee: u128,
    /// The share fee (the token-gated fee).
    pub share_fee: u128,
    /// The protocol's share.
    pub protocol_fee: u128,
    /// The creator's share.
    pub creator_fee: u128,
}

impl FeeSplit {
    /// The total of the four components.
    pub fn total(&self) -> u128 {
        self.platform_fee + self.share_fee + self.protocol_fee + self.creator_fee
    }
}

/// Split a total fee into its components.
///
/// Source: `Curve.splitFee`.
pub fn split_fee(
    total_fee: u128,
    protocol_fee_rate: u64,
    platform_fee_rate: u64,
    share_fee_rate: u64,
    creator_fee_rate: u64,
) -> BotResult<FeeSplit> {
    let total_fee_rate = total_fee_rate(
        protocol_fee_rate,
        platform_fee_rate,
        share_fee_rate,
        creator_fee_rate,
    )?;
    if total_fee_rate == 0 {
        return Ok(FeeSplit {
            platform_fee: 0,
            share_fee: 0,
            protocol_fee: 0,
            creator_fee: 0,
        });
    }
    let rate = u128::from(total_fee_rate);
    let platform_fee = total_fee
        .checked_mul(u128::from(platform_fee_rate))
        .ok_or_else(|| BotError::solana("LaunchLab fee split overflow"))?
        / rate;
    let share_fee = total_fee
        .checked_mul(u128::from(share_fee_rate))
        .ok_or_else(|| BotError::solana("LaunchLab fee split overflow"))?
        / rate;
    let creator_fee = total_fee
        .checked_mul(u128::from(creator_fee_rate))
        .ok_or_else(|| BotError::solana("LaunchLab fee split overflow"))?
        / rate;
    let protocol_fee = total_fee
        .checked_sub(platform_fee)
        .and_then(|v| v.checked_sub(share_fee))
        .and_then(|v| v.checked_sub(creator_fee))
        .ok_or_else(|| BotError::solana("LaunchLab fee split underflow"))?;
    Ok(FeeSplit {
        platform_fee,
        share_fee,
        protocol_fee,
        creator_fee,
    })
}

// --------------------------------------------------------------------------
// Curve implementations
// --------------------------------------------------------------------------

/// The constant-product LaunchLab curve.
///
/// Source: `LaunchConstantProductCurve` in the SDK.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConstantProductCurve;

/// The fixed-price LaunchLab curve.
///
/// Source: `FixedPriceCurve` in the SDK.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedPriceCurve;

/// The linear-price LaunchLab curve.
///
/// Source: `LinearPriceCurve` in the SDK.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinearPriceCurve;

impl ConstantProductCurve {
    /// The base amount bought by an exact quote input.
    ///
    /// Floored: `floor(amount_in * out_reserve / (in_reserve + amount_in))`.
    ///
    /// Source: `LaunchConstantProductCurve.getAmountOut`.
    pub fn buy_exact_in(state: &CurveState, amount_in: u128) -> BotResult<u128> {
        let input_reserve = u128::from(state.virtual_quote)
            .checked_add(u128::from(state.real_quote))
            .ok_or_else(|| BotError::solana("LaunchLab input reserve overflow"))?;
        let output_reserve = u128::from(state.virtual_base)
            .checked_sub(u128::from(state.real_base))
            .ok_or_else(|| BotError::solana("LaunchLab output reserve underflow"))?;
        let numerator = amount_in
            .checked_mul(output_reserve)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        let denominator = input_reserve
            .checked_add(amount_in)
            .ok_or_else(|| BotError::solana("LaunchLab denominator overflow"))?;
        if denominator == 0 {
            return Err(BotError::solana("LaunchLab denominator is zero"));
        }
        Ok(numerator / denominator)
    }

    /// The quote needed for an exact base output.
    ///
    /// Ceiled: `ceil(in_reserve * amount_out / (out_reserve - amount_out))`.
    ///
    /// Source: `LaunchConstantProductCurve.getAmountIn`.
    pub fn buy_exact_out(state: &CurveState, amount_out: u128) -> BotResult<u128> {
        let input_reserve = u128::from(state.virtual_quote)
            .checked_add(u128::from(state.real_quote))
            .ok_or_else(|| BotError::solana("LaunchLab input reserve overflow"))?;
        let output_reserve = u128::from(state.virtual_base)
            .checked_sub(u128::from(state.real_base))
            .ok_or_else(|| BotError::solana("LaunchLab output reserve underflow"))?;
        let numerator = input_reserve
            .checked_mul(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        let denominator = output_reserve
            .checked_sub(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab output exceeds the reserve"))?;
        ceil_div(numerator, denominator)
    }

    /// The quote received for an exact base input (a sell).
    ///
    /// Floored.
    ///
    /// Source: `LaunchConstantProductCurve.sellExactIn`.
    pub fn sell_exact_in(state: &CurveState, amount_in: u128) -> BotResult<u128> {
        let input_reserve = u128::from(state.virtual_base)
            .checked_sub(u128::from(state.real_base))
            .ok_or_else(|| BotError::solana("LaunchLab input reserve underflow"))?;
        let output_reserve = u128::from(state.virtual_quote)
            .checked_add(u128::from(state.real_quote))
            .ok_or_else(|| BotError::solana("LaunchLab output reserve overflow"))?;
        let numerator = amount_in
            .checked_mul(output_reserve)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        let denominator = input_reserve
            .checked_add(amount_in)
            .ok_or_else(|| BotError::solana("LaunchLab denominator overflow"))?;
        if denominator == 0 {
            return Err(BotError::solana("LaunchLab denominator is zero"));
        }
        Ok(numerator / denominator)
    }

    /// The base that must be sold for an exact quote output.
    ///
    /// Ceiled.
    ///
    /// Source: `LaunchConstantProductCurve.sellExactOut`.
    pub fn sell_exact_out(state: &CurveState, amount_out: u128) -> BotResult<u128> {
        let input_reserve = u128::from(state.virtual_base)
            .checked_sub(u128::from(state.real_base))
            .ok_or_else(|| BotError::solana("LaunchLab input reserve underflow"))?;
        let output_reserve = u128::from(state.virtual_quote)
            .checked_add(u128::from(state.real_quote))
            .ok_or_else(|| BotError::solana("LaunchLab output reserve overflow"))?;
        let numerator = input_reserve
            .checked_mul(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        let denominator = output_reserve
            .checked_sub(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab output exceeds the reserve"))?;
        ceil_div(numerator, denominator)
    }
}

impl FixedPriceCurve {
    /// The base amount bought by an exact quote input.
    ///
    /// Floored: `floor(init_output * amount_in / init_input)`.
    ///
    /// Source: `FixedPriceCurve.getAmountOut`.
    pub fn buy_exact_in(state: &CurveState, amount_in: u128) -> BotResult<u128> {
        let numerator = u128::from(state.virtual_base)
            .checked_mul(amount_in)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        if state.virtual_quote == 0 {
            return Err(BotError::solana("LaunchLab virtual quote is zero"));
        }
        Ok(numerator / u128::from(state.virtual_quote))
    }

    /// The quote needed for an exact base output.
    ///
    /// Ceiled.
    ///
    /// Source: `FixedPriceCurve.getAmountIn`.
    pub fn buy_exact_out(state: &CurveState, amount_out: u128) -> BotResult<u128> {
        let numerator = u128::from(state.virtual_quote)
            .checked_mul(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        if state.virtual_base == 0 {
            return Err(BotError::solana("LaunchLab virtual base is zero"));
        }
        ceil_div(numerator, u128::from(state.virtual_base))
    }

    /// The quote received for an exact base input (a sell).
    ///
    /// Floored.
    ///
    /// Source: `FixedPriceCurve.sellExactIn`.
    pub fn sell_exact_in(state: &CurveState, amount_in: u128) -> BotResult<u128> {
        let numerator = u128::from(state.virtual_quote)
            .checked_mul(amount_in)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        if state.virtual_base == 0 {
            return Err(BotError::solana("LaunchLab virtual base is zero"));
        }
        Ok(numerator / u128::from(state.virtual_base))
    }

    /// The base that must be sold for an exact quote output.
    ///
    /// Ceiled.
    ///
    /// Source: `FixedPriceCurve.sellExactOut`.
    pub fn sell_exact_out(state: &CurveState, amount_out: u128) -> BotResult<u128> {
        let numerator = u128::from(state.virtual_base)
            .checked_mul(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        if state.virtual_quote == 0 {
            return Err(BotError::solana("LaunchLab virtual quote is zero"));
        }
        ceil_div(numerator, u128::from(state.virtual_quote))
    }
}

impl LinearPriceCurve {
    /// The base amount bought by an exact quote input.
    ///
    /// The square root is **floored** (the SDK's `ROUND_FLOOR`), so rounding
    /// it up would hand out base the paid quote does not buy.
    ///
    /// Source: `LinearPriceCurve.buyExactIn`.
    pub fn buy_exact_in(state: &CurveState, amount_in: u128) -> BotResult<u128> {
        let new_quote = u128::from(state.real_quote)
            .checked_add(amount_in)
            .ok_or_else(|| BotError::solana("LaunchLab quote overflow"))?;
        let term_inside_sqrt = 2u128
            .checked_mul(new_quote)
            .and_then(|v| v.checked_mul(RAYDIUM_LAUNCHLAB_Q64))
            .ok_or_else(|| BotError::solana("LaunchLab sqrt term overflow"))?
            / u128::from(state.virtual_base);
        let sqrt_term = isqrt(term_inside_sqrt);
        sqrt_term
            .checked_sub(u128::from(state.real_base))
            .ok_or_else(|| BotError::solana("LaunchLab linear buy underflow"))
    }

    /// The quote needed for an exact base output.
    ///
    /// Ceiled.
    ///
    /// Source: `LinearPriceCurve.buyExactOut`.
    pub fn buy_exact_out(state: &CurveState, amount_out: u128) -> BotResult<u128> {
        let new_base = u128::from(state.real_base)
            .checked_add(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab base overflow"))?;
        let new_base_squared = new_base
            .checked_mul(new_base)
            .ok_or_else(|| BotError::solana("LaunchLab base squared overflow"))?;
        let numerator = u128::from(state.virtual_base)
            .checked_mul(new_base_squared)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        let new_quote = ceil_div(numerator, 2u128 * RAYDIUM_LAUNCHLAB_Q64)?;
        new_quote
            .checked_sub(u128::from(state.real_quote))
            .ok_or_else(|| BotError::solana("LaunchLab linear buy underflow"))
    }

    /// The quote received for an exact base input (a sell).
    ///
    /// Ceiled (the SDK ceils here).
    ///
    /// Source: `LinearPriceCurve.sellExactIn`.
    pub fn sell_exact_in(state: &CurveState, amount_in: u128) -> BotResult<u128> {
        let new_base = u128::from(state.real_base)
            .checked_sub(amount_in)
            .ok_or_else(|| BotError::solana("LaunchLab base underflow"))?;
        let new_base_squared = new_base
            .checked_mul(new_base)
            .ok_or_else(|| BotError::solana("LaunchLab base squared overflow"))?;
        let numerator = u128::from(state.virtual_base)
            .checked_mul(new_base_squared)
            .ok_or_else(|| BotError::solana("LaunchLab numerator overflow"))?;
        let new_quote = ceil_div(numerator, 2u128 * RAYDIUM_LAUNCHLAB_Q64)?;
        u128::from(state.real_quote)
            .checked_sub(new_quote)
            .ok_or_else(|| BotError::solana("LaunchLab linear sell underflow"))
    }

    /// The base that must be sold for an exact quote output.
    ///
    /// The square root is **floored**, so the input is ceiled: the seller
    /// never receives less than the requested exact-out amount.
    ///
    /// Source: `LinearPriceCurve.sellExactOut`.
    pub fn sell_exact_out(state: &CurveState, amount_out: u128) -> BotResult<u128> {
        let new_b = u128::from(state.real_quote)
            .checked_sub(amount_out)
            .ok_or_else(|| BotError::solana("LaunchLab quote underflow"))?;
        let term_inside_sqrt = 2u128
            .checked_mul(new_b)
            .and_then(|v| v.checked_mul(RAYDIUM_LAUNCHLAB_Q64))
            .ok_or_else(|| BotError::solana("LaunchLab sqrt term overflow"))?
            / u128::from(state.virtual_base);
        let sqrt_term = isqrt(term_inside_sqrt);
        u128::from(state.real_base)
            .checked_sub(sqrt_term)
            .ok_or_else(|| BotError::solana("LaunchLab linear sell underflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> CurveState {
        CurveState {
            virtual_base: 1_073_471_847_374_405,
            virtual_quote: 30_050_573_465,
            real_base: 0,
            real_quote: 0,
            total_base_sell: 793_100_000_000_000,
            total_quote_fund_raising: 85_000_000_000,
        }
    }

    #[test]
    fn isqrt_is_the_floor_root() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(3), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(15), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(17), 4);
        assert_eq!(isqrt(10_000_000_000_000_000_000), 3_162_277_660);
        // The result must satisfy root^2 <= n < (root+1)^2.
        for n in [0u128, 1, 2, 3, 99, 100, 101, 1_000_000, 123_456_789_012_345] {
            let r = isqrt(n);
            assert!(r * r <= n, "isqrt({n}) = {r} squares above");
            assert!((r + 1) * (r + 1) > n, "isqrt({n}) = {r} squares below");
        }
    }

    #[test]
    fn curve_type_decoding_is_exhaustive_and_strict() {
        assert_eq!(CurveType::from_u8(0).unwrap(), CurveType::ConstantProduct);
        assert_eq!(CurveType::from_u8(1).unwrap(), CurveType::FixedPrice);
        assert_eq!(CurveType::from_u8(2).unwrap(), CurveType::LinearPrice);
        assert!(CurveType::from_u8(3).is_err());
        assert_eq!(CurveType::ConstantProduct.to_u8(), 0);
    }

    #[test]
    fn fee_helpers_round_the_way_the_sdk_does() {
        // calculateFee ceils.
        assert_eq!(calculate_fee(1, 1).unwrap(), 1);
        assert_eq!(calculate_fee(1_000_000, 2_500).unwrap(), 2_500);
        assert_eq!(calculate_fee(1_000_001, 2_500).unwrap(), 2_501);
        // calculatePreFee ceils and refuses a 100% rate.
        assert_eq!(calculate_pre_fee(750, 250_000).unwrap(), 1_000);
        assert_eq!(calculate_pre_fee(123, 0).unwrap(), 123);
        assert!(calculate_pre_fee(123, 1_000_000).is_err());
    }

    #[test]
    fn fee_split_adds_up_and_gives_the_remainder_to_the_protocol() {
        let split = split_fee(1_000, 500, 300, 100, 100).unwrap();
        assert_eq!(split.platform_fee, 300);
        assert_eq!(split.share_fee, 100);
        assert_eq!(split.creator_fee, 100);
        assert_eq!(split.protocol_fee, 500);
        assert_eq!(split.total(), 1_000);
        // A zero total rate means no fee at all.
        let split = split_fee(1_000, 0, 0, 0, 0).unwrap();
        assert_eq!(split.total(), 0);
    }

    #[test]
    fn total_fee_rate_refuses_more_than_the_denominator() {
        assert_eq!(total_fee_rate(1, 2, 3, 4).unwrap(), 10);
        // The SDK throws only when the sum is *greater* than the denominator,
        // so a total of exactly 1_000_000 is accepted.
        assert_eq!(
            total_fee_rate(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR, 0, 0, 0).unwrap(),
            RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR
        );
        assert!(total_fee_rate(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR, 1, 0, 0).is_err());
    }

    #[test]
    fn constant_product_buy_exact_in_floors() {
        let s = state();
        let out = ConstantProductCurve::buy_exact_in(&s, 1_000_000).unwrap();
        // floor(1_000_000 * 1_073_471_847_374_405 /
        //       (30_050_573_465 + 1_000_000))
        assert_eq!(out, 35_720_986_411);
    }

    #[test]
    fn constant_product_buy_exact_out_ceils() {
        let s = state();
        let amount_out = 1_000_000u128;
        let cost = ConstantProductCurve::buy_exact_out(&s, amount_out).unwrap();
        // Feeding the cost back must deliver at least the requested output.
        let back = ConstantProductCurve::buy_exact_in(&s, cost).unwrap();
        assert!(back >= amount_out, "got {back}, wanted {amount_out}");
    }

    #[test]
    fn constant_product_sell_is_the_mirror_of_buy() {
        let mut s = state();
        s.real_base = 100_000_000_000;
        s.real_quote = 1_000_000_000;
        let out = ConstantProductCurve::sell_exact_in(&s, 1_000_000).unwrap();
        assert!(out > 0);
        // The exact-out variant ceils, so one unit less must undershoot.
        let needed = ConstantProductCurve::sell_exact_out(&s, out).unwrap();
        let back = ConstantProductCurve::sell_exact_in(&s, needed).unwrap();
        assert!(back >= out);
    }

    #[test]
    fn fixed_price_buy_is_linear() {
        let s = CurveState {
            virtual_base: 1_000_000,
            virtual_quote: 2_000,
            real_base: 0,
            real_quote: 0,
            total_base_sell: 1_000_000,
            total_quote_fund_raising: 2_000,
        };
        // Price: 2_000 quote buys 1_000_000 base, so 1 quote buys 500 base.
        assert_eq!(FixedPriceCurve::buy_exact_in(&s, 1_000).unwrap(), 500_000);
        // Exact out ceils.
        assert_eq!(FixedPriceCurve::buy_exact_out(&s, 500_000).unwrap(), 1_000);
        assert_eq!(FixedPriceCurve::buy_exact_out(&s, 1).unwrap(), 1);
        // The sell is the mirror.
        assert_eq!(FixedPriceCurve::sell_exact_in(&s, 500_000).unwrap(), 1_000);
        assert_eq!(FixedPriceCurve::sell_exact_out(&s, 1_000).unwrap(), 500_000);
    }

    #[test]
    fn fixed_price_price_is_constant() {
        let s = CurveState {
            virtual_base: 1_000_000,
            virtual_quote: 2_000,
            real_base: 0,
            real_quote: 0,
            total_base_sell: 1_000_000,
            total_quote_fund_raising: 2_000,
        };
        let small = FixedPriceCurve::buy_exact_in(&s, 100).unwrap();
        let large = FixedPriceCurve::buy_exact_in(&s, 100_000).unwrap();
        // Doubling the quote doubles the base: the price never changes.
        assert_eq!(small * 1_000, large);
    }

    #[test]
    fn linear_price_is_monotonic_in_the_quote() {
        let s = CurveState {
            // a = 2 * Q64 * fundRaising / totalSell^2 (see the SDK's
            // `getInitParam`), chosen so the curve is sane.
            virtual_base: 1_073_471_847_374_405,
            virtual_quote: 0,
            real_base: 0,
            real_quote: 0,
            total_base_sell: 793_100_000_000_000,
            total_quote_fund_raising: 85_000_000_000,
        };
        let mut last = 0u128;
        for amount in [1u128, 1_000, 1_000_000, 100_000_000] {
            let out = LinearPriceCurve::buy_exact_in(&s, amount).unwrap();
            assert!(out >= last, "the linear curve must not go backwards");
            last = out;
        }
    }

    #[test]
    fn linear_price_sell_is_the_mirror_of_buy() {
        // The linear curve's `virtual_quote` is the `b` parameter, which the
        // SDK's `getInitParam` sets to zero, and the quote accumulates in
        // `real_quote`.
        let mut s = state();
        s.virtual_quote = 0;
        let bought = LinearPriceCurve::buy_exact_in(&s, 10_000_000).unwrap();
        assert!(bought > 0);
        // After the buy the pool holds the base and the quote that was paid.
        let mut after_buy = s;
        after_buy.real_base = u64::try_from(bought).expect("the bought amount fits a u64");
        after_buy.real_quote = 10_000_000;
        // Selling the same base back must not return more quote than was put
        // in (the curve has a spread).
        let back = LinearPriceCurve::sell_exact_in(&after_buy, bought).unwrap();
        assert!(
            back <= 10_000_000,
            "round trip returned {back} > 10_000_000"
        );
    }

    #[test]
    fn overflow_edges_are_errors() {
        let s = state();
        // A denominator of zero is refused: with a zero input reserve *and*
        // a zero input amount the curve has no denominator at all.
        let zero = CurveState {
            virtual_quote: 0,
            virtual_base: 0,
            ..state()
        };
        assert!(ConstantProductCurve::buy_exact_in(&zero, 0).is_err());
        // The fixed-price curve divides by `virtual_base` (the init input),
        // so a zero there is refused too.
        assert!(FixedPriceCurve::buy_exact_in(&zero, 1).is_err());
        // The output exceeding the reserve is refused.
        assert!(ConstantProductCurve::buy_exact_out(&s, u128::from(s.virtual_base) + 1).is_err());
        // A fee rate at the denominator is refused.
        assert!(calculate_fee(u128::MAX, u64::MAX).is_err());
    }
}
