//! Machine-readable integration verification matrix (Batch 3).
//!
//! Tracks unit, PostgreSQL integration, Redis integration, frontend, provider fixture,
//! live provider, staking validator, production deployment with PASS/NOT_RUN/BLOCKED/EXTERNAL_REQUIRED.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MatrixStatus {
    Pass,
    NotRun,
    Blocked,
    ExternalRequired,
}

impl MatrixStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MatrixStatus::Pass => "PASS",
            MatrixStatus::NotRun => "NOT_RUN",
            MatrixStatus::Blocked => "BLOCKED",
            MatrixStatus::ExternalRequired => "EXTERNAL_REQUIRED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixEntry {
    pub status: MatrixStatus,
    pub detail: String,
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationMatrix {
    pub version: String,
    pub generated_at: String,
    pub entries: BTreeMap<String, MatrixEntry>,
}

impl IntegrationMatrix {
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            entries: BTreeMap::new(),
        }
    }

    pub fn set(
        &mut self,
        key: impl Into<String>,
        status: MatrixStatus,
        detail: impl Into<String>,
        command: Option<String>,
    ) {
        self.entries.insert(
            key.into(),
            MatrixEntry {
                status,
                detail: detail.into(),
                command,
            },
        );
    }

    pub fn overall_pass(&self) -> bool {
        // Overall pass requires unit PASS; external checks are allowed to be EXTERNAL_REQUIRED
        self.entries
            .get("unit")
            .map(|e| e.status == MatrixStatus::Pass)
            .unwrap_or(false)
    }

    /// Generate default matrix from current environment evidence (no fabrication).
    pub fn default_for_current(version: &str) -> Self {
        let mut m = Self::new(version);
        // These will be filled by caller with real results; defaults are NOT_RUN
        m.set(
            "unit",
            MatrixStatus::NotRun,
            "not yet executed in this run",
            Some("cargo test --workspace -- --test-threads=1".into()),
        );
        m.set(
            "postgres_integration",
            MatrixStatus::NotRun,
            "requires POSTGRES_URL",
            Some("cargo test -p bot-core --test db_integration -- --ignored".into()),
        );
        m.set(
            "redis_integration",
            MatrixStatus::NotRun,
            "requires REDIS_URL",
            Some("cargo test -p bot-core --test redis_integration -- --ignored".into()),
        );
        m.set(
            "frontend",
            MatrixStatus::NotRun,
            "requires npm",
            Some("cd apps/control-plane && npm ci && npm run typecheck && npm run build".into()),
        );
        m.set(
            "provider_fixture",
            MatrixStatus::NotRun,
            "fixture tests not yet run",
            Some("cargo test -p bot-core --lib billing::provider_events".into()),
        );
        m.set(
            "live_provider",
            MatrixStatus::ExternalRequired,
            "requires STRIPE_API_KEY/PADDLE + network",
            None,
        );
        m.set(
            "staking_validator",
            MatrixStatus::ExternalRequired,
            "requires solana-test-validator + agave",
            None,
        );
        m.set(
            "production_deployment",
            MatrixStatus::ExternalRequired,
            "requires funded deployment",
            None,
        );
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_not_run() {
        let m = IntegrationMatrix::default_for_current("0.1.0");
        assert_eq!(m.entries["unit"].status, MatrixStatus::NotRun);
        assert_eq!(
            m.entries["live_provider"].status,
            MatrixStatus::ExternalRequired
        );
    }

    #[test]
    fn external_not_converted_to_pass() {
        let mut m = IntegrationMatrix::new("0.1.0");
        m.set(
            "live_provider",
            MatrixStatus::ExternalRequired,
            "needs creds",
            None,
        );
        m.set("unit", MatrixStatus::Pass, "ok", None);
        assert_ne!(m.entries["live_provider"].status, MatrixStatus::Pass);
        assert!(m.overall_pass());
    }

    #[test]
    fn blocked_is_distinct() {
        let mut m = IntegrationMatrix::new("0.1.0");
        m.set(
            "postgres_integration",
            MatrixStatus::Blocked,
            "docker unavailable",
            Some("cargo test".into()),
        );
        assert_eq!(
            m.entries["postgres_integration"].status,
            MatrixStatus::Blocked
        );
    }
}
