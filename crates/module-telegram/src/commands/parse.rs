//! The **only** owner of command parsing and input validation.
//!
//! Everything a Telegram user types is hostile until proven otherwise, so the
//! rules are strict and every rejection is typed and explained:
//!
//! * **mint** — must decode from base58 to exactly 32 bytes *and* re-encode to
//!   the same string (canonical form), so `So111…` is accepted while
//!   `0OIl`, padding tricks and truncated addresses are not;
//! * **SOL amount** — parsed as an exact decimal into integer lamports; no
//!   `f64`, no `f32`, no rounding, at most 9 fractional digits;
//! * **percent** — `all` or a decimal percentage with at most 2 fractional
//!   digits, converted to basis points in `(0, 10000]`;
//! * **slippage** — an integer inside the configured window; out-of-range
//!   values are **rejected**, never clamped.
//!
//! Parsing never decides policy (caps, consent, modes). It only answers "is
//! this a well-formed number/address/value at all".

use std::str::FromStr;

use crate::consent::ConsentBounds;

/// Lamports in one SOL.
pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Maximum fractional digits a SOL amount may carry.
pub const MAX_SOL_DECIMALS: u32 = 9;

/// Maximum fractional digits a percentage may carry (1 bp granularity).
pub const MAX_PERCENT_DECIMALS: u32 = 2;

/// Basis points that mean "the whole position".
pub const PERCENT_ALL_BPS: u16 = 10_000;

/// Bytes in a Solana public key.
pub const MINT_BYTES: usize = 32;

/// The slippage window a deployment applies when the operator omits one.
pub const DEFAULT_SLIPPAGE_BPS: u16 = 100;

/// Default lower slippage bound.
pub const MIN_SLIPPAGE_BPS: u16 = 1;

/// Default upper slippage bound (5%).
pub const MAX_SLIPPAGE_BPS: u16 = 500;

/// Typed parse/validation failure. Every variant carries enough detail to be
/// explained to the operator without leaking anything sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A required positional argument was missing or empty.
    MissingArgument(&'static str),
    /// The mint is not valid base58.
    MintNotBase58,
    /// The mint decoded to the wrong number of bytes.
    MintWrongLength(usize),
    /// The amount string was empty.
    AmountEmpty,
    /// The amount was not a plain decimal number.
    AmountNotNumeric,
    /// The amount was negative.
    AmountNegative,
    /// The amount was zero.
    AmountZero,
    /// The amount does not fit in a `u64` of lamports.
    AmountTooLarge,
    /// The amount carried more than 9 fractional digits.
    AmountTooManyDecimals,
    /// The percentage string was empty.
    PercentEmpty,
    /// The percentage was not a plain decimal number.
    PercentNotNumeric,
    /// The percentage carried more than 2 fractional digits.
    PercentTooManyDecimals,
    /// The percentage was not inside `(0, 100]`.
    PercentOutOfRange,
    /// The slippage was not an integer.
    SlippageNotNumeric,
    /// The slippage was outside the configured window.
    SlippageOutOfRange {
        /// Inclusive minimum.
        min: u16,
        /// Inclusive maximum.
        max: u16,
    },
    /// The TTL was not an integer number of hours.
    TtlNotNumeric,
    /// The TTL was outside the configured window.
    TtlOutOfRange {
        /// Inclusive minimum.
        min: u32,
        /// Inclusive maximum.
        max: u32,
    },
}

