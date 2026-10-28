//! Exact port of the Raydium CPMM (`cp-swap`) swap maths.
//!
//! # Provenance
//!
//! Every function here is a line-by-line port of the official program:
//! `raydium-io/raydium-cp-swap`, commit
//! `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`:
//!
//! | Rust here | Program |
//! |---|---|
//! | [`trading_fee`] | `curve::fees::Fees::trading_fee` |
//! | [`protocol_fee`] | `curve::fees::Fees::protocol_fee` |
//! | [`fund_fee`] | `curve::fees::Fees::fund_fee` |
//! | [`creator_fee`] | `curve::fees::Fees::creator_fee` |
//! | [`split_creator_fee`] | `curve::fees::Fees::split_creator_fee` |
//! | [`calculate_pre_fee_amount`] | `curve::fees::Fees::calculate_pre_fee_amount` |
//! | [`swap_base_input_without_fees`] | `curve::constant_product::ConstantProductCurve::swap_base_input_without_fees` |
//! | [`swap_base_output_without_fees`] | `curve::constant_product::ConstantProductCurve::swap_base_output_without_fees` |
//! | [`swap_base_input`] | `curve::calculator::CurveCalculator::swap_base_input` |
//! | [`swap_base_output`] | `curve::calculator::CurveCalculator::swap_base_output` |
//!
//! # Exactness rules
//!
//! * All arithmetic is integer (`u128` intermediates, `u64` inputs/outputs).
//! * The rounding direction is the program's, not the convenient one:
//!   the trading fee and the creator fee are **ceiled**; the protocol fee and
//!   the fund fee are **floored**; `swap_base_output_without_fees` **ceils**;
//!   `swap_base_input_without_fees` **floors**.
//! * Every intermediate is a checked operation. An overflow is an error, never
//!   a wrapped value.
//! * The fee denominator is `1_000_000` ("hundredths of a bip", 10^-6) — NOT
//!   the 10 000 basis-point denominator other venues in this crate use.

use serde::{Deserialize, Serialize};

use bot_core::error::{BotError, BotResult};

use crate::consts::RAYDIUM_CPMM_FEE_RATE_DENOMINATOR;
use crate::venues::raydium_cpmm::accounts::CpmmSide;

/// Ceiling division: `(amount * numerator + denominator - 1) / denominator`.
///
/// Source: `curve::fees::ceil_div` in the CPMM program.
fn ceil_div(amount: u128, numerator: u128, denominator: u128) -> BotResult<u128> {
    if denominator == 0 {
        return Err(BotError::solana("CPMM fee denominator is zero"));
    }
    amount
        .checked_mul(numerator)
        .and_then(|v| v.checked_add(denominator))
        .and_then(|v| v.checked_sub(1))
        .map(|v| v / denominator)
        .ok_or_else(|| BotError::solana("CPMM ceiling division overflow"))
}

/// Floor division: `(amount * numerator) / denominator`.
///
/// Source: `curve::fees::floor_div` in the CPMM program.
fn floor_div(amount: u128, numerator: u128, denominator: u128) -> BotResult<u128> {
    if denominator == 0 {
        return Err(BotError::solana("CPMM fee denominator is zero"));
    }
    amount
        .checked_mul(numerator)
        .map(|v| v / denominator)
        .ok_or_else(|| BotError::solana("CPMM floor division overflow"))
}

/// Ceiling division without a numerator: `ceil(amount / denominator)`.
///
/// Source: `utils::math::CheckedCeilDiv::checked_ceil_div` in the CPMM program.
fn checked_ceil_div(amount: u128, denominator: u128) -> BotResult<u128> {
    if denominator == 0 {
        return Err(BotError::solana("CPMM division denominator is zero"));
    }
    let quotient = amount / denominator;
    let remainder = amount % denominator;
    if remainder != 0 {
        quotient
            .checked_add(1)
            .ok_or_else(|| BotError::solana("CPMM ceiling division overflow"))
    } else {
        Ok(quotient)
    }
}

