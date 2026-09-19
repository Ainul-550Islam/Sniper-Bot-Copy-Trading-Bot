//! On-chain CTF balance reads (ERC-1155 `balanceOf` on Polygon) — §D/§O/§Q.
//!
//! The CLOB API stays the source of truth for ORDER LIFECYCLE; the Conditional
//! Token Framework (CTF) contract is the on-chain truth for SETTLED FILLS:
//! every matched Polymarket position mints/transfer outcome tokens (ERC-1155
//! ids = the CLOB `asset_id`/token_id) to the funder wallet. Reading the
//! balance therefore distinguishes:
//!
//! * venue says matched AND tokens are held  → fill settled on chain;
//! * venue says matched AND tokens are absent → sold/transferred later, or a
//!   settlement problem — evidence for operators, never a silent conclusion;
//! * RPC unreadable → "could not read", NEVER "no balance" (§O).
//!
//! This client deliberately knows nothing about orders or strategy state; it
//! answers one question (`balanceOf(owner, tokenId)`) and surfaces transport
//! and protocol errors as `Err`.

use std::time::Duration;

use crate::error::{PolyError, PolyResult};

/// `keccak256("balanceOf(address,uint256)")[0..4]` — the ERC-1155 balance
/// selector. Hard-coded constant (verified against the CTF ABI).
pub const BALANCE_OF_SELECTOR: &str = "00fdd58e";

/// The verified CTF (Conditional Tokens Framework) proxy on Polygon mainnet.
/// Mirrors `[polymarket].conditional_tokens_address` in config; kept here for
/// tests and as the documented reference value.
pub const CTF_ADDRESS_POLYGON: &str = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045";

/// Minimal Polygon JSON-RPC reader for CTF balances.
#[derive(Clone)]
pub struct CtfClient {
    rpc_url: String,
    ctf_address: String,
    http: reqwest::Client,
}

impl CtfClient {
    /// `rpc_url` — any Polygon JSON-RPC endpoint (`eth_call` support is the
    /// only requirement). `ctf_address` — the CTF contract (0x + 40 hex).
    /// An empty `rpc_url` means "not configured": construction fails with
    /// `not_configured` so callers can decide (the bot treats it as the CTF
    /// check being disabled, never as balance zero).
    pub fn new(rpc_url: impl Into<String>, ctf_address: impl Into<String>) -> PolyResult<Self> {
        let rpc_url = rpc_url.into();
        let ctf_address = ctf_address.into();
        if rpc_url.trim().is_empty() {
            return Err(PolyError::not_configured("ctf rpc url is empty"));
        }
        if !is_hex_address(&ctf_address) {
            return Err(PolyError::invalid(format!(
                "ctf address is not 0x+40 hex: {ctf_address}"
            )));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| PolyError::http(format!("ctf http client: {e}")))?;
        Ok(Self {
            rpc_url,
            ctf_address,
            http,
        })
    }

    /// ERC-1155 `balanceOf(owner, tokenId)` via `eth_call` at `latest`.
    /// `token_id` is the decimal string form used by the CLOB API
    /// (`asset_id`). Errors are transport/protocol/input errors — per §O a
    /// caller must treat `Err` as "could not read", never as "no balance".
    pub async fn balance_of(&self, owner: &str, token_id: &str) -> PolyResult<u128> {
        if !is_hex_address(owner) {
            return Err(PolyError::invalid(format!(
                "owner is not 0x+40 hex: {owner}"
            )));
        }
        let id = u256_from_dec(token_id)?;
        let data = format!(
            "0x{BALANCE_OF_SELECTOR}{}{}",
            pad32_address(owner)?,
            hex::encode(id)
        );
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": self.ctf_address, "data": data }, "latest"],
        });
        let resp = self
            .http
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PolyError::http(format!("ctf rpc send: {e}")))?;
        let status = resp.status();
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| PolyError::http(format!("ctf rpc decode: {e}")))?;
        if !status.is_success() {
            return Err(PolyError::http(format!("ctf rpc status {status}")));
        }
        if let Some(err) = v.get("error") {
            return Err(PolyError::http(format!("ctf rpc error: {err}")));
        }
        let result = v
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| PolyError::http("ctf rpc response has no result"))?;
        decode_uint256(result)
    }
}

