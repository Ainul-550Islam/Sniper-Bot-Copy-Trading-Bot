//! LaunchLab curve maths, checked against the committed synthetic vectors.
//!
//! The vectors
//! (`crates/solana-kit/fixtures/raydium/launchlab_curve_vectors.json`) are
//! **synthetic**: they are produced by the crate's own curve module, which is
//! an exact integer port of the official `raydium-io/raydium-sdk-V2` @
//! `f8549145afe75d4c486e30c43331c2c4da32b1eb`
//! `src/raydium/launchpad/curve/**`.
//!
//! All arithmetic is integer (`u128` intermediates), the rounding directions
//! are the SDK's, and there is no float anywhere.

use serde_json::Value;
use solana_kit::consts::{
    RAYDIUM_LAUNCHLAB_CURVE_CONSTANT, RAYDIUM_LAUNCHLAB_CURVE_FIXED,
    RAYDIUM_LAUNCHLAB_CURVE_LINEAR, RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR, RAYDIUM_LAUNCHLAB_Q64,
};
use solana_kit::venues::raydium_launchlab::curve::{
    calculate_fee, calculate_pre_fee, isqrt, split_fee, total_fee_rate, ConstantProductCurve,
    CurveState, CurveType, FixedPriceCurve, LinearPriceCurve,
};

const FIXTURE: &str = include_str!("../fixtures/raydium/launchlab_curve_vectors.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the LaunchLab curve fixture is valid JSON")
}

fn state_from(value: &Value) -> CurveState {
    CurveState {
        virtual_base: value["virtual_base"].as_u64().unwrap(),
        virtual_quote: value["virtual_quote"].as_u64().unwrap(),
        real_base: value["real_base"].as_u64().unwrap(),
        real_quote: value["real_quote"].as_u64().unwrap(),
        total_base_sell: 0,
        total_quote_fund_raising: 0,
    }
}

#[test]
fn the_fixture_is_labelled_synthetic() {
    let f = fixture();
    assert_eq!(f["synthetic"], Value::Bool(true));
    let comment = f["_comment"]
        .as_array()
        .expect("_comment is an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        comment.contains("SYNTHETIC") && comment.contains("NOT MAINNET DATA"),
        "the fixture must say, in plain words, that it is synthetic: {comment}"
    );
}

#[test]
fn the_curve_type_codes_match_the_official_ones() {
    let f = fixture();
    assert_eq!(
        f["curve_types"]["constant_product"].as_u64().unwrap(),
        RAYDIUM_LAUNCHLAB_CURVE_CONSTANT as u64
    );
    assert_eq!(
        f["curve_types"]["fixed_price"].as_u64().unwrap(),
        RAYDIUM_LAUNCHLAB_CURVE_FIXED as u64
    );
    assert_eq!(
        f["curve_types"]["linear_price"].as_u64().unwrap(),
        RAYDIUM_LAUNCHLAB_CURVE_LINEAR as u64
    );
    // And they round-trip through the typed enum.
    assert_eq!(CurveType::from_u8(0).unwrap(), CurveType::ConstantProduct);
    assert_eq!(CurveType::from_u8(1).unwrap(), CurveType::FixedPrice);
    assert_eq!(CurveType::from_u8(2).unwrap(), CurveType::LinearPrice);
    assert!(CurveType::from_u8(3).is_err());
}

#[test]
fn the_constant_product_rows_reproduce() {
    let f = fixture();
    let state = state_from(&f["constant_product"]["state"]);
    for row in f["constant_product"]["rows"].as_array().expect("rows") {
        let amount = row["amount"].as_u64().unwrap() as u128;
        assert_eq!(
            ConstantProductCurve::buy_exact_in(&state, amount).unwrap(),
            row["buy_exact_in"].as_u64().unwrap() as u128
        );
        assert_eq!(
            ConstantProductCurve::buy_exact_out(&state, amount).unwrap(),
            row["buy_exact_out"].as_u64().unwrap() as u128
        );
        assert_eq!(
            ConstantProductCurve::sell_exact_in(&state, amount).unwrap(),
            row["sell_exact_in"].as_u64().unwrap() as u128
        );
        assert_eq!(
            ConstantProductCurve::sell_exact_out(&state, amount).unwrap(),
            row["sell_exact_out"].as_u64().unwrap() as u128
        );
    }
}

