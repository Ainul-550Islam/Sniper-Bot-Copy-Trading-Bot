//! Polymarket CLOB REST client.
//!
//! Public endpoints (books, prices, markets) need no auth. Order placement,
//! cancellation and the heartbeat need L2 (HMAC) auth, which in turn needs API
//! credentials derived once via L1 (EIP-712) auth. The client holds an optional
//! [`ApiKey`] and signer; authenticated calls fail clearly when they are absent.

use std::collections::HashMap;

use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};

use crate::auth::{l1_headers, l2_headers, ApiKey};
use crate::error::{PolyError, PolyResult};
use crate::orders::{sign_order_bundle, OrderParams, SignedOrderBundle};
use crate::strategy::Quote;

/// One level of the order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookLevel {
    /// Price as a decimal string.
    pub price: String,
    /// Size as a decimal string.
    pub size: String,
}

impl BookLevel {
    /// Parse the price to f64 (0.0 on failure).
    pub fn price_f64(&self) -> f64 {
        self.price.trim().parse::<f64>().unwrap_or(0.0)
    }
    /// Parse the size to f64 (0.0 on failure).
    pub fn size_f64(&self) -> f64 {
        self.size.trim().parse::<f64>().unwrap_or(0.0)
    }
}

/// An order book for one token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    /// Condition id (`0x…` hex) of the market this book belongs to.
    pub market: Option<String>,
    /// CTF outcome token id (decimal uint256 as string) this book quotes.
    pub asset_id: Option<String>,
    /// Bid levels. The API does not guarantee ordering — the best bid is
    /// computed by scanning ([`OrderBook::best_bid`]).
    #[serde(default)]
    pub bids: Vec<BookLevel>,
    /// Ask levels; ordering likewise not guaranteed.
    #[serde(default)]
    pub asks: Vec<BookLevel>,
    /// Opaque book-state hash usable for change detection.
    pub hash: Option<String>,
    /// Server timestamp of the snapshot (ms-since-epoch string).
    pub timestamp: Option<String>,
}

impl OrderBook {
    /// Best (highest) bid, or `None` when the bid side is empty.
    pub fn best_bid(&self) -> Option<f64> {
        self.bids
            .iter()
            .map(|l| l.price_f64())
            .filter(|p| *p > 0.0)
            .fold(None, |acc, p| Some(acc.map_or(p, |a: f64| a.max(p))))
    }
    /// Best (lowest) ask, or `None` when the ask side is empty.
    pub fn best_ask(&self) -> Option<f64> {
        self.asks
            .iter()
            .map(|l| l.price_f64())
            .filter(|p| *p > 0.0)
            .fold(None, |acc, p| Some(acc.map_or(p, |a: f64| a.min(p))))
    }
    /// Reduce the book to a [`Quote`] (empty side -> 0.0).
    pub fn to_quote(&self) -> Quote {
        let bid = self.best_bid().unwrap_or(0.0);
        let ask = self.best_ask().unwrap_or(0.0);
        let mid = match (self.best_bid(), self.best_ask()) {
            (Some(b), Some(a)) => (b + a) / 2.0,
            (Some(b), None) => b,
            (None, Some(a)) => a,
            (None, None) => 0.0,
        };
        Quote {
            best_bid: bid,
            best_ask: ask,
            midpoint: mid,
        }
    }
}

/// A CLOB market (token-level metadata: tick size, neg-risk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobMarket {
    /// Market condition id (`0x…` hex, the CTF conditional token).
    #[serde(default)]
    pub condition_id: Option<String>,
    /// Human-readable market question.
    #[serde(default)]
    pub question: Option<String>,
    /// The outcome tokens (id/outcome/price/winner) comprising this market.
    #[serde(default)]
    pub tokens: Vec<ClobToken>,
    /// Market is live (not deactivated).
    #[serde(default)]
    pub active: Option<bool>,
    /// Market is closed (resolved/finalized).
    #[serde(default)]
    pub closed: Option<bool>,
    /// The CLOB currently accepts orders for this market.
    #[serde(default)]
    pub accepting_orders: Option<bool>,
    /// Minimum price increment for orders (e.g. 0.01 / 0.001).
    #[serde(default)]
    pub minimum_tick_size: Option<f64>,
}

