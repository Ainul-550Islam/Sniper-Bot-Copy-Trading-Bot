//! Live leader-trade feed for the copy engine.
//!
//! `copy.rs` owns the decision logic ([`crate::copy::CopyEngine`]) and
//! `leaders.rs` owns leader statistics, but nothing ever fed real leader
//! trades into either of them: `CopyEngine::evaluate` had **no production call
//! site**, so "copy a wallet" was config-only. This module is the missing
//! source.
//!
//! ## What it reads
//!
//! Polymarket's public data API (`data-api.polymarket.com/trades`), the same
//! source the leaderboard uses. One request per followed wallet returns the
//! most recent fills with `proxyWallet`, `side`, `asset` (CTF token id),
//! `conditionId`, `size`, `price` and `timestamp` — exactly the fields
//! [`LeaderTrade`] needs.
//!
//! ## Honesty rules
//!
//! * A failed or malformed response yields an **error**, never an empty
//!   "no trades" success: the caller logs and retries, and no decision is
//!   invented from missing data.
//! * Parsing is pure and total: a row missing a required field is skipped, and
//!   a skipped row is counted and reported, never silently treated as a fill.
//! * `liquidity_usd` is **not** in this response. It is left at `0.0` rather
//!   than guessed, which makes `CopyEngine`'s own `min_liquidity_usd` gate
//!   reject the trade unless the operator sets that floor to zero — the
//!   conservative direction, and documented in `docs/POLYMARKET-ENGINE.md`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::copy::LeaderTrade;

/// Public data API base URL (no key required).
pub const POLYMARKET_DATA_API: &str = "https://data-api.polymarket.com";

/// How many recent trades to request per wallet per poll.
pub const DEFAULT_TRADE_LIMIT: usize = 50;

/// One parsed page plus what parsing had to drop.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TradePage {
    /// Trades that parsed into a [`LeaderTrade`].
    pub trades: Vec<LeaderTrade>,
    /// Rows the API returned that could not be parsed (missing/invalid field).
    pub skipped: usize,
}

/// Thin client over the Polymarket public data API.
#[derive(Debug, Clone)]
pub struct CopyFeedClient {
    http: reqwest::Client,
    base_url: String,
}

impl Default for CopyFeedClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyFeedClient {
    /// Client against the public data API (`https://data-api.polymarket.com`).
    pub fn new() -> Self {
        Self::with_base_url(POLYMARKET_DATA_API)
    }

