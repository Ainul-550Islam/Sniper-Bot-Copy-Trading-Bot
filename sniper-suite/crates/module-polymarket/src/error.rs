//! Errors for the Polymarket module.
//!
//! Kept separate from `bot_core::error::BotError` because the Polymarket client
//! has failure modes (EIP-712 encoding, CLOB auth, tick-size rounding) that do
//! not map cleanly onto the Solana-centric core errors. A `From<PolyError> for
//! BotError` lets module code bubble up into the shared error channel.

use thiserror::Error;

/// Polymarket module result.
pub type PolyResult<T> = Result<T, PolyError>;

/// Polymarket module error.
#[derive(Debug, Error)]
pub enum PolyError {
    /// A configuration or input value was malformed.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// An HTTP/transport failure talking to Gamma, CLOB or the data API.
    #[error("http error: {0}")]
    Http(String),
    /// The CLOB returned a non-success or an error payload.
    #[error("clob error: {0}")]
    Clob(String),
    /// The POST ended in a transport failure: the signed order MAY or may
    /// not be resting on the book. Carries the locally derived CLOB order id
    /// so reconciliation can query the venue for the truth.
    #[error("submit unknown (derived order {order_id}): {reason}")]
    SubmitUnknown {
        /// Locally derived CLOB order id (`0x`-hex EIP-712 struct hash).
        order_id: String,
        /// Underlying transport failure description.
        reason: String,
    },
    /// A JSON (de)serialization failure.
    #[error("encoding error: {0}")]
    Encoding(String),
    /// Signing / key material problem.
    #[error("signing error: {0}")]
    Signing(String),
    /// The module is not configured (no key, disabled, etc.).
    #[error("not configured: {0}")]
    NotConfigured(String),
    /// A websocket failure.
    #[error("websocket error: {0}")]
    Ws(String),
    /// The real collateral balance/decimals needed for LIVE sizing could not
    /// be verified (reader not configured, RPC unreadable, implausible
    /// decimals, …). Live entries MUST be rejected on this error — it is
    /// never a licence to fall back to a cached or paper balance.
    #[error("balance unavailable: {0}")]
    BalanceUnavailable(String),
    /// The funder's on-chain collateral allowance for the exchange contract
    /// (or the balance itself) does not cover the order the risk engine
    /// approved. Live orders MUST be rejected until the operator funds or
    /// approves the wallet.
    #[error("insufficient collateral funding: {0}")]
    InsufficientFunding(String),
    /// An order-lifecycle invariant was violated (illegal transition,
    /// matched size above the order size, unknown venue order …). Surfaced
    /// instead of silently coercing state.
    #[error("order lifecycle error: {0}")]
    Lifecycle(String),
    /// The durable Polymarket journal (migration 0014) refused a write or a
    /// read. Journal failures never fail a trade; callers meter and log.
    #[error("journal error: {0}")]
    Journal(String),
}

impl PolyError {
    /// Malformed input.
    pub fn invalid(msg: impl Into<String>) -> Self {
        PolyError::Invalid(msg.into())
    }
    /// Transport failure.
    pub fn http(msg: impl Into<String>) -> Self {
        PolyError::Http(msg.into())
    }
    /// CLOB-level failure.
    pub fn clob(msg: impl Into<String>) -> Self {
        PolyError::Clob(msg.into())
    }
    /// JSON failure.
    pub fn encoding(msg: impl Into<String>) -> Self {
        PolyError::Encoding(msg.into())
    }
    /// Signing failure.
    pub fn signing(msg: impl Into<String>) -> Self {
        PolyError::Signing(msg.into())
    }
    /// Missing configuration.
    pub fn not_configured(msg: impl Into<String>) -> Self {
        PolyError::NotConfigured(msg.into())
    }
    /// Websocket failure.
    pub fn ws(msg: impl Into<String>) -> Self {
        PolyError::Ws(msg.into())
    }
    /// Live sizing balance could not be verified.
    pub fn balance_unavailable(msg: impl Into<String>) -> Self {
        PolyError::BalanceUnavailable(msg.into())
    }
    /// On-chain funding/allowance does not cover the approved order.
    pub fn insufficient_funding(msg: impl Into<String>) -> Self {
        PolyError::InsufficientFunding(msg.into())
    }
    /// Order-lifecycle invariant violated.
    pub fn lifecycle(msg: impl Into<String>) -> Self {
        PolyError::Lifecycle(msg.into())
    }
    /// Durable journal failure.
    pub fn journal(msg: impl Into<String>) -> Self {
        PolyError::Journal(msg.into())
    }
    /// True when the failure means "the venue may still hold the order"
    /// (transport failure or an ambiguous submit) — the caller must hand off
    /// to reconciliation instead of treating the order as rejected.
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, PolyError::Http(_) | PolyError::SubmitUnknown { .. })
    }
}

