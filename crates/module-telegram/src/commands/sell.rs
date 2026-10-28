//! `/sell <mint> <percent|all> [slippage_bps]`.
//!
//! Mirror of [`crate::commands::buy`] for the exit side: it owns the
//! sell-specific grammar (`percent` or `all`, converted to basis points) and
//! hands the validated order to the shared
//! [`crate::commands::TradeConsole::submit`] flow.
//!
//! A sell is expressed as a fraction of the position, never as a SOL amount, so
//! the rolling SOL cap does not apply to it — the per-order cap, the rate limit
//! and the consent scope do.

use chrono::{DateTime, Utc};

use crate::commands::limits::TradeLimits;
use crate::commands::parse::{self, ParseError};
use crate::commands::reply::Reply;
use crate::commands::TradeConsole;

/// A validated sell request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellOrder {
    /// Canonical base58 mint.
    pub mint: String,
    /// Fraction of the position to sell, in basis points (`1..=10000`).
    pub percent_bps: u16,
    /// Slippage tolerance in basis points, inside the configured window.
    pub slippage_bps: u16,
}

/// `true` when the request asks to liquidate the whole position.
pub fn is_full_exit(percent_bps: u16) -> bool {
    percent_bps == parse::PERCENT_ALL_BPS
}

/// Validate the three arguments of `/sell`.
pub fn parse_sell_args(
    mint: &str,
    percent: &str,
    slippage: Option<&str>,
    limits: &TradeLimits,
) -> Result<SellOrder, ParseError> {
    let mint = parse::parse_mint(mint)?;
    if percent.trim().is_empty() {
        return Err(ParseError::MissingArgument("percent"));
    }
    let percent_bps = parse::parse_percent_bps(percent)?;
    let slippage_bps = match slippage {
        Some(raw) if !raw.trim().is_empty() => {
            parse::parse_slippage_bps(raw, limits.slippage.min_bps, limits.slippage.max_bps)?
        }
        _ => parse::DEFAULT_SLIPPAGE_BPS,
    };
    Ok(SellOrder {
        mint,
        percent_bps,
        slippage_bps,
    })
}

impl TradeConsole {
    /// Handle `/sell`.
    pub(crate) async fn handle_sell(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        mint: &str,
        percent: &str,
        slippage: Option<&str>,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        self.submit_sell(chat_id, user_id, mint, percent, slippage, now)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> TradeLimits {
        TradeLimits::default()
    }

    const MINT: &str = "So11111111111111111111111111111111111111112";

    #[test]
    fn accepts_percent_and_all() {
        let half = parse_sell_args(MINT, "50", None, &limits()).unwrap();
        assert_eq!(half.percent_bps, 5_000);
        assert!(!is_full_exit(half.percent_bps));
        let all = parse_sell_args(MINT, "all", None, &limits()).unwrap();
        assert_eq!(all.percent_bps, 10_000);
        assert!(is_full_exit(all.percent_bps));
        let frac = parse_sell_args(MINT, "12.5", None, &limits()).unwrap();
        assert_eq!(frac.percent_bps, 1_250);
        assert_eq!(frac.slippage_bps, parse::DEFAULT_SLIPPAGE_BPS);
    }

    #[test]
    fn rejects_out_of_range_and_malformed_percentages() {
        assert_eq!(
            parse_sell_args("", "50", None, &limits()),
            Err(ParseError::MissingArgument("mint"))
        );
        assert_eq!(
            parse_sell_args(MINT, "", None, &limits()),
            Err(ParseError::MissingArgument("percent"))
        );
        assert_eq!(
            parse_sell_args(MINT, "0", None, &limits()),
            Err(ParseError::PercentOutOfRange)
        );
        assert_eq!(
            parse_sell_args(MINT, "100.01", None, &limits()),
            Err(ParseError::PercentOutOfRange)
        );
        assert_eq!(
            parse_sell_args(MINT, "12.345", None, &limits()),
            Err(ParseError::PercentTooManyDecimals)
        );
        assert_eq!(
            parse_sell_args(MINT, "some", None, &limits()),
            Err(ParseError::PercentNotNumeric)
        );
        assert_eq!(
            parse_sell_args(MINT, "50", Some("0"), &limits()),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
    }
}
