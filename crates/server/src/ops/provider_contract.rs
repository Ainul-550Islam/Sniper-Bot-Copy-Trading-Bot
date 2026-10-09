//! Canonical external-provider contract model (Batch 7).
//! Defines provider name, capability, required config refs, verification command,
//! expected evidence, and status PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED/BLOCKED.
//! Never defaults an external provider to PASS.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProviderStatus {
    Pass,
    Fail,
    NotRun,
    ExternalRequired,
    Blocked,
}

impl ProviderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotRun => "NOT_RUN",
            Self::ExternalRequired => "EXTERNAL_REQUIRED",
            Self::Blocked => "BLOCKED",
        }
    }

    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapability {
    BillingCheckout,
    BillingWebhook,
    BillingSubscriptionSync,
    CustodySign,
    CustodyKeyDerivation,
    DatabaseMigration,
    RedisReady,
    SolanaRpc,
    SolanaWs,
    SolanaGeyser,
    StakingDeployment,
    StakingValidator,
    DeploymentHealth,
    FundedPreflight,
}

impl ProviderCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BillingCheckout => "billing_checkout",
            Self::BillingWebhook => "billing_webhook",
            Self::BillingSubscriptionSync => "billing_subscription_sync",
            Self::CustodySign => "custody_sign",
            Self::CustodyKeyDerivation => "custody_key_derivation",
            Self::DatabaseMigration => "database_migration",
            Self::RedisReady => "redis_ready",
            Self::SolanaRpc => "solana_rpc",
            Self::SolanaWs => "solana_ws",
            Self::SolanaGeyser => "solana_geyser",
            Self::StakingDeployment => "staking_deployment",
            Self::StakingValidator => "staking_validator",
            Self::DeploymentHealth => "deployment_health",
            Self::FundedPreflight => "funded_preflight",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderContract {
    pub provider: String,
    pub capability: ProviderCapability,
    pub required_refs: Vec<String>,
    pub verification_command: String,
    pub expected_evidence: String,
    pub status: ProviderStatus,
    pub detail: String,
}

