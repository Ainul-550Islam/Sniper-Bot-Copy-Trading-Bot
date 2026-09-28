//! Canonical external validation registry (Batch 5).
//! Tracks Stripe/Paddle, Vault/KMS/HSM, Postgres/Redis, staking, deployment, funded trading, audit.
//! Status: VERIFIED/NOT_EXECUTED/BLOCKED/REQUIRES_EXTERNAL/EXPIRED. Never default to VERIFIED.
//!
//! Batch 10 state-machine rules (enforced, not documented-only):
//! * `set_status` can NEVER produce `VERIFIED` — promotion goes through
//!   `mark_verified(id, evidence_ref, verified_at, detail)`, which requires a non-empty
//!   evidence reference and timestamp. `NOT_RUN -> PASS`-style silent promotion is
//!   impossible because this registry has no path that sets `VERIFIED` without evidence.
//! * `ExternalValidationRegistry::new` rejects a bare `VERIFIED` entry with no
//!   `verified_at`/evidence, so a tampered constructor cannot invent verification.
//! * Every entry carries the canonical `gap_id` (`GAP-001..GAP-006` from
//!   `ops::final_gap_ledger`) or `""` when the validation is not one of the six buyer gaps.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExternalStatus {
    Verified,
    NotExecuted,
    Blocked,
    RequiresExternal,
    Expired,
}

impl ExternalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verified => "VERIFIED",
            Self::NotExecuted => "NOT_EXECUTED",
            Self::Blocked => "BLOCKED",
            Self::RequiresExternal => "REQUIRES_EXTERNAL",
            Self::Expired => "EXPIRED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalValidationEntry {
    pub id: String,
    pub area: String,
    /// Canonical buyer-gap id (`GAP-001..GAP-006`) or `""` when not one of the six gaps.
    pub gap_id: String,
    pub status: ExternalStatus,
    pub detail: String,
    pub evidence_ref: String,
    pub verified_at: Option<String>,
}

