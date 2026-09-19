//! On-chain collateral (ERC-20) reads on Polygon — live sizing truth.
//!
//! The CTF client ([`crate::ctf`]) answers "how many OUTCOME tokens does the
//! funder hold" (settlement truth). This client answers the other money
//! question: "how much COLLATERAL (USDC/pUSD) can the funder actually spend",
//! plus the ERC-20 `allowance` the CLOB exchange needs to pull it.
//!
//! Live-mode rules (enforced by the caller in `lib.rs`):
//!
//! * a live order's sizing balance MUST come from a real on-chain read of the
//!   configured `[polymarket].collateral_address` — never from a cached demo
//!   seed and never from the paper figure;
//! * `Err` means "could not verify" and the caller MUST reject the order —
//!   it is never interpreted as a zero or fallback balance;
//! * `decimals()` is read from the chain and validated before conversion, so a
//!   mis-configured token address cannot silently rescale the balance.
//!
//! Like the CTF client, this module knows nothing about orders or strategy
//! state; it performs `eth_call`s and surfaces transport/protocol errors.

use std::time::Duration;

use crate::ctf::{decode_uint256, is_hex_address, pad32_address};
use crate::error::{PolyError, PolyResult};

/// `keccak256("balanceOf(address)")[0..4]` — the ERC-20 balance selector.
pub const ERC20_BALANCE_OF_SELECTOR: &str = "70a08231";
/// `keccak256("decimals()")[0..4]` — the ERC-20 decimals selector.
pub const ERC20_DECIMALS_SELECTOR: &str = "313ce567";
/// `keccak256("allowance(address,address)")[0..4]` — the ERC-20 allowance
/// selector.
pub const ERC20_ALLOWANCE_SELECTOR: &str = "dd62ed3e";

/// The verified Polymarket collateral (pUSD proxy) on Polygon mainnet.
/// Mirrors `[polymarket].collateral_address` in config; kept here for tests
/// and as the documented reference value.
pub const COLLATERAL_ADDRESS_POLYGON: &str = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB";

/// Minimal Polygon JSON-RPC reader for ERC-20 collateral state.
#[derive(Clone)]
pub struct CollateralClient {
    rpc_url: String,
    collateral_address: String,
    http: reqwest::Client,
}

impl CollateralClient {
    /// `rpc_url` — any Polygon JSON-RPC endpoint (`eth_call` support is the
    /// only requirement). `collateral_address` — the ERC-20 the CLOB settles
    /// in (0x + 40 hex). An empty `rpc_url` means "not configured":
    /// construction fails with `not_configured`, and live sizing then REJECTS
    /// orders instead of guessing a balance.
    pub fn new(
        rpc_url: impl Into<String>,
        collateral_address: impl Into<String>,
    ) -> PolyResult<Self> {
        let rpc_url = rpc_url.into();
        let collateral_address = collateral_address.into();
        if rpc_url.trim().is_empty() {
            return Err(PolyError::not_configured("collateral rpc url is empty"));
        }
        if !is_hex_address(&collateral_address) {
            return Err(PolyError::invalid(format!(
                "collateral address is not 0x+40 hex: {collateral_address}"
            )));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| PolyError::http(format!("collateral http client: {e}")))?;
        Ok(Self {
            rpc_url,
            collateral_address,
            http,
        })
    }

    /// The collateral token address this client reads (identity check for
    /// callers: every read below targets exactly this contract).
    pub fn address(&self) -> &str {
        &self.collateral_address
    }

    /// One `eth_call` against the collateral contract at `latest`, returning
    /// the raw `0x`-hex result word.
    async fn eth_call(&self, data: &str) -> PolyResult<String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": self.collateral_address, "data": data }, "latest"],
        });
        let resp = self
            .http
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PolyError::http(format!("collateral rpc send: {e}")))?;
        let status = resp.status();
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| PolyError::http(format!("collateral rpc decode: {e}")))?;
        if !status.is_success() {
            return Err(PolyError::http(format!("collateral rpc status {status}")));
        }
        if let Some(err) = v.get("error") {
            return Err(PolyError::http(format!("collateral rpc error: {err}")));
        }
        let result = v
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| PolyError::http("collateral rpc response has no result"))?;
        Ok(result.to_string())
    }

    /// ERC-20 `balanceOf(owner)` in raw token units. `Err` = could not read;
    /// callers must never treat that as a zero balance.
    pub async fn balance_of(&self, owner: &str) -> PolyResult<u128> {
        if !is_hex_address(owner) {
            return Err(PolyError::invalid(format!(
                "owner is not 0x+40 hex: {owner}"
            )));
        }
        let data = format!("0x{ERC20_BALANCE_OF_SELECTOR}{}", pad32_address(owner)?);
        decode_uint256(&self.eth_call(&data).await?)
    }

    /// ERC-20 `decimals()`. Validated to a sane range here (1..=18 would be
    /// policy; the chain value itself just has to fit a `u8`), so unit
    /// conversion can never silently rescale by a wrong power of ten.
    pub async fn decimals(&self) -> PolyResult<u8> {
        let data = format!("0x{ERC20_DECIMALS_SELECTOR}");
        let v = decode_uint256(&self.eth_call(&data).await?)?;
        u8::try_from(v)
            .map_err(|_| PolyError::http(format!("collateral decimals out of u8 range: {v}")))
    }

    /// ERC-20 `allowance(owner, spender)` in raw token units — what `spender`
    /// (the CLOB exchange contract) may pull from `owner`.
    pub async fn allowance(&self, owner: &str, spender: &str) -> PolyResult<u128> {
        if !is_hex_address(owner) {
            return Err(PolyError::invalid(format!(
                "owner is not 0x+40 hex: {owner}"
            )));
        }
        if !is_hex_address(spender) {
            return Err(PolyError::invalid(format!(
                "spender is not 0x+40 hex: {spender}"
            )));
        }
        let data = format!(
            "0x{ERC20_ALLOWANCE_SELECTOR}{}{}",
            pad32_address(owner)?,
            pad32_address(spender)?
        );
        decode_uint256(&self.eth_call(&data).await?)
    }
}

