//! Solana RPC/WS/Geyser connectivity contract (Batch 7).
//! Validate configured endpoints and minimal connectivity.
//! Return reachable, authenticated/unauthenticated, latency, slot/health where safe.
//! Do not expose auth credentials. No trading.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::ops::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConnectionConfig {
    pub rpc_url: String,
    pub ws_url: Option<String>,
    pub timeout: Duration,
}

impl SolanaConnectionConfig {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            ws_url: None,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn with_ws(mut self, ws_url: impl Into<String>) -> Self {
        self.ws_url = Some(ws_url.into());
        self
    }

    pub fn redacted_rpc_url(&self) -> String {
        redact_url(&self.rpc_url)
    }
}

fn redact_url(url: &str) -> String {
    // Hide credentials in URL (if url contains @, redact userinfo)
    if let Some(at) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            return format!("{}://<redacted>@{}", &url[..scheme_end], &url[at + 1..]);
        }
        return "<redacted url>".into();
    }
    // Hide query secrets
    if url.contains('?') {
        let base = url.split('?').next().unwrap_or(url);
        return format!("{}?<redacted>", base);
    }
    url.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConnectionResult {
    pub rpc_reachable: bool,
    pub ws_reachable: Option<bool>,
    pub authenticated: Option<bool>,
    pub latency_ms: Option<u64>,
    pub slot: Option<u64>,
    pub health: Option<String>,
    pub status: ProviderStatus,
    pub detail: String,
    pub redacted_endpoint: String,
}

impl SolanaConnectionResult {
    pub fn not_run(detail: impl Into<String>) -> Self {
        Self {
            rpc_reachable: false,
            ws_reachable: None,
            authenticated: None,
            latency_ms: None,
            slot: None,
            health: None,
            status: ProviderStatus::NotRun,
            detail: detail.into(),
            redacted_endpoint: "<not configured>".into(),
        }
    }

    pub fn external_required(detail: impl Into<String>) -> Self {
        Self {
            rpc_reachable: false,
            ws_reachable: None,
            authenticated: None,
            latency_ms: None,
            slot: None,
            health: None,
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
            redacted_endpoint: "<redacted>".into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rpc_reachable": self.rpc_reachable,
            "ws_reachable": self.ws_reachable,
            "authenticated": self.authenticated,
            "latency_ms": self.latency_ms,
            "slot": self.slot,
            "health": self.health,
            "status": self.status.as_str(),
            "detail": self.detail,
            "redacted_endpoint": self.redacted_endpoint,
        })
    }
}

pub struct SolanaConnectionContract;

impl SolanaConnectionContract {
    pub fn check(config: Option<SolanaConnectionConfig>) -> SolanaConnectionResult {
        let cfg = match config {
            Some(c) if !c.rpc_url.trim().is_empty() => c,
            _ => {
                return SolanaConnectionResult::external_required(
                    "RPC_URL not set — Solana RPC check NOT_RUN (no endpoint, no trading, no credentials)",
                );
            }
        };

        // Do not expose auth credentials
        let redacted = cfg.redacted_rpc_url();

        // Hermetic mode: no network — return NOT_RUN with expected evidence description
        // Real live would: reqwest get health, getSlot, getHealth with timeout, measure latency
        // But we never trade in this contract
        SolanaConnectionResult {
            rpc_reachable: false,
            ws_reachable: cfg.ws_url.as_ref().map(|_| false),
            authenticated: None,
            latency_ms: None,
            slot: None,
            health: None,
            status: ProviderStatus::NotRun,
            detail: format!("Solana RPC check NOT_RUN — endpoint configured but not tested in hermetic mode; redacted_endpoint={}", redacted),
            redacted_endpoint: redacted,
        }
    }

    pub fn check_live(config: SolanaConnectionConfig) -> SolanaConnectionResult {
        // Live would attempt real RPC — here we simulate external_required if live flag not set
        if std::env::var("SOLANA_LIVE").unwrap_or_default() != "1" {
            let mut r = Self::check(Some(config));
            r.status = ProviderStatus::ExternalRequired;
            r.detail = "SOLANA_LIVE=1 required for live RPC check".into();
            return r;
        }
        // Even with live flag, we would do read-only health/slot without trading
        Self::check(Some(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_url_is_external_required() {
        let r = SolanaConnectionContract::check(None);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
        assert!(!r.rpc_reachable);
        assert!(r.detail.contains("RPC_URL"));
    }

    #[test]
    fn empty_url_is_external_required() {
        let cfg = SolanaConnectionConfig::new("");
        let r = SolanaConnectionContract::check(Some(cfg));
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
    }

    #[test]
    fn with_url_is_not_run_in_hermetic() {
        let cfg = SolanaConnectionConfig::new("https://api.mainnet-beta.solana.com");
        let r = SolanaConnectionContract::check(Some(cfg));
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(!r.rpc_reachable);
        assert!(r.redacted_endpoint.contains("https://"));
        assert!(!r.redacted_endpoint.contains("secret"));
    }

    #[test]
    fn redacts_credentials_in_url() {
        let cfg = SolanaConnectionConfig::new("https://user:pass@rpc.example.com");
        let r = SolanaConnectionContract::check(Some(cfg));
        assert!(!r.redacted_endpoint.contains("user:pass"));
        assert!(r.redacted_endpoint.contains("<redacted>"));
    }

    #[test]
    fn no_trading_in_contract() {
        let cfg = SolanaConnectionConfig::new("https://api.mainnet-beta.solana.com");
        let r = SolanaConnectionContract::check(Some(cfg));
        // Ensure no trading fields
        let json = r.to_safe_json().to_string();
        assert!(!json.to_lowercase().contains("trade"));
        assert!(!json.to_lowercase().contains("order"));
    }
}
