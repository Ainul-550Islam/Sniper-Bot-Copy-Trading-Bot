//! Staking validator E2E contract wrapper (Batch 7).
//! Execute/read status only when STAKING_E2E=1 and required validator/toolchain available.
//! Otherwise return NOT_RUN. Never convert historical evidence into current PASS.

use serde::{Deserialize, Serialize};

use crate::ops::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorContractConfig {
    pub staking_e2e: bool,
    pub validator_available: bool,
    pub toolchain_available: bool,
}

impl ValidatorContractConfig {
    pub fn from_env() -> Self {
        let staking_e2e = std::env::var("STAKING_E2E")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            staking_e2e,
            validator_available: false,
            toolchain_available: false,
        }
    }

    pub fn enabled(
        staking_e2e: bool,
        validator_available: bool,
        toolchain_available: bool,
    ) -> Self {
        Self {
            staking_e2e,
            validator_available,
            toolchain_available,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorContractResult {
    pub e2e_executed: bool,
    pub tests_passed: Option<u32>,
    pub tests_failed: Option<u32>,
    pub validator_version: Option<String>,
    pub status: ProviderStatus,
    pub detail: String,
}

impl ValidatorContractResult {
    pub fn not_run(detail: impl Into<String>) -> Self {
        Self {
            e2e_executed: false,
            tests_passed: None,
            tests_failed: None,
            validator_version: None,
            status: ProviderStatus::NotRun,
            detail: detail.into(),
        }
    }

    pub fn external_required(detail: impl Into<String>) -> Self {
        Self {
            e2e_executed: false,
            tests_passed: None,
            tests_failed: None,
            validator_version: None,
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "e2e_executed": self.e2e_executed,
            "tests_passed": self.tests_passed,
            "tests_failed": self.tests_failed,
            "validator_version": self.validator_version,
            "status": self.status.as_str(),
            "detail": self.detail,
        })
    }
}

pub struct ValidatorContract;

impl ValidatorContract {
    pub fn check(config: ValidatorContractConfig) -> ValidatorContractResult {
        if !config.staking_e2e {
            return ValidatorContractResult::not_run(
                "STAKING_E2E != 1 — validator E2E NOT_RUN (requires STAKING_E2E=1 + solana-test-validator + cargo build-sbf)",
            );
        }
        if !config.validator_available {
            return ValidatorContractResult::external_required(
                "STAKING_E2E=1 but validator not available — EXTERNAL_REQUIRED (install agave/solana-test-validator)",
            );
        }
        if !config.toolchain_available {
            return ValidatorContractResult::external_required(
                "STAKING_E2E=1 but toolchain not available — EXTERNAL_REQUIRED (install agave 2.1.21)",
            );
        }

        // Hermetic: even with STAKING_E2E=1, if we are in unit test without real validator, return NOT_RUN
        // Real E2E would: cargo build-sbf + solana-test-validator + 3 tests, capture output, hash
        ValidatorContractResult {
            e2e_executed: false,
            tests_passed: None,
            tests_failed: None,
            validator_version: None,
            status: ProviderStatus::NotRun,
            detail: "validator E2E NOT_RUN — not executed in this hermetic harness; run `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` with validator (programs/staking-suite is its own excluded workspace)".into(),
        }
    }

    pub fn check_env() -> ValidatorContractResult {
        Self::check(ValidatorContractConfig::from_env())
    }

    pub fn never_convert_historical(passed: u32, is_current: bool) -> ProviderStatus {
        if is_current && passed > 0 {
            ProviderStatus::Pass
        } else {
            ProviderStatus::NotRun
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_run_when_not_enabled() {
        let cfg = ValidatorContractConfig::enabled(false, false, false);
        let r = ValidatorContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(!r.e2e_executed);
        assert!(r.detail.contains("STAKING_E2E"));
    }

    #[test]
    fn external_required_when_validator_missing() {
        let cfg = ValidatorContractConfig::enabled(true, false, true);
        let r = ValidatorContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
    }

    #[test]
    fn never_converts_historical() {
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
    fn with_all_available_still_not_run_in_hermetic() {
        let cfg = ValidatorContractConfig::enabled(true, true, true);
        let r = ValidatorContract::check(cfg);
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(!r.e2e_executed);
    }
}
