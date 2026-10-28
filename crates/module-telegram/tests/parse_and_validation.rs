//! BATCH 04 — parsing and validation, table-driven.
//!
//! Everything a Telegram user can type is exercised here: mints (valid,
//! invalid alphabet, wrong length), SOL amounts (zero, negative, NaN, huge,
//! too many decimals), percentages (including `all`), slippage and the
//! `/consent grant` argument grammar. Every case asserts the **exact** typed
//! error, so a rejection can never be silently swallowed.

use module_telegram::commands::buy::parse_buy_args;
use module_telegram::commands::limits::{SlippageBounds, TradeLimits};
use module_telegram::commands::parse::{
    parse_consent_grant, parse_mint, parse_percent_bps, parse_slippage_bps, parse_sol_lamports,
    ParseError, LAMPORTS_PER_SOL, PERCENT_ALL_BPS,
};
use module_telegram::commands::sell::parse_sell_args;
use module_telegram::consent::ConsentBounds;

const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const RAY: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";

#[test]
fn mints_must_decode_to_exactly_32_bytes() {
    for mint in [USDC, RAY, "So11111111111111111111111111111111111111112"] {
        assert_eq!(parse_mint(mint).as_deref(), Ok(mint));
        // Surrounding whitespace is trimmed, never embedded.
        assert_eq!(parse_mint(&format!("  {mint} ")).as_deref(), Ok(mint));
    }
    assert_eq!(parse_mint(""), Err(ParseError::MissingArgument("mint")));
    assert_eq!(parse_mint("  "), Err(ParseError::MissingArgument("mint")));
    assert_eq!(parse_mint("0OIl"), Err(ParseError::MintNotBase58));
    assert_eq!(
        parse_mint("So1111111111111111111111111111111111111111I"),
        Err(ParseError::MintNotBase58)
    );
    // Base58 but the wrong size.
    assert_eq!(
        parse_mint(&bs58::encode([7u8; 31]).into_string()),
        Err(ParseError::MintWrongLength(31))
    );
    assert_eq!(
        parse_mint(&bs58::encode([7u8; 33]).into_string()),
        Err(ParseError::MintWrongLength(33))
    );
    assert_eq!(
        parse_mint(&bs58::encode([7u8; 64]).into_string()),
        Err(ParseError::MintWrongLength(64))
    );
}

#[test]
fn lamport_conversion_is_exact_and_never_uses_floats() {
    let cases: &[(&str, Result<u64, ParseError>)] = &[
        ("0.000000001", Ok(1)),
        ("0.000000010", Ok(10)),
        ("0.1", Ok(100_000_000)),
        ("0.25", Ok(250_000_000)),
        ("0.05", Ok(50_000_000)),
        ("1", Ok(LAMPORTS_PER_SOL)),
        ("2.5", Ok(2_500_000_000)),
        ("  7  ", Ok(7 * LAMPORTS_PER_SOL)),
        // 2^64-1 lamports is the largest representable amount.
        ("18446744073.709551615", Ok(u64::MAX)),
        ("0", Err(ParseError::AmountZero)),
        ("0.00", Err(ParseError::AmountZero)),
        ("000", Err(ParseError::AmountZero)),
        ("-0.000000001", Err(ParseError::AmountNegative)),
        ("-5", Err(ParseError::AmountNegative)),
        ("1e-9", Err(ParseError::AmountNotNumeric)),
        ("1E9", Err(ParseError::AmountNotNumeric)),
        ("NaN", Err(ParseError::AmountNotNumeric)),
        ("inf", Err(ParseError::AmountNotNumeric)),
        ("infinity", Err(ParseError::AmountNotNumeric)),
        ("0x10", Err(ParseError::AmountNotNumeric)),
        ("1_000", Err(ParseError::AmountNotNumeric)),
        ("1,5", Err(ParseError::AmountNotNumeric)),
        ("1 000", Err(ParseError::AmountNotNumeric)),
        ("", Err(ParseError::AmountEmpty)),
        ("   ", Err(ParseError::AmountEmpty)),
        ("1.", Err(ParseError::AmountNotNumeric)),
        (".", Err(ParseError::AmountNotNumeric)),
        ("0.0000000001", Err(ParseError::AmountTooManyDecimals)),
        ("1.1234567891", Err(ParseError::AmountTooManyDecimals)),
        ("18446744074", Err(ParseError::AmountTooLarge)),
        ("999999999999999999999999", Err(ParseError::AmountTooLarge)),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_sol_lamports(input), *expected, "input {input:?}");
    }
}

#[test]
fn percentages_become_basis_points() {
    let cases: &[(&str, Result<u16, ParseError>)] = &[
        ("all", Ok(PERCENT_ALL_BPS)),
        ("All", Ok(PERCENT_ALL_BPS)),
        (" ALL ", Ok(PERCENT_ALL_BPS)),
        ("100", Ok(10_000)),
        ("100.00", Ok(10_000)),
        ("75", Ok(7_500)),
        ("33.33", Ok(3_333)),
        ("1", Ok(100)),
        ("0.5", Ok(50)),
        ("0.01", Ok(1)),
        ("0", Err(ParseError::PercentOutOfRange)),
        ("0.00", Err(ParseError::PercentOutOfRange)),
        ("100.01", Err(ParseError::PercentOutOfRange)),
        ("250", Err(ParseError::PercentOutOfRange)),
        ("1.234", Err(ParseError::PercentTooManyDecimals)),
        ("half", Err(ParseError::PercentNotNumeric)),
        ("-10", Err(ParseError::PercentNotNumeric)),
        ("", Err(ParseError::PercentEmpty)),
        ("1.", Err(ParseError::PercentNotNumeric)),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_percent_bps(input), *expected, "input {input:?}");
    }
}