impl ProviderContract {
    pub fn new(
        provider: impl Into<String>,
        capability: ProviderCapability,
        required_refs: Vec<String>,
        verification_command: impl Into<String>,
        expected_evidence: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            capability,
            required_refs,
            verification_command: verification_command.into(),
            expected_evidence: expected_evidence.into(),
            status: ProviderStatus::NotRun,
            detail: "not executed — requires explicit enablement".into(),
        }
    }

    pub fn external_required(mut self, detail: impl Into<String>) -> Self {
        self.status = ProviderStatus::ExternalRequired;
        self.detail = detail.into();
        self
    }

    pub fn blocked(mut self, detail: impl Into<String>) -> Self {
        self.status = ProviderStatus::Blocked;
        self.detail = detail.into();
        self
    }

    pub fn pass(mut self, detail: impl Into<String>) -> Self {
        self.status = ProviderStatus::Pass;
        self.detail = detail.into();
        self
    }

    pub fn fail(mut self, detail: impl Into<String>) -> Self {
        self.status = ProviderStatus::Fail;
        self.detail = detail.into();
        self
    }

    pub fn not_run(mut self, detail: impl Into<String>) -> Self {
        self.status = ProviderStatus::NotRun;
        self.detail = detail.into();
        self
    }

    /// Never default to PASS — constructor always NOT_RUN.
    pub fn is_default_pass(&self) -> bool {
        false
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider,
            "capability": self.capability.as_str(),
            "required_refs": self.required_refs,
            "verification_command": self.verification_command,
            "expected_evidence": self.expected_evidence,
            "status": self.status.as_str(),
            "detail": self.detail,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderContractRegistry {
    pub contracts: Vec<ProviderContract>,
}

impl ProviderContractRegistry {
    pub fn new(contracts: Vec<ProviderContract>) -> Self {
        let mut c = Self { contracts };
        c.contracts.sort_by(|a, b| {
            a.provider
                .cmp(&b.provider)
                .then(a.capability.as_str().cmp(b.capability.as_str()))
        });
        c
    }

    pub fn default_registry() -> Self {
        Self::new(vec![
            ProviderContract::new(
                "stripe",
                ProviderCapability::BillingCheckout,
                vec!["STRIPE_API_KEY".into(), "STRIPE_WEBHOOK_SECRET".into()],
                "LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored --nocapture",
                "evidence: provider=stripe, checkout session id redacted, status, timestamp, hash",
            ).external_required("live Stripe requires STRIPE_API_KEY + STRIPE_WEBHOOK_SECRET + LIVE_BILLING=1"),
            ProviderContract::new(
                "paddle",
                ProviderCapability::BillingCheckout,
                vec!["PADDLE_API_KEY".into(), "PADDLE_WEBHOOK_SECRET".into()],
                "LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored --nocapture",
                "evidence: provider=paddle, transaction id redacted, status, timestamp, hash",
            ).external_required("live Paddle requires PADDLE_API_KEY + LIVE_BILLING=1"),
            ProviderContract::new(
                "vault",
                ProviderCapability::CustodySign,
                vec!["VAULT_ADDR".into(), "VAULT_TOKEN".into()],
                "LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored --nocapture",
                "evidence: provider=vault, public key/address, reference id, status, timestamp, hash",
            ).external_required("Vault requires VAULT_ADDR + VAULT_TOKEN + LIVE_CUSTODY=1"),
            ProviderContract::new(
                "kms",
                ProviderCapability::CustodySign,
                vec!["KMS_KEY_ID".into()],
                "LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored --nocapture",
                "evidence: provider=kms, key id redacted, status",
            ).external_required("KMS requires KMS_KEY_ID"),
            ProviderContract::new(
                "hsm",
                ProviderCapability::CustodySign,
                vec!["HSM_SLOT".into()],
                "LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored --nocapture",
                "evidence: provider=hsm, slot redacted",
            ).external_required("HSM requires HSM_SLOT"),
            ProviderContract::new(
                "postgres",
                ProviderCapability::DatabaseMigration,
                vec!["POSTGRES_URL".into()],
                "POSTGRES_URL=postgres://... cargo test --test provider_contracts -- --nocapture",
                "evidence: migration high-water 0021, table count, row count, hash",
            ).external_required("Postgres requires POSTGRES_URL"),
            ProviderContract::new(
                "redis",
                ProviderCapability::RedisReady,
                vec!["REDIS_URL".into()],
                "REDIS_URL=redis://... cargo test --test provider_contracts -- --nocapture",
                "evidence: redis ping latency, status",
            ).external_required("Redis requires REDIS_URL"),
            ProviderContract::new(
                "solana_rpc",
                ProviderCapability::SolanaRpc,
                vec!["RPC_URL".into()],
                "RPC_URL=https://... cargo test --test solana_contract -- --nocapture",
                "evidence: rpc health, slot, latency, no credentials",
            ).external_required("Solana RPC requires RPC_URL"),
            ProviderContract::new(
                "geyser",
                ProviderCapability::SolanaGeyser,
                vec!["GEYSER_URL".into()],
                "GEYSER_URL=... cargo test --test solana_contract -- --nocapture",
                "evidence: geyser subscription, message decode, latency",
            ).external_required("Geyser requires GEYSER_URL"),
            ProviderContract::new(
                "staking_program",
                ProviderCapability::StakingDeployment,
                vec!["RPC_URL".into(), "STAKING_PROGRAM_ID".into()],
                "RPC_URL=... STAKING_PROGRAM_ID=... cargo test --test staking_contract -- --nocapture",
                "evidence: program id, executable, binary hash, slot",
            ).external_required("Staking deployment requires RPC_URL + STAKING_PROGRAM_ID"),
            ProviderContract::new(
                "staking_validator",
                ProviderCapability::StakingValidator,
                vec!["STAKING_E2E".into()],
                "STAKING_E2E=1 cargo test --test staking_contract -- --ignored --nocapture",
                "evidence: validator version, slot, program deployment, 3 E2E tests",
            ).external_required("Validator E2E requires STAKING_E2E=1 + solana-test-validator"),
            ProviderContract::new(
                "deployment",
                ProviderCapability::DeploymentHealth,
                vec!["DEPLOYMENT_BASE_URL".into()],
                "DEPLOYMENT_BASE_URL=https://... cargo test --test deployment_smoke -- --nocapture",
                "evidence: /api/health, /ready, /api/saas/openapi.json, headers, latency",
            ).external_required("Deployment smoke requires DEPLOYMENT_BASE_URL"),
        ])
    }

    pub fn get(&self, provider: &str, capability: ProviderCapability) -> Option<&ProviderContract> {
        self.contracts
            .iter()
            .find(|c| c.provider == provider && c.capability == capability)
    }

    pub fn all_not_pass_without_evidence(&self) -> bool {
        self.contracts
            .iter()
            .all(|c| c.status != ProviderStatus::Pass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_never_pass() {
        let r = ProviderContractRegistry::default_registry();
        for c in &r.contracts {
            assert_ne!(
                c.status,
                ProviderStatus::Pass,
                "provider {} must not default to PASS",
                c.provider
            );
        }
        assert!(r.all_not_pass_without_evidence());
    }

    #[test]
    fn missing_credentials_external_required() {
        let c = ProviderContract::new(
            "stripe",
            ProviderCapability::BillingCheckout,
            vec!["STRIPE_API_KEY".into()],
            "cmd",
            "evidence",
        )
        .external_required("missing STRIPE_API_KEY");
        assert_eq!(c.status, ProviderStatus::ExternalRequired);
    }

    #[test]
    fn status_semantics_exact() {
        assert_eq!(ProviderStatus::Pass.as_str(), "PASS");
        assert_eq!(ProviderStatus::Fail.as_str(), "FAIL");
        assert_eq!(ProviderStatus::NotRun.as_str(), "NOT_RUN");
        assert_eq!(
            ProviderStatus::ExternalRequired.as_str(),
            "EXTERNAL_REQUIRED"
        );
        assert_eq!(ProviderStatus::Blocked.as_str(), "BLOCKED");
    }

    #[test]
    fn to_safe_json_no_secrets() {
        let c = ProviderContract::new(
            "test",
            ProviderCapability::SolanaRpc,
            vec!["RPC_URL".into()],
            "cmd",
            "evidence",
        );
        let v = c.to_safe_json();
        let s = v.to_string();
        assert!(!s.contains("sk_live"));
        assert!(s.contains("test"));
    }
}