impl ExternalValidationEntry {
    pub fn new(
        id: impl Into<String>,
        area: impl Into<String>,
        gap_id: impl Into<String>,
        status: ExternalStatus,
        detail: impl Into<String>,
        evidence_ref: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            area: area.into(),
            gap_id: gap_id.into(),
            status,
            detail: detail.into(),
            evidence_ref: evidence_ref.into(),
            verified_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalValidationRegistry {
    pub entries: Vec<ExternalValidationEntry>,
}

impl ExternalValidationRegistry {
    /// Deterministic sort by id. Fails closed on an entry that claims `VERIFIED`
    /// without a `verified_at` timestamp and an evidence reference.
    pub fn new(mut entries: Vec<ExternalValidationEntry>) -> Result<Self, String> {
        for e in &entries {
            if e.status == ExternalStatus::Verified
                && (e.verified_at.as_deref().unwrap_or("").trim().is_empty()
                    || e.evidence_ref.trim().is_empty())
            {
                return Err(format!(
                    "{}: VERIFIED requires verified_at + evidence_ref — cannot be constructed bare",
                    e.id
                ));
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self { entries })
    }

    pub fn default_registry() -> Self {
        Self::new(vec![
            ExternalValidationEntry::new(
                "stripe_live",
                "billing",
                "GAP-001",
                ExternalStatus::NotExecuted,
                "live Stripe webhook+checkout requires external keys",
                "docs/BUYER-TRUTH-REGISTER.md",
            ),
            ExternalValidationEntry::new(
                "paddle_live",
                "billing",
                "GAP-001",
                ExternalStatus::NotExecuted,
                "live Paddle requires external keys",
                "docs/BUYER-TRUTH-REGISTER.md",
            ),
            ExternalValidationEntry::new(
                "vault_live",
                "custody",
                "GAP-002",
                ExternalStatus::NotExecuted,
                "Vault live signing requires external Vault",
                "docs/BUYER-TRUTH-REGISTER.md",
            ),
            ExternalValidationEntry::new(
                "kms_live",
                "custody",
                "GAP-002",
                ExternalStatus::NotExecuted,
                "KMS live signing requires external KMS",
                "docs/BUYER-TRUTH-REGISTER.md",
            ),
            ExternalValidationEntry::new(
                "hsm_live",
                "custody",
                "GAP-002",
                ExternalStatus::NotExecuted,
                "HSM live signing requires external HSM",
                "docs/BUYER-TRUTH-REGISTER.md",
            ),
            ExternalValidationEntry::new(
                "postgres_integration",
                "database",
                "",
                ExternalStatus::RequiresExternal,
                "requires POSTGRES_URL and real migration run",
                "scripts/release-evidence.sh",
            ),
            ExternalValidationEntry::new(
                "redis_integration",
                "database",
                "",
                ExternalStatus::RequiresExternal,
                "requires REDIS_URL and real redis",
                "scripts/release-evidence.sh",
            ),
            ExternalValidationEntry::new(
                "staking_e2e",
                "staking",
                "GAP-005",
                ExternalStatus::RequiresExternal,
                "requires STAKING_E2E=1 with validator",
                "programs/staking-suite/tests/validator_e2e.rs",
            ),
            ExternalValidationEntry::new(
                "deployment_smoke",
                "deployment",
                "GAP-003",
                ExternalStatus::NotExecuted,
                "requires production docker smoke",
                "docs/BUYER-DEPLOYMENT.md",
            ),
            ExternalValidationEntry::new(
                "funded_trading",
                "trading",
                "GAP-004",
                ExternalStatus::NotExecuted,
                "requires funded keys and live mode",
                "docs/LIVE-VALIDATION.md",
            ),
            ExternalValidationEntry::new(
                "external_audit",
                "audit",
                "GAP-006",
                ExternalStatus::NotExecuted,
                "requires external security audit report",
                "docs/SECURITY.md",
            ),
        ])
        .expect("default registry is valid")
    }

    pub fn get(&self, id: &str) -> Option<&ExternalValidationEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// All entries mapped to a canonical buyer gap (`GAP-001..GAP-006`).
    pub fn entries_for_gap(&self, gap_id: &str) -> Vec<&ExternalValidationEntry> {
        self.entries.iter().filter(|e| e.gap_id == gap_id).collect()
    }

    /// The set of non-empty gap ids referenced by this registry.
    pub fn gap_ids(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|e| !e.gap_id.trim().is_empty())
            .map(|e| e.gap_id.clone())
            .collect()
    }

    /// Set a non-VERIFIED status. `VERIFIED` is rejected here on purpose: use
    /// [`mark_verified`] with real evidence instead.
    pub fn set_status(
        &mut self,
        id: &str,
        status: ExternalStatus,
        detail: impl Into<String>,
    ) -> Result<(), String> {
        if status == ExternalStatus::Verified {
            return Err(format!(
                "{id}: set_status cannot set VERIFIED — call mark_verified(id, evidence_ref, verified_at, detail) with the real verification command output"
            ));
        }
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(format!("unknown external id {id}"))?;
        e.status = status;
        e.detail = detail.into();
        e.verified_at = None;
        Ok(())
    }

    /// The only path to `VERIFIED`. Requires a non-empty evidence reference (file the
    /// verification command wrote) and the timestamp the command was executed at.
    pub fn mark_verified(
        &mut self,
        id: &str,
        evidence_ref: impl Into<String>,
        verified_at: impl Into<String>,
        detail: impl Into<String>,
    ) -> Result<(), String> {
        let evidence_ref = evidence_ref.into();
        let verified_at = verified_at.into();
        if evidence_ref.trim().is_empty() {
            return Err(format!("{id}: VERIFIED requires a non-empty evidence_ref"));
        }
        if verified_at.trim().is_empty() {
            return Err(format!(
                "{id}: VERIFIED requires the verified_at timestamp of the executed verification command"
            ));
        }
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(format!("unknown external id {id}"))?;
        e.status = ExternalStatus::Verified;
        e.evidence_ref = evidence_ref;
        e.verified_at = Some(verified_at);
        e.detail = detail.into();
        Ok(())
    }

    pub fn all_verified(&self) -> bool {
        self.entries
            .iter()
            .all(|e| e.status == ExternalStatus::Verified)
    }

    pub fn count_by_status(&self, status: ExternalStatus) -> usize {
        self.entries.iter().filter(|e| e.status == status).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::final_gap_ledger::FinalGapLedger;

    #[test]
    fn default_is_not_verified() {
        let r = ExternalValidationRegistry::default_registry();
        assert!(!r.all_verified());
        assert_eq!(r.count_by_status(ExternalStatus::Verified), 0);
        for e in &r.entries {
            assert_ne!(e.status, ExternalStatus::Verified, "no default VERIFIED");
        }
    }

    #[test]
    fn requires_external_for_staking() {
        let r = ExternalValidationRegistry::default_registry();
        assert_eq!(
            r.get("staking_e2e").unwrap().status,
            ExternalStatus::RequiresExternal
        );
    }

    /// Batch 10: a bare VERIFIED entry cannot even be constructed into a registry.
    #[test]
    fn cannot_construct_verified_without_evidence() {
        let r = ExternalValidationRegistry::new(vec![ExternalValidationEntry::new(
            "x",
            "area",
            "GAP-001",
            ExternalStatus::Verified,
            "detail",
            "evidence",
        )]);
        assert!(r.is_err(), "bare VERIFIED must be rejected");
        // And the default registry never contains one.
        let def = ExternalValidationRegistry::default_registry();
        assert!(def
            .entries
            .iter()
            .all(|e| e.status != ExternalStatus::Verified));
    }

    /// Batch 10: `set_status` must never be able to promote anything to VERIFIED.
    #[test]
    fn set_status_cannot_promote_to_verified() {
        let mut r = ExternalValidationRegistry::default_registry();
        let err = r
            .set_status("stripe_live", ExternalStatus::Verified, "looks good")
            .expect_err("set_status(VERIFIED) must be refused");
        assert!(err.contains("mark_verified"), "{err}");
        assert_eq!(
            r.get("stripe_live").unwrap().status,
            ExternalStatus::NotExecuted,
            "refused promotion must not mutate state"
        );
    }

    /// Batch 10: the only promotion path requires evidence + timestamp.
    #[test]
    fn mark_verified_requires_evidence_and_timestamp() {
        let mut r = ExternalValidationRegistry::default_registry();
        assert!(r
            .mark_verified("stripe_live", "", "2026-09-27T00:00:00Z", "d")
            .is_err());
        assert!(r
            .mark_verified(
                "stripe_live",
                "evidence/external/billing_stripe.json",
                " ",
                "d"
            )
            .is_err());
        assert_eq!(
            r.get("stripe_live").unwrap().status,
            ExternalStatus::NotExecuted
        );

        r.mark_verified(
            "stripe_live",
            "evidence/external/billing_stripe.json",
            "2026-09-27T00:00:00Z",
            "live checkout created",
        )
        .expect("valid evidence promotes");
        let e = r.get("stripe_live").unwrap();
        assert_eq!(e.status, ExternalStatus::Verified);
        assert_eq!(e.verified_at.as_deref(), Some("2026-09-27T00:00:00Z"));
    }

    /// Batch 10: demotion clears the verification timestamp (no stale VERIFIED metadata).
    #[test]
    fn demotion_clears_verified_at() {
        let mut r = ExternalValidationRegistry::default_registry();
        r.mark_verified(
            "vault_live",
            "evidence/x.json",
            "2026-09-27T00:00:00Z",
            "ok",
        )
        .unwrap();
        assert!(r.get("vault_live").unwrap().verified_at.is_some());
        r.set_status("vault_live", ExternalStatus::Expired, "evidence expired")
            .unwrap();
        let e = r.get("vault_live").unwrap();
        assert_eq!(e.status, ExternalStatus::Expired);
        assert!(e.verified_at.is_none());
    }

    #[test]
    fn status_change_preserves_sorting() {
        let mut r = ExternalValidationRegistry::default_registry();
        r.set_status("stripe_live", ExternalStatus::Blocked, "blocked")
            .unwrap();
        assert_eq!(
            r.get("stripe_live").unwrap().status,
            ExternalStatus::Blocked
        );
    }

    #[test]
    fn deterministic_sorting_by_id() {
        let r = ExternalValidationRegistry::new(vec![
            ExternalValidationEntry::new("z", "a", "", ExternalStatus::NotExecuted, "d", "e"),
            ExternalValidationEntry::new("a", "a", "", ExternalStatus::NotExecuted, "d", "e"),
        ])
        .unwrap();
        assert_eq!(r.entries[0].id, "a");
        assert_eq!(r.entries[1].id, "z");
    }

    /// Batch 10: the registry's gap mapping and the canonical ledger must agree —
    /// no registry entry references a gap the ledger does not list, and every
    /// ledger gap has at least one registry entry to verify it.
    #[test]
    fn gap_mapping_matches_ledger() {
        let ledger = FinalGapLedger::default_ledger();
        let registry = ExternalValidationRegistry::default_registry();

        let ledger_ids: BTreeSet<String> = ledger.gaps.iter().map(|g| g.id.clone()).collect();
        let registry_ids = registry.gap_ids();

        assert_eq!(
            ledger_ids, registry_ids,
            "registry gap ids must match the six ledger gaps exactly"
        );
        for id in &ledger_ids {
            assert!(
                !registry.entries_for_gap(id).is_empty(),
                "{id} has no registry entry"
            );
        }
        // DB integrations are intentionally not buyer gaps.
        assert!(registry
            .get("postgres_integration")
            .unwrap()
            .gap_id
            .is_empty());
        assert!(registry.get("redis_integration").unwrap().gap_id.is_empty());
    }
}