/// One outcome token within a CLOB market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobToken {
    /// CTF ERC-1155 token id (decimal uint256 as string) — the `tokenId`
    /// used in signed orders.
    pub token_id: String,
    /// Outcome label (e.g. "Yes" / "No").
    #[serde(default)]
    pub outcome: Option<String>,
    /// Last known price of this outcome (0..1 USDC).
    #[serde(default)]
    pub price: Option<f64>,
    /// Set after resolution: whether this outcome won.
    #[serde(default)]
    pub winner: Option<bool>,
}

/// Response from `POST /order`.
///
/// The live CLOB returns camelCase keys (`orderID`, `errorMsg`,
/// `takingAmount`, `makingAmount`); aliases accept both wire forms so the
/// parsed id is never silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostOrderResponse {
    /// Accepted order id (`orderID` on the wire) — the handle for
    /// `/data/order/{id}` status polling and cancellation.
    #[serde(default, alias = "orderID", alias = "orderId")]
    pub order_id: Option<String>,
    /// Whether the exchange accepted the order.
    #[serde(default)]
    pub success: Option<bool>,
    /// Rejection reason (`errorMsg`) when not accepted.
    #[serde(default, alias = "errorMsg")]
    pub error_msg: Option<String>,
    /// Exchange status string, e.g. `matched` / `live` / `unmatched`.
    #[serde(default)]
    pub status: Option<String>,
    /// Filled taking amount as a decimal string (raw 6-dec units).
    #[serde(default, alias = "takingAmount")]
    pub taking_amount: Option<String>,
    /// Filled making amount as a decimal string (raw 6-dec units).
    #[serde(default, alias = "makingAmount")]
    pub making_amount: Option<String>,
}

/// The CLOB REST client.
#[derive(Clone)]
pub struct ClobClient {
    base_url: String,
    http: reqwest::Client,
    chain_id: u64,
    address: String,
    api_key: Option<ApiKey>,
}

