//! CPMM quote maths, checked against the committed synthetic vectors.
//!
//! The vectors (`crates/solana-kit/fixtures/raydium/cpmm_quote_vectors.json`)
//! are **synthetic**: they are produced by the crate's own quote module,
//! which is a byte-exact integer port of the official
//! `raydium-io/raydium-cp-swap` @
//! `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`
//! `programs/cp-swap/src/curve/{calculator,fees,constant_product}.rs`.
//!
//! The maths is integer-only: `u128` intermediates, the program's rounding
//! directions, and checked arithmetic. No float is involved anywhere.

use serde_json::Value;
use solana_kit::consts::RAYDIUM_CPMM_FEE_RATE_DENOMINATOR;
use solana_kit::venues::raydium_cpmm::quote::{
    calculate_pre_fee_amount, creator_fee, fund_fee, max_in_with_slippage_bps,
    min_out_with_slippage_bps, protocol_fee, split_creator_fee, swap_base_input, swap_base_output,
    trading_fee, FeeRates,
};

const FIXTURE: &str = include_str!("../fixtures/raydium/cpmm_quote_vectors.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the CPMM quote fixture is valid JSON")
}

fn fees_for(case: &str) -> FeeRates {
    match case {
        "no_fees" => FeeRates {
            trade_fee_rate: 0,
            protocol_fee_rate: 0,
            fund_fee_rate: 0,
            creator_fee_rate: 0,
            creator_fee_on_input: false,
        },
        "trade_fee_only" => FeeRates {
            trade_fee_rate: 2_500,
            protocol_fee_rate: 500,
            fund_fee_rate: 500,
            creator_fee_rate: 0,
            creator_fee_on_input: false,
        },
        "creator_fee_on_output" => FeeRates {
            trade_fee_rate: 2_500,
            protocol_fee_rate: 500,
            fund_fee_rate: 500,
            creator_fee_rate: 1_000,
            creator_fee_on_input: false,
        },
        "creator_fee_on_input" => FeeRates {
            trade_fee_rate: 2_500,
            protocol_fee_rate: 500,
            fund_fee_rate: 500,
            creator_fee_rate: 1_000,
            creator_fee_on_input: true,
        },
        other => panic!("unknown fixture case {other}"),
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
fn every_exact_in_vector_reproduces() {
    let f = fixture();
    let reserve_in = f["reserves"]["input_vault_amount"].as_u64().unwrap() as u128;
    let reserve_out = f["reserves"]["output_vault_amount"].as_u64().unwrap() as u128;
    let rows = f["exact_in"].as_array().expect("exact_in is an array");
    assert!(!rows.is_empty(), "the fixture must carry vectors");
    for row in rows {
        let case = row["case"].as_str().unwrap();
        let amount_in = row["amount_in"].as_u64().unwrap() as u128;
        let r = swap_base_input(amount_in, reserve_in, reserve_out, fees_for(case)).unwrap();
        assert_eq!(
            r.output_amount,
            row["output_amount"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.input_amount,
            row["input_amount"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.trade_fee,
            row["trade_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.protocol_fee,
            row["protocol_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.fund_fee,
            row["fund_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.creator_fee,
            row["creator_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.new_input_vault_amount,
            row["new_input_vault_amount"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.new_output_vault_amount,
            row["new_output_vault_amount"].as_u64().unwrap() as u128,
            "case {case}"
        );
    }
}

#[test]
fn every_exact_out_vector_reproduces() {
    let f = fixture();
    let reserve_in = f["reserves"]["input_vault_amount"].as_u64().unwrap() as u128;
    let reserve_out = f["reserves"]["output_vault_amount"].as_u64().unwrap() as u128;
    let rows = f["exact_out"].as_array().expect("exact_out is an array");
    assert!(!rows.is_empty(), "the fixture must carry vectors");
    for row in rows {
        let case = row["case"].as_str().unwrap();
        let amount_out = row["amount_out"].as_u64().unwrap() as u128;
        let r = swap_base_output(amount_out, reserve_in, reserve_out, fees_for(case)).unwrap();
        assert_eq!(
            r.input_amount,
            row["input_amount"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.output_amount,
            row["output_amount"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.trade_fee,
            row["trade_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.protocol_fee,
            row["protocol_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.fund_fee,
            row["fund_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
        assert_eq!(
            r.creator_fee,
            row["creator_fee"].as_u64().unwrap() as u128,
            "case {case}"
        );
    }
}

#[test]
fn every_fee_helper_vector_reproduces() {
    let f = fixture();
    for row in f["fee_helpers"]
        .as_array()
        .expect("fee_helpers is an array")
    {
        let case = row["case"].as_str().unwrap();
        let expected = row["value"].as_u64().unwrap() as u128;
        let actual = match case {
            "trading_fee_is_ceiled" => trading_fee(
                row["amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "protocol_fee_is_floored" => protocol_fee(
                row["amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "fund_fee_is_floored" => fund_fee(
                row["amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "creator_fee_is_ceiled" => creator_fee(
                row["amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "split_creator_fee_is_floored" => split_creator_fee(
                row["total_fee"].as_u64().unwrap() as u128,
                row["trade_fee_rate"].as_u64().unwrap(),
                row["creator_fee_rate"].as_u64().unwrap(),
            )
            .unwrap(),
            "pre_fee_amount_is_ceiled" => calculate_pre_fee_amount(
                row["post_fee_amount"].as_u64().unwrap() as u128,
                row["rate"].as_u64().unwrap(),
            )
            .unwrap(),
            other => panic!("unknown fee helper case {other}"),
        };
        assert_eq!(actual, expected, "case {case}");
    }
}

#[test]
fn the_slippage_helper_vectors_reproduce() {
    let f = fixture();
    let min = &f["slippage_helpers"]["min_out_with_slippage_bps"];
    assert_eq!(
        min_out_with_slippage_bps(
            min["expected_out"].as_u64().unwrap(),
            min["slippage_bps"].as_u64().unwrap()
        )
        .unwrap(),
        min["value"].as_u64().unwrap()
    );
    let max = &f["slippage_helpers"]["max_in_with_slippage_bps"];
    assert_eq!(
        max_in_with_slippage_bps(
            max["expected_in"].as_u64().unwrap(),
            max["slippage_bps"].as_u64().unwrap()
        )
        .unwrap(),
        max["value"].as_u64().unwrap()
    );
}

#[test]
fn the_fee_denominator_is_the_official_one() {
    assert_eq!(RAYDIUM_CPMM_FEE_RATE_DENOMINATOR, 1_000_000);
    assert_eq!(
        fixture()["fee_rate_denominator"].as_u64().unwrap(),
        RAYDIUM_CPMM_FEE_RATE_DENOMINATOR
    );
}

#[test]
fn the_trade_fee_is_ceiled_and_the_split_is_floored() {
    // ceil(1 * 1 / 1_000_000) = 1: even a dust trade pays a whole unit.
    assert_eq!(trading_fee(1, 1).unwrap(), 1);
    // floor(1 * 1 / 1_000_000) = 0: the protocol's share is dust.
    assert_eq!(protocol_fee(1, 1).unwrap(), 0);
    assert_eq!(fund_fee(1, 1).unwrap(), 0);
    assert_eq!(creator_fee(1, 1).unwrap(), 1);
    // The creator's share of a combined fee is floored.
    assert_eq!(split_creator_fee(1_000_000, 2_500, 1_000).unwrap(), 285_714);
}

#[test]
fn the_pre_fee_gross_up_is_ceiled() {
    // To net 750 at a 25% fee you must pay 1_000.
    assert_eq!(calculate_pre_fee_amount(750, 250_000).unwrap(), 1_000);
    // A zero rate is a pass-through.
    assert_eq!(calculate_pre_fee_amount(123, 0).unwrap(), 123);
    // A rate at the denominator has no finite gross-up and is refused.
    assert!(calculate_pre_fee_amount(123, 1_000_000).is_err());
}

#[test]
fn the_exact_out_quote_always_covers_the_exact_in_quote() {
    let reserve_in: u128 = 1_000_000_000;
    let reserve_out: u128 = 2_000_000_000;
    let fees = FeeRates {
        trade_fee_rate: 2_500,
        protocol_fee_rate: 500,
        fund_fee_rate: 500,
        creator_fee_rate: 1_000,
        creator_fee_on_input: false,
    };
    for amount_in in [1_000u128, 1_000_000, 100_000_000] {
        let exact_in = swap_base_input(amount_in, reserve_in, reserve_out, fees).unwrap();
        let exact_out =
            swap_base_output(exact_in.output_amount, reserve_in, reserve_out, fees).unwrap();
        assert!(
            exact_out.input_amount <= amount_in,
            "asking for exactly what the exact-in quote delivered must not cost more \
             than the exact-in input: {} > {amount_in}",
            exact_out.input_amount
        );
    }
}

#[test]
fn the_pool_invariant_never_decreases() {
    let reserve_in: u128 = 1_000_000_000;
    let reserve_out: u128 = 2_000_000_000;
    let fees = FeeRates {
        trade_fee_rate: 2_500,
        protocol_fee_rate: 500,
        fund_fee_rate: 500,
        creator_fee_rate: 1_000,
        creator_fee_on_input: false,
    };
    for amount_in in [1_000u128, 1_000_000, 100_000_000] {
        let r = swap_base_input(amount_in, reserve_in, reserve_out, fees).unwrap();
        let before = reserve_in * reserve_out;
        let after = r.new_input_vault_amount * r.new_output_vault_amount;
        assert!(
            after >= before,
            "the invariant decreased: {after} < {before}"
        );
    }
}

#[test]
fn an_overflow_is_an_error_not_a_wrap() {
    assert!(swap_base_input(u128::MAX, u128::MAX, u128::MAX, fees_for("no_fees")).is_err());
    assert!(trading_fee(u128::MAX, 1_000_000).is_err());
    // The output cannot exceed the reserve.
    assert!(swap_base_output(2, u128::MAX, 1, fees_for("no_fees")).is_err());
    // A zero denominator is refused.
    assert!(
        solana_kit::venues::raydium_cpmm::quote::swap_base_input_without_fees(0, 0, 0).is_err()
    );
}

#[test]
fn official_program_constant_product_rounding_vectors_match_verbatim() {
    let f = fixture();
    let source = f["official_program_tests_source"]
        .as_str()
        .expect("official_program_tests_source header present");
    assert!(
        source.contains("programs/cp-swap/src/curve/constant_product.rs"),
        "{source}"
    );
    let rows = f["official_constant_product_rounding"]
        .as_array()
        .expect("official_constant_product_rounding array");
    assert_eq!(rows.len(), 10);
    for (idx, row) in rows.iter().enumerate() {
        let input_amount: u128 = row["input_amount"].as_str().unwrap().parse().unwrap();
        let input_vault: u128 = row["input_vault_amount"].as_str().unwrap().parse().unwrap();
        let output_vault: u128 = row["output_vault_amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let expected_out: u128 = row["expected_output_amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let actual = solana_kit::venues::raydium_cpmm::quote::swap_base_input_without_fees(
            input_amount,
            input_vault,
            output_vault,
        )
        .unwrap();
        assert_eq!(
            actual, expected_out,
            "official rounding vector #{idx} mismatch"
        );
    }
}