/// The CPMM trading fee on `amount`.
///
/// Ceiled: the pool never charges less than the rate implies.
///
/// Source: `curve::fees::Fees::trading_fee`.
pub fn trading_fee(amount: u128, trade_fee_rate: u64) -> BotResult<u128> {
    ceil_div(
        amount,
        u128::from(trade_fee_rate),
        u128::from(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR),
    )
}

/// The CPMM protocol fee, taken from the trading fee.
///
/// Floored.
///
/// Source: `curve::fees::Fees::protocol_fee`.
pub fn protocol_fee(amount: u128, protocol_fee_rate: u64) -> BotResult<u128> {
    floor_div(
        amount,
        u128::from(protocol_fee_rate),
        u128::from(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR),
    )
}

/// The CPMM fund fee, taken from the trading fee.
///
/// Floored.
///
/// Source: `curve::fees::Fees::fund_fee`.
pub fn fund_fee(amount: u128, fund_fee_rate: u64) -> BotResult<u128> {
    floor_div(
        amount,
        u128::from(fund_fee_rate),
        u128::from(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR),
    )
}

/// The CPMM creator fee on `amount`.
///
/// Ceiled.
///
/// Source: `curve::fees::Fees::creator_fee`.
pub fn creator_fee(amount: u128, creator_fee_rate: u64) -> BotResult<u128> {
    ceil_div(
        amount,
        u128::from(creator_fee_rate),
        u128::from(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR),
    )
}

/// Split a combined fee into the creator's part and the protocol's part.
///
/// Floored: the protocol keeps the remainder.
///
/// Source: `curve::fees::Fees::split_creator_fee`.
pub fn split_creator_fee(
    total_fee: u128,
    trade_fee_rate: u64,
    creator_fee_rate: u64,
) -> BotResult<u128> {
    let denominator = trade_fee_rate
        .checked_add(creator_fee_rate)
        .ok_or_else(|| BotError::solana("CPMM fee rate overflow"))?;
    floor_div(
        total_fee,
        u128::from(creator_fee_rate),
        u128::from(denominator),
    )
}

/// Gross up a post-fee amount: how much must be paid *before* the fee is
/// deducted to receive `post_fee_amount` net of a `fee_rate` fee.
///
/// Ceiled.
///
/// Source: `curve::fees::Fees::calculate_pre_fee_amount`.
pub fn calculate_pre_fee_amount(post_fee_amount: u128, fee_rate: u64) -> BotResult<u128> {
    if fee_rate == 0 {
        return Ok(post_fee_amount);
    }
    let numerator = post_fee_amount
        .checked_mul(u128::from(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR))
        .ok_or_else(|| BotError::solana("CPMM pre-fee amount overflow"))?;
    let denominator = u128::from(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR)
        .checked_sub(u128::from(fee_rate))
        .ok_or_else(|| BotError::solana("CPMM pre-fee amount overflow"))?;
    if denominator == 0 {
        return Err(BotError::solana(
            "CPMM fee rate equals the denominator — the pre-fee amount is undefined",
        ));
    }
    numerator
        .checked_add(denominator)
        .and_then(|v| v.checked_sub(1))
        .map(|v| v / denominator)
        .ok_or_else(|| BotError::solana("CPMM pre-fee amount overflow"))
}

/// The constant-product output for an exact input, before any fee.
///
/// Floored: `floor(amount_in * out_reserve / (in_reserve + amount_in))`.
///
/// Source: `curve::constant_product::ConstantProductCurve::swap_base_input_without_fees`.
pub fn swap_base_input_without_fees(
    input_amount: u128,
    input_vault_amount: u128,
    output_vault_amount: u128,
) -> BotResult<u128> {
    let numerator = input_amount
        .checked_mul(output_vault_amount)
        .ok_or_else(|| BotError::solana("CPMM constant-product numerator overflow"))?;
    let denominator = input_vault_amount
        .checked_add(input_amount)
        .ok_or_else(|| BotError::solana("CPMM constant-product denominator overflow"))?;
    if denominator == 0 {
        return Err(BotError::solana(
            "CPMM constant-product denominator is zero",
        ));
    }
    Ok(numerator / denominator)
}

