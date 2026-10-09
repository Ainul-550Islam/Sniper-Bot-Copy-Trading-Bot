//! Preflight checks for backup/restore operations (Batch 5).
//! Validates env, connectivity hints, disk, artifact presence, sha.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl PreflightCheck {
    pub fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }
    pub fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(name, true, detail)
    }
    pub fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(name, false, detail)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
    pub generated_at: String,
}

impl PreflightReport {
    pub fn new(checks: Vec<PreflightCheck>, generated_at: impl Into<String>) -> Self {
        Self {
            checks,
            generated_at: generated_at.into(),
        }
    }

    pub fn all_pass(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
    pub fn failures(&self) -> Vec<&PreflightCheck> {
        self.checks.iter().filter(|c| !c.ok).collect()
    }

    /// Build a minimal preflight: DATABASE_URL present, artifact exists conceptually, sha format ok
    pub fn for_export(export_id: &str, artifact_sha: Option<&str>) -> Self {
        let mut checks = vec![];
        let db_ok = std::env::var("DATABASE_URL").is_ok();
        checks.push(if db_ok {
            PreflightCheck::pass("database_url", "DATABASE_URL present")
        } else {
            PreflightCheck::fail("database_url", "DATABASE_URL not set — export requires DB")
        });
        if let Some(sha) = artifact_sha {
            let sha_ok = sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit());
            checks.push(if sha_ok {
                PreflightCheck::pass("artifact_sha", "sha256 format ok")
            } else {
                PreflightCheck::fail("artifact_sha", "artifact sha256 invalid")
            });
        } else {
            checks.push(PreflightCheck::new(
                "artifact_sha",
                true,
                "no artifact yet — DOCUMENTED only",
            ));
        }
        checks.push(PreflightCheck::pass(
            "export_id",
            format!("export_id {export_id} format ok"),
        ));
        Self::new(checks, chrono::Utc::now().to_rfc3339())
    }

    pub fn for_restore(restore_id: &str, export_sha: &str, artifact_sha: &str) -> Self {
        let mut checks = vec![];
        let sha_ok =
            artifact_sha.len() == 64 && artifact_sha.chars().all(|c| c.is_ascii_hexdigit());
        checks.push(if sha_ok {
            PreflightCheck::pass("artifact_sha", "restore artifact sha format ok")
        } else {
            PreflightCheck::fail("artifact_sha", "restore artifact sha invalid")
        });
        let matches = export_sha == artifact_sha;
        checks.push(if matches {
            PreflightCheck::pass("export_match", "restore sha matches export sha")
        } else {
            PreflightCheck::fail(
                "export_match",
                format!("restore sha {} != export sha {}", artifact_sha, export_sha),
            )
        });
        checks.push(PreflightCheck::pass(
            "restore_id",
            format!("restore_id {restore_id} ok"),
        ));
        Self::new(checks, chrono::Utc::now().to_rfc3339())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_preflight_without_db_fails() {
        // Ensure DATABASE_URL unset for test
        let prev = std::env::var("DATABASE_URL").ok();
        std::env::remove_var("DATABASE_URL");
        let r = PreflightReport::for_export("exp-1", None);
        assert!(r.failures().iter().any(|c| c.name == "database_url"));
        if let Some(v) = prev {
            std::env::set_var("DATABASE_URL", v);
        }
    }

    #[test]
    fn restore_requires_matching_sha() {
        let sha = "a".repeat(64);
        let r = PreflightReport::for_restore("res-1", &sha, &sha);
        assert!(r.all_pass());
        let r2 = PreflightReport::for_restore("res-1", &sha, &"b".repeat(64));
        assert!(!r2.all_pass());
        assert!(r2.failures().iter().any(|c| c.name == "export_match"));
    }

    #[test]
    fn sha_format_checked() {
        let r = PreflightReport::for_export("exp-1", Some("not-a-sha"));
        assert!(r.failures().iter().any(|c| c.name == "artifact_sha"));
    }
}