#[test]
fn the_fixed_price_rows_reproduce() {
    let f = fixture();
    let state = state_from(&f["fixed_price"]["state"]);
    for row in f["fixed_price"]["rows"].as_array().expect("rows") {
        let amount = row["amount"].as_u64().unwrap() as u128;
        assert_eq!(
            FixedPriceCurve::buy_exact_in(&state, amount).unwrap(),
            row["buy_exact_in"].as_u64().unwrap() as u128
        );
        assert_eq!(
            FixedPriceCurve::buy_exact_out(&state, amount).unwrap(),
            row["buy_exact_out"].as_u64().unwrap() as u128
        );
        assert_eq!(
            FixedPriceCurve::sell_exact_in(&state, amount).unwrap(),
            row["sell_exact_in"].as_u64().unwrap() as u128
        );
        assert_eq!(
            FixedPriceCurve::sell_exact_out(&state, amount).unwrap(),
            row["sell_exact_out"].as_u64().unwrap() as u128
        );
    }
}

#[test]
fn the_linear_price_rows_reproduce() {
    let f = fixture();
    let state = state_from(&f["linear_price"]["state"]);
    for row in f["linear_price"]["rows"].as_array().expect("rows") {
        let amount = row["amount"].as_u64().unwrap() as u128;
        assert_eq!(
            LinearPriceCurve::buy_exact_in(&state, amount).unwrap(),
            row["buy_exact_in"].as_u64().unwrap() as u128
        );
        assert_eq!(
            LinearPriceCurve::buy_exact_out(&state, amount).unwrap(),
            row["buy_exact_out"].as_u64().unwrap() as u128
        );
        assert_eq!(
            LinearPriceCurve::sell_exact_in(&state, amount).unwrap(),
            row["sell_exact_in"].as_u64().unwrap() as u128
        );
        assert_eq!(
            LinearPriceCurve::sell_exact_out(&state, amount).unwrap(),
            row["sell_exact_out"].as_u64().unwrap() as u128
        );
    }
}

