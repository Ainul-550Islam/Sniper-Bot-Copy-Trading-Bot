//! Production/staging deployment smoke model (Batch 7).
//! Validates /api/health, readiness, OpenAPI, migration state, Redis, CORS/security headers, frontend.
//! Never claims deployment occurred when no URL was tested.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider_contract::ProviderStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSmokeConfig {
    pub base_url: String,
    pub timeout: Duration,
    pub expect_version: Option<String>,
    pub check_frontend: bool,
}

impl DeploymentSmokeConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(10),
            expect_version: None,
            check_frontend: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmokeCheck {
    pub name: String,
    pub endpoint: String,
    pub status: ProviderStatus,
    pub detail: String,
    pub latency_ms: Option<u64>,
    pub http_status: Option<u16>,
}

impl SmokeCheck {
    pub fn not_run(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            endpoint: endpoint.into(),
            status: ProviderStatus::NotRun,
            detail: detail.into(),
            latency_ms: None,
            http_status: None,
        }
    }

    pub fn pass(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        detail: impl Into<String>,
        latency_ms: u64,
        http_status: u16,
    ) -> Self {
        Self {
            name: name.into(),
            endpoint: endpoint.into(),
            status: ProviderStatus::Pass,
            detail: detail.into(),
            latency_ms: Some(latency_ms),
            http_status: Some(http_status),
        }
    }

    pub fn fail(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            endpoint: endpoint.into(),
            status: ProviderStatus::Fail,
            detail: detail.into(),
            latency_ms: None,
            http_status: None,
        }
    }

    pub fn external_required(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            endpoint: endpoint.into(),
            status: ProviderStatus::ExternalRequired,
            detail: detail.into(),
            latency_ms: None,
            http_status: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSmokeReport {
    pub base_url: Option<String>,
    pub overall_status: ProviderStatus,
    pub checks: Vec<SmokeCheck>,
    pub detail: String,
}

impl DeploymentSmokeReport {
    pub fn not_run(detail: impl Into<String>) -> Self {
        Self {
            base_url: None,
            overall_status: ProviderStatus::NotRun,
            checks: vec![],
            detail: detail.into(),
        }
    }

    pub fn external_required(detail: impl Into<String>) -> Self {
        Self {
            base_url: None,
            overall_status: ProviderStatus::ExternalRequired,
            checks: vec![],
            detail: detail.into(),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "base_url": self.base_url.as_ref().map(|u| redact_url(u)),
            "overall_status": self.overall_status.as_str(),
            "checks": self.checks,
            "detail": self.detail,
        })
    }
}

fn redact_url(url: &str) -> String {
    // Redact query secrets, keep host/path
    if let Some(idx) = url.find('?') {
        format!("{}?<redacted>", &url[..idx])
    } else {
        url.to_string()
    }
}

pub struct DeploymentSmokeRunner;

impl DeploymentSmokeRunner {
    pub fn run(config: Option<DeploymentSmokeConfig>) -> DeploymentSmokeReport {
        let cfg = match config {
            Some(c) if !c.base_url.trim().is_empty() => c,
            None => {
                return DeploymentSmokeReport::not_run(
                    "DEPLOYMENT_BASE_URL not set — deployment smoke NOT_RUN (no URL tested, no claim of deployment)",
                );
            }
            Some(_) => {
                return DeploymentSmokeReport::external_required(
                    "DEPLOYMENT_BASE_URL empty — deployment smoke EXTERNAL_REQUIRED (no URL tested, no claim of deployment)",
                );
            }
        };

        // Never claim deployment occurred when no URL was tested — we have URL, now perform read-only checks
        // In this harness, without real network we return NOT_RUN with checks marked EXTERNAL_REQUIRED
        // Real implementation would use reqwest with timeout to hit:
        // - GET {base_url}/api/health
        // - GET {base_url}/ready
        // - GET {base_url}/api/saas/openapi.json
        // - GET {base_url}/ (frontend if check_frontend)
        // And verify CORS/security headers, migration state via health payload

        // For hermetic mode (no network), mark as NOT_RUN with expected checks listed as EXTERNAL_REQUIRED
        let checks = vec![
            SmokeCheck::external_required(
                "health",
                format!("{}/api/health", cfg.base_url),
                "requires real deployment — GET /api/health not executed in hermetic mode",
            ),
            SmokeCheck::external_required(
                "readiness",
                format!("{}/ready", cfg.base_url),
                "requires real deployment — GET /ready not executed",
            ),
            SmokeCheck::external_required(
                "openapi",
                format!("{}/api/saas/openapi.json", cfg.base_url),
                "requires real deployment — GET /api/saas/openapi.json not executed",
            ),
            SmokeCheck::external_required(
                "migration_state",
                format!("{}/api/health", cfg.base_url),
                "requires real deployment — migration state from health payload not executed",
            ),
            SmokeCheck::external_required(
                "redis_readiness",
                format!("{}/ready", cfg.base_url),
                "requires real deployment + Redis where required",
            ),
            SmokeCheck::external_required(
                "cors_headers",
                format!("{}/api/health", cfg.base_url),
                "requires real deployment — CORS/security headers not checked",
            ),
        ];

        let mut report = DeploymentSmokeReport {
            base_url: Some(cfg.base_url.clone()),
            overall_status: ProviderStatus::NotRun,
            checks,
            detail: "deployment smoke NOT_RUN — no live URL tested in this harness; real checks require DEPLOYMENT_BASE_URL and network".into(),
        };

        // If env DEPLOYMENT_SMOKE_LIVE=1 and we could actually fetch, we would set PASS/FAIL per check
        // But we never claim PASS without real evidence
        if std::env::var("DEPLOYMENT_SMOKE_LIVE").unwrap_or_default() == "1" {
            report.detail = "DEPLOYMENT_SMOKE_LIVE=1 set but live smoke not executed in this hermetic runner — requires explicit live runner".into();
            report.overall_status = ProviderStatus::ExternalRequired;
        }

        report
    }