/// `0x` + exactly 40 hex digits.
pub(crate) fn is_hex_address(s: &str) -> bool {
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    matches!(s, Some(h) if h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Address → 32-byte left-padded hex (no 0x), as an ABI `address` argument.
pub(crate) fn pad32_address(addr: &str) -> PolyResult<String> {
    let h = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .ok_or_else(|| PolyError::invalid(format!("address missing 0x: {addr}")))?;
    if h.len() != 40 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PolyError::invalid(format!(
            "address is not 40 hex digits: {addr}"
        )));
    }
    Ok(format!("{}{}", "0".repeat(24), h.to_ascii_lowercase()))
}

/// Decimal string (CLOB `asset_id`) → big-endian 32-byte uint256.
/// Schoolbook base-10 accumulation; rejects non-digits and >u256 values.
fn u256_from_dec(s: &str) -> PolyResult<[u8; 32]> {
    if s.is_empty() {
        return Err(PolyError::invalid("token_id is empty"));
    }
    let mut out = [0u8; 32];
    for ch in s.chars() {
        let d = ch
            .to_digit(10)
            .ok_or_else(|| PolyError::invalid(format!("token_id is not decimal: {s}")))?
            as u16;
        let mut carry = d;
        for byte in out.iter_mut().rev() {
            let v = u16::from(*byte) * 10 + carry;
            *byte = (v & 0xff) as u8;
            carry = v >> 8;
        }
        if carry > 0 {
            return Err(PolyError::invalid(format!(
                "token_id overflows uint256: {s}"
            )));
        }
    }
    Ok(out)
}

