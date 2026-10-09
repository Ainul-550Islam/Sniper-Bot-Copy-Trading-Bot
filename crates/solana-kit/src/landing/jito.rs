//! Jito bundle landing provider (GAP-MAP v2, P2).
//!
//! Wraps the Jito Block-Engine `sendBundle` JSON-RPC behind the
//! [`LandingProvider`] trait so it can be composed with the circuit breaker
//! and router in [`crate::landing`]. The wire format mirrors the executor's
//! proven `broadcast_jito_inner` path exactly: base64 transaction, one-tx
//! bundle, `{"encoding":"base64"}`, and the bundle id returned in `result`.
//!
//! This provider only SUBMITS. Tip selection, blockhash freshness, and
//! landing confirmation remain the executor's concerns — a bundle id is not
//! a landed transaction, and callers must not treat acceptance as finality.

use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use bot_core::error::{BotError, BotResult};

use super::{LandingProvider, SendReceipt};
use crate::consts::JITO_BUNDLE_PATH;

/// Jito Block-Engine landing route.
#[derive(Clone)]
pub struct JitoLanding {
    http: Client,
    /// Block-engine base URL, e.g. the regional endpoint from config.
    base_url: String,
}

impl JitoLanding {
    /// Build a route against `base_url` (no trailing slash required).
    pub fn new(base_url: impl Into<String>) -> Self {
        JitoLanding {
            http: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Inject a shared HTTP client (connection reuse across providers).
    pub fn with_client(base_url: impl Into<String>, http: Client) -> Self {
        JitoLanding {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// The fully-qualified bundle endpoint.
    pub fn bundle_url(&self) -> String {
        format!("{}{}", self.base_url, JITO_BUNDLE_PATH)
    }
}

#[async_trait]
impl LandingProvider for JitoLanding {
    fn name(&self) -> &'static str {
        "jito"
    }

    async fn send(&self, signed_tx_base64: &str, signature: &str) -> BotResult<SendReceipt> {
        if signed_tx_base64.is_empty() {
            return Err(BotError::invalid(
                "landing jito: empty transaction bytes",
            ));
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[signed_tx_base64], {"encoding": "base64"}]
        });
        let response = self
            .http
            .post(self.bundle_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| BotError::http(format!("landing jito post: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| BotError::http(format!("landing jito body: {e}")))?;
        if !status.is_success() {
            return Err(BotError::http(format!(
                "landing jito http {status}: {}",
                truncate(&text, 300)
            )));
        }
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| BotError::encoding(format!("landing jito json: {e}")))?;
        if let Some(err) = value.get("error") {
            return Err(BotError::solana(format!("landing jito error: {err}")));
        }
        // sendBundle returns the bundle id, NOT the tx signature.
        let bundle_id = value
            .get("result")
            .and_then(|r| r.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        Ok(SendReceipt {
            provider: self.name().to_string(),
            signature: signature.to_string(),
            provider_receipt: bundle_id,
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

    #[test]
    fn bundle_url_uses_the_pinned_path() {
        let j = JitoLanding::new("https://mainnet.block-engine.jito.wtf/");
        assert_eq!(
            j.bundle_url(),
            "https://mainnet.block-engine.jito.wtf/api/v1/bundles"
        );
    }

    #[tokio::test]
    async fn empty_bytes_are_rejected_before_any_network_call() {
        let j = JitoLanding::new("http://127.0.0.1:1"); // unroutable on purpose
        let err = j.send("", "sig").await.unwrap_err();
        assert!(err.to_string().contains("empty transaction"));
    }
}
