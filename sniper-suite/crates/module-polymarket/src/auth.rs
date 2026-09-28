//! Polymarket CLOB authentication.
//!
//! Two layers:
//! * **L1** — an EIP-712 signature over the `ClobAuthDomain` used once to
//!   derive/create API credentials from a wallet (`POST /auth/derive-api-key`).
//! * **L2** — an HMAC-SHA256 signature over `timestamp + method + path + body`
//!   used on every authenticated request (placing/cancelling orders,
//!   heartbeat).
//!
//! Header names follow the CLOB spec: `POLY_ADDRESS`, `POLY_SIGNATURE`,
//! `POLY_TIMESTAMP`, `POLY_NONCE` (L1) and `POLY_API_KEY`, `POLY_PASSPHRASE`
//! (L2).

use std::collections::HashMap;

use base64::Engine;
use hmac::{Hmac, Mac};
use k256::ecdsa::SigningKey;
use sha2::Sha256;

use crate::eip712::{address_word, keccak256, sign_digest};
use crate::error::{PolyError, PolyResult};

type HmacSha256 = Hmac<Sha256>;

/// The attestation message Polymarket expects for L1 auth.
pub const L1_AUTH_MESSAGE: &str = "This message attests that I control the given wallet";

/// EIP-712 domain/type constants for the L1 `ClobAuth` signature.
const CLOB_AUTH_DOMAIN_TYPE: &str = "EIP712Domain(string name,string version,uint256 chainId)";
const CLOB_AUTH_TYPE: &str =
    "ClobAuth(address address,string timestamp,uint256 nonce,string message)";
const CLOB_AUTH_DOMAIN_NAME: &str = "ClobAuthDomain";
const CLOB_AUTH_DOMAIN_VERSION: &str = "1";

/// API credentials returned by `derive-api-key`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiKey {
    /// L2 API key (`apiKey` on the wire) — identifies the signing identity
    /// in the `POLY_API_KEY` header.
    #[serde(rename = "apiKey")]
    pub key: String,
    /// Base64 HMAC secret used to sign L2 requests (`POLY_SIGNATURE`).
    /// Sensitive: never log or expose via the API.
    #[serde(rename = "secret")]
    pub secret: String,
    /// Passphrase sent alongside the HMAC headers (`POLY_PASSPHRASE`).
    #[serde(rename = "passphrase")]
    pub passphrase: String,
}

impl ApiKey {
    /// The `auth` object of the user-channel websocket subscribe frame.
    /// Same three credentials as the L2 headers, in the wire field names.
    pub fn user_ws_auth(&self) -> serde_json::Value {
        serde_json::json!({
            "apiKey": self.key,
            "secret": self.secret,
            "passphrase": self.passphrase,
        })
    }

    /// A log-safe description: the key id prefix only, never the secret or
    /// passphrase.
    pub fn redacted(&self) -> String {
        let shown: String = self.key.chars().take(6).collect();
        format!("apiKey {shown}… (secret/passphrase redacted)")
    }
}

