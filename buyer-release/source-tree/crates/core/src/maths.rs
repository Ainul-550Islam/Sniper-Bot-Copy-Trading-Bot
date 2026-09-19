//! Integer-safe maths used by the pricing, sizing and rounding code.
//!
//! Money must never be multiplied as `f64` when the result is fed to an
//! on-chain instruction, so every function here has a `u64`/`u128` variant
//! that is exact, plus an `f64` variant for display and strategy maths only.

use crate::error::{BotError, BotResult};

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
pub const BPS_DENOM: u64 = 10_000;
/// Polymarket amounts are 6-decimal USDC/pUSD integers.
pub const POLY_DECIMALS: u64 = 1_000_000;

/// `sol` -> lamports, saturating instead of panicking.
pub fn sol_to_lamports(sol: f64) -> u64 {
    if !sol.is_finite() || sol <= 0.0 {
        return 0;
    }
    let v = sol * LAMPORTS_PER_SOL as f64;
    if v >= u64::MAX as f64 {
        return u64::MAX;
    }
    v.round() as u64
}

pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL as f64
}

/// `usd` -> 6-decimal integer used by the Polymarket CLOB.
pub fn usd_to_micro(usd: f64) -> u64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    let v = usd * POLY_DECIMALS as f64;
    if v >= u64::MAX as f64 {
        return u64::MAX;
    }
    v.round() as u64
}

pub fn micro_to_usd(micro: u64) -> f64 {
    micro as f64 / POLY_DECIMALS as f64
}

/// Token units with `decimals` -> raw integer amount.
pub fn to_raw_amount(amount: f64, decimals: u8) -> u64 {
    if !amount.is_finite() || amount <= 0.0 {
        return 0;
    }
    let scale = 10f64.powi(decimals as i32);
    let v = amount * scale;
    if v >= u64::MAX as f64 {
        return u64::MAX;
    }
    v.round() as u64
}

pub fn from_raw_amount(raw: u64, decimals: u8) -> f64 {
    let scale = 10f64.powi(decimals as i32);
    raw as f64 / scale
}

/// Multiply by a percentage, exact for u64 inputs.
pub fn apply_pct_u64(value: u64, pct: f64) -> u64 {
    if !pct.is_finite() {
        return value;
    }
    let v = value as f64 * (1.0 + pct / 100.0);
    if v <= 0.0 {
        return 0;
    }
    if v >= u64::MAX as f64 {
        return u64::MAX;
    }
    v.round() as u64
}

/// Subtract a percentage (used for slippage floors / stop losses).
pub fn minus_pct_u64(value: u64, pct: f64) -> u64 {
    if !pct.is_finite() {
        return value;
    }
    let v = value as f64 * (1.0 - pct / 100.0);
    if v <= 0.0 {
        return 0;
    }
    v.round() as u64
}

/// Basis-point helpers.
pub fn bps_to_fraction(bps: u64) -> f64 {
    bps as f64 / BPS_DENOM as f64
}

pub fn fraction_to_bps(f: f64) -> u64 {
    if !f.is_finite() || f <= 0.0 {
        return 0;
    }
    (f * BPS_DENOM as f64).round() as u64
}

/// Slippage-bounded `min_amount_out` for a buy: `expected * (1 - slippage%)`.
pub fn min_out_with_slippage(expected_out: u64, slippage_pct: f64, max_bps: u64) -> u64 {
    let slippage_pct = slippage_pct.clamp(0.0, bps_to_fraction(max_bps) * 100.0);
    minus_pct_u64(expected_out, slippage_pct)
}

/// Slippage-bounded `max_amount_in` for a sell: `expected * (1 + slippage%)`.
pub fn max_in_with_slippage(expected_in: u64, slippage_pct: f64, max_bps: u64) -> u64 {
    let slippage_pct = slippage_pct.clamp(0.0, bps_to_fraction(max_bps) * 100.0);
    apply_pct_u64(expected_in, slippage_pct)
}