    pub fn run_live(cfg: DeploymentSmokeConfig) -> DeploymentSmokeReport {
        // This would perform real HTTP with reqwest, timeout, redacted logging
        // For now, return external_required to avoid fake PASS
        let mut r = Self::run(Some(cfg));
        r.detail = "live deployment smoke requires real network — not executed in unit test".into();
        r.overall_status = ProviderStatus::ExternalRequired;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_url_never_claims_deployment() {
        let r = DeploymentSmokeRunner::run(None);
        assert_eq!(r.overall_status, ProviderStatus::NotRun);
        assert!(r.base_url.is_none());
        assert!(r.detail.contains("no URL tested") || r.detail.contains("DEPLOYMENT_BASE_URL"));
        assert_eq!(r.checks.len(), 0);
    }

    #[test]
    fn empty_url_is_not_run() {
        let cfg = DeploymentSmokeConfig::new("");
        let r = DeploymentSmokeRunner::run(Some(cfg));
        assert_eq!(r.overall_status, ProviderStatus::ExternalRequired);
    }

    #[test]
    fn with_url_checks_are_external_required_not_pass() {
        let cfg = DeploymentSmokeConfig::new("https://example.com");
        let r = DeploymentSmokeRunner::run(Some(cfg));
        assert_eq!(r.overall_status, ProviderStatus::NotRun);
        assert!(r.base_url.is_some());
        for c in &r.checks {
            assert_ne!(
                c.status,
                ProviderStatus::Pass,
                "must not auto PASS without real fetch"
            );
            assert_eq!(c.status, ProviderStatus::ExternalRequired);
        }
        assert!(r.checks.iter().any(|c| c.name == "health"));
        assert!(r.checks.iter().any(|c| c.name == "readiness"));
        assert!(r.checks.iter().any(|c| c.name == "openapi"));
    }

    #[test]
    fn redact_url_hides_query() {
        assert_eq!(
            redact_url("https://example.com/api/health?token=secret"),
            "https://example.com/api/health?<redacted>"
        );
        assert_eq!(
            redact_url("https://example.com/api/health"),
            "https://example.com/api/health"
        );
    }

    /// Batch 10: the smoke checks must name routes the server actually serves
    /// (`/api/health`, `/ready`, `/api/saas/openapi.json`). A check pointing at a
    /// route that does not exist would produce a confusing 404 FAIL for a buyer.
    #[test]
    fn smoke_check_endpoints_are_real_routes() {
        let cfg = DeploymentSmokeConfig::new("https://prod.example.com");
        let r = DeploymentSmokeRunner::run(Some(cfg));
        assert!(!r.checks.is_empty());
        for c in &r.checks {
            assert!(
                !c.endpoint.contains("/api/ready"),
                "{}: /api/ready is not served (real route is /ready)",
                c.name
            );
            let path = c
                .endpoint
                .strip_prefix("https://prod.example.com")
                .unwrap_or(&c.endpoint);
            assert!(
                ["/api/health", "/ready", "/api/saas/openapi.json"].contains(&path),
                "{}: unexpected endpoint {}",
                c.name,
                c.endpoint
            );
        }
    }

    #[test]
    fn never_claim_pass_without_evidence() {
        let cfg = DeploymentSmokeConfig::new("https://example.com");
        let r = DeploymentSmokeRunner::run(Some(cfg));
        assert_ne!(r.overall_status, ProviderStatus::Pass);
    }
}