/// The constant-product input for an exact output, before any fee.
///
/// Ceiled: `ceil(in_reserve * amount_out / (out_reserve - amount_out))`.
///
/// Source: `curve::constant_product::ConstantProductCurve::swap_base_output_without_fees`.
pub fn swap_base_output_without_fees(
    output_amount: u128,
    input_vault_amount: u128,
    output_vault_amount: u128,
) -> BotResult<u128> {
    let numerator = input_vault_amount
        .checked_mul(output_amount)
        .ok_or_else(|| BotError::solana("CPMM constant-product numerator overflow"))?;
    let denominator = output_vault_amount
        .checked_sub(output_amount)
        .ok_or_else(|| BotError::solana("CPMM output amount exceeds the reserve"))?;
    checked_ceil_div(numerator, denominator)
}

/// The full result of a CPMM swap, mirroring the program's `SwapResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapResult {
    /// The input vault balance after the swap, excluding trade fees.
    pub new_input_vault_amount: u128,
    /// The output vault balance after the swap, excluding trade fees.
    pub new_output_vault_amount: u128,
    /// The user's input amount, including trade fees, excluding transfer fees.
    pub input_amount: u128,
    /// The amount transferred to the user, including transfer fees.
    pub output_amount: u128,
    /// Trading fee (goes to the liquidity providers).
    pub trade_fee: u128,
    /// Protocol fee.
    pub protocol_fee: u128,
    /// Fund fee.
    pub fund_fee: u128,
    /// Creator fee.
    pub creator_fee: u128,
}

/// The fee configuration a quote needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeRates {
    /// Trade fee rate, in hundredths of a bip.
    pub trade_fee_rate: u64,
    /// Creator fee rate, in hundredths of a bip.
    pub creator_fee_rate: u64,
    /// Protocol fee rate, in hundredths of a bip.
    pub protocol_fee_rate: u64,
    /// Fund fee rate, in hundredths of a bip.
    pub fund_fee_rate: u64,
    /// Whether the creator fee is charged on the input token (rather than on
    /// the output token).
    pub creator_fee_on_input: bool,
}