#[test]
fn the_fee_rows_reproduce() {
    let f = fixture();
    for row in f["fees"].as_array().expect("fees") {
        let case = row["case"].as_str().unwrap();
        let expected = row["value"].as_u64().unwrap() as u128;
        let actual = match case {
            "fee_is_ceiled" => calculate_fee(
                row["amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "pre_fee_is_ceiled" => calculate_pre_fee(
                row["post_fee_amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "total_fee_rate_is_the_sum" => total_fee_rate(
                row["protocol"].as_u64().unwrap(),
                row["platform"].as_u64().unwrap(),
                row["share"].as_u64().unwrap(),
                row["creator"].as_u64().unwrap(),
            )
            .unwrap() as u128,
            other => panic!("unknown fee case {other}"),
        };
        assert_eq!(actual, expected, "case {case}");
    }
}

#[test]
fn the_denominator_and_q64_are_the_official_ones() {
    let f = fixture();
    assert_eq!(
        f["fee_rate_denominator"].as_u64().unwrap(),
        RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR
    );
    assert_eq!(
        f["q64"].as_str().unwrap().parse::<u128>().unwrap(),
        RAYDIUM_LAUNCHLAB_Q64
    );
    assert_eq!(RAYDIUM_LAUNCHLAB_Q64, 1u128 << 64);
}

#[test]
fn the_constant_product_buy_floors_and_the_exact_out_ceils() {
    let s = CurveState {
        virtual_base: 1_000_000_000,
        virtual_quote: 200_000_000,
        real_base: 0,
        real_quote: 0,
        total_base_sell: 1_000_000_000,
        total_quote_fund_raising: 200_000_000,
    };
    // The exact-out quote ceils: feeding it back must deliver at least the
    // requested output.
    for amount_out in [1_000u128, 1_000_000, 100_000_000] {
        let cost = ConstantProductCurve::buy_exact_out(&s, amount_out).unwrap();
        let back = ConstantProductCurve::buy_exact_in(&s, cost).unwrap();
        assert!(back >= amount_out, "got {back}, wanted {amount_out}");
    }
    // One unit less than the cost must undershoot (the exact-out ceils).
    let amount_out = 1_000_000u128;
    let cost = ConstantProductCurve::buy_exact_out(&s, amount_out).unwrap();
    let back = ConstantProductCurve::buy_exact_in(&s, cost - 1).unwrap();
    assert!(
        back < amount_out,
        "got {back}, wanted less than {amount_out}"
    );
}

#[test]
fn the_fixed_price_is_linear() {
    let s = CurveState {
        virtual_base: 1_000_000,
        virtual_quote: 2_000,
        real_base: 0,
        real_quote: 0,
        total_base_sell: 1_000_000,
        total_quote_fund_raising: 2_000,
    };
    // 2_000 quote buys 1_000_000 base, so 1 quote buys 500 base.
    assert_eq!(FixedPriceCurve::buy_exact_in(&s, 1_000).unwrap(), 500_000);
    // Doubling the quote doubles the base: the price never changes.
    assert_eq!(FixedPriceCurve::buy_exact_in(&s, 2_000).unwrap(), 1_000_000);
    // The exact-out ceils.
    assert_eq!(FixedPriceCurve::buy_exact_out(&s, 500_000).unwrap(), 1_000);
    assert_eq!(FixedPriceCurve::buy_exact_out(&s, 1).unwrap(), 1);
}

#[test]
fn the_linear_curve_is_monotonic_in_the_quote() {
    let s = CurveState {
        virtual_base: 1_000_000_000_000_000,
        virtual_quote: 0,
        real_base: 6_074_000,
        real_quote: 1_000_000_000,
        total_base_sell: 1_000_000_000_000,
        total_quote_fund_raising: 1_000_000_000,
    };
    let mut last = 0u128;
    for amount in [1u128, 1_000, 1_000_000, 100_000_000] {
        let out = LinearPriceCurve::buy_exact_in(&s, amount).unwrap();
        assert!(out >= last, "the linear curve must not go backwards");
        last = out;
    }
}

#[test]
fn the_integer_square_root_is_the_floor_root() {
    assert_eq!(isqrt(0), 0);
    assert_eq!(isqrt(1), 1);
    assert_eq!(isqrt(3), 1);
    assert_eq!(isqrt(4), 2);
    assert_eq!(isqrt(8), 2);
    assert_eq!(isqrt(9), 3);
    // sqrt(2^128 - 1) = 2^64 - 1 exactly.
    assert_eq!(isqrt(u128::MAX), 18_446_744_073_709_551_615);
    // The floor property: r^2 <= n < (r+1)^2.
    for n in [0u128, 1, 2, 3, 99, 100, 101, 1_000_000, u128::MAX / 3] {
        let r = isqrt(n);
        assert!(r.saturating_mul(r) <= n, "r^2 > n for n = {n}");
        assert!(
            (r + 1).saturating_mul(r + 1) > n,
            "(r+1)^2 <= n for n = {n}"
        );
    }
}

#[test]
fn the_fee_split_gives_the_remainder_to_the_protocol() {
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
fn the_total_fee_rate_refuses_more_than_the_denominator() {
    assert_eq!(total_fee_rate(1, 2, 3, 4).unwrap(), 10);
    // The SDK throws only when the sum is *greater* than the denominator, so
    // a total of exactly 1_000_000 is accepted.
    assert_eq!(
        total_fee_rate(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR, 0, 0, 0).unwrap(),
        RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR
    );
    assert!(total_fee_rate(RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR, 1, 0, 0).is_err());
}

#[test]
fn the_fee_and_gross_up_are_ceiled() {
    // ceil(1 * 1 / 1_000_000) = 1.
    assert_eq!(calculate_fee(1, 1).unwrap(), 1);
    // To net 750 at a 25% fee you must pay 1_000.
    assert_eq!(calculate_pre_fee(750, 250_000).unwrap(), 1_000);
    // A zero rate is a pass-through.
    assert_eq!(calculate_pre_fee(123, 0).unwrap(), 123);
    // A rate at the denominator has no finite gross-up and is refused.
    assert!(calculate_pre_fee(123, 1_000_000).is_err());
}

#[test]
fn an_overflow_or_an_impossible_reserve_is_an_error_not_a_wrap() {
    let zero = CurveState {
        virtual_quote: 0,
        virtual_base: 0,
        real_base: 0,
        real_quote: 0,
        total_base_sell: 0,
        total_quote_fund_raising: 0,
    };
    // A zero denominator is refused.
    assert!(ConstantProductCurve::buy_exact_in(&zero, 0).is_err());
    assert!(FixedPriceCurve::buy_exact_in(&zero, 1).is_err());

    let s = CurveState {
        virtual_base: 1_000,
        virtual_quote: 1_000,
        real_base: 0,
        real_quote: 0,
        total_base_sell: 0,
        total_quote_fund_raising: 0,
    };
    // The output cannot exceed the reserve.
    assert!(ConstantProductCurve::buy_exact_out(&s, 1_001).is_err());
    // The fee multiplication overflows.
    assert!(calculate_fee(u128::MAX, u64::MAX).is_err());
}
