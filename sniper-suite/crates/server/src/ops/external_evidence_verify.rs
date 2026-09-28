//! Verify an external evidence record independently (Batch 7).
//! Validate checksum/hash and schema, reject tampered evidence.
//! Never upgrade NOT_RUN to PASS simply because a file exists.

use super::external_evidence::ExternalEvidence;
use super::provider_contract::ProviderStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub valid: bool,
    pub status: ProviderStatus,
    pub detail: String,
    pub evidence_hash_match: bool,
    pub schema_valid: bool,
}

impl VerifyResult {
    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "valid": self.valid,
            "status": self.status.as_str(),
            "detail": self.detail,
            "evidence_hash_match": self.evidence_hash_match,
            "schema_valid": self.schema_valid,
        })
    }
}

pub struct ExternalEvidenceVerifier;

impl ExternalEvidenceVerifier {
    pub fn verify(evidence: &ExternalEvidence) -> VerifyResult {
        let hash_match = evidence.verify_hash();
        let schema_valid = Self::validate_schema(evidence);
        let valid = hash_match && schema_valid;

        // Never upgrade NOT_RUN to PASS simply because file exists — status is what evidence says
        let status = evidence.status;
        let detail = if !hash_match {
            "evidence hash mismatch — tampered or corrupted".into()
        } else if !schema_valid {
            "schema invalid — missing required fields".into()
        } else {
            format!("evidence valid, status={}", status.as_str())
        };

        VerifyResult {
            valid,
            status,
            detail,
            evidence_hash_match: hash_match,
            schema_valid,
        }
    }

    pub fn verify_file(path: &str) -> Result<VerifyResult, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path, e))?;
        let v: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("json parse {}: {}", path, e))?;
        // Validate required fields
        for field in [
            "validation_id",
            "gap_id",
            "provider",
            "environment",
            "timestamp",
            "command",
            "status",
            "evidence_hash",
        ] {
            if v.get(field).is_none() {
                return Ok(VerifyResult {
                    valid: false,
                    status: ProviderStatus::Fail,
                    detail: format!("missing field {}", field),
                    evidence_hash_match: false,
                    schema_valid: false,
                });
            }
        }
        let evidence: ExternalEvidence =
            serde_json::from_value(v).map_err(|e| format!("deserialize: {}", e))?;
        Ok(Self::verify(&evidence))
    }

    fn validate_schema(evidence: &ExternalEvidence) -> bool {
        if evidence.validation_id.trim().is_empty() {
            return false;
        }
        if evidence.provider.trim().is_empty() {
            return false;
        }
        if evidence.environment.trim().is_empty() {
            return false;
        }
        if evidence.timestamp.trim().is_empty() {
            return false;
        }
        if evidence.command.trim().is_empty() {
            return false;
        }
        if evidence.evidence_hash.trim().is_empty() {
            return false;
        }
        // Ensure no secrets in redacted fields (already redacted)
        let json = evidence.to_safe_json().to_string();
        if json.contains("BEGIN PRIVATE KEY") && !json.contains("<redacted>") {
            return false;
        }
        true
    }

    pub fn never_upgrade_not_run(evidence: &ExternalEvidence, file_exists: bool) -> ProviderStatus {
        // File existence does NOT change status — only hash-verified evidence status matters
        if file_exists && evidence.status == ProviderStatus::NotRun {
            ProviderStatus::NotRun
        } else {
            evidence.status
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::external_evidence::ExternalEvidence;
    use crate::ops::provider_contract::ProviderStatus;

    #[test]
    fn valid_evidence_passes() {
        let e = ExternalEvidence::new(
            "v-001",
            "GAP-001",
            "stripe",
            "test",
            "cmd",
            "billing",
            ProviderStatus::NotRun,
            serde_json::json!({"ok": true}),
            "ref",
        );
        let r = ExternalEvidenceVerifier::verify(&e);
        assert!(r.valid);
        assert!(r.evidence_hash_match);
        assert!(r.schema_valid);
        assert_eq!(r.status, ProviderStatus::NotRun);
    }

    #[test]
    fn tampered_fails() {
        let mut e = ExternalEvidence::new(
            "v-002",
            "GAP-002",
            "vault",
            "test",
            "cmd",
            "custody",
            ProviderStatus::Pass,
            serde_json::json!({"pk": "abc"}),
            "ref",
        );
        e.evidence_hash = "tampered".into();
        let r = ExternalEvidenceVerifier::verify(&e);
        assert!(!r.valid);
        assert!(!r.evidence_hash_match);
    }

    #[test]
    fn never_upgrades_not_run_even_if_file_exists() {
        let e = ExternalEvidence::new(
            "v-003",
            "n/a",
            "postgres",
            "test",
            "cmd",
            "db",
            ProviderStatus::NotRun,
            serde_json::json!({}),
            "ref",
        );
        let status = ExternalEvidenceVerifier::never_upgrade_not_run(&e, true);
        assert_eq!(status, ProviderStatus::NotRun);
    }

    #[test]
    fn schema_invalid_when_missing_fields() {
        let mut e = ExternalEvidence::new(
            "v-004",
            "GAP-005",
            "solana",
            "test",
            "cmd",
            "solana",
            ProviderStatus::Pass,
            serde_json::json!({}),
            "ref",
        );
        e.validation_id = "".into();
        let r = ExternalEvidenceVerifier::verify(&e);
        assert!(!r.valid);
        assert!(!r.schema_valid);
    }

    /// Batch 10: an evidence file without a `gap_id` is schema-invalid
    /// (the canonical gap mapping must be present for every validation).
    #[test]
    fn schema_invalid_when_gap_id_missing_from_file() {
        let dir = std::env::temp_dir().join("sniper_evidence_gap_id_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("no_gap.json");
        let e = ExternalEvidence::new(
            "v-005",
            "GAP-003",
            "deployment",
            "test",
            "cmd",
            "deployment",
            ProviderStatus::NotRun,
            serde_json::json!({}),
            "ref",
        );
        let mut json = e.to_safe_json();
        json.as_object_mut().unwrap().remove("gap_id");
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        let r = ExternalEvidenceVerifier::verify_file(path.to_str().unwrap()).unwrap();
        assert!(!r.valid);
        assert!(!r.schema_valid);
        assert!(r.detail.contains("gap_id"));
        let _ = std::fs::remove_file(&path);
    }

    /// Batch 10: a well-formed current-format file verifies (hash + schema).
    #[test]
    fn current_format_file_verifies() {
        let dir = std::env::temp_dir().join("sniper_evidence_roundtrip_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("billing_stripe.json");
        let e = ExternalEvidence::new(
            "billing",
            "GAP-001",
            "stripe",
            "test",
            "LIVE_BILLING=1 cargo test --test live_billing_contract",
            "all-safe",
            ProviderStatus::NotRun,
            serde_json::json!({"checkout_created": null, "signature_verified": null}),
            "stripe ref",
        );
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&e.to_safe_json()).unwrap(),
        )
        .unwrap();
        let r = ExternalEvidenceVerifier::verify_file(path.to_str().unwrap()).unwrap();
        assert!(r.valid, "{:?}", r);
        assert!(r.evidence_hash_match);
        assert!(r.schema_valid);
        assert_eq!(r.status, ProviderStatus::NotRun);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_not_found_is_error() {
        let r = ExternalEvidenceVerifier::verify_file("/tmp/nonexistent_evidence_12345.json");
        assert!(r.is_err());
    }
}
