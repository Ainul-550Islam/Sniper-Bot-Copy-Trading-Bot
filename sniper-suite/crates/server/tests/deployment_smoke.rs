//! Deployment smoke harness (Batch 7).
//! If DEPLOYMENT_BASE_URL is absent: NOT_RUN. If present: execute real safe health/readiness/OpenAPI/security-header checks.
//! Do not require funded trading. PRODUCTION_SMOKE when URL present, NOT_RUN otherwise.

use sniper_suite::ops::deployment_smoke::{DeploymentSmokeConfig, DeploymentSmokeRunner};
use sniper_suite::ops::provider_contract::ProviderStatus;

#[test]
fn deployment_smoke_not_run_without_url() {
    // PRODUCTION_SMOKE — but without URL should be NOT_RUN/EXTERNAL_REQUIRED
    let r = DeploymentSmokeRunner::run(None);
    assert!(
        r.overall_status == ProviderStatus::NotRun
            || r.overall_status == ProviderStatus::ExternalRequired
    );
    assert!(r.base_url.is_none());
    assert!(r.detail.contains("DEPLOYMENT_BASE_URL") || r.detail.contains("no URL"));
}

#[test]
fn deployment_smoke_with_url_not_auto_pass() {
    // PRODUCTION_SMOKE
    let cfg = DeploymentSmokeConfig::new("https://example.com");
    let r = DeploymentSmokeRunner::run(Some(cfg));
    assert_ne!(
        r.overall_status,
        ProviderStatus::Pass,
        "must not auto PASS without real fetch"
    );
    assert!(r.base_url.is_some());
    for c in &r.checks {
        assert_ne!(c.status, ProviderStatus::Pass);
    }
    assert!(r.checks.iter().any(|c| c.name == "health"));
    assert!(r.checks.iter().any(|c| c.name == "readiness"));
    assert!(r.checks.iter().any(|c| c.name == "openapi"));
}

#[test]
fn deployment_smoke_never_claims_without_url() {
    // PRODUCTION_SMOKE
    let r = DeploymentSmokeRunner::run(None);
    assert_ne!(r.overall_status, ProviderStatus::Pass);
    assert_eq!(r.checks.len(), 0);
}

#[test]
fn deployment_smoke_empty_url_is_external_required() {
    // PRODUCTION_SMOKE
    let cfg = DeploymentSmokeConfig::new("");
    let r = DeploymentSmokeRunner::run(Some(cfg));
    assert_eq!(r.overall_status, ProviderStatus::ExternalRequired);
}

#[test]
fn deployment_smoke_with_live_url_is_not_run_in_hermetic() {
    // PRODUCTION_SMOKE — even with URL, hermetic harness returns NOT_RUN (real live requires network)
    // This test documents that DEPLOYMENT_BASE_URL=... alone does not make it PASS
    // Real live smoke would be executed via run-external-validation.sh deployment
    let cfg = DeploymentSmokeConfig::new("https://prod.example.com");
    let r = DeploymentSmokeRunner::run(Some(cfg));
    // In hermetic, still NOT_RUN/EXTERNAL_REQUIRED, not PASS
    assert!(
        r.overall_status == ProviderStatus::NotRun
            || r.overall_status == ProviderStatus::ExternalRequired
    );
}