impl From<reqwest::Error> for PolyError {
    fn from(e: reqwest::Error) -> Self {
        PolyError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for PolyError {
    fn from(e: serde_json::Error) -> Self {
        PolyError::Encoding(e.to_string())
    }
}

impl From<hex::FromHexError> for PolyError {
    fn from(e: hex::FromHexError) -> Self {
        PolyError::Encoding(format!("hex: {e}"))
    }
}

impl From<bot_core::error::BotError> for PolyError {
    fn from(e: bot_core::error::BotError) -> Self {
        PolyError::Clob(e.to_string())
    }
}

impl From<PolyError> for bot_core::error::BotError {
    fn from(e: PolyError) -> Self {
        match e {
            PolyError::Invalid(m) => bot_core::error::BotError::invalid(m),
            PolyError::Http(m) => bot_core::error::BotError::http(m),
            PolyError::Encoding(m) => bot_core::error::BotError::encoding(m),
            PolyError::Signing(m) => bot_core::error::BotError::other(m),
            PolyError::NotConfigured(m) => bot_core::error::BotError::config(m),
            PolyError::Ws(m) => bot_core::error::BotError::ws(m),
            PolyError::Clob(m) => bot_core::error::BotError::other(m),
            PolyError::SubmitUnknown { order_id, reason } => bot_core::error::BotError::other(
                format!("polymarket submit unknown (order {order_id}): {reason}"),
            ),
            PolyError::BalanceUnavailable(m) => {
                bot_core::error::BotError::other(format!("polymarket balance unavailable: {m}"))
            }
            PolyError::InsufficientFunding(m) => {
                bot_core::error::BotError::other(format!("polymarket funding: {m}"))
            }
            PolyError::Lifecycle(m) => {
                bot_core::error::BotError::other(format!("polymarket order lifecycle: {m}"))
            }
            PolyError::Journal(m) => {
                bot_core::error::BotError::db(format!("polymarket journal: {m}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguity_classification_covers_transport_and_submit_unknown() {
        assert!(PolyError::http("timeout").is_ambiguous());
        assert!(PolyError::SubmitUnknown {
            order_id: "0xabc".into(),
            reason: "reset".into()
        }
        .is_ambiguous());
        for definite in [
            PolyError::clob("rejected"),
            PolyError::invalid("x"),
            PolyError::lifecycle("bad transition"),
            PolyError::journal("db down"),
            PolyError::insufficient_funding("allowance"),
            PolyError::balance_unavailable("rpc"),
            PolyError::not_configured("key"),
        ] {
            assert!(!definite.is_ambiguous(), "{definite}");
        }
    }

    #[test]
    fn lifecycle_and_journal_errors_map_into_core_errors() {
        let e: bot_core::error::BotError = PolyError::lifecycle("filled -> resting").into();
        assert!(e.to_string().contains("order lifecycle"));
        let e: bot_core::error::BotError = PolyError::journal("pool exhausted").into();
        assert!(e.to_string().contains("journal"));
        assert_eq!(
            PolyError::lifecycle("x").to_string(),
            "order lifecycle error: x"
        );
    }
}
