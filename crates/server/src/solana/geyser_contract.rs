//! Real Geyser subscription contract verification (Batch 7).
//! Validate connection/subscription/message decoding where existing code supports it.
//! Use read-only verification. No live trading. External credentials required.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::ops::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeyserContractConfig {
    pub geyser_url: String,
    pub x_token: Option<String>,
    pub timeout: Duration,
}

impl GeyserContractConfig {
    pub fn new(geyser_url: impl Into<String>) -> Self {
        Self {
            geyser_url: geyser_url.into(),
            x_token: None,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.x_token = Some(token.into());
        self
    }

    pub fn redacted_url(&self) -> String {
        if self.geyser_url.contains('@') {
            return "<redacted geyser url>".into();
        }
        if self.geyser_url.contains('?') {
            let base = self
                .geyser_url
                .split('?')
                .next()
                .unwrap_or(&self.geyser_url);
            return format!("{}?<redacted>", base);
        }
        self.geyser_url.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeyserContractResult {
    pub connected: bool,
    pub subscribed: bool,
    pub message_decoded: bool,
    pub latency_ms: Option<u64>,
    pub status: ProviderStatus,
    pub detail: String,
    pub redacted_endpoint: String,
}

impl GeyserContractResult {
    pub fn not_run(detail: impl Into<String>) -> Self {
        Self {
            connected: false,
            subscribed: false,
            message_decoded: false,
            latency_ms: None,
            status: ProviderStatus::NotRun,
            detail: detail.into(),
            redacted_endpoint: "<not configured>".into(),
        }
    }

    pub fn external_required(detail: impl Into<String>) -> Self {
        Self {
            connected: false,
            subscribed: false,
            message_decoded: false,
            latency_ms: None,
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
            redacted_endpoint: "<redacted>".into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "connected": self.connected,
            "subscribed": self.subscribed,
            "message_decoded": self.message_decoded,
            "latency_ms": self.latency_ms,
            "status": self.status.as_str(),
            "detail": self.detail,
            "redacted_endpoint": self.redacted_endpoint,
        })
    }
}

pub struct GeyserContract;

impl GeyserContract {
    pub fn check(config: Option<GeyserContractConfig>) -> GeyserContractResult {
        let cfg = match config {
            Some(c) if !c.geyser_url.trim().is_empty() => c,
            _ => {
                return GeyserContractResult::external_required(
                    "GEYSER_URL not set — Geyser check NOT_RUN (no endpoint, read-only)",
                );
            }
        };

        let redacted = cfg.redacted_url();

        // Hermetic: no network, return NOT_RUN
        GeyserContractResult {
            connected: false,
            subscribed: false,
            message_decoded: false,
            latency_ms: None,
            status: ProviderStatus::NotRun,
            detail: format!("Geyser check NOT_RUN — endpoint configured but not tested in hermetic mode; redacted_endpoint={}", redacted),
            redacted_endpoint: redacted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_url_is_external_required() {
        let r = GeyserContract::check(None);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
        assert!(!r.connected);
    }

    #[test]
    fn with_url_is_not_run() {
        let cfg = GeyserContractConfig::new("https://geyser.example.com");
        let r = GeyserContract::check(Some(cfg));
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(!r.connected);
        assert!(!r.subscribed);
    }

    #[test]
    fn redacts_token_in_url() {
        let cfg = GeyserContractConfig::new("https://token:secret@geyser.example.com");
        assert!(cfg.redacted_url().contains("<redacted"));
        assert!(!cfg.redacted_url().contains("secret"));
    }

    #[test]
    fn never_trades() {
        let cfg = GeyserContractConfig::new("https://geyser.example.com");
        let r = GeyserContract::check(Some(cfg));
        let json = r.to_safe_json().to_string();
        assert!(!json.to_lowercase().contains("trade"));
    }
}