impl ParseError {
    /// Stable machine code (used as `failure_code` and in audit rows).
    pub fn code(&self) -> &'static str {
        match self {
            ParseError::MissingArgument(_) => "missing_argument",
            ParseError::MintNotBase58 => "mint_not_base58",
            ParseError::MintWrongLength(_) => "mint_wrong_length",
            ParseError::AmountEmpty => "amount_empty",
            ParseError::AmountNotNumeric => "amount_not_numeric",
            ParseError::AmountNegative => "amount_negative",
            ParseError::AmountZero => "amount_zero",
            ParseError::AmountTooLarge => "amount_too_large",
            ParseError::AmountTooManyDecimals => "amount_too_many_decimals",
            ParseError::PercentEmpty => "percent_empty",
            ParseError::PercentNotNumeric => "percent_not_numeric",
            ParseError::PercentTooManyDecimals => "percent_too_many_decimals",
            ParseError::PercentOutOfRange => "percent_out_of_range",
            ParseError::SlippageNotNumeric => "slippage_not_numeric",
            ParseError::SlippageOutOfRange { .. } => "slippage_out_of_range",
            ParseError::TtlNotNumeric => "ttl_not_numeric",
            ParseError::TtlOutOfRange { .. } => "ttl_out_of_range",
        }
    }

    /// The exact text shown in the chat (embedded into the reply template by
    /// [`crate::commands::reply`], never assembled ad hoc).
    pub fn message(&self) -> String {
        match self {
            ParseError::MissingArgument(what) => format!("missing argument: {what}"),
            ParseError::MintNotBase58 => "that is not a base58 mint address".to_string(),
            ParseError::MintWrongLength(n) => {
                format!("the mint decodes to {n} bytes, expected 32")
            }
            ParseError::AmountEmpty => "the SOL amount is empty".to_string(),
            ParseError::AmountNotNumeric => {
                "the SOL amount must be a plain decimal, e.g. 0.25".to_string()
            }
            ParseError::AmountNegative => "the SOL amount must be positive".to_string(),
            ParseError::AmountZero => "the SOL amount must be greater than zero".to_string(),
            ParseError::AmountTooLarge => "the SOL amount is too large".to_string(),
            ParseError::AmountTooManyDecimals => {
                format!("the SOL amount may carry at most {MAX_SOL_DECIMALS} decimals")
            }
            ParseError::PercentEmpty => "the percentage is empty".to_string(),
            ParseError::PercentNotNumeric => "the percentage must be a number or `all`".to_string(),
            ParseError::PercentTooManyDecimals => {
                format!("the percentage may carry at most {MAX_PERCENT_DECIMALS} decimals")
            }
            ParseError::PercentOutOfRange => {
                "the percentage must be greater than 0 and at most 100".to_string()
            }
            ParseError::SlippageNotNumeric => "slippage must be an integer in basis points".into(),
            ParseError::SlippageOutOfRange { min, max } => {
                format!("slippage must be between {min} and {max} basis points")
            }
            ParseError::TtlNotNumeric => "the TTL must be an integer number of hours".to_string(),
            ParseError::TtlOutOfRange { min, max } => {
                format!("the TTL must be between {min} and {max} hours")
            }
        }
    }
}

/// Validate a base58 mint and return its canonical spelling.
pub fn parse_mint(raw: &str) -> Result<String, ParseError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ParseError::MissingArgument("mint"));
    }
    let bytes = bs58::decode(text)
        .into_vec()
        .map_err(|_| ParseError::MintNotBase58)?;
    if bytes.len() != MINT_BYTES {
        return Err(ParseError::MintWrongLength(bytes.len()));
    }
    // Canonical round-trip: rejects non-minimal encodings that would let the
    // same account appear under two different strings.
    let canonical = bs58::encode(&bytes).into_string();
    if canonical != text {
        return Err(ParseError::MintNotBase58);
    }
    Ok(canonical)
}