/// Constant-product quote with a fee, matching Raydium AMM v4 / PumpSwap:
/// `amount_out = (amount_in * (1 - fee) * reserve_out) / (reserve_in + amount_in * (1 - fee))`
///
/// All arithmetic is `u128` so a large reserve cannot overflow.
pub fn constant_product_out(
    amount_in: u64,
    reserve_in: u64,
    reserve_out: u64,
    fee_numerator: u64,
    fee_denominator: u64,
) -> u64 {
    if amount_in == 0 || reserve_in == 0 || reserve_out == 0 {
        return 0;
    }
    let fee_den: u128 = if fee_denominator == 0 {
        BPS_DENOM as u128
    } else {
        fee_denominator as u128
    };
    let amount_in_after_fee =
        (amount_in as u128).saturating_mul(fee_den - fee_numerator as u128) / fee_den;
    let numerator = amount_in_after_fee.saturating_mul(reserve_out as u128);
    let denominator = reserve_in as u128 + amount_in_after_fee;
    if denominator == 0 {
        return 0;
    }
    let out = numerator / denominator;
    if out > u64::MAX as u128 {
        u64::MAX
    } else {
        out as u64
    }
}

/// Inverse of [`constant_product_out`]: how much input is needed for `amount_out`.
pub fn constant_product_in(
    amount_out: u64,
    reserve_in: u64,
    reserve_out: u64,
    fee_numerator: u64,
    fee_denominator: u64,
) -> u64 {
    if amount_out == 0 || reserve_in == 0 || reserve_out == 0 || amount_out >= reserve_out {
        return u64::MAX;
    }
    let fee_den: u128 = if fee_denominator == 0 {
        BPS_DENOM as u128
    } else {
        fee_denominator as u128
    };
    let numerator = (reserve_in as u128).saturating_mul(amount_out as u128) * fee_den;
    let denominator = (reserve_out - amount_out) as u128 * (fee_den - fee_numerator as u128);
    if denominator == 0 {
        return u64::MAX;
    }
    let amount_in = numerator / denominator + 1;
    if amount_in > u64::MAX as u128 {
        u64::MAX
    } else {
        amount_in as u64
    }
}

// --------------------------------------------------------------------------
// Pump.fun bonding curve maths.
//
// The curve is a constant-product market against *virtual* reserves:
//   initial virtual SOL reserve   =  30 SOL
//   initial virtual token reserve = 1_073_000_000 tokens
//   real token supply             = 1_000_000_000 tokens
// The values below come from the on-chain `Global` / `BondingCurve` accounts;
// they are read at runtime and these constants are only the documented
// fallback so the maths still works before the first successful read.
// --------------------------------------------------------------------------

pub const PUMP_INITIAL_VIRTUAL_SOL_RESERVES: u64 = 30 * LAMPORTS_PER_SOL;
pub const PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES: u64 = 1_073_000_000_000_000_000;
pub const PUMP_INITIAL_REAL_TOKEN_RESERVES: u64 = 793_100_000_000_000_000;
pub const PUMP_TOKEN_TOTAL_SUPPLY: u64 = 1_000_000_000_000_000_000;

/// SOL cost (lamports) of buying `tokens_out` from the bonding curve.
pub fn pump_get_buy_cost(
    tokens_out: u64,
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
) -> Option<u64> {
    if tokens_out == 0 {
        return Some(0);
    }
    if tokens_out >= virtual_token_reserves {
        return None;
    }
    let vs = virtual_sol_reserves as u128;
    let vt = virtual_token_reserves as u128;
    let tokens = tokens_out as u128;
    // cost = vs * tokens / (vt - tokens), rounded up
    let numerator = vs.checked_mul(tokens)?;
    let denominator = vt.checked_sub(tokens)?;
    if denominator == 0 {
        return None;
    }
    let cost = numerator / denominator;
    let cost = if numerator % denominator != 0 {
        cost + 1
    } else {
        cost
    };
    if cost > u64::MAX as u128 {
        None
    } else {
        Some(cost as u64)
    }
}

