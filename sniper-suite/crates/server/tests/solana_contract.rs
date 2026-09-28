//! Solana RPC contract test (Batch 7).
//! If RPC_URL/WS_URL absent: NOT_RUN. If present: perform read-only health/slot checks. No order placement.
//! LIVE_EXTERNAL when URL present, NOT_RUN otherwise. Never trades.

use sniper_suite::ops::provider_contract::ProviderStatus;
use sniper_suite::solana::connection_contract::{SolanaConnectionConfig, SolanaConnectionContract};
use sniper_suite::solana::geyser_contract::{GeyserContract, GeyserContractConfig};

#[test]
fn solana_rpc_not_run_without_url() {
    // LIVE_EXTERNAL — without RPC_URL should be EXTERNAL_REQUIRED/NOT_RUN
    let r = SolanaConnectionContract::check(None);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
    assert!(!r.rpc_reachable);
}

#[test]
fn solana_rpc_with_url_not_auto_pass_in_hermetic() {
    // LIVE_EXTERNAL — with URL still NOT_RUN in hermetic (no network)
    let cfg = SolanaConnectionConfig::new("https://api.mainnet-beta.solana.com");
    let r = SolanaConnectionContract::check(Some(cfg));
    assert_eq!(r.status, ProviderStatus::NotRun);
    assert!(!r.rpc_reachable);
    assert!(r.redacted_endpoint.contains("https://"));
}

#[test]
fn solana_rpc_redacts_credentials() {
    // LIVE_EXTERNAL
    let cfg = SolanaConnectionConfig::new("https://user:pass@rpc.example.com");
    let r = SolanaConnectionContract::check(Some(cfg));
    assert!(!r.redacted_endpoint.contains("user:pass"));
    assert!(r.redacted_endpoint.contains("<redacted>"));
}

#[test]
fn solana_no_trading_in_contract() {
    // LIVE_EXTERNAL
    let cfg = SolanaConnectionConfig::new("https://api.mainnet-beta.solana.com");
    let r = SolanaConnectionContract::check(Some(cfg));
    let json = r.to_safe_json().to_string().to_lowercase();
    assert!(!json.contains("trade"));
    assert!(!json.contains("order"));
}

#[test]
fn geyser_not_run_without_url() {
    // LIVE_EXTERNAL
    let r = GeyserContract::check(None);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
    assert!(!r.connected);
}

#[test]
fn geyser_with_url_not_run_in_hermetic() {
    // LIVE_EXTERNAL
    let cfg = GeyserContractConfig::new("https://geyser.example.com");
    let r = GeyserContract::check(Some(cfg));
    assert_eq!(r.status, ProviderStatus::NotRun);
    assert!(!r.connected);
    assert!(!r.subscribed);
}

#[test]
fn geyser_never_trades() {
    // LIVE_EXTERNAL
    let cfg = GeyserContractConfig::new("https://geyser.example.com");
    let r = GeyserContract::check(Some(cfg));
    assert!(!r
        .to_safe_json()
        .to_string()
        .to_lowercase()
        .contains("trade"));
}