/// The exact-input CPMM quote.
///
/// Port of `curve::calculator::CurveCalculator::swap_base_input`.
///
/// * `input_amount` is the amount the **user** pays (the gross amount, before
///   the trading fee is taken).
/// * `input_vault_amount` / `output_vault_amount` are the tradable reserves
///   (vault balances minus accrued fees).
pub fn swap_base_input(
    input_amount: u128,
    input_vault_amount: u128,
    output_vault_amount: u128,
    fees: FeeRates,
) -> BotResult<SwapResult> {
    let mut creator_fee_amount = 0u128;
    let trade_fee: u128;

    let input_amount_less_fees = if fees.creator_fee_on_input {
        let total_fee = trading_fee(
            input_amount,
            fees.trade_fee_rate
                .checked_add(fees.creator_fee_rate)
                .ok_or_else(|| BotError::solana("CPMM fee rate overflow"))?,
        )?;
        creator_fee_amount =
            split_creator_fee(total_fee, fees.trade_fee_rate, fees.creator_fee_rate)?;
        trade_fee = total_fee
            .checked_sub(creator_fee_amount)
            .ok_or_else(|| BotError::solana("CPMM trade fee underflow"))?;
        input_amount
            .checked_sub(total_fee)
            .ok_or_else(|| BotError::solana("CPMM input amount is below the total fee"))?
    } else {
        trade_fee = trading_fee(input_amount, fees.trade_fee_rate)?;
        input_amount
            .checked_sub(trade_fee)
            .ok_or_else(|| BotError::solana("CPMM input amount is below the trade fee"))?
    };

    let protocol_fee = protocol_fee(trade_fee, fees.protocol_fee_rate)?;
    let fund_fee = fund_fee(trade_fee, fees.fund_fee_rate)?;

    let output_amount_swapped = swap_base_input_without_fees(
        input_amount_less_fees,
        input_vault_amount,
        output_vault_amount,
    )?;

    let output_amount = if fees.creator_fee_on_input {
        output_amount_swapped
    } else {
        creator_fee_amount = creator_fee(output_amount_swapped, fees.creator_fee_rate)?;
        output_amount_swapped
            .checked_sub(creator_fee_amount)
            .ok_or_else(|| BotError::solana("CPMM creator fee exceeds the output amount"))?
    };

    let new_input_vault_amount = input_vault_amount
        .checked_add(input_amount_less_fees)
        .ok_or_else(|| BotError::solana("CPMM input vault amount overflow"))?;
    let new_output_vault_amount = output_vault_amount
        .checked_sub(output_amount_swapped)
        .ok_or_else(|| BotError::solana("CPMM output vault amount underflow"))?;

    Ok(SwapResult {
        new_input_vault_amount,
        new_output_vault_amount,
        input_amount,
        output_amount,
        trade_fee,
        protocol_fee,
        fund_fee,
        creator_fee: creator_fee_amount,
    })
}

/// The exact-output CPMM quote.
///
/// Port of `curve::calculator::CurveCalculator::swap_base_output`.
///
/// * `output_amount` is the amount the user must **receive** (net of the
///   creator fee when the creator fee is charged on the output).
pub fn swap_base_output(
    output_amount: u128,
    input_vault_amount: u128,
    output_vault_amount: u128,
    fees: FeeRates,
) -> BotResult<SwapResult> {
    let mut creator_fee_amount = 0u128;
    let trade_fee: u128;

    let actual_output_amount = if fees.creator_fee_on_input {
        output_amount
    } else {
        let out_amount_with_creator_fee =
            calculate_pre_fee_amount(output_amount, fees.creator_fee_rate)?;
        creator_fee_amount = out_amount_with_creator_fee
            .checked_sub(output_amount)
            .ok_or_else(|| BotError::solana("CPMM creator fee underflow"))?;
        out_amount_with_creator_fee
    };

    let input_amount_swapped = swap_base_output_without_fees(
        actual_output_amount,
        input_vault_amount,
        output_vault_amount,
    )?;

    let input_amount = if fees.creator_fee_on_input {
        let input_amount_with_fee = calculate_pre_fee_amount(
            input_amount_swapped,
            fees.trade_fee_rate
                .checked_add(fees.creator_fee_rate)
                .ok_or_else(|| BotError::solana("CPMM fee rate overflow"))?,
        )?;
        let total_fee = input_amount_with_fee
            .checked_sub(input_amount_swapped)
            .ok_or_else(|| BotError::solana("CPMM total fee underflow"))?;
        creator_fee_amount =
            split_creator_fee(total_fee, fees.trade_fee_rate, fees.creator_fee_rate)?;
        trade_fee = total_fee
            .checked_sub(creator_fee_amount)
            .ok_or_else(|| BotError::solana("CPMM trade fee underflow"))?;
        input_amount_with_fee
    } else {
        let input_amount_with_fee =
            calculate_pre_fee_amount(input_amount_swapped, fees.trade_fee_rate)?;
        trade_fee = input_amount_with_fee
            .checked_sub(input_amount_swapped)
            .ok_or_else(|| BotError::solana("CPMM trade fee underflow"))?;
        input_amount_with_fee
    };

    let protocol_fee = protocol_fee(trade_fee, fees.protocol_fee_rate)?;
    let fund_fee = fund_fee(trade_fee, fees.fund_fee_rate)?;

    let new_input_vault_amount = input_vault_amount
        .checked_add(input_amount_swapped)
        .ok_or_else(|| BotError::solana("CPMM input vault amount overflow"))?;
    let new_output_vault_amount = output_vault_amount
        .checked_sub(actual_output_amount)
        .ok_or_else(|| BotError::solana("CPMM output vault amount underflow"))?;

    Ok(SwapResult {
        new_input_vault_amount,
        new_output_vault_amount,
        input_amount,
        output_amount,
        trade_fee,
        protocol_fee,
        fund_fee,
        creator_fee: creator_fee_amount,
    })
}