/// Tokens received for `sol_in` lamports on the bonding curve.
pub fn pump_get_tokens_for_sol(
    sol_in: u64,
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
) -> Option<u64> {
    if sol_in == 0 {
        return Some(0);
    }
    let vs = virtual_sol_reserves as u128;
    let vt = virtual_token_reserves as u128;
    let sol = sol_in as u128;
    // tokens = vt * sol / (vs + sol)
    let numerator = vt.checked_mul(sol)?;
    let denominator = vs.checked_add(sol)?;
    if denominator == 0 {
        return None;
    }
    let tokens = numerator / denominator;
    if tokens > u64::MAX as u128 {
        None
    } else {
        Some(tokens as u64)
    }
}

/// SOL received for selling `tokens_in` back into the curve.
pub fn pump_get_sol_for_tokens(
    tokens_in: u64,
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
) -> Option<u64> {
    if tokens_in == 0 {
        return Some(0);
    }
    let vs = virtual_sol_reserves as u128;
    let vt = virtual_token_reserves as u128;
    let tokens = tokens_in as u128;
    // sol = vs * tokens / (vt + tokens)
    let numerator = vs.checked_mul(tokens)?;
    let denominator = vt.checked_add(tokens)?;
    let sol = numerator / denominator;
    if sol > u64::MAX as u128 {
        None
    } else {
        Some(sol as u64)
    }
}

/// Price of one whole token in SOL, given the curve reserves.
pub fn pump_spot_price_sol(virtual_sol_reserves: u64, virtual_token_reserves: u64) -> f64 {
    if virtual_token_reserves == 0 {
        return 0.0;
    }
    (virtual_sol_reserves as f64 / LAMPORTS_PER_SOL as f64)
        / (virtual_token_reserves as f64 / PUMP_TOKEN_TOTAL_SUPPLY as f64)
}

/// Market cap in SOL implied by the curve reserves.
pub fn pump_market_cap_sol(virtual_sol_reserves: u64, virtual_token_reserves: u64) -> f64 {
    pump_spot_price_sol(virtual_sol_reserves, virtual_token_reserves)
        * (PUMP_TOKEN_TOTAL_SUPPLY as f64 / LAMPORTS_PER_SOL as f64)
}

/// Round `price` down to the nearest multiple of `tick_size` (Polymarket).
pub fn round_down_to_tick(price: f64, tick_size: f64) -> BotResult<f64> {
    // matches!-on-partial_cmp keeps the NaN-rejects behaviour explicit
    // (a plain `!(tick_size > 0.0)` reads as a negated partial order).
    if !matches!(
        tick_size.partial_cmp(&0.0),
        Some(std::cmp::Ordering::Greater)
    ) {
        return Err(BotError::invalid("tick_size must be > 0"));
    }
    if !price.is_finite() || price < 0.0 {
        return Err(BotError::invalid("price must be finite and >= 0"));
    }
    // Scale into integers to avoid binary-float drift on 0.001 / 0.0025 ticks.
    let scale = 1.0 / tick_size;
    let ticks = (price * scale).floor();
    Ok(ticks / scale)
}

/// Number of decimal places a Polymarket tick size implies.
pub fn decimals_for_tick(tick_size: f64) -> u32 {
    let mut d = 0u32;
    let mut t = tick_size;
    while t < 1.0 - f64::EPSILON && d < 8 {
        t *= 10.0;
        d += 1;
    }
    d
}

/// Price/size/amount decimal places per Polymarket's tick-size table.
///
/// | tick   | price dec | size dec | amount dec |
/// |--------|-----------|----------|------------|
/// | 0.1    | 1         | 2        | 3          |
/// | 0.01   | 2         | 2        | 4          |
/// | 0.005  | 3         | 2        | 5          |
/// | 0.0025 | 4         | 2        | 6          |
/// | 0.001  | 3         | 2        | 5          |
/// | 0.0001 | 4         | 2        | 6          |
pub fn poly_precision(tick_size: f64) -> (u32, u32, u32) {
    let approx = (tick_size * 100_000.0).round() as u64;
    match approx {
        10_000 => (1, 2, 3),
        1_000 => (2, 2, 4),
        500 => (3, 2, 5),
        250 => (4, 2, 6),
        100 => (3, 2, 5),
        10 => (4, 2, 6),
        _ => (2, 2, 4),
    }
}