/// Build the L2 (HMAC) auth headers for a request.
///
/// `method` is uppercased internally; `request_path` must include the leading
/// `/` and any query string; `body` is the exact JSON string sent (empty for
/// GET/DELETE without a body).
pub fn l2_headers(
    api_key: &ApiKey,
    address: &str,
    method: &str,
    request_path: &str,
    body: &str,
    timestamp: u64,
) -> PolyResult<HashMap<String, String>> {
    // The secret is base64-encoded; decode it to the raw HMAC key.
    let secret_bytes = base64::engine::general_purpose::STANDARD
        .decode(api_key.secret.as_bytes())
        .map_err(|e| PolyError::invalid(format!("api secret is not base64: {e}")))?;

    let message = format!(
        "{}{}{}{}",
        timestamp,
        method.to_ascii_uppercase(),
        request_path,
        body
    );

    let mut mac = HmacSha256::new_from_slice(&secret_bytes)
        .map_err(|e| PolyError::invalid(format!("hmac key: {e}")))?;
    mac.update(message.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    let mut headers = HashMap::new();
    headers.insert("POLY_ADDRESS".to_string(), address.to_string());
    headers.insert("POLY_SIGNATURE".to_string(), signature);
    headers.insert("POLY_TIMESTAMP".to_string(), timestamp.to_string());
    headers.insert("POLY_API_KEY".to_string(), api_key.key.clone());
    headers.insert("POLY_PASSPHRASE".to_string(), api_key.passphrase.clone());
    Ok(headers)
}

/// Compute the L1 `ClobAuth` EIP-712 digest for an address at a timestamp.
pub fn l1_auth_digest(
    address: &str,
    timestamp: &str,
    nonce: u64,
    chain_id: u64,
) -> PolyResult<[u8; 32]> {
    // Domain separator (no verifyingContract for the auth domain).
    let mut domain_buf = Vec::with_capacity(4 * 32);
    domain_buf.extend_from_slice(&keccak256(CLOB_AUTH_DOMAIN_TYPE.as_bytes()));
    domain_buf.extend_from_slice(&keccak256(CLOB_AUTH_DOMAIN_NAME.as_bytes()));
    domain_buf.extend_from_slice(&keccak256(CLOB_AUTH_DOMAIN_VERSION.as_bytes()));
    let mut chain_word = [0u8; 32];
    chain_word[24..].copy_from_slice(&chain_id.to_be_bytes());
    domain_buf.extend_from_slice(&chain_word);
    let domain_separator = keccak256(&domain_buf);

    // Struct hash.
    let mut struct_buf = Vec::with_capacity(5 * 32);
    struct_buf.extend_from_slice(&keccak256(CLOB_AUTH_TYPE.as_bytes()));
    struct_buf.extend_from_slice(&address_word(address)?);
    struct_buf.extend_from_slice(&keccak256(timestamp.as_bytes()));
    let mut nonce_word = [0u8; 32];
    nonce_word[24..].copy_from_slice(&nonce.to_be_bytes());
    struct_buf.extend_from_slice(&nonce_word);
    struct_buf.extend_from_slice(&keccak256(L1_AUTH_MESSAGE.as_bytes()));
    let struct_hash = keccak256(&struct_buf);

    let mut buf = Vec::with_capacity(66);
    buf.push(0x19);
    buf.push(0x01);
    buf.extend_from_slice(&domain_separator);
    buf.extend_from_slice(&struct_hash);
    Ok(keccak256(&buf))
}

/// Build the L1 auth headers (used for `derive-api-key`).
pub fn l1_headers(
    key: &SigningKey,
    address: &str,
    timestamp: u64,
    chain_id: u64,
) -> PolyResult<HashMap<String, String>> {
    let digest = l1_auth_digest(address, &timestamp.to_string(), 0, chain_id)?;
    let sig = sign_digest(key, &digest)?;
    let mut headers = HashMap::new();
    headers.insert("POLY_ADDRESS".to_string(), address.to_string());
    headers.insert(
        "POLY_SIGNATURE".to_string(),
        format!("0x{}", hex::encode(sig)),
    );
    headers.insert("POLY_TIMESTAMP".to_string(), timestamp.to_string());
    headers.insert("POLY_NONCE".to_string(), "0".to_string());
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ApiKey {
        ApiKey {
            key: "my-api-key".into(),
            // base64 of "super-secret-key-bytes" (any valid base64 works).
            secret: base64::engine::general_purpose::STANDARD.encode(b"super-secret-key-bytes"),
            passphrase: "my-passphrase".into(),
        }
    }

    #[test]
    fn l2_headers_are_complete_and_deterministic() {
        let k = key();
        let a = l2_headers(&k, "0xabc", "POST", "/order", "{\"x\":1}", 1_700_000_000).unwrap();
        let b = l2_headers(&k, "0xabc", "POST", "/order", "{\"x\":1}", 1_700_000_000).unwrap();
        assert_eq!(a, b);
        for h in [
            "POLY_ADDRESS",
            "POLY_SIGNATURE",
            "POLY_TIMESTAMP",
            "POLY_API_KEY",
            "POLY_PASSPHRASE",
        ] {
            assert!(a.contains_key(h), "missing header {h}");
        }
        assert_eq!(a["POLY_ADDRESS"], "0xabc");
        assert_eq!(a["POLY_API_KEY"], "my-api-key");
    }

    #[test]
    fn l2_signature_changes_with_body() {
        let k = key();
        let a = l2_headers(&k, "0xabc", "POST", "/order", "{\"x\":1}", 1).unwrap();
        let b = l2_headers(&k, "0xabc", "POST", "/order", "{\"x\":2}", 1).unwrap();
        assert_ne!(a["POLY_SIGNATURE"], b["POLY_SIGNATURE"]);
    }

    #[test]
    fn l2_method_is_case_insensitive() {
        let k = key();
        let a = l2_headers(&k, "0xabc", "post", "/order", "", 1).unwrap();
        let b = l2_headers(&k, "0xabc", "POST", "/order", "", 1).unwrap();
        assert_eq!(a["POLY_SIGNATURE"], b["POLY_SIGNATURE"]);
    }

    #[test]
    fn l2_rejects_non_base64_secret() {
        let bad = ApiKey {
            key: "k".into(),
            secret: "!!!not base64!!!".into(),
            passphrase: "p".into(),
        };
        assert!(l2_headers(&bad, "0xabc", "GET", "/x", "", 1).is_err());
    }

    #[test]
    fn l1_digest_is_deterministic_and_chain_specific() {
        let a = l1_auth_digest(
            "0x0000000000000000000000000000000000000001",
            "1700000000",
            0,
            137,
        )
        .unwrap();
        let b = l1_auth_digest(
            "0x0000000000000000000000000000000000000001",
            "1700000000",
            0,
            137,
        )
        .unwrap();
        let c = l1_auth_digest(
            "0x0000000000000000000000000000000000000001",
            "1700000000",
            0,
            1,
        )
        .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn l1_headers_sign_and_include_nonce() {
        let key_bytes =
            hex::decode("4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318")
                .unwrap();
        let sk = SigningKey::from_slice(&key_bytes).unwrap();
        let addr = crate::eip712::address_from_signing_key(&sk);
        let h = l1_headers(&sk, &addr, 1_700_000_000, 137).unwrap();
        assert_eq!(h["POLY_NONCE"], "0");
        assert!(h["POLY_SIGNATURE"].starts_with("0x"));
        assert_eq!(h["POLY_ADDRESS"], addr);
    }

    #[test]
    fn api_key_user_ws_auth_and_redaction() {
        let k = ApiKey {
            key: "abcdef123456".into(),
            secret: "c2VjcmV0".into(),
            passphrase: "hunter2-xyz".into(),
        };
        let v = k.user_ws_auth();
        assert_eq!(v["apiKey"], "abcdef123456");
        assert_eq!(v["secret"], "c2VjcmV0");
        assert_eq!(v["passphrase"], "hunter2-xyz");
        let r = k.redacted();
        assert!(r.starts_with("apiKey abcdef"));
        assert!(!r.contains("123456"), "only a key prefix is shown");
        assert!(!r.contains("c2VjcmV0"));
        assert!(!r.contains("hunter2"));
    }
}