/// The minimum output a swap must deliver, given an expected output and a
/// slippage tolerance in basis points.
///
/// The bound is floored, so the guard is never looser than requested.
pub fn min_out_with_slippage_bps(expected_out: u64, slippage_bps: u64) -> BotResult<u64> {
    if slippage_bps >= 10_000 {
        return Err(BotError::invalid(
            "CPMM slippage must be below 10000 basis points",
        ));
    }
    let numerator = u128::from(expected_out)
        .checked_mul(u128::from(10_000u64.saturating_sub(slippage_bps)))
        .ok_or_else(|| BotError::solana("CPMM slippage multiplication overflow"))?;
    Ok((numerator / 10_000) as u64)
}

/// The maximum input a swap may spend, given an expected input and a slippage
/// tolerance in basis points.
///
/// The bound is ceiled, so the guard is never looser than requested.
pub fn max_in_with_slippage_bps(expected_in: u64, slippage_bps: u64) -> BotResult<u64> {
    if slippage_bps >= 10_000 {
        return Err(BotError::invalid(
            "CPMM slippage must be below 10000 basis points",
        ));
    }
    let numerator = u128::from(expected_in)
        .checked_mul(u128::from(10_000u64.saturating_add(slippage_bps)))
        .ok_or_else(|| BotError::solana("CPMM slippage multiplication overflow"))?;
    let denominator = 10_000u128;
    let value = numerator
        .checked_add(denominator - 1)
        .ok_or_else(|| BotError::solana("CPMM slippage addition overflow"))?
        / denominator;
    u64::try_from(value).map_err(|_| BotError::solana("CPMM maximum input exceeds the u64 range"))
}

/// Which side of the pool the user is spending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuoteSide {
    /// The user spends token_0 and receives token_1.
    Token0ToToken1,
    /// The user spends token_1 and receives token_0.
    Token1ToToken0,
}