#[test]
fn slippage_is_rejected_outside_the_window_never_clamped() {
    assert_eq!(parse_slippage_bps("1", 1, 500), Ok(1));
    assert_eq!(parse_slippage_bps("500", 1, 500), Ok(500));
    for bad in ["0", "501", "65536"] {
        assert_eq!(
            parse_slippage_bps(bad, 1, 500),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 }),
            "input {bad:?}"
        );
    }
    for not_numeric in [
        "abc",
        "1.5",
        "-1",
        "+5",
        "0x10",
        "10_000",
        "99999999999999999999",
    ] {
        assert_eq!(
            parse_slippage_bps(not_numeric, 1, 500),
            Err(ParseError::SlippageNotNumeric),
            "input {not_numeric:?}"
        );
    }
    assert_eq!(
        parse_slippage_bps("", 1, 500),
        Err(ParseError::MissingArgument("slippage_bps"))
    );
}

#[test]
fn buy_and_sell_argument_parsing_uses_the_configured_window() {
    let limits = TradeLimits {
        slippage: SlippageBounds {
            min_bps: 5,
            max_bps: 50,
        },
        ..TradeLimits::default()
    };
    assert_eq!(
        parse_buy_args(USDC, "0.02", None, &limits)
            .expect("buy")
            .amount_lamports,
        20_000_000
    );
    assert_eq!(
        parse_buy_args(USDC, "0.02", Some("5"), &limits)
            .expect("buy")
            .slippage_bps,
        5
    );
    assert_eq!(
        parse_buy_args(USDC, "0.02", Some("4"), &limits).unwrap_err(),
        ParseError::SlippageOutOfRange { min: 5, max: 50 }
    );
    assert_eq!(
        parse_sell_args(RAY, "all", None, &limits)
            .expect("sell")
            .percent_bps,
        PERCENT_ALL_BPS
    );
    assert_eq!(
        parse_sell_args(RAY, "10", Some("100"), &limits).unwrap_err(),
        ParseError::SlippageOutOfRange { min: 5, max: 50 }
    );
}

#[test]
fn consent_grant_takes_trailing_phrase() {
    let bounds = ConsentBounds::default();
    let args: Vec<String> = [
        "0.01", "0.05", "6", "I", "CONSENT", "TO", "TELEGRAM", "TRADING",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let parsed = parse_consent_grant(&args, &bounds).expect("grant args");
    assert_eq!(parsed.order_lamports, Some(10_000_000));
    assert_eq!(parsed.daily_lamports, Some(50_000_000));
    assert_eq!(parsed.ttl_hours, Some(6));
    assert_eq!(parsed.phrase, "I CONSENT TO TELEGRAM TRADING");

    // Only the phrase: every numeric argument takes its default.
    let phrase_only: Vec<String> = ["I", "CONSENT", "TO", "TELEGRAM", "TRADING"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let parsed = parse_consent_grant(&phrase_only, &bounds).expect("defaults");
    assert_eq!(parsed.order_lamports, None);
    assert_eq!(parsed.daily_lamports, None);
    assert_eq!(parsed.ttl_hours, None);

    // A bad number is an error, never silently part of the phrase.
    let bad: Vec<String> = ["0.01", "0", "I", "CONSENT"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        parse_consent_grant(&bad, &bounds),
        Err(ParseError::AmountZero)
    );
    // A missing phrase is an error.
    let no_phrase: Vec<String> = ["0.01", "0.05"].iter().map(|s| s.to_string()).collect();
    assert_eq!(
        parse_consent_grant(&no_phrase, &bounds),
        Err(ParseError::MissingArgument("phrase"))
    );
}

#[test]
fn parser_errors_have_stable_codes_and_messages() {
    let errors = [
        (ParseError::MissingArgument("mint"), "missing_argument"),
        (ParseError::MintNotBase58, "mint_not_base58"),
        (ParseError::MintWrongLength(1), "mint_wrong_length"),
        (ParseError::AmountZero, "amount_zero"),
        (ParseError::AmountNegative, "amount_negative"),
        (ParseError::AmountTooLarge, "amount_too_large"),
        (
            ParseError::AmountTooManyDecimals,
            "amount_too_many_decimals",
        ),
        (ParseError::PercentOutOfRange, "percent_out_of_range"),
        (
            ParseError::SlippageOutOfRange { min: 1, max: 2 },
            "slippage_out_of_range",
        ),
        (
            ParseError::TtlOutOfRange { min: 1, max: 2 },
            "ttl_out_of_range",
        ),
    ];
    for (error, code) in errors {
        assert_eq!(error.code(), code);
        assert!(!error.message().is_empty());
        // Codes are what lands in the audit trail: they must be secret-free.
        assert!(!error.message().contains('/'));
    }
}

#[test]
fn validation_never_accepts_a_float_round_trip() {
    // 0.1 + 0.2 in f64 is 0.30000000000000004; the exact parser must not care.
    assert_eq!(
        parse_sol_lamports("0.1").unwrap() + parse_sol_lamports("0.2").unwrap(),
        300_000_000
    );
    assert_eq!(
        parse_sol_lamports("0.3").unwrap(),
        parse_sol_lamports("0.1").unwrap() + parse_sol_lamports("0.2").unwrap()
    );
    // Ten thousand 0.000000001-lamport additions stay exact.
    let one = parse_sol_lamports("0.000000001").unwrap();
    let total: u64 = std::iter::repeat_n(one, 10_000).sum();
    assert_eq!(total, 10_000);
}