/// Raw token units → human USD amount using the ON-CHAIN decimals.
/// `raw / 10^decimals` computed in f64; inputs this large are far inside f64's
/// exact-integer range for realistic balances.
pub fn raw_to_usd(raw: u128, decimals: u8) -> f64 {
    let scale = 10f64.powi(i32::from(decimals));
    (raw as f64) / scale
}

/// Human USD amount → raw token units (floor), for allowance/coverage checks.
/// Rejects non-finite or negative inputs instead of saturating them, so a
/// corrupt sizing value can never turn into a bogus order.
pub fn usd_to_raw(usd: f64, decimals: u8) -> PolyResult<u128> {
    if !usd.is_finite() || usd < 0.0 {
        return Err(PolyError::invalid(format!(
            "usd amount is not a finite non-negative number: {usd}"
        )));
    }
    let scale = 10f64.powi(i32::from(decimals));
    let raw = (usd * scale).floor();
    if raw > u128::MAX as f64 {
        return Err(PolyError::invalid(format!(
            "usd amount overflows u128 raw units: {usd}"
        )));
    }
    Ok(raw as u128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ---- pure conversion helpers --------------------------------------------

    #[test]
    fn raw_to_usd_uses_the_given_decimals() {
        // 6 decimals (USDC/pUSD): 1_500_000 raw == 1.5 USD.
        assert!((raw_to_usd(1_500_000, 6) - 1.5).abs() < 1e-12);
        assert!((raw_to_usd(0, 6)).abs() < 1e-12);
        // 18 decimals: 10^18 raw == 1.0.
        assert!((raw_to_usd(1_000_000_000_000_000_000, 18) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn usd_to_raw_floors_and_rejects_nonsense() {
        assert_eq!(usd_to_raw(1.5, 6).unwrap(), 1_500_000);
        // Floor, never round-up: 0.0000009 USD at 6 decimals is 0 raw.
        assert_eq!(usd_to_raw(0.000_000_9, 6).unwrap(), 0);
        assert_eq!(usd_to_raw(0.0, 6).unwrap(), 0);
        assert!(usd_to_raw(-1.0, 6).is_err());
        assert!(usd_to_raw(f64::NAN, 6).is_err());
        assert!(usd_to_raw(f64::INFINITY, 6).is_err());
        // f64::MAX * 10^6 exceeds u128 -> error, not a wrapped value.
        assert!(usd_to_raw(f64::MAX, 6).is_err());
    }

    #[test]
    fn usd_to_raw_round_trips_with_raw_to_usd() {
        for usd in [0.0, 0.01, 5.0, 123.456_789, 100_000.0] {
            let raw = usd_to_raw(usd, 6).unwrap();
            let back = raw_to_usd(raw, 6);
            // Floor loses at most one raw unit (< 1e-6 USD at 6 decimals).
            assert!(
                (back - usd).abs() < 1e-6,
                "round trip drifted: {usd} -> {raw} -> {back}"
            );
        }
    }

    // ---- client construction --------------------------------------------------

    #[test]
    fn construction_validates_inputs() {
        assert!(CollateralClient::new("", COLLATERAL_ADDRESS_POLYGON).is_err());
        assert!(CollateralClient::new("https://polygon-rpc.com", "").is_err());
        assert!(CollateralClient::new("https://polygon-rpc.com", "0x123").is_err());
        let c = CollateralClient::new("https://polygon-rpc.com", COLLATERAL_ADDRESS_POLYGON)
            .expect("valid inputs construct");
        assert_eq!(c.address(), COLLATERAL_ADDRESS_POLYGON);
    }

    #[test]
    fn address_validation_happens_before_any_network_call() {
        let c = CollateralClient::new("http://127.0.0.1:1/", COLLATERAL_ADDRESS_POLYGON).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            assert!(c.balance_of("not-an-address").await.is_err());
            assert!(c
                .allowance("0x1234", COLLATERAL_ADDRESS_POLYGON)
                .await
                .is_err());
            assert!(c
                .allowance(COLLATERAL_ADDRESS_POLYGON, "nope")
                .await
                .is_err());
        });
    }

    // ---- wire behaviour against a mock JSON-RPC endpoint ----------------------

    async fn mock_rpc(response: serde_json::Value) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
        use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
        let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let state = (seen.clone(), response);
        async fn handler(
            State((seen, response)): State<(Arc<Mutex<Vec<serde_json::Value>>>, serde_json::Value)>,
            body: Json<serde_json::Value>,
        ) -> (StatusCode, Json<serde_json::Value>) {
            seen.lock().unwrap().push(body.0);
            (StatusCode::OK, Json(response))
        }
        let app = Router::new().route("/", post(handler)).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Yield once so the server task is polling before the first request.
        tokio::task::yield_now().await;
        (format!("http://{addr}/"), seen)
    }

    fn word(v: u128) -> String {
        format!("0x{:064x}", v)
    }

    #[tokio::test]
    async fn balance_of_encodes_call_and_decodes_result() {
        let (url, seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": word(2_500_000)
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        let owner = "0x1111111111111111111111111111111111111111";
        let raw = c.balance_of(owner).await.expect("balance read");
        assert_eq!(raw, 2_500_000);
        assert!((raw_to_usd(raw, 6) - 2.5).abs() < 1e-12);

        let req = &seen.lock().unwrap()[0];
        assert_eq!(req["method"], "eth_call");
        assert_eq!(req["params"][1], "latest");
        let params = &req["params"][0];
        assert_eq!(
            params["to"].as_str().unwrap().to_ascii_lowercase(),
            COLLATERAL_ADDRESS_POLYGON.to_ascii_lowercase()
        );
        // data = selector || pad32(owner)
        let expected = format!(
            "0x{ERC20_BALANCE_OF_SELECTOR}{}{}",
            "0".repeat(24),
            "1111111111111111111111111111111111111111",
        );
        assert_eq!(params["data"].as_str().unwrap(), expected);
    }

    #[tokio::test]
    async fn decimals_encodes_call_and_decodes_result() {
        let (url, seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": word(6)
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        assert_eq!(c.decimals().await.expect("decimals read"), 6);
        let data = seen.lock().unwrap()[0]["params"][0]["data"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(data, format!("0x{ERC20_DECIMALS_SELECTOR}"));
    }

    #[tokio::test]
    async fn allowance_encodes_both_addresses_and_decodes() {
        let (url, seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": word(9_999_999)
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        let owner = "0x2222222222222222222222222222222222222222";
        let spender = "0xE111180000d2663C0091e4f400237545B87B996B";
        let allowance = c.allowance(owner, spender).await.expect("allowance read");
        assert_eq!(allowance, 9_999_999);
        let data = seen.lock().unwrap()[0]["params"][0]["data"]
            .as_str()
            .unwrap()
            .to_string();
        // data = selector || pad32(owner) || pad32(spender)
        let expected = format!(
            "0x{ERC20_ALLOWANCE_SELECTOR}{}{}{}{}",
            "0".repeat(24),
            "2222222222222222222222222222222222222222",
            "0".repeat(24),
            "e111180000d2663c0091e4f400237545b87b996b",
        );
        assert_eq!(data, expected);
    }

    #[tokio::test]
    async fn rpc_errors_are_errors_never_zero() {
        // JSON-RPC error object -> Err (could-not-read, never no-balance).
        let (url, _seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32005, "message": "limit exceeded" }
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        let owner = "0x1111111111111111111111111111111111111111";
        let err = c
            .balance_of(owner)
            .await
            .expect_err("rpc error must surface as Err");
        assert!(matches!(err, PolyError::Http(_)));

        // Missing result field -> Err.
        let (url, _seen) = mock_rpc(serde_json::json!({"jsonrpc": "2.0", "id": 1})).await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        assert!(c.balance_of(owner).await.is_err());

        // A result that is not a 32-byte word -> Err.
        let (url, _seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": "0x1234"
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        assert!(c.balance_of(owner).await.is_err());

        // A balance whose upper 128 bits are set must error, not truncate.
        let (url, _seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": format!("0x{}{:032x}", "f".repeat(32), 1u128)
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        assert!(c.balance_of(owner).await.is_err());

        // Unreachable endpoint -> Err.
        let c = CollateralClient::new("http://127.0.0.1:1/", COLLATERAL_ADDRESS_POLYGON).unwrap();
        assert!(c.balance_of(owner).await.is_err());
    }

    #[tokio::test]
    async fn decimals_out_of_u8_range_is_an_error() {
        let (url, _seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": word(u128::MAX)
        }))
        .await;
        let c = CollateralClient::new(&url, COLLATERAL_ADDRESS_POLYGON).unwrap();
        assert!(c.decimals().await.is_err());
    }
}
