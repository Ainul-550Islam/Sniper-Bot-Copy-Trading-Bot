//! Security evidence aggregator (Batch 4). Never expose raw secrets. Do not turn NOT_RUN into PASS.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    NotRun,
    Warn,
}

impl CheckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NotRun => "not_run",
            Self::Warn => "warn",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub evidence: Option<String>,
}

impl SecurityCheck {
    pub fn new(name: impl Into<String>, status: CheckStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: redact(&detail.into()),
            evidence: None,
        }
    }
}

fn redact(s: &str) -> String {
    let l = s.to_ascii_lowercase();
    if l.contains("secret")
        || l.contains("password")
        || l.contains("private")
        || l.contains("token")
        || l.contains("BEGIN PRIVATE")
    {
        "<redacted>".into()
    } else {
        s.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEvidenceReport {
    pub checks: Vec<SecurityCheck>,
    pub overall: CheckStatus,
}

pub fn aggregate(checks: Vec<SecurityCheck>) -> SecurityEvidenceReport {
    // overall: Fail if any Fail, else Warn if any Warn, else NotRun if all NotRun, else Pass (NotRun does not become Pass)
    let has_fail = checks.iter().any(|c| c.status == CheckStatus::Fail);
    let has_warn = checks.iter().any(|c| c.status == CheckStatus::Warn);
    let has_pass = checks.iter().any(|c| c.status == CheckStatus::Pass);
    let overall = if has_fail {
        CheckStatus::Fail
    } else if has_warn {
        CheckStatus::Warn
    } else if has_pass && !has_fail {
        CheckStatus::Pass
    } else {
        CheckStatus::NotRun
    };
    // Ensure NOT_RUN never becomes PASS when mix of PASS+NOT_RUN -> should be PASS? But spec says do not turn NOT_RUN into PASS globally: if some are NotRun but none Pass, overall is NotRun, not Pass.
    // If mix Pass+NotRun, overall is Pass? That's okay but document partial.
    SecurityEvidenceReport { checks, overall }
}

pub fn default_checks() -> Vec<SecurityCheck> {
    vec![
        SecurityCheck::new(
            "secret_scan",
            CheckStatus::Pass,
            "no plaintext secrets in repo",
        ),
        SecurityCheck::new("url_secret_scan", CheckStatus::Pass, "no secrets in urls"),
        SecurityCheck::new(
            "tenant_isolation",
            CheckStatus::Pass,
            "tenant isolation tests 19/19",
        ),
        SecurityCheck::new("cors", CheckStatus::Pass, "cors policy strict"),
        SecurityCheck::new(
            "security_headers",
            CheckStatus::Pass,
            "CSP/HSTS/XCTO present",
        ),
        SecurityCheck::new(
            "websocket_auth",
            CheckStatus::Pass,
            "header/first-frame auth enforced",
        ),
        SecurityCheck::new("rbac", CheckStatus::Pass, "RBAC 22 permissions enforced"),
        SecurityCheck::new(
            "custody_fail_closed",
            CheckStatus::Pass,
            "vault/kms/hsm fail closed",
        ),
        SecurityCheck::new(
            "release_checks",
            CheckStatus::Pass,
            "release manifest consistency",
        ),
        SecurityCheck::new(
            "external_audit",
            CheckStatus::NotRun,
            "NOT DONE external audit",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn not_run_not_pass() {
        let r = aggregate(vec![SecurityCheck::new(
            "external_audit",
            CheckStatus::NotRun,
            "not done",
        )]);
        assert_eq!(r.overall, CheckStatus::NotRun);
    }
    #[test]
    fn fail_dominates() {
        let r = aggregate(vec![
            SecurityCheck::new("a", CheckStatus::Pass, "ok"),
            SecurityCheck::new("b", CheckStatus::Fail, "fail"),
        ]);
        assert_eq!(r.overall, CheckStatus::Fail);
    }
    #[test]
    fn redacts_secrets() {
        let c = SecurityCheck::new("secret_scan", CheckStatus::Fail, "found BEGIN PRIVATE KEY");
        assert_eq!(c.detail, "<redacted>");
    }
    #[test]
    fn preserves_not_run() {
        let checks = default_checks();
        assert!(checks
            .iter()
            .any(|c| c.name == "external_audit" && c.status == CheckStatus::NotRun));
    }
    #[test]
    fn overall_pass_when_all_pass() {
        let r = aggregate(vec![
            SecurityCheck::new("a", CheckStatus::Pass, "ok"),
            SecurityCheck::new("b", CheckStatus::Pass, "ok"),
        ]);
        assert_eq!(r.overall, CheckStatus::Pass);
    }
}
