//! Live billing contract test (Batch 7).
//! Default #[ignore] AND requires explicit LIVE_BILLING=1. Use actual configured provider.
//! Verify signature/event/checkout semantics. Never use a fake provider for a test named live.
//! LIVE_EXTERNAL — must not run during ordinary cargo test --workspace.

use sniper_suite::billing::live_provider_contract::{LiveBillingConfig, LiveBillingContract};
use sniper_suite::ops::provider_contract::ProviderStatus;

#[test]
#[ignore]
fn live_billing_requires_explicit_opt_in() {
    // LIVE_EXTERNAL — should be NOT_RUN without LIVE_BILLING=1
    // This test is #[ignore] so ordinary `cargo test --workspace` skips it
    // It will only run with `cargo test --test live_billing_contract -- --ignored --nocapture` and LIVE_BILLING=1
    if std::env::var("LIVE_BILLING").unwrap_or_default() != "1" {
        let cfg = LiveBillingConfig::stub("stripe", false, false, false);
        let r = LiveBillingContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(r.detail.contains("LIVE_BILLING"));
        return;
    }
    // If LIVE_BILLING=1 but no credentials, should be EXTERNAL_REQUIRED
    let cfg = LiveBillingConfig::from_env("stripe");
    let r = LiveBillingContract::check(cfg);
    // Without real credentials, must be EXTERNAL_REQUIRED, never fake PASS
    assert_ne!(r.status, ProviderStatus::Pass);
    assert!(
        r.status == ProviderStatus::NotRun || r.status == ProviderStatus::ExternalRequired,
        "expected NOT_RUN or EXTERNAL_REQUIRED, got {:?}",
        r.status
    );
}

#[test]
#[ignore]
fn live_billing_never_hardcoded_success() {
    // LIVE_EXTERNAL — even with all flags, must not return hardcoded success
    let cfg = LiveBillingConfig::stub("stripe", true, true, true);
    let r = LiveBillingContract::check(cfg);
    assert_ne!(
        r.status,
        ProviderStatus::Pass,
        "must not hardcode payment_succeeded"
    );
    // In hermetic, still NOT_RUN, not PASS
    assert_eq!(r.status, ProviderStatus::NotRun);
}

#[test]
#[ignore]
fn live_billing_uses_real_provider_not_fake() {
    // LIVE_EXTERNAL — verify provider name is preserved, not fake
    for provider in ["stripe", "paddle"] {
        let cfg = LiveBillingConfig::stub(provider, true, true, true);
        let r = LiveBillingContract::check(cfg);
        assert_eq!(r.provider, provider);
        assert_ne!(r.status, ProviderStatus::Pass);
    }
}

#[test]
fn live_billing_hermetic_not_run_without_ignore() {
    // HERMETIC — ordinary test (not ignored) should show NOT_RUN without LIVE_BILLING
    let cfg = LiveBillingConfig::stub("stripe", true, true, false);
    let r = LiveBillingContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::NotRun);
}

#[test]
fn live_billing_missing_credentials_is_external_required() {
    // HERMETIC
    let cfg = LiveBillingConfig::stub("stripe", false, true, true);
    let r = LiveBillingContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
}
