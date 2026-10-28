//! `/buy <mint> <amount_sol> [slippage_bps]`.
//!
//! This file owns the **buy-specific** half of the flow: the exact argument
//! grammar and the validation that turns chat text into a [`BuyOrder`]. The
//! flow itself (link → consent → caps → mode/kill → nonce → audit) lives in
//! [`crate::commands::TradeConsole::submit`], which is shared with `/sell` so
//! the two commands can never drift apart.

use chrono::{DateTime, Utc};

use crate::commands::limits::TradeLimits;
use crate::commands::parse::{self, ParseError};
use crate::commands::reply::Reply;
use crate::commands::TradeConsole;

/// A validated buy request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyOrder {
    /// Canonical base58 mint.
    pub mint: String,
    /// SOL to spend, in lamports (always > 0).
    pub amount_lamports: u64,
    /// Slippage tolerance in basis points, inside the configured window.
    pub slippage_bps: u16,
}

/// Validate the three arguments of `/buy`.
///
/// Order of checks is deliberate: the mint (cheapest, most common typo) first,
/// then the amount, then slippage. A rejection is always typed — nothing is
/// clamped and nothing falls back to a default.
pub fn parse_buy_args(
    mint: &str,
    sol: &str,
    slippage: Option<&str>,
    limits: &TradeLimits,
) -> Result<BuyOrder, ParseError> {
    let mint = parse::parse_mint(mint)?;
    if sol.trim().is_empty() {
        return Err(ParseError::MissingArgument("amount_sol"));
    }
    let amount_lamports = parse::parse_sol_lamports(sol)?;
    let slippage_bps = match slippage {
        Some(raw) if !raw.trim().is_empty() => {
            parse::parse_slippage_bps(raw, limits.slippage.min_bps, limits.slippage.max_bps)?
        }
        _ => parse::DEFAULT_SLIPPAGE_BPS,
    };
    Ok(BuyOrder {
        mint,
        amount_lamports,
        slippage_bps,
    })
}

impl TradeConsole {
    /// Handle `/buy`. Returns the reply for the chat (never a silent no-op).
    pub(crate) async fn handle_buy(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        mint: &str,
        sol: &str,
        slippage: Option<&str>,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            // A message with no `from` cannot be attributed to a linked user.
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        self.submit_buy(chat_id, user_id, mint, sol, slippage, now)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> TradeLimits {
        TradeLimits::default()
    }

    #[test]
    fn accepts_a_minimal_buy() {
        let order = parse_buy_args(
            "So11111111111111111111111111111111111111112",
            "0.05",
            None,
            &limits(),
        )
        .unwrap();
        assert_eq!(order.amount_lamports, 50_000_000);
        assert_eq!(order.slippage_bps, parse::DEFAULT_SLIPPAGE_BPS);
        assert_eq!(order.mint, "So11111111111111111111111111111111111111112");
    }

    #[test]
    fn accepts_explicit_slippage_inside_the_window() {
        let order = parse_buy_args(
            "So11111111111111111111111111111111111111112",
            "1",
            Some("250"),
            &limits(),
        )
        .unwrap();
        assert_eq!(order.amount_lamports, 1_000_000_000);
        assert_eq!(order.slippage_bps, 250);
        // An empty slippage token falls back to the default, not to "no limit".
        let order = parse_buy_args(
            "So11111111111111111111111111111111111111112",
            "1",
            Some("  "),
            &limits(),
        )
        .unwrap();
        assert_eq!(order.slippage_bps, parse::DEFAULT_SLIPPAGE_BPS);
    }

    #[test]
    fn rejects_bad_arguments() {
        let mint = "So11111111111111111111111111111111111111112";
        assert_eq!(
            parse_buy_args("", "1", None, &limits()),
            Err(ParseError::MissingArgument("mint"))
        );
        assert_eq!(
            parse_buy_args(mint, "", None, &limits()),
            Err(ParseError::MissingArgument("amount_sol"))
        );
        assert_eq!(
            parse_buy_args(mint, "0", None, &limits()),
            Err(ParseError::AmountZero)
        );
        assert_eq!(
            parse_buy_args(mint, "-1", None, &limits()),
            Err(ParseError::AmountNegative)
        );
        assert_eq!(
            parse_buy_args(mint, "1", Some("0"), &limits()),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
        assert_eq!(
            parse_buy_args(mint, "1", Some("501"), &limits()),
            Err(ParseError::SlippageOutOfRange { min: 1, max: 500 })
        );
        // "nope" is valid base58 (3 bytes): the length check catches it.
        assert_eq!(
            parse_buy_args("nope", "1", None, &limits()),
            Err(ParseError::MintWrongLength(3))
        );
        // Characters outside the base58 alphabet are caught first.
        assert_eq!(
            parse_buy_args("0OIl", "1", None, &limits()),
            Err(ParseError::MintNotBase58)
        );
    }

    #[test]
    fn slippage_window_comes_from_the_limits() {
        let tight = TradeLimits {
            slippage: crate::commands::limits::SlippageBounds {
                min_bps: 10,
                max_bps: 20,
            },
            ..TradeLimits::default()
        };
        let mint = "So11111111111111111111111111111111111111112";
        assert!(parse_buy_args(mint, "1", Some("15"), &tight).is_ok());
        assert_eq!(
            parse_buy_args(mint, "1", Some("5"), &tight),
            Err(ParseError::SlippageOutOfRange { min: 10, max: 20 })
        );
        assert_eq!(
            parse_buy_args(mint, "1", Some("21"), &tight),
            Err(ParseError::SlippageOutOfRange { min: 10, max: 20 })
        );
    }
}