/// Parse an exact decimal SOL string into integer lamports.
///
/// `"1"` → 1_000_000_000, `"0.05"` → 50_000_000, `".5"` → 500_000_000,
/// `"0.000000001"` → 1. Everything else is rejected with a typed error.
pub fn parse_sol_lamports(raw: &str) -> Result<u64, ParseError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ParseError::AmountEmpty);
    }
    if let Some(rest) = text.strip_prefix('-') {
        // `-0` is still a negative amount as far as money is concerned.
        let _ = rest;
        return Err(ParseError::AmountNegative);
    }
    if text.starts_with('+') {
        return Err(ParseError::AmountNotNumeric);
    }
    let (whole, fraction) = match text.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (text, None),
    };
    if whole.is_empty() && fraction.is_none() {
        return Err(ParseError::AmountNotNumeric);
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) {
        return Err(ParseError::AmountNotNumeric);
    }
    let fraction = match fraction {
        None => "",
        Some(f) => {
            if f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()) {
                return Err(ParseError::AmountNotNumeric);
            }
            if f.len() > MAX_SOL_DECIMALS as usize {
                return Err(ParseError::AmountTooManyDecimals);
            }
            f
        }
    };
    if whole.len() > 20 {
        return Err(ParseError::AmountTooLarge);
    }
    let whole_value: u64 = if whole.is_empty() {
        0
    } else {
        u64::from_str(whole).map_err(|_| ParseError::AmountTooLarge)?
    };
    let mut padded = fraction.to_string();
    while padded.len() < MAX_SOL_DECIMALS as usize {
        padded.push('0');
    }
    let fraction_value: u64 = if padded.is_empty() {
        0
    } else {
        u64::from_str(&padded).map_err(|_| ParseError::AmountTooManyDecimals)?
    };
    let lamports = whole_value
        .checked_mul(LAMPORTS_PER_SOL)
        .and_then(|v| v.checked_add(fraction_value))
        .ok_or(ParseError::AmountTooLarge)?;
    if lamports == 0 {
        return Err(ParseError::AmountZero);
    }
    Ok(lamports)
}

/// Parse `all`, `100`, `12.5`, `0.5` … into basis points of a position.
pub fn parse_percent_bps(raw: &str) -> Result<u16, ParseError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ParseError::PercentEmpty);
    }
    if text.eq_ignore_ascii_case("all") {
        return Ok(PERCENT_ALL_BPS);
    }
    let (whole, fraction) = match text.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (text, None),
    };
    if whole.is_empty() && fraction.is_none() {
        return Err(ParseError::PercentNotNumeric);
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) {
        return Err(ParseError::PercentNotNumeric);
    }
    let fraction = match fraction {
        None => "",
        Some(f) => {
            if f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()) {
                return Err(ParseError::PercentNotNumeric);
            }
            if f.len() > MAX_PERCENT_DECIMALS as usize {
                return Err(ParseError::PercentTooManyDecimals);
            }
            f
        }
    };
    let whole_value: u32 = if whole.is_empty() {
        0
    } else {
        u32::from_str(whole).map_err(|_| ParseError::PercentOutOfRange)?
    };
    let mut padded = fraction.to_string();
    while padded.len() < MAX_PERCENT_DECIMALS as usize {
        padded.push('0');
    }
    let fraction_value: u32 = if padded.is_empty() {
        0
    } else {
        u32::from_str(&padded).map_err(|_| ParseError::PercentTooManyDecimals)?
    };
    let bps = whole_value
        .checked_mul(100)
        .and_then(|v| v.checked_add(fraction_value))
        .ok_or(ParseError::PercentOutOfRange)?;
    if bps == 0 {
        return Err(ParseError::PercentOutOfRange);
    }
    if bps > u32::from(PERCENT_ALL_BPS) {
        return Err(ParseError::PercentOutOfRange);
    }
    Ok(bps as u16)
}

/// Parse and range-check a slippage value in basis points.
pub fn parse_slippage_bps(raw: &str, min: u16, max: u16) -> Result<u16, ParseError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ParseError::MissingArgument("slippage_bps"));
    }
    if !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(ParseError::SlippageNotNumeric);
    }
    let value: u32 = u32::from_str(text).map_err(|_| ParseError::SlippageNotNumeric)?;
    if value > u32::from(u16::MAX) || value < u32::from(min) || value > u32::from(max) {
        return Err(ParseError::SlippageOutOfRange { min, max });
    }
    Ok(value as u16)
}

