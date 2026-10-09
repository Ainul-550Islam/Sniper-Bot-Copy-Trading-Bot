//! Helius sendTransaction landing provider (GAP-MAP v2, P2).
//!
//! A thin `sendTransaction` route aimed at Helius-style RPC endpoints
//! (standard JSON-RPC; the endpoint URL carries whatever auth the operator's
//! plan uses — we never parse or log it). Tuned for the hot path the same
//! way the executor's RPC broadcast is: `skipPreflight` and `maxRetries: 0`
//! so the sender returns as soon as the node ACCEPTS the transaction instead
//! of holding the connection for preflight simulation or node-internal
//! retries (both are the executor's job, with fresh blockhashes).
//!
//! Provider docs change; this file intentionally implements only stable
//! Solana JSON-RPC (`sendTransaction`), so it keeps working even if vendor
//! extras rotate. Any Helius-specific option should be re-verified against
//! the CURRENT docs before being added here.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use bot_core::error::{BotError, BotResult};

use super::{LandingProvider, SendReceipt};

/// Helius (or any plain JSON-RPC) sendTransaction route.
#[derive(Clone)]
pub struct HeliusSender {
    http: Client,
    /// Full RPC endpoint, including any API-key query parameter.
    endpoint: String,
    /// Extra headers (e.g. vendor routing labels). Keys are sent verbatim.
    extra_headers: BTreeMap<String, String>,
    /// Commitment used for the send's preflight WHEN preflight is enabled.
    /// Preflight is off by default on the hot path; the field exists so an
    /// operator can flip `skip_preflight` for debugging without code edits.
    preflight_commitment: &'static str,
    /// Default true: accept-and-return, no node-side simulation.
    skip_preflight: bool,
}

impl HeliusSender {
    /// Build a sender against `endpoint`.
    pub fn new(endpoint: impl Into<String>) -> Self {
        HeliusSender {
            http: Client::new(),
            endpoint: endpoint.into(),
            extra_headers: BTreeMap::new(),
            preflight_commitment: "processed",
            skip_preflight: true,
        }
    }

    /// Share an HTTP client across providers (connection pooling).
    pub fn with_client(mut self, http: Client) -> Self {
        self.http = http;
        self
    }

    /// Add a header sent on every request (e.g. a vendor routing label).
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.insert(key.into(), value.into());
        self
    }

    /// Enable preflight simulation (debugging aid; off by default).
    pub fn with_preflight(mut self, commitment: &'static str) -> Self {
        self.skip_preflight = false;
        self.preflight_commitment = commitment;
        self
    }

    /// The configured endpoint (NOT logged elsewhere — may contain a key).
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl LandingProvider for HeliusSender {
    fn name(&self) -> &'static str {
        "helius"
    }

    async fn send(&self, signed_tx_base64: &str, signature: &str) -> BotResult<SendReceipt> {
        if signed_tx_base64.is_empty() {
            return Err(BotError::invalid("landing helius: empty transaction bytes"));
        }
        if self.endpoint.is_empty() {
            return Err(BotError::config("landing helius: endpoint is not configured"));
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [signed_tx_base64, {
                "encoding": "base64",
                "skipPreflight": self.skip_preflight,
                "preflightCommitment": self.preflight_commitment,
                "maxRetries": 0
            }]
        });
        let mut request = self.http.post(&self.endpoint).json(&body);
        for (key, value) in &self.extra_headers {
            request = request.header(key.as_str(), value.as_str());
        }
        let response = request
            .send()
            .await
            .map_err(|e| BotError::http(format!("landing helius post: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| BotError::http(format!("landing helius body: {e}")))?;
        if !status.is_success() {
            return Err(BotError::http(format!(
                "landing helius http {status}: {}",
                truncate(&text, 300)
            )));
        }
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| BotError::encoding(format!("landing helius json: {e}")))?;
        if let Some(err) = value.get("error") {
            return Err(BotError::solana(format!("landing helius error: {err}")));
        }
        // sendTransaction echoes the signature on success; trust OURS.
        Ok(SendReceipt {
            provider: self.name().to_string(),
            signature: signature.to_string(),
            provider_receipt: None,
            accepted_at: Instant::now(),
        })
    }
}

/// Shorten a message for logs without splitting a UTF-8 char.
fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = n;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_bytes_are_rejected_offline() {
        let sender = HeliusSender::new("http://127.0.0.1:1");
        let err = sender.send("", "sig").await.unwrap_err();
        assert!(err.to_string().contains("empty transaction"));
    }

    #[tokio::test]
    async fn missing_endpoint_is_a_configuration_error() {
        let sender = HeliusSender::new("");
        let err = sender.send("AAAA", "sig").await.unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }

    #[test]
    fn builder_keeps_headers_and_preflight_flags() {
        let sender = HeliusSender::new("https://example.invalid/rpc")
            .with_header("x-route", "staked")
            .with_preflight("confirmed");
        assert_eq!(sender.extra_headers.get("x-route").map(String::as_str), Some("staked"));
        assert!(!sender.skip_preflight);
        assert_eq!(sender.preflight_commitment, "confirmed");
    }
}
