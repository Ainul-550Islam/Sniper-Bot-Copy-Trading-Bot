//! Staking-program deployment verification model (Batch 7).
//! Verify program ID, deployed account exists, executable state, expected binary hash if configured.
//! No claim of deployment without real RPC evidence.

use serde::{Deserialize, Serialize};

use crate::ops::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingDeploymentConfig {
    pub rpc_url: String,
    pub program_id: String,
    pub expected_binary_hash: Option<String>,
}

impl StakingDeploymentConfig {
    pub fn new(rpc_url: impl Into<String>, program_id: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            program_id: program_id.into(),
            expected_binary_hash: None,
        }
    }

    pub fn with_expected_hash(mut self, hash: impl Into<String>) -> Self {
        self.expected_binary_hash = Some(hash.into());
        self
    }

    pub fn redacted_program_id(&self) -> String {
        // Program ID is public, but we redact if it looks like placeholder
        if self.program_id.starts_with("3vEEMM") {
            "<placeholder program id>".into()
        } else {
            self.program_id.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingDeploymentResult {
    pub program_exists: bool,
    pub executable: bool,
    pub binary_hash_match: Option<bool>,
    pub slot: Option<u64>,
    pub status: ProviderStatus,
    pub detail: String,
    pub redacted_program_id: String,
}

impl StakingDeploymentResult {
    pub fn not_run(detail: impl Into<String>) -> Self {
        Self {
            program_exists: false,
            executable: false,
            binary_hash_match: None,
            slot: None,
            status: ProviderStatus::NotRun,
            detail: detail.into(),
            redacted_program_id: "<not configured>".into(),
        }
    }

    pub fn external_required(detail: impl Into<String>) -> Self {
        Self {
            program_exists: false,
            executable: false,
            binary_hash_match: None,
            slot: None,
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
            redacted_program_id: "<redacted>".into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "program_exists": self.program_exists,
            "executable": self.executable,
            "binary_hash_match": self.binary_hash_match,
            "slot": self.slot,
            "status": self.status.as_str(),
            "detail": self.detail,
            "redacted_program_id": self.redacted_program_id,
        })
    }
}

pub struct StakingDeploymentContract;

impl StakingDeploymentContract {
    pub fn check(config: Option<StakingDeploymentConfig>) -> StakingDeploymentResult {
        let cfg = match config {
            Some(c) if !c.rpc_url.trim().is_empty() && !c.program_id.trim().is_empty() => c,
            _ => {
                return StakingDeploymentResult::external_required(
                    "RPC_URL or STAKING_PROGRAM_ID not set — staking deployment check NOT_RUN (no RPC evidence)",
                );
            }
        };

        // Placeholder program ID should be flagged
        if cfg.program_id.starts_with("3vEEMM") {
            return StakingDeploymentResult {
                program_exists: false,
                executable: false,
                binary_hash_match: None,
                slot: None,
                status: ProviderStatus::Blocked,
                detail: "placeholder program id — deployment not configured, run staking-identity.sh set-id".into(),
                redacted_program_id: cfg.redacted_program_id(),
            };
        }

        // Hermetic: no RPC, return NOT_RUN
        StakingDeploymentResult {
            program_exists: false,
            executable: false,
            binary_hash_match: None,
            slot: None,
            status: ProviderStatus::NotRun,
            detail: format!(
                "staking deployment check NOT_RUN — program {} not verified in hermetic mode; requires real RPC",
                cfg.redacted_program_id()
            ),
            redacted_program_id: cfg.redacted_program_id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_config_is_external_required() {
        let r = StakingDeploymentContract::check(None);
        assert_eq!(r.status, ProviderStatus::ExternalRequired);
        assert!(!r.program_exists);
    }

    #[test]
    fn placeholder_is_blocked() {
        let cfg = StakingDeploymentConfig::new(
            "https://api.mainnet-beta.solana.com",
            "3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy",
        );
        let r = StakingDeploymentContract::check(Some(cfg));
        assert_eq!(r.status, ProviderStatus::Blocked);
        assert!(r.detail.contains("placeholder"));
    }

    #[test]
    fn with_real_id_is_not_run_in_hermetic() {
        let cfg = StakingDeploymentConfig::new(
            "https://api.mainnet-beta.solana.com",
            "Stak1ng11111111111111111111111111111111111",
        );
        let r = StakingDeploymentContract::check(Some(cfg));
        assert_eq!(r.status, ProviderStatus::NotRun);
        assert!(!r.program_exists);
    }

    #[test]
    fn never_claims_deployment_without_rpc() {
        let cfg = StakingDeploymentConfig::new(
            "https://api.mainnet-beta.solana.com",
            "Stak1ng11111111111111111111111111111111111",
        );
        let r = StakingDeploymentContract::check(Some(cfg));
        assert_ne!(r.status, ProviderStatus::Pass);
    }
}