impl ClobClient {
    /// The chain id this client is bound to (Polygon mainnet = 137).
    /// Order signing happens in the EIP-712 layer with the same value from
    /// config; this getter keeps the binding observable and used.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Create an unauthenticated client (public endpoints only).
    pub fn new(base_url: impl Into<String>, chain_id: u64) -> PolyResult<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| PolyError::http(format!("clob http client: {e}")))?;
        Ok(ClobClient {
            base_url: base_url.into(),
            http,
            chain_id,
            address: String::new(),
            api_key: None,
        })
    }

    /// Attach the signer address and API credentials for authenticated calls.
    pub fn with_auth(mut self, address: impl Into<String>, api_key: ApiKey) -> Self {
        self.address = address.into();
        self.api_key = Some(api_key);
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    // ---- public ----------------------------------------------------------

    /// `GET /time` — server unix timestamp (seconds).
    pub async fn server_time(&self) -> PolyResult<u64> {
        let v: serde_json::Value = self
            .http
            .get(self.url("/time"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // The endpoint returns a bare number (possibly as a string).
        let secs = match &v {
            serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
            serde_json::Value::String(s) => s.trim().parse::<u64>().unwrap_or(0),
            _ => 0,
        };
        Ok(secs)
    }

    /// `GET /book?token_id=...`
    pub async fn order_book(&self, token_id: &str) -> PolyResult<OrderBook> {
        let book: OrderBook = self
            .http
            .get(self.url("/book"))
            .query(&[("token_id", token_id)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(book)
    }

    /// `GET /books` for several tokens at once.
    pub async fn order_books(&self, token_ids: &[String]) -> PolyResult<Vec<OrderBook>> {
        let body: Vec<HashMap<String, String>> = token_ids
            .iter()
            .map(|t| {
                let mut m = HashMap::new();
                m.insert("token_id".to_string(), t.clone());
                m
            })
            .collect();
        let books: Vec<OrderBook> = self
            .http
            .post(self.url("/books"))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(books)
    }

    /// `GET /price?token_id=...&side=buy|sell`
    pub async fn price(&self, token_id: &str, side: &str) -> PolyResult<f64> {
        let v: serde_json::Value = self
            .http
            .get(self.url("/price"))
            .query(&[("token_id", token_id), ("side", side)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(extract_f64(&v, "price"))
    }

    /// `GET /midpoint?token_id=...`
    pub async fn midpoint(&self, token_id: &str) -> PolyResult<f64> {
        let v: serde_json::Value = self
            .http
            .get(self.url("/midpoint"))
            .query(&[("token_id", token_id)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(extract_f64(&v, "mid"))
    }

    /// `GET /markets/...` for a single condition id.
    pub async fn market(&self, condition_id: &str) -> PolyResult<ClobMarket> {
        let m: ClobMarket = self
            .http
            .get(self.url(&format!("/markets/{condition_id}")))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(m)
    }

    /// The tick size for a token, via its market. Falls back to "0.01".
    pub async fn tick_size(&self, condition_id: &str) -> PolyResult<String> {
        let m = self.market(condition_id).await?;
        let tick = m.minimum_tick_size.filter(|t| *t > 0.0).unwrap_or(0.01);
        Ok(format_tick(tick))
    }

    // ---- authenticated (L2) ---------------------------------------------

    fn require_auth(&self) -> PolyResult<&ApiKey> {
        self.api_key
            .as_ref()
            .ok_or_else(|| PolyError::not_configured("CLOB api credentials not set"))
    }

    async fn l2_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> PolyResult<serde_json::Value> {
        let api_key = self.require_auth()?.clone();
        if self.address.is_empty() {
            return Err(PolyError::not_configured("signer address not set"));
        }
        let body_str = match body {
            Some(b) => serde_json::to_string(b)?,
            None => String::new(),
        };
        let timestamp = chrono::Utc::now().timestamp() as u64;
        let headers = l2_headers(&api_key, &self.address, method, path, &body_str, timestamp)?;

        let mut req = self.http.request(
            match method {
                "GET" => reqwest::Method::GET,
                "DELETE" => reqwest::Method::DELETE,
                _ => reqwest::Method::POST,
            },
            self.url(path),
        );
        for (k, v) in headers {
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.json().await?)
    }

    /// `POST /order` — submit a signed order bundle.
    pub async fn post_order(
        &self,
        bundle: &SignedOrderBundle,
        order_type: &str,
    ) -> PolyResult<PostOrderResponse> {
        let order_json = serde_json::json!({
            "salt": bundle.order.salt,
            "maker": bundle.order.maker,
            "signer": bundle.order.signer,
            "tokenId": bundle.order.token_id,
            "makerAmount": bundle.order.maker_amount,
            "takerAmount": bundle.order.taker_amount,
            "side": if bundle.order.side == 0 { "BUY" } else { "SELL" },
            "signatureType": bundle.order.signature_type,
            "timestamp": bundle.order.timestamp,
            "metadata": zero_if_empty(&bundle.order.metadata),
            "builder": zero_if_empty(&bundle.order.builder),
            "signature": bundle.signature,
        });
        let body = serde_json::json!({
            "order": order_json,
            "owner": self.require_auth()?.key,
            "orderType": order_type,
        });
        let v = self.l2_request("POST", "/order", Some(&body)).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// `GET /data/order?order_id=…` — provider-side truth about one order.
    /// Used by reconciliation after restarts/ambiguous submits. Requires L2
    /// auth (same as every /data owner endpoint).
    pub async fn order_status(&self, order_id: &str) -> PolyResult<serde_json::Value> {
        self.l2_request("GET", &format!("/data/order?order_id={order_id}"), None)
            .await
    }

    /// `DELETE /order` — cancel one order by id.
    pub async fn cancel_order(&self, order_id: &str) -> PolyResult<serde_json::Value> {
        let body = serde_json::json!({ "orderID": order_id });
        self.l2_request("DELETE", "/order", Some(&body)).await
    }

    /// `DELETE /cancel-all` — cancel every open order.
    pub async fn cancel_all(&self) -> PolyResult<serde_json::Value> {
        self.l2_request("DELETE", "/cancel-all", None).await
    }

    /// `POST /heartbeat` — dead-man's switch; the CLOB cancels our resting
    /// orders if it stops arriving.
    pub async fn heartbeat(&self) -> PolyResult<serde_json::Value> {
        self.l2_request("POST", "/heartbeat", None).await
    }

    /// Derive (or create) API credentials from a signing key via L1 auth.
    pub async fn derive_api_key(
        base_url: &str,
        chain_id: u64,
        key: &SigningKey,
        address: &str,
    ) -> PolyResult<ApiKey> {
        let http = reqwest::Client::new();
        let timestamp = chrono::Utc::now().timestamp() as u64;
        let headers = l1_headers(key, address, timestamp, chain_id)?;
        let url = format!("{}/auth/derive-api-key", base_url.trim_end_matches('/'));
        let mut req = http.post(&url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await?.error_for_status()?;
        let creds: ApiKey = resp.json().await?;
        Ok(creds)
    }
}

/// Pull an f64 out of a CLOB price response (`{"price":"0.42"}` or a number).
fn extract_f64(v: &serde_json::Value, key: &str) -> f64 {
    let field = v.get(key).unwrap_or(v);
    match field {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        serde_json::Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Map a numeric minimum tick to the CLOB's tick-size string.
fn format_tick(tick: f64) -> String {
    if (tick - 0.1).abs() < 1e-9 {
        "0.1".into()
    } else if (tick - 0.001).abs() < 1e-9 {
        "0.001".into()
    } else if (tick - 0.0001).abs() < 1e-9 {
        "0.0001".into()
    } else {
        "0.01".into()
    }
}

/// The CLOB requires explicit `0x0..0` (64 zeros) for empty bytes32 fields.
fn zero_if_empty(s: &str) -> String {
    if s.trim().is_empty() {
        format!("0x{}", "0".repeat(64))
    } else if s.starts_with("0x") {
        s.to_string()
    } else {
        format!("0x{s}")
    }
}

/// Convenience: build + sign an order (re-exported so callers need only ClobClient).
pub fn build_signed_order(key: &SigningKey, params: &OrderParams) -> PolyResult<SignedOrderBundle> {
    sign_order_bundle(key, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBook {
        OrderBook {
            market: None,
            asset_id: None,
            bids: bids
                .iter()
                .map(|(p, s)| BookLevel {
                    price: p.to_string(),
                    size: s.to_string(),
                })
                .collect(),
            asks: asks
                .iter()
                .map(|(p, s)| BookLevel {
                    price: p.to_string(),
                    size: s.to_string(),
                })
                .collect(),
            hash: None,
            timestamp: None,
        }
    }

    #[test]
    fn best_bid_is_highest() {
        let b = book(
            &[("0.40", "10"), ("0.42", "5"), ("0.38", "20")],
            &[("0.45", "1")],
        );
        assert_eq!(b.best_bid(), Some(0.42));
    }

    #[test]
    fn best_ask_is_lowest() {
        let b = book(
            &[("0.40", "10")],
            &[("0.47", "5"), ("0.45", "1"), ("0.50", "2")],
        );
        assert_eq!(b.best_ask(), Some(0.45));
    }

    #[test]
    fn empty_side_is_none() {
        let b = book(&[], &[("0.45", "1")]);
        assert_eq!(b.best_bid(), None);
        assert_eq!(b.best_ask(), Some(0.45));
        let q = b.to_quote();
        assert_eq!(q.best_bid, 0.0);
        assert!((q.midpoint - 0.45).abs() < 1e-9);
    }

    #[test]
    fn to_quote_computes_midpoint() {
        let b = book(&[("0.40", "10")], &[("0.50", "10")]);
        let q = b.to_quote();
        assert!((q.best_bid - 0.40).abs() < 1e-9);
        assert!((q.best_ask - 0.50).abs() < 1e-9);
        assert!((q.midpoint - 0.45).abs() < 1e-9);
    }

    #[test]
    fn extract_f64_handles_string_and_number() {
        let v: serde_json::Value = serde_json::from_str(r#"{"price":"0.42"}"#).unwrap();
        assert!((extract_f64(&v, "price") - 0.42).abs() < 1e-9);
        let v2: serde_json::Value = serde_json::from_str(r#"{"mid":0.5}"#).unwrap();
        assert!((extract_f64(&v2, "mid") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn format_tick_maps_known_values() {
        assert_eq!(format_tick(0.1), "0.1");
        assert_eq!(format_tick(0.01), "0.01");
        assert_eq!(format_tick(0.001), "0.001");
        assert_eq!(format_tick(0.0001), "0.0001");
        assert_eq!(format_tick(0.02), "0.01"); // unknown -> default
    }

    #[test]
    fn zero_if_empty_pads_bytes32() {
        let z = zero_if_empty("");
        assert_eq!(z.len(), 2 + 64);
        assert!(z.starts_with("0x"));
        assert_eq!(zero_if_empty("0xabcd"), "0xabcd");
        assert_eq!(zero_if_empty("abcd"), "0xabcd");
    }
}