/// `eth_call` result (`0x` + 64 hex) → u128. A nonzero upper 16 bytes means
/// the balance cannot be represented — surfaced as an error, never truncated.
/// Shared with [`crate::collateral`] so both readers decode identically.
pub(crate) fn decode_uint256(result: &str) -> PolyResult<u128> {
    let h = result
        .strip_prefix("0x")
        .or_else(|| result.strip_prefix("0X"))
        .ok_or_else(|| PolyError::http(format!("ctf result is not hex: {result}")))?;
    if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PolyError::http(format!(
            "ctf result is not 32 bytes of hex: {result}"
        )));
    }
    let bytes = hex::decode(h).map_err(|e| PolyError::http(format!("ctf result hex: {e}")))?;
    if bytes[..16].iter().any(|b| *b != 0) {
        return Err(PolyError::http(
            "ctf balance exceeds u128 — refusing to truncate",
        ));
    }
    let mut lo = [0u8; 16];
    lo.copy_from_slice(&bytes[16..]);
    Ok(u128::from_be_bytes(lo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ---- pure encoding/decoding --------------------------------------------

    #[test]
    fn address_validation_and_padding() {
        assert!(is_hex_address("0x4D97DCd97eC945f40cF65F87097ACe5EA0476045"));
        assert!(!is_hex_address("0x123"));
        assert!(!is_hex_address("4D97DCd97eC945f40cF65F87097ACe5EA0476045"));
        assert!(!is_hex_address(
            "0xZZZ7DCd97eC945f40cF65F87097ACe5EA0476045"
        ));
        let padded = pad32_address("0xAbC0000000000000000000000000000000000001").unwrap();
        assert_eq!(
            padded,
            "000000000000000000000000abc0000000000000000000000000000000000001"
        );
    }

    #[test]
    fn decimal_token_ids_encode_big_endian() {
        let one = u256_from_dec("1").unwrap();
        assert_eq!(hex::encode(one)[..62], "0".repeat(62));
        assert_eq!(&hex::encode(one)[62..], "01");
        // A realistic 77-digit CLOB asset id (u256, above u128 range start).
        let big = "71321075685313568811525804339278071228656824048554047931573198635799981172737";
        let bytes = u256_from_dec(big).unwrap();
        // Round-trip: decode back to decimal and compare.
        assert_eq!(to_dec(&bytes), big);
        assert!(u256_from_dec("12x34").is_err());
        assert!(u256_from_dec("").is_err());
        let overflow = "1".to_string() + &"0".repeat(78); // 10^78 > u256 max
        assert!(u256_from_dec(&overflow).is_err());
    }

    #[test]
    fn uint256_results_decode() {
        let r = format!("0x{}{}", "0".repeat(63), "1");
        assert_eq!(decode_uint256(&r).unwrap(), 1);
        let max128 = format!("0x{}{:032x}", "0".repeat(32), u128::MAX);
        assert_eq!(decode_uint256(&max128).unwrap(), u128::MAX);
        // Nonzero upper half must ERROR, never truncate.
        let over = format!("0x{:032x}{}", 1u128, "0".repeat(32));
        assert!(decode_uint256(&over).is_err());
        assert!(decode_uint256("0x1234").is_err());
        assert!(decode_uint256("nope").is_err());
    }

    /// Test helper: big-endian u256 → decimal string.
    fn to_dec(bytes: &[u8; 32]) -> String {
        let mut digits: Vec<u8> = vec![0];
        for b in bytes {
            // digits = digits*256 + b (little-endian decimal digits)
            let mut carry = u16::from(*b);
            for d in digits.iter_mut() {
                let v = u16::from(*d) * 256 + carry;
                *d = (v % 10) as u8;
                carry = v / 10;
            }
            while carry > 0 {
                digits.push((carry % 10) as u8);
                carry /= 10;
            }
        }
        while digits.len() > 1 && *digits.last().unwrap() == 0 {
            digits.pop();
        }
        digits.iter().rev().map(|d| char::from(b'0' + d)).collect()
    }

    // ---- wire behaviour against a mock JSON-RPC endpoint --------------------

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

    #[tokio::test]
    async fn balance_of_encodes_call_and_decodes_result() {
        let result = format!("0x{}{:032x}", "0".repeat(32), 12345u128);
        let (url, seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": result
        }))
        .await;
        let ctf = CtfClient::new(&url, CTF_ADDRESS_POLYGON).unwrap();
        let owner = "0x1111111111111111111111111111111111111111";
        let bal = ctf.balance_of(owner, "42").await.unwrap();
        assert_eq!(bal, 12345);

        let req = &seen.lock().unwrap()[0];
        assert_eq!(req["method"], "eth_call");
        assert_eq!(req["params"][1], "latest");
        let params = &req["params"][0];
        assert_eq!(
            params["to"].as_str().unwrap().to_ascii_lowercase(),
            CTF_ADDRESS_POLYGON.to_ascii_lowercase()
        );
        // data = selector || pad32(owner) || pad32(42)
        let expected = format!(
            "0x00fdd58e{}{}",
            "0".repeat(24),
            "1111111111111111111111111111111111111111",
        ) + &"0".repeat(62)
            + "2a";
        assert_eq!(params["data"].as_str().unwrap(), expected);
    }

    #[tokio::test]
    async fn rpc_errors_are_errors_never_zero() {
        // JSON-RPC error object → Err (§O: could-not-read, not no-balance).
        let (url, _seen) = mock_rpc(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32000, "message": "execution reverted" }
        }))
        .await;
        let ctf = CtfClient::new(&url, CTF_ADDRESS_POLYGON).unwrap();
        let owner = "0x1111111111111111111111111111111111111111";
        assert!(ctf.balance_of(owner, "42").await.is_err());

        // Missing result field → Err.
        let (url, _seen) = mock_rpc(serde_json::json!({"jsonrpc": "2.0", "id": 1})).await;
        let ctf = CtfClient::new(&url, CTF_ADDRESS_POLYGON).unwrap();
        assert!(ctf.balance_of(owner, "42").await.is_err());

        // Unreachable endpoint → Err.
        let ctf = CtfClient::new("http://127.0.0.1:1/", CTF_ADDRESS_POLYGON).unwrap();
        assert!(ctf.balance_of(owner, "42").await.is_err());
    }

    #[test]
    fn input_validation() {
        let ctf = CtfClient::new("http://x/", CTF_ADDRESS_POLYGON).unwrap();
        assert!(CtfClient::new("", CTF_ADDRESS_POLYGON).is_err());
        assert!(CtfClient::new("http://x/", "not-an-address").is_err());
        // Bad owner / token id are rejected before any network call.
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(ctf.balance_of("nope", "1")).is_err());
        assert!(rt
            .block_on(ctf.balance_of("0x1111111111111111111111111111111111111111", "0x12"))
            .is_err());
    }
}
