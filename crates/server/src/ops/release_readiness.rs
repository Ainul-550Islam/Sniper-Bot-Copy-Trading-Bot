//! Operator-only release readiness evaluation (Batch 3).
//!
//! Checks migrations, workspace build, test gate, frontend build gate,
//! release-manifest consistency, secrets scan, stale-doc scan, production-risk flags,
//! external-validation requirements. Outputs structured PASS/WARN/BLOCK.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Warn,
    Block,
    NotRun,
}

impl GateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateStatus::Pass => "PASS",
            GateStatus::Warn => "WARN",
            GateStatus::Block => "BLOCK",
            GateStatus::NotRun => "NOT_RUN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    pub gate: String,
    pub status: GateStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseReadiness {
    pub version: String,
    pub as_of: String,
    pub gates: Vec<GateResult>,
    pub overall: GateStatus,
}

impl ReleaseReadiness {
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            as_of: chrono::Utc::now().to_rfc3339(),
            gates: Vec::new(),
            overall: GateStatus::Pass,
        }
    }

    pub fn push(&mut self, gate: impl Into<String>, status: GateStatus, detail: impl Into<String>) {
        self.gates.push(GateResult {
            gate: gate.into(),
            status,
            detail: detail.into(),
        });
        self.recompute();
    }

    fn recompute(&mut self) {
        // BLOCK > WARN > PASS; NOT_RUN is WARN unless explicitly allowed
        let mut overall = GateStatus::Pass;
        for g in &self.gates {
            match g.status {
                GateStatus::Block => {
                    overall = GateStatus::Block;
                    break;
                }
                GateStatus::Warn | GateStatus::NotRun => {
                    if overall != GateStatus::Block {
                        overall = GateStatus::Warn;
                    }
                }
                GateStatus::Pass => {}
            }
        }
        self.overall = overall;
    }
}

/// Evaluate readiness from supplied evidence (no I/O, purely functional).
/// Each check is deterministic; callers supply file contents / command results.
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    version: &str,
    migration_high_water: &str,
    expected_migration: &str,
    workspace_members: usize,
    expected_members: usize,
    test_passed: Option<bool>,
    frontend_build: Option<bool>,
    manifest_consistent: bool,
    secrets_found: bool,
    stale_claims: usize,
    external_validations_claimed: bool,
) -> ReleaseReadiness {
    let mut r = ReleaseReadiness::new(version);
    // Migration
    if migration_high_water == expected_migration {
        r.push(
            "migrations",
            GateStatus::Pass,
            format!("high_water={}", migration_high_water),
        );
    } else {
        r.push(
            "migrations",
            GateStatus::Block,
            format!(
                "high_water={} expected={}",
                migration_high_water, expected_migration
            ),
        );
    }
    // Workspace members
    if workspace_members == expected_members {
        r.push(
            "workspace_members",
            GateStatus::Pass,
            format!("members={}", workspace_members),
        );
    } else {
        r.push(
            "workspace_members",
            GateStatus::Block,
            format!(
                "members={} expected={}",
                workspace_members, expected_members
            ),
        );
    }
    // Test gate
    match test_passed {
        Some(true) => r.push("tests", GateStatus::Pass, "cargo test passed"),
        Some(false) => r.push("tests", GateStatus::Block, "cargo test failed"),
        None => r.push("tests", GateStatus::NotRun, "cargo test NOT RUN"),
    }
    // Frontend
    match frontend_build {
        Some(true) => r.push("frontend_build", GateStatus::Pass, "npm run build ok"),
        Some(false) => r.push("frontend_build", GateStatus::Block, "frontend build failed"),
        None => r.push("frontend_build", GateStatus::Warn, "frontend build NOT RUN"),
    }
    // Manifest
    if manifest_consistent {
        r.push("release_manifest", GateStatus::Pass, "consistent");
    } else {
        r.push(
            "release_manifest",
            GateStatus::Block,
            "inconsistent with BUYER-TRUTH-REGISTER",
        );
    }
    // Secrets
    if secrets_found {
        r.push("secrets_scan", GateStatus::Block, "secrets found in scan");
    } else {
        r.push("secrets_scan", GateStatus::Pass, "no secrets");
    }
    // Stale docs
    if stale_claims == 0 {
        r.push("stale_claims", GateStatus::Pass, "no stale claims");
    } else {
        r.push(
            "stale_claims",
            GateStatus::Warn,
            format!("{} stale claims", stale_claims),
        );
    }
    // External validation honesty
    if external_validations_claimed {
        r.push(
            "external_validation",
            GateStatus::Block,
            "falsely claimed external validation without execution",
        );
    } else {
        r.push("external_validation", GateStatus::Pass, "no false claims");
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pass_when_evidence_ok() {
        let r = evaluate(
            "0.1.0",
            "0021",
            "0021",
            8,
            8,
            Some(true),
            Some(true),
            true,
            false,
            0,
            false,
        );
        assert_eq!(r.overall, GateStatus::Pass);
    }

    #[test]
    fn block_on_migration_mismatch() {
        let r = evaluate(
            "0.1.0",
            "0018",
            "0021",
            8,
            8,
            Some(true),
            Some(true),
            true,
            false,
            0,
            false,
        );
        assert_eq!(r.overall, GateStatus::Block);
        assert!(r
            .gates
            .iter()
            .any(|g| g.gate == "migrations" && g.status == GateStatus::Block));
    }

    #[test]
    fn warn_on_stale() {
        let r = evaluate(
            "0.1.0",
            "0021",
            "0021",
            8,
            8,
            Some(true),
            Some(true),
            true,
            false,
            2,
            false,
        );
        assert_eq!(r.overall, GateStatus::Warn);
    }

    #[test]
    fn block_on_false_external_claim() {
        let r = evaluate(
            "0.1.0",
            "0021",
            "0021",
            8,
            8,
            Some(true),
            Some(true),
            true,
            false,
            0,
            true,
        );
        assert_eq!(r.overall, GateStatus::Block);
    }

    #[test]
    fn not_run_is_warn() {
        let r = evaluate(
            "0.1.0", "0021", "0021", 8, 8, None, None, true, false, 0, false,
        );
        assert_eq!(r.overall, GateStatus::Warn);
    }
}