impl From<CpmmSide> for QuoteSide {
    fn from(side: CpmmSide) -> Self {
        match side {
            CpmmSide::Token0 => QuoteSide::Token0ToToken1,
            CpmmSide::Token1 => QuoteSide::Token1ToToken0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_FEES: FeeRates = FeeRates {
        trade_fee_rate: 0,
        creator_fee_rate: 0,
        protocol_fee_rate: 0,
        fund_fee_rate: 0,
        creator_fee_on_input: false,
    };

    #[test]
    fn trading_fee_is_ceiled() {
        // 1 unit at 1/1_000_000 must still cost 1 unit (ceil), not 0.
        assert_eq!(trading_fee(1, 1).unwrap(), 1);
        // 1_000_000 at 2_500/1_000_000 = 2_500 exactly.
        assert_eq!(trading_fee(1_000_000, 2_500).unwrap(), 2_500);
        // 1_000_001 at 2_500/1_000_000 = 2_500.0025 -> 2_501 (ceil).
        assert_eq!(trading_fee(1_000_001, 2_500).unwrap(), 2_501);
        // Zero fee is zero.
        assert_eq!(trading_fee(u128::MAX, 0).unwrap(), 0);
    }

    #[test]
    fn protocol_and_fund_fees_are_floored() {
        assert_eq!(protocol_fee(1, 1).unwrap(), 0);
        assert_eq!(protocol_fee(1_000_000, 500).unwrap(), 500);
        assert_eq!(fund_fee(1_000_000, 500).unwrap(), 500);
    }

    #[test]
    fn creator_fee_is_ceiled_and_split_is_floored() {
        assert_eq!(creator_fee(1, 1).unwrap(), 1);
        // The official source: floor(total_fee * creator_rate / (trade_rate
        // + creator_rate)). 1_000_000 * 1_000 / 3_500 = 285_714.285… -> 285_714.
        assert_eq!(split_creator_fee(1_000_000, 2_500, 1_000).unwrap(), 285_714);
        // Zero creator rate -> zero share.
        assert_eq!(split_creator_fee(1_000_000, 2_500, 0).unwrap(), 0);
    }

    #[test]
    fn pre_fee_amount_grosses_up_and_ceils() {
        // A 25% fee: to net 750 you must pay 1000.
        assert_eq!(calculate_pre_fee_amount(750, 250_000).unwrap(), 1_000);
        // Zero rate is a pass-through.
        assert_eq!(calculate_pre_fee_amount(123, 0).unwrap(), 123);
        // A 100% rate is undefined and must be refused.
        assert!(calculate_pre_fee_amount(123, 1_000_000).is_err());
    }

    /// These vectors are the `constant_product_swap_rounding` table from
    /// `programs/cp-swap/src/curve/constant_product.rs` in the official
    /// program (commit f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5).
    #[test]
    fn constant_product_rounding_matches_the_program_vectors() {
        let vectors: &[(u128, u128, u128, u128)] = &[
            (10, 4_000_000, 70_000_000_000, 174_999),
            (20, 30_000 - 20, 10_000, 6),
            (19, 30_000 - 20, 10_000, 6),
            (18, 30_000 - 20, 10_000, 6),
            (10, 20_000, 30_000, 14),
            (10, 20_000 - 9, 30_000, 14),
            (10, 20_000 - 10, 30_000, 15),
            (100, 60_000, 30_000, 49),
            (99, 60_000, 30_000, 49),
            (98, 60_000, 30_000, 48),
        ];
        for (amount_in, reserve_in, reserve_out, expected_out) in vectors {
            let out = swap_base_input_without_fees(*amount_in, *reserve_in, *reserve_out).unwrap();
            assert_eq!(out, *expected_out, "amount_in={amount_in}");
        }
    }

    #[test]
    fn constant_product_never_decreases_the_invariant() {
        // Same property the program's `test_truncation` helper asserts.
        let cases: &[(u128, u128, u128)] = &[
            (10, 4_000_000, 70_000_000_000),
            (18, 29_980, 10_000),
            (100, 60_000, 30_000),
            (1, 1, 1),
            (u64::MAX as u128, u64::MAX as u128, u64::MAX as u128),
        ];
        for (amount_in, reserve_in, reserve_out) in cases {
            let out = swap_base_input_without_fees(*amount_in, *reserve_in, *reserve_out).unwrap();
            let invariant_before = reserve_in * reserve_out;
            let invariant_after = (reserve_in + amount_in) * (reserve_out - out);
            assert!(
                invariant_after >= invariant_before,
                "invariant decreased: {invariant_after} < {invariant_before}"
            );
        }
    }

    #[test]
    fn exact_output_ceils_and_matches_the_inverse_of_exact_input() {
        // For a zero-fee pool, the exact-output input must be enough to buy
        // the requested output, and no less.
        let reserve_in: u128 = 1_000_000;
        let reserve_out: u128 = 2_000_000;
        for amount_out in [1u128, 10, 1_000, 100_000, 999_999] {
            let amount_in =
                swap_base_output_without_fees(amount_out, reserve_in, reserve_out).unwrap();
            let got = swap_base_input_without_fees(amount_in, reserve_in, reserve_out).unwrap();
            assert!(
                got >= amount_out,
                "ceil rounding must not undershoot: got {got}, wanted {amount_out}"
            );
            if amount_in > 0 {
                let less =
                    swap_base_input_without_fees(amount_in - 1, reserve_in, reserve_out).unwrap();
                assert!(
                    less < amount_out,
                    "one unit less must undershoot: got {less}, wanted {amount_out}"
                );
            }
        }
    }

    #[test]
    fn exact_output_refuses_more_than_the_reserve() {
        assert!(swap_base_output_without_fees(2_000_000, 1_000_000, 2_000_000).is_err());
    }

    #[test]
    fn swap_base_input_with_no_fees_is_the_plain_constant_product() {
        let r = swap_base_input(1_000, 1_000_000, 2_000_000, NO_FEES).unwrap();
        // floor(1000 * 2_000_000 / 1_001_000) = 1998
        assert_eq!(r.output_amount, 1_998);
        assert_eq!(r.trade_fee, 0);
        assert_eq!(r.protocol_fee, 0);
        assert_eq!(r.fund_fee, 0);
        assert_eq!(r.creator_fee, 0);
        assert_eq!(r.new_input_vault_amount, 1_001_000);
        assert_eq!(r.new_output_vault_amount, 2_000_000 - 1_998);
    }

    #[test]
    fn swap_base_input_charges_the_trade_fee_on_the_input() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            ..NO_FEES
        };
        let r = swap_base_input(1_000_000, 1_000_000, 2_000_000, fees).unwrap();
        // Trading fee: ceil(1_000_000 * 2_500 / 1_000_000) = 2_500.
        assert_eq!(r.trade_fee, 2_500);
        // The curve sees 1_000_000 - 2_500 = 997_500.
        // floor(997_500 * 2_000_000 / (1_000_000 + 997_500)) = 998_748.
        assert_eq!(r.output_amount, 998_748);
        assert_eq!(r.new_input_vault_amount, 1_997_500);
    }

