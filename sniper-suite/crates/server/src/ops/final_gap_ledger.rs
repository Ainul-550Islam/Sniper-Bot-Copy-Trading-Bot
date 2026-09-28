//! Final machine-readable buyer gap ledger (Batch 5).
//! Each gap: ID, area, status, severity, evidence, owner, external dep, verification command.
//! Only genuine unresolved gaps. Deterministic sorting. Duplicate IDs rejected.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GapStatus {
    Open,
    Partial,
    ExternalRequired,
    BuyerAction,
    SellerAction,
    Closed,
}

impl GapStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Partial => "PARTIAL",
            Self::ExternalRequired => "EXTERNAL_REQUIRED",
            Self::BuyerAction => "BUYER_ACTION",
            Self::SellerAction => "SELLER_ACTION",
            Self::Closed => "CLOSED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GapSeverity {
    Low,
    Medium,
    High,
    Critical,
    Blocker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GapOwner {
    Buyer,
    Seller,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapEntry {
    pub id: String,
    pub area: String,
    pub status: GapStatus,
    pub severity: GapSeverity,
    pub evidence_ref: String,
    pub owner: GapOwner,
    pub external_dependency: Option<String>,
    pub verification_command: String,
    pub detail: String,
}

impl GapEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        area: impl Into<String>,
        status: GapStatus,
        severity: GapSeverity,
        evidence_ref: impl Into<String>,
        owner: GapOwner,
        external_dependency: Option<String>,
        verification_command: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            area: area.into(),
            status,
            severity,
            evidence_ref: evidence_ref.into(),
            owner,
            external_dependency,
            verification_command: verification_command.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalGapLedger {
    pub gaps: Vec<GapEntry>,
    pub generated_at: String,
}

impl FinalGapLedger {
    pub fn new(mut gaps: Vec<GapEntry>, generated_at: impl Into<String>) -> Result<Self, String> {
        // Check duplicates
        let mut seen = std::collections::HashSet::new();
        for g in &gaps {
            if !seen.insert(g.id.clone()) {
                return Err(format!("duplicate gap id: {}", g.id));
            }
            // Only genuine unresolved gaps: Closed should not be included
            if g.status == GapStatus::Closed {
                return Err(format!(
                    "closed gap {} must not be in final ledger (already complete)",
                    g.id
                ));
            }
        }
        gaps.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            gaps,
            generated_at: generated_at.into(),
        })
    }

    pub fn default_ledger() -> Self {
        Self::new(vec![
            GapEntry::new("GAP-001", "billing", GapStatus::ExternalRequired, GapSeverity::High, "docs/BUYER-TRUTH-REGISTER.md", GapOwner::Buyer, Some("Stripe/Paddle live keys".into()), "LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored --nocapture", "live billing webhook+checkout requires external provider keys and funded account. Adapter + end-to-end wiring are separately verified (see docs/FINAL-BUYER-GAP-LEDGER.md MATERIAL-GAP section); only live execution is NOT_RUN"),
            GapEntry::new("GAP-002", "custody", GapStatus::ExternalRequired, GapSeverity::High, "docs/BUYER-TRUTH-REGISTER.md", GapOwner::Buyer, Some("Vault/KMS/HSM".into()), "LIVE_CUSTODY=1 VAULT_ADDR=... cargo test --test live_custody_contract -- --ignored --nocapture", "remote custody live signing requires external Vault/KMS/HSM cluster; no local private-key fallback ever"),
            GapEntry::new("GAP-003", "deployment", GapStatus::BuyerAction, GapSeverity::Blocker, "docs/BUYER-DEPLOYMENT.md", GapOwner::Buyer, Some("production infra".into()), "DEPLOYMENT_BASE_URL=... cargo test --test deployment_smoke -- --nocapture", "production deployment requires buyer-provided host, env, TLS, Postgres+Redis and a read-only smoke against DEPLOYMENT_BASE_URL. A local `docker run` + `curl http://localhost:8080/api/health` proves the image only — it is documented as LOCAL CONTAINER SMOKE and is never production verification"),
            GapEntry::new("GAP-004", "trading", GapStatus::ExternalRequired, GapSeverity::Critical, "docs/LIVE-VALIDATION.md", GapOwner::Buyer, Some("funded keys + live mode".into()), "cargo test -p sniper-suite --lib funded_mode_guard", "funded live trading requires funded keys and explicit live approval; the guard (paper/dry_run default, live requires ALLOW_LIVE_TRADING + wallet funded + owner + risk gates) is verified by funded_mode_guard tests, the funded step itself is operator-only per docs/LIVE-VALIDATION.md"),
            GapEntry::new("GAP-005", "staking", GapStatus::ExternalRequired, GapSeverity::High, "programs/staking-suite/tests/validator_e2e.rs", GapOwner::External, Some("solana-test-validator".into()), "cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1", "staking validator E2E requires local validator with compiled .so; programs/staking-suite is its own (excluded) workspace, so the command must cd into it — the placeholder program id 3vEEMM… must never be read as a deployed program"),
            GapEntry::new("GAP-006", "audit", GapStatus::ExternalRequired, GapSeverity::Critical, "docs/SECURITY.md", GapOwner::External, Some("audit firm".into()), "n/a — external auditor deliverable (handover slot: docs/EXTERNAL-VALIDATION-RUNBOOK.md § GAP-006)", "external security audit not performed — do not claim audited; internal checks (cargo audit/deny, tests, static gates) are evidence of internal review only"),
        ], chrono::Utc::now().to_rfc3339()).expect("default ledger valid")
    }

    pub fn get(&self, id: &str) -> Option<&GapEntry> {
        self.gaps.iter().find(|g| g.id == id)
    }

    pub fn to_sorted_map(&self) -> BTreeMap<String, &GapEntry> {
        let mut m = BTreeMap::new();
        for g in &self.gaps {
            m.insert(g.id.clone(), g);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_sorting() {
        let l = FinalGapLedger::new(
            vec![
                GapEntry::new(
                    "GAP-Z",
                    "a",
                    GapStatus::Open,
                    GapSeverity::Low,
                    "e",
                    GapOwner::Buyer,
                    None,
                    "cmd",
                    "d",
                ),
                GapEntry::new(
                    "GAP-A",
                    "a",
                    GapStatus::Open,
                    GapSeverity::Low,
                    "e",
                    GapOwner::Buyer,
                    None,
                    "cmd",
                    "d",
                ),
            ],
            "now",
        )
        .unwrap();
        assert_eq!(l.gaps[0].id, "GAP-A");
        assert_eq!(l.gaps[1].id, "GAP-Z");
    }

    #[test]
    fn duplicate_ids_rejected() {
        let r = FinalGapLedger::new(
            vec![
                GapEntry::new(
                    "GAP-001",
                    "a",
                    GapStatus::Open,
                    GapSeverity::Low,
                    "e",
                    GapOwner::Buyer,
                    None,
                    "cmd",
                    "d",
                ),
                GapEntry::new(
                    "GAP-001",
                    "a",
                    GapStatus::Open,
                    GapSeverity::Low,
                    "e",
                    GapOwner::Buyer,
                    None,
                    "cmd",
                    "d",
                ),
            ],
            "now",
        );
        assert!(r.is_err());
    }

    #[test]
    fn closed_gaps_excluded() {
        let r = FinalGapLedger::new(
            vec![GapEntry::new(
                "GAP-001",
                "a",
                GapStatus::Closed,
                GapSeverity::Low,
                "e",
                GapOwner::Buyer,
                None,
                "cmd",
                "d",
            )],
            "now",
        );
        assert!(r.is_err());
    }

    #[test]
    fn default_preserves_external_required() {
        let l = FinalGapLedger::default_ledger();
        assert!(l.get("GAP-001").unwrap().status == GapStatus::ExternalRequired);
        assert!(l.get("GAP-006").unwrap().owner == GapOwner::External);
    }

    /// Batch 10: every gap command must point at a harness that actually exists, and
    /// must never present a local/container step as external verification.
    #[test]
    fn default_commands_point_at_executable_harnesses() {
        let l = FinalGapLedger::default_ledger();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let expectations = [
            (
                "GAP-001",
                "live_billing_contract",
                "crates/server/tests/live_billing_contract.rs",
            ),
            (
                "GAP-002",
                "live_custody_contract",
                "crates/server/tests/live_custody_contract.rs",
            ),
            (
                "GAP-003",
                "deployment_smoke",
                "crates/server/tests/deployment_smoke.rs",
            ),
            (
                "GAP-004",
                "funded_mode_guard",
                "crates/server/src/ops/funded_mode_guard.rs",
            ),
            (
                "GAP-005",
                "validator_e2e",
                "programs/staking-suite/tests/validator_e2e.rs",
            ),
        ];
        for (id, token, path) in expectations {
            let g = l.get(id).expect("gap present");
            assert!(
                g.verification_command.contains(token),
                "{id} command must reference {token}: {}",
                g.verification_command
            );
            assert!(
                root.join(path).exists(),
                "{id} harness file {path} must exist"
            );
        }
        // The audit gap has no executable command — it is an external deliverable.
        assert!(l
            .get("GAP-006")
            .unwrap()
            .verification_command
            .starts_with("n/a"));
        // Live provider commands must carry their explicit opt-in flag.
        assert!(l
            .get("GAP-001")
            .unwrap()
            .verification_command
            .contains("LIVE_BILLING=1"));
        assert!(l
            .get("GAP-002")
            .unwrap()
            .verification_command
            .contains("LIVE_CUSTODY=1"));
        // The staking E2E lives in the excluded programs workspace.
        assert!(l
            .get("GAP-005")
            .unwrap()
            .verification_command
            .contains("cd programs/staking-suite"));
        // No gap may substitute a local container/URL for external verification...
        assert!(!l
            .get("GAP-003")
            .unwrap()
            .verification_command
            .contains("localhost"));
        // ...and no gap may reference a CLI flag the binary does not implement.
        for g in &l.gaps {
            assert!(
                !g.verification_command.contains("--dry-run-check"),
                "{} references a non-existent CLI flag",
                g.id
            );
        }
    }

    /// Batch 10: the six gaps keep their canonical status/owner — no silent
    /// reclassification (e.g. flipping an external gap to something that looks done).
    #[test]
    fn default_status_ownership_is_pinned() {
        let l = FinalGapLedger::default_ledger();
        let expected = [
            ("GAP-001", GapStatus::ExternalRequired, GapOwner::Buyer),
            ("GAP-002", GapStatus::ExternalRequired, GapOwner::Buyer),
            ("GAP-003", GapStatus::BuyerAction, GapOwner::Buyer),
            ("GAP-004", GapStatus::ExternalRequired, GapOwner::Buyer),
            ("GAP-005", GapStatus::ExternalRequired, GapOwner::External),
            ("GAP-006", GapStatus::ExternalRequired, GapOwner::External),
        ];
        assert_eq!(l.gaps.len(), 6, "exactly the six canonical gaps");
        for (id, status, owner) in expected {
            let g = l.get(id).expect("gap present");
            assert_eq!(g.status, status, "{id} status must not be reclassified");
            assert_eq!(g.owner, owner, "{id} owner must not change");
        }
    }

    #[test]
    fn completed_gaps_excluded_batch1_4_not_listed() {
        let l = FinalGapLedger::default_ledger();
        // Ensure no gap claims billing_state or custody rotation as open — those are Batch3 complete
        for g in &l.gaps {
            assert!(!g
                .detail
                .to_ascii_lowercase()
                .contains("billing_state complete missing"));
            assert!(!g.detail.contains("custody rotation not implemented"));
        }
    }
}