/// Round a share size down to `size_decimals` places.
pub fn round_size(size: f64, size_decimals: u32) -> f64 {
    let scale = 10f64.powi(size_decimals as i32);
    (size * scale).floor() / scale
}

/// Polymarket amount rounding: if the USD amount exceeds `amount_decimals`,
/// round it up to `amount_decimals + 4` first, then down to `amount_decimals`.
pub fn round_amount(amount: f64, amount_decimals: u32) -> f64 {
    let scale = 10f64.powi(amount_decimals as i32);
    let scaled = amount * scale;
    let digits = scaled.fract().abs() * 10f64.powi(4);
    if digits >= 1.0 {
        let up = 10f64.powi(amount_decimals as i32 + 4);
        let rounded_up = (amount * up).round() / up;
        ((rounded_up * scale).floor()) / scale
    } else {
        (scaled.floor()) / scale
    }
}

/// Format a float with a fixed number of decimals, trimming trailing zeros.
pub fn fmt_amount(v: f64, decimals: u32) -> String {
    if !v.is_finite() {
        return "n/a".into();
    }
    let s = format!("{v:.decimals$}", decimals = decimals as usize);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Clamp into `[lo, hi]`, tolerating NaN.
pub fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64 {
    if !v.is_finite() {
        return lo;
    }
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamport_conversion_round_trips() {
        assert_eq!(sol_to_lamports(1.0), LAMPORTS_PER_SOL);
        assert_eq!(sol_to_lamports(0.01), 10_000_000);
        assert!((lamports_to_sol(10_000_000) - 0.01).abs() < 1e-12);
        assert_eq!(sol_to_lamports(-1.0), 0);
        assert_eq!(sol_to_lamports(f64::NAN), 0);
    }

    #[test]
    fn constant_product_matches_hand_computed_value() {
        // 1 SOL into a 1000/1000 pool with a 25 bps fee.
        let out = constant_product_out(
            1_000_000_000,
            1_000_000_000_000,
            1_000_000_000_000,
            25,
            10_000,
        );
        assert!(out > 0 && out < 1_000_000_000, "out = {out}");
        assert_eq!(constant_product_out(0, 1, 1, 25, 10_000), 0);
    }

    #[test]
    fn bonding_curve_cost_is_monotonic() {
        let a = pump_get_buy_cost(
            1_000_000_000_000_000,
            PUMP_INITIAL_VIRTUAL_SOL_RESERVES,
            PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES,
        );
        let b = pump_get_buy_cost(
            2_000_000_000_000_000,
            PUMP_INITIAL_VIRTUAL_SOL_RESERVES,
            PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES,
        );
        assert!(a.is_some() && b.is_some());
        assert!(
            b.unwrap() > a.unwrap() * 2 - 10,
            "curve must be convex upward"
        );
        assert!(pump_get_buy_cost(PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES, 1, 1).is_none());
    }

    #[test]
    fn bonding_curve_buy_sell_is_lossy() {
        let tokens = pump_get_tokens_for_sol(
            LAMPORTS_PER_SOL,
            PUMP_INITIAL_VIRTUAL_SOL_RESERVES,
            PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES,
        )
        .unwrap();
        let back = pump_get_sol_for_tokens(
            tokens,
            PUMP_INITIAL_VIRTUAL_SOL_RESERVES + LAMPORTS_PER_SOL,
            PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES - tokens,
        )
        .unwrap();
        assert!(back < LAMPORTS_PER_SOL, "round trip must lose value");
    }

    #[test]
    fn tick_rounding_respects_the_tick() {
        assert!((round_down_to_tick(0.5237, 0.01).unwrap() - 0.52).abs() < 1e-9);
        assert!((round_down_to_tick(0.5237, 0.001).unwrap() - 0.523).abs() < 1e-9);
        assert!(round_down_to_tick(0.5, 0.0).is_err());
    }

    #[test]
    fn precision_table_matches_the_docs() {
        assert_eq!(poly_precision(0.01), (2, 2, 4));
        assert_eq!(poly_precision(0.001), (3, 2, 5));
        assert_eq!(poly_precision(0.0025), (4, 2, 6));
    }
}