    #[test]
    fn swap_base_input_with_the_creator_fee_on_the_output() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            creator_fee_rate: 1_000,
            ..NO_FEES
        };
        let r = swap_base_input(1_000_000, 1_000_000, 2_000_000, fees).unwrap();
        // Trade fee 2_500, curve input 997_500.
        // Curve output: floor(997_500 * 2_000_000 / 1_997_500) = 998_748.
        // Creator fee on the output: ceil(998_748 * 1_000 / 1_000_000) = 999.
        assert_eq!(r.creator_fee, 999);
        assert_eq!(r.output_amount, 998_748 - 999);
        assert_eq!(r.trade_fee, 2_500);
    }

    #[test]
    fn swap_base_input_with_the_creator_fee_on_the_input() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            creator_fee_rate: 1_000,
            creator_fee_on_input: true,
            ..NO_FEES
        };
        let r = swap_base_input(1_000_000, 1_000_000, 2_000_000, fees).unwrap();
        // Total fee: ceil(1_000_000 * 3_500 / 1_000_000) = 3_500.
        // Creator share: floor(3_500 * 1_000 / 3_500) = 1_000.
        // Trade fee: 3_500 - 1_000 = 2_500.
        assert_eq!(r.creator_fee, 1_000);
        assert_eq!(r.trade_fee, 2_500);
        // The curve sees 1_000_000 - 3_500 = 996_500.
        // floor(996_500 * 2_000_000 / 1_996_500) = 998_246.
        assert_eq!(r.output_amount, 998_246);
    }

    #[test]
    fn swap_base_output_is_the_inverse_of_swap_base_input() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            ..NO_FEES
        };
        let reserve_in: u128 = 1_000_000;
        let reserve_out: u128 = 2_000_000;
        let amount_out: u128 = 500_000;
        let r = swap_base_output(amount_out, reserve_in, reserve_out, fees).unwrap();
        // The user pays `input_amount`; feeding that back must deliver at
        // least the requested output.
        let back = swap_base_input(r.input_amount, reserve_in, reserve_out, fees).unwrap();
        assert!(
            back.output_amount >= amount_out,
            "exact-output quote must round-trip: got {}, wanted {amount_out}",
            back.output_amount
        );
    }

    #[test]
    fn swap_base_output_charges_the_creator_fee_on_the_output() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            creator_fee_rate: 1_000,
            ..NO_FEES
        };
        let r = swap_base_output(500_000, 1_000_000, 2_000_000, fees).unwrap();
        // Pre-fee output: ceil(500_000 * 1_000_000 / 999_000) = 500_501.
        // Creator fee: 500_501 - 500_000 = 501.
        assert_eq!(r.creator_fee, 501);
        // The curve must move 500_501 out of the output reserve.
        assert_eq!(r.new_output_vault_amount, 2_000_000 - 500_501);
    }

    #[test]
    fn swap_base_output_with_the_creator_fee_on_the_input() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            creator_fee_rate: 1_000,
            creator_fee_on_input: true,
            ..NO_FEES
        };
        let r = swap_base_output(500_000, 1_000_000, 2_000_000, fees).unwrap();
        // The creator fee is taken from the input, so the curve moves exactly
        // the requested output.
        assert_eq!(r.new_output_vault_amount, 2_000_000 - 500_000);
        assert!(r.creator_fee > 0);
    }

    #[test]
    fn output_never_exceeds_the_reserve() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            creator_fee_rate: 1_000,
            ..NO_FEES
        };
        for amount_in in [1u128, 100, 10_000, 1_000_000] {
            let r = swap_base_input(amount_in, 1_000_000, 2_000_000, fees).unwrap();
            assert!(r.output_amount <= 2_000_000);
            assert!(r.new_output_vault_amount <= 2_000_000);
        }
    }

    #[test]
    fn the_invariant_never_decreases() {
        let fees = FeeRates {
            trade_fee_rate: 2_500,
            creator_fee_rate: 1_000,
            ..NO_FEES
        };
        let reserve_in: u128 = 1_000_000_000;
        let reserve_out: u128 = 2_000_000_000;
        for amount_in in [1u128, 1_000, 1_000_000, 100_000_000] {
            let r = swap_base_input(amount_in, reserve_in, reserve_out, fees).unwrap();
            let before = reserve_in * reserve_out;
            let after = r.new_input_vault_amount * r.new_output_vault_amount;
            assert!(after >= before, "invariant decreased: {after} < {before}");
        }
    }

    #[test]
    fn overflow_edges_are_errors_not_wraps() {
        // The u128 numerator overflows.
        assert!(swap_base_input_without_fees(u128::MAX, u128::MAX, u128::MAX).is_err());
        // The fee multiplication overflows.
        assert!(trading_fee(u128::MAX, 1_000_000).is_err());
        // The output exceeds the reserve.
        assert!(swap_base_output_without_fees(2, u128::MAX, 1).is_err());
        // A zero denominator is refused (the input reserve is zero and the
        // input amount is zero too, so `reserve_in + amount_in` is zero).
        assert!(swap_base_input_without_fees(0, 0, 0).is_err());
    }

    #[test]
    fn slippage_helpers_round_the_right_way() {
        // min out is floored: 1_000 with 1% slippage -> 990.
        assert_eq!(min_out_with_slippage_bps(1_000, 100).unwrap(), 990);
        // max in is ceiled: 1_000 with 1% slippage -> 1_010.
        assert_eq!(max_in_with_slippage_bps(1_000, 100).unwrap(), 1_010);
        // A 100% tolerance is refused.
        assert!(min_out_with_slippage_bps(1_000, 10_000).is_err());
        assert!(max_in_with_slippage_bps(1_000, 10_000).is_err());
    }
}