/// Parse an integer number of hours and range-check it.
pub fn parse_ttl_hours(raw: &str, min: u32, max: u32) -> Result<u32, ParseError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ParseError::MissingArgument("ttl_hours"));
    }
    if !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(ParseError::TtlNotNumeric);
    }
    let value: u32 = u32::from_str(text).map_err(|_| ParseError::TtlNotNumeric)?;
    if value < min || value > max {
        return Err(ParseError::TtlOutOfRange { min, max });
    }
    Ok(value)
}

/// The three optional numeric arguments of `/consent grant` plus the typed
/// confirmation phrase that must follow them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantArgs {
    /// Per-order cap in lamports, when given.
    pub order_lamports: Option<u64>,
    /// Rolling 24h cap in lamports, when given.
    pub daily_lamports: Option<u64>,
    /// Lifetime in hours, when given.
    pub ttl_hours: Option<u32>,
    /// The words the operator typed as the confirmation phrase.
    pub phrase: String,
}

/// Parse `[max_order_sol] [daily_cap_sol] [ttl_hours] <phrase…>`.
///
/// Numeric arguments are positional and must come first; the first token that
/// is not numeric starts the phrase. A numeric token that fails to parse is an
/// error (never silently reinterpreted as phrase text).
pub fn parse_consent_grant(
    args: &[String],
    bounds: &ConsentBounds,
) -> Result<GrantArgs, ParseError> {
    let mut order_lamports = None;
    let mut daily_lamports = None;
    let mut ttl_hours = None;
    let mut index = 0usize;

    while index < args.len() && index < 3 {
        let token = args[index].trim();
        if token.is_empty() {
            index += 1;
            continue;
        }
        let looks_numeric = token
            .chars()
            .next()
            .map(|c| c.is_ascii_digit() || c == '.')
            .unwrap_or(false);
        if !looks_numeric {
            break;
        }
        match index {
            0 => order_lamports = Some(parse_sol_lamports(token)?),
            1 => daily_lamports = Some(parse_sol_lamports(token)?),
            _ => {
                ttl_hours = Some(parse_ttl_hours(
                    token,
                    bounds.min_ttl_hours,
                    bounds.max_ttl_hours,
                )?)
            }
        }
        index += 1;
    }

    let phrase = args[index..].join(" ");
    if phrase.trim().is_empty() {
        return Err(ParseError::MissingArgument("phrase"));
    }
    Ok(GrantArgs {
        order_lamports,
        daily_lamports,
        ttl_hours,
        phrase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_mints_are_accepted_verbatim() {
        for mint in [
            "So11111111111111111111111111111111111111112",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
            "11111111111111111111111111111111",
        ] {
            assert_eq!(parse_mint(mint).as_deref(), Ok(mint), "{mint}");
        }
    }

    #[test]
    fn invalid_mints_are_rejected() {
        assert_eq!(parse_mint(""), Err(ParseError::MissingArgument("mint")));
        assert_eq!(parse_mint("   "), Err(ParseError::MissingArgument("mint")));
        assert_eq!(parse_mint("0OIl"), Err(ParseError::MintNotBase58));
        assert_eq!(parse_mint("not a mint"), Err(ParseError::MintNotBase58));
        // Valid base58 but only 31 bytes.
        let short = bs58::encode([1u8; 31]).into_string();
        assert_eq!(parse_mint(&short), Err(ParseError::MintWrongLength(31)));
        // Valid base58, 33 bytes.
        let long = bs58::encode([1u8; 33]).into_string();
        assert_eq!(parse_mint(&long), Err(ParseError::MintWrongLength(33)));
        // A 32-byte key with a leading zero byte has a non-canonical spelling
        // only when padded; the canonical encoder must still match.
        let zero = bs58::encode([0u8; 32]).into_string();
        assert_eq!(zero, "11111111111111111111111111111111");
        assert_eq!(parse_mint(&zero).as_deref(), Ok(zero.as_str()));
    }

    #[test]
    fn lamport_conversion_is_exact() {
        let table: [(&str, Result<u64, ParseError>); 18] = [
            ("1", Ok(1_000_000_000)),
            ("0.1", Ok(100_000_000)),
            ("0.05", Ok(50_000_000)),
            ("0.000000001", Ok(1)),
            (".5", Ok(500_000_000)),
            ("  0.25  ", Ok(250_000_000)),
            ("123.456789", Ok(123_456_789_000)),
            ("18446744073.709551615", Ok(18_446_744_073_709_551_615)),
            ("0", Err(ParseError::AmountZero)),
            ("0.0", Err(ParseError::AmountZero)),
            ("0.0000000001", Err(ParseError::AmountTooManyDecimals)),
            ("-1", Err(ParseError::AmountNegative)),
            ("-0.5", Err(ParseError::AmountNegative)),
            ("+1", Err(ParseError::AmountNotNumeric)),
            ("1e5", Err(ParseError::AmountNotNumeric)),
            ("NaN", Err(ParseError::AmountNotNumeric)),
            ("", Err(ParseError::AmountEmpty)),
            ("1.", Err(ParseError::AmountNotNumeric)),
        ];
        for (input, expected) in table {
            assert_eq!(parse_sol_lamports(input), expected, "input {input:?}");
        }
        assert_eq!(
            parse_sol_lamports("99999999999999999999999"),
            Err(ParseError::AmountTooLarge)
        );
        assert_eq!(
            parse_sol_lamports("18446744074"),
            Err(ParseError::AmountTooLarge)
        );
        // "inf" is not a decimal: rejected, never treated as a number.
        assert_eq!(parse_sol_lamports("inf"), Err(ParseError::AmountNotNumeric));
    }

    #[test]
    fn percent_conversion_table() {
        let table: [(&str, Result<u16, ParseError>); 14] = [
            ("all", Ok(10_000)),
            ("ALL", Ok(10_000)),
            ("100", Ok(10_000)),
            ("50", Ok(5_000)),
            ("12.5", Ok(1_250)),
            ("0.5", Ok(50)),
            ("0.01", Ok(1)),
            ("33.33", Ok(3_333)),
            ("0", Err(ParseError::PercentOutOfRange)),
            ("0.00", Err(ParseError::PercentOutOfRange)),
            ("100.01", Err(ParseError::PercentOutOfRange)),
            ("101", Err(ParseError::PercentOutOfRange)),
            ("1.234", Err(ParseError::PercentTooManyDecimals)),
            ("", Err(ParseError::PercentEmpty)),
        ];
        for (input, expected) in table {
            assert_eq!(parse_percent_bps(input), expected, "input {input:?}");
        }
        assert_eq!(parse_percent_bps("-5"), Err(ParseError::PercentNotNumeric));
        assert_eq!(
            parse_percent_bps("half"),
            Err(ParseError::PercentNotNumeric)
        );
    }

    #[test]
    fn slippage_window_is_enforced_not_clamped() {
        assert_eq!(parse_slippage_bps("100", 1, 500), Ok(100));
        assert_eq!(parse_slippage_bps("1", 1, 500), Ok(1));
        assert_eq!(parse_slippage_bps("500", 1, 500), Ok(500));
        assert_eq!(
            parse_slippage_bps("0", 1, 500),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
        assert_eq!(
            parse_slippage_bps("501", 1, 500),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
        assert_eq!(
            parse_slippage_bps("10000", 1, 500),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
        assert_eq!(
            parse_slippage_bps("999999999", 1, 500),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
        assert_eq!(
            parse_slippage_bps("2.5", 1, 500),
            Err(ParseError::SlippageNotNumeric)
        );
        assert_eq!(
            parse_slippage_bps("", 1, 500),
            Err(ParseError::MissingArgument("slippage_bps"))
        );
    }

    #[test]
    fn ttl_window_is_enforced() {
        assert_eq!(parse_ttl_hours("24", 1, 720), Ok(24));
        assert_eq!(
            parse_ttl_hours("0", 1, 720),
            Err(ParseError::TtlOutOfRange { min: 1, max: 720 })
        );
        assert_eq!(
            parse_ttl_hours("721", 1, 720),
            Err(ParseError::TtlOutOfRange { min: 1, max: 720 })
        );
        assert_eq!(parse_ttl_hours("x", 1, 720), Err(ParseError::TtlNotNumeric));
        assert_eq!(
            parse_ttl_hours("", 1, 720),
            Err(ParseError::MissingArgument("ttl_hours"))
        );
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn grant_args_take_positional_numbers_then_the_phrase() {
        let bounds = ConsentBounds::default();
        let got = parse_consent_grant(
            &args(&[
                "0.05", "0.2", "24", "I", "CONSENT", "TO", "TELEGRAM", "TRADING",
            ]),
            &bounds,
        )
        .unwrap();
        assert_eq!(got.order_lamports, Some(50_000_000));
        assert_eq!(got.daily_lamports, Some(200_000_000));
        assert_eq!(got.ttl_hours, Some(24));
        assert_eq!(got.phrase, "I CONSENT TO TELEGRAM TRADING");

        let got = parse_consent_grant(
            &args(&["I", "CONSENT", "TO", "TELEGRAM", "TRADING"]),
            &bounds,
        )
        .unwrap();
        assert_eq!(got.order_lamports, None);
        assert_eq!(got.daily_lamports, None);
        assert_eq!(got.ttl_hours, None);
        assert_eq!(got.phrase, "I CONSENT TO TELEGRAM TRADING");

        let got = parse_consent_grant(&args(&["0.1", "I", "CONSENT"]), &bounds).unwrap();
        assert_eq!(got.order_lamports, Some(100_000_000));
        assert_eq!(got.daily_lamports, None);
        assert_eq!(got.phrase, "I CONSENT");
    }

    #[test]
    fn grant_args_reject_bad_numbers_and_missing_phrase() {
        let bounds = ConsentBounds::default();
        assert_eq!(
            parse_consent_grant(&args(&["0.05", "0.2", "0", "I", "CONSENT"]), &bounds),
            Err(ParseError::TtlOutOfRange { min: 1, max: 720 })
        );
        assert_eq!(
            parse_consent_grant(&args(&["0", "I", "CONSENT"]), &bounds),
            Err(ParseError::AmountZero)
        );
        assert_eq!(
            parse_consent_grant(&args(&["0.05", "0.2", "24"]), &bounds),
            Err(ParseError::MissingArgument("phrase"))
        );
        assert_eq!(
            parse_consent_grant(&args(&[]), &bounds),
            Err(ParseError::MissingArgument("phrase"))
        );
    }

    #[test]
    fn every_parse_error_has_a_message_and_a_code() {
        let errors = [
            ParseError::MissingArgument("mint"),
            ParseError::MintNotBase58,
            ParseError::MintWrongLength(31),
            ParseError::AmountEmpty,
            ParseError::AmountNotNumeric,
            ParseError::AmountNegative,
            ParseError::AmountZero,
            ParseError::AmountTooLarge,
            ParseError::AmountTooManyDecimals,
            ParseError::PercentEmpty,
            ParseError::PercentNotNumeric,
            ParseError::PercentTooManyDecimals,
            ParseError::PercentOutOfRange,
            ParseError::SlippageNotNumeric,
            ParseError::SlippageOutOfRange { min: 1, max: 2 },
            ParseError::TtlNotNumeric,
            ParseError::TtlOutOfRange { min: 1, max: 2 },
        ];
        for error in errors {
            assert!(!error.message().is_empty(), "{error:?}");
            assert!(!error.code().is_empty(), "{error:?}");
        }
    }
}
