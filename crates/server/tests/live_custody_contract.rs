//! Live Vault/KMS/HSM contract test (Batch 7).
//! Default #[ignore]. Requires explicit provider configuration. No local fallback.
//! Verify remote signing behavior where safely possible. No private key extraction.
//! LIVE_EXTERNAL — must not run during ordinary cargo test --workspace.

use sniper_suite::custody::live_provider_contract::{LiveCustodyConfig, LiveCustodyContract};
use sniper_suite::ops::provider_contract::ProviderStatus;

#[test]
#[ignore]
fn live_custody_requires_explicit_opt_in() {
    // LIVE_EXTERNAL — #[ignore] so ordinary cargo test skips
    if std::env::var("LIVE_CUSTODY").unwrap_or_default() != "1" {
        let cfg = LiveCustodyConfig::stub("vault", true, true, false);
        let r = LiveCustodyContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::NotRun);
        return;
    }
    let cfg = LiveCustodyConfig::from_env("vault");
    let r = LiveCustodyContract::check(cfg);
    // Without real credentials, must be EXTERNAL_REQUIRED, never fake PASS via local fallback
    assert!(
        r.status == ProviderStatus::NotRun
            || r.status == ProviderStatus::ExternalRequired
            || r.status == ProviderStatus::Fail
    );
    assert_ne!(r.status, ProviderStatus::Pass);
}

#[test]
#[ignore]
fn live_custody_no_local_fallback() {
    // LIVE_EXTERNAL — even if LIVE_CUSTODY=1 but creds missing, must not fallback to local
    let cfg = LiveCustodyConfig::stub("vault", false, true, true);
    let r = LiveCustodyContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
    assert!(r.detail.contains("no local fallback") || r.detail.contains("EXTERNAL_REQUIRED"));
}

#[test]
#[ignore]
fn live_custody_never_extracts_private_key() {
    // LIVE_EXTERNAL
    let cfg = LiveCustodyConfig::stub("vault", true, true, true);
    let r = LiveCustodyContract::check(cfg);
    let json = r.to_safe_json().to_string();
    assert!(!json.to_lowercase().contains("private"));
    assert!(!json.contains("BEGIN PRIVATE KEY"));
}

#[test]
fn live_custody_hermetic_no_fallback() {
    // HERMETIC — ordinary test must show no local fallback
    let cfg = LiveCustodyConfig::stub("vault", false, true, true);
    let r = LiveCustodyContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
}

#[test]
fn live_custody_fail_when_sign_unavailable() {
    // HERMETIC
    let cfg = LiveCustodyConfig::stub("hsm", true, false, true);
    let r = LiveCustodyContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::Fail);
}