    /// Client against an explicit base URL (tests, or a mirror/proxy of the
    /// public data API).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.into(),
        }
    }

    /// The base URL this client reads from.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Recent trades for one wallet, newest first as the API returns them.
    pub async fn recent_trades(&self, wallet: &str, limit: usize) -> Result<TradePage, String> {
        let wallet = wallet.trim();
        if wallet.is_empty() {
            return Err("copy feed: empty wallet".to_string());
        }
        let url = format!("{}/trades", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .get(&url)
            .query(&[
                ("user", wallet.to_string()),
                ("limit", limit.max(1).to_string()),
            ])
            .send()
            .await
            .map_err(|e| format!("copy feed: request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("copy feed: http {}", response.status()));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("copy feed: invalid json: {e}"))?;
        // A non-array body is a contract change, not "no trades".
        let rows = body
            .as_array()
            .ok_or_else(|| "copy feed: response is not an array".to_string())?;
        Ok(parse_trade_rows(rows))
    }
}

/// Pure parser: API rows → [`LeaderTrade`]s, counting what it had to drop.
pub fn parse_trade_rows(rows: &[Value]) -> TradePage {
    let mut page = TradePage::default();
    for row in rows {
        match parse_trade_row(row) {
            Some(trade) => page.trades.push(trade),
            None => page.skipped += 1,
        }
    }
    page
}

fn parse_trade_row(row: &Value) -> Option<LeaderTrade> {
    let wallet = row.get("proxyWallet")?.as_str()?.to_ascii_lowercase();
    let token_id = row.get("asset")?.as_str()?.to_string();
    let condition_id = row.get("conditionId")?.as_str()?.to_string();
    let side = row.get("side")?.as_str()?.to_ascii_uppercase();
    if wallet.is_empty() || token_id.is_empty() || condition_id.is_empty() {
        return None;
    }
    let price = row.get("price")?.as_f64()?;
    let size = row.get("size")?.as_f64()?;
    if !price.is_finite() || !size.is_finite() || size <= 0.0 || !(0.0..=1.0).contains(&price) {
        return None;
    }
    let timestamp = row.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
    let filled_at = Utc
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Utc::now);
    // Stable, content-derived id: the API exposes no trade id here, so the
    // venue's own fields that identify the fill form the dedup key. Re-polling
    // the same page therefore cannot double-count a trade.
    let trade_id = format!("{wallet}:{condition_id}:{token_id}:{timestamp}:{size}:{price}");
    Some(LeaderTrade {
        trade_id,
        wallet,
        condition_id,
        token_id,
        outcome: row
            .get("outcome")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        is_buy: side == "BUY",
        price,
        size_tokens: size,
        stake_usd: price * size,
        neg_risk: row
            .get("negRisk")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        question: row
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        // Not reported by this endpoint — deliberately 0.0, never guessed.
        liquidity_usd: 0.0,
        tick_size: row
            .get("tickSize")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        at: filled_at,
    })
}

/// Shared handle the bot owns.
pub type SharedCopyFeed = Arc<CopyFeedClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_buy_with_the_fields_the_engine_needs() {
        let rows = vec![json!({
            "proxyWallet": "0xAbC0000000000000000000000000000000000001",
            "side": "BUY",
            "asset": "12345",
            "conditionId": "0xdeadbeef",
            "size": 10.0,
            "price": 0.42,
            "timestamp": 1_700_000_000i64,
            "outcome": "Yes",
            "negRisk": true
        })];
        let page = parse_trade_rows(&rows);
        assert_eq!(page.skipped, 0);
        let trade = &page.trades[0];
        assert!(trade.is_buy);
        assert_eq!(trade.price, 0.42);
        assert_eq!(trade.size_tokens, 10.0);
        assert!((trade.stake_usd - 4.2).abs() < 1e-9);
        assert_eq!(trade.wallet, "0xabc0000000000000000000000000000000000001");
        assert_eq!(trade.outcome, "Yes");
        assert!(trade.neg_risk);
        // Never invented: this endpoint does not report liquidity.
        assert_eq!(trade.liquidity_usd, 0.0);
    }

    #[test]
    fn sells_are_parsed_but_flagged_not_buy() {
        let rows = vec![json!({
            "proxyWallet": "0x1", "side": "SELL", "asset": "1", "conditionId": "0xc",
            "size": 1.0, "price": 0.5, "timestamp": 1i64
        })];
        let page = parse_trade_rows(&rows);
        assert_eq!(page.trades.len(), 1);
        assert!(!page.trades[0].is_buy);
    }

    #[test]
    fn malformed_rows_are_dropped_and_counted_not_faked() {
        let rows = vec![
            json!({"proxyWallet": "0x1"}),                // missing half the row
            json!({"proxyWallet": "0x1", "side": "BUY"}), // still missing
            json!({"proxyWallet": "0x1", "side": "BUY", "asset": "1",
                   "conditionId": "0xc", "size": 0.0, "price": 0.5}), // size 0
            json!({"proxyWallet": "0x1", "side": "BUY", "asset": "1",
                   "conditionId": "0xc", "size": 1.0, "price": 7.5}), // price out of range
            json!({"proxyWallet": "0x1", "side": "BUY", "asset": "1",
                   "conditionId": "0xc", "size": 1.0, "price": 0.5}), // valid
        ];
        let page = parse_trade_rows(&rows);
        assert_eq!(page.trades.len(), 1);
        assert_eq!(page.skipped, 4);
    }

    #[test]
    fn trade_id_is_stable_for_the_same_fill() {
        let row = json!({
            "proxyWallet": "0x1", "side": "BUY", "asset": "1", "conditionId": "0xc",
            "size": 2.0, "price": 0.25, "timestamp": 99i64
        });
        let a = parse_trade_rows(std::slice::from_ref(&row));
        let b = parse_trade_rows(std::slice::from_ref(&row));
        assert_eq!(a.trades[0].trade_id, b.trades[0].trade_id);
    }
}
