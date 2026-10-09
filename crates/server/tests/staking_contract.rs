//! Staking contract harness (Batch 7).
//! If STAKING_E2E != 1: NOT_RUN. If enabled: verify required validator/program deployment contract.
//! Never claim historical results are current. LIVE_EXTERNAL when STAKING_E2E=1, NOT_RUN otherwise.

use sniper_suite::ops::provider_contract::ProviderStatus;
use sniper_suite::staking::deployment_contract::{
    StakingDeploymentConfig, StakingDeploymentContract,
};
use sniper_suite::staking::validator_contract::{ValidatorContract, ValidatorContractConfig};

#[test]
fn staking_deployment_not_run_without_config() {
    // LIVE_EXTERNAL
    let r = StakingDeploymentContract::check(None);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
    assert!(!r.program_exists);
}

#[test]
fn staking_deployment_placeholder_is_blocked() {
    // LIVE_EXTERNAL
    let cfg = StakingDeploymentConfig::new(
        "https://api.mainnet-beta.solana.com",
        "3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy",
    );
    let r = StakingDeploymentContract::check(Some(cfg));
    assert_eq!(r.status, ProviderStatus::Blocked);
    assert!(r.detail.contains("placeholder"));
}

#[test]
fn staking_deployment_with_real_id_not_run_in_hermetic() {
    // LIVE_EXTERNAL
    let cfg = StakingDeploymentConfig::new(
        "https://api.mainnet-beta.solana.com",
        "Stak1ng11111111111111111111111111111111111",
    );
    let r = StakingDeploymentContract::check(Some(cfg));
    assert_eq!(r.status, ProviderStatus::NotRun);
    assert_ne!(r.status, ProviderStatus::Pass);
}

#[test]
fn validator_not_run_when_not_enabled() {
    // LIVE_EXTERNAL
    let cfg = ValidatorContractConfig::enabled(false, false, false);
    let r = ValidatorContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::NotRun);
    assert!(!r.e2e_executed);
    assert!(r.detail.contains("STAKING_E2E"));
}

#[test]
fn validator_external_required_when_validator_missing() {
    // LIVE_EXTERNAL
    let cfg = ValidatorContractConfig::enabled(true, false, true);
    let r = ValidatorContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
}

#[test]
fn validator_never_converts_historical() {
    // HERMETIC — historical evidence must not become current PASS
    assert_eq!(
        ValidatorContract::never_convert_historical(3, false),
        ProviderStatus::NotRun
    );
    assert_eq!(
        ValidatorContract::never_convert_historical(3, true),
        ProviderStatus::Pass
    );
    assert_eq!(
        ValidatorContract::never_convert_historical(0, true),
        ProviderStatus::NotRun
    );
}

#[test]
fn validator_with_all_available_still_not_run_in_hermetic() {
    // LIVE_EXTERNAL — even with STAKING_E2E=1 and validator available, hermetic harness returns NOT_RUN
    let cfg = ValidatorContractConfig::enabled(true, true, true);
    let r = ValidatorContract::check(cfg);
    assert_eq!(r.status, ProviderStatus::NotRun);
    assert!(!r.e2e_executed);
}
