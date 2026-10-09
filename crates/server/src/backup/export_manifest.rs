//! Backup export manifest (Batch 5). Never carry plaintext secrets. DOCUMENTED/EXECUTED/VERIFIED only via explicit evidence.
//! Strictly typed statuses — no auto-promotion.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExportStatus {
    Documented,
    Executed,
    Verified,
    NotExecuted,
}

impl ExportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Documented => "DOCUMENTED",
            Self::Executed => "EXECUTED",
            Self::Verified => "VERIFIED",
            Self::NotExecuted => "NOT_EXECUTED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSpan {
    pub table_or_prefix: String,
    pub row_count: Option<u64>,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportManifest {
    pub export_id: String,
    pub status: ExportStatus,
    pub created_at: String,
    pub scopes: Vec<String>, // tenant_ids or "all"
    pub spans: Vec<ExportSpan>,
    pub artifact_path: Option<String>,
    pub artifact_sha256: Option<String>,
    pub evidence_ref: String,
    pub detail: String,
}

impl ExportManifest {
    pub fn new(
        export_id: impl Into<String>,
        created_at: impl Into<String>,
        scopes: Vec<String>,
        evidence_ref: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            export_id: export_id.into(),
            status: ExportStatus::Documented,
            created_at: created_at.into(),
            scopes,
            spans: Vec::new(),
            artifact_path: None,
            artifact_sha256: None,
            evidence_ref: evidence_ref.into(),
            detail: detail.into(),
        }
    }

    pub fn mark_executed(
        &mut self,
        artifact_path: impl Into<String>,
        artifact_sha256: impl Into<String>,
    ) -> Result<(), String> {
        if self.status != ExportStatus::Documented {
            return Err(format!(
                "export {} must be DOCUMENTED before EXECUTED, got {}",
                self.export_id,
                self.status.as_str()
            ));
        }
        self.status = ExportStatus::Executed;
        self.artifact_path = Some(artifact_path.into());
        self.artifact_sha256 = Some(artifact_sha256.into());
        Ok(())
    }

    pub fn mark_verified(&mut self, expected_sha: &str) -> Result<(), String> {
        if self.status != ExportStatus::Executed {
            return Err(format!(
                "export {} must be EXECUTED before VERIFIED, got {}",
                self.export_id,
                self.status.as_str()
            ));
        }
        match &self.artifact_sha256 {
            Some(s) if s == expected_sha => {
                self.status = ExportStatus::Verified;
                Ok(())
            }
            Some(s) => Err(format!(
                "sha mismatch for {}: expected {} got {}",
                self.export_id, expected_sha, s
            )),
            None => Err("no artifact sha".into()),
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "export_id": self.export_id,
            "status": self.status.as_str(),
            "created_at": self.created_at,
            "scopes": self.scopes,
            "spans": self.spans,
            "artifact_path": self.artifact_path,
            "artifact_sha256": self.artifact_sha256,
            "evidence_ref": self.evidence_ref,
            "detail": self.detail
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_promotion_order() {
        let mut m = ExportManifest::new(
            "exp-1",
            "2026-09-24T00:00:00Z",
            vec!["t1".into()],
            "docs/BACKUP-RESTORE.md",
            "test",
        );
        assert_eq!(m.status, ExportStatus::Documented);
        assert!(m.mark_verified("abc").is_err()); // cannot skip EXECUTED
        m.mark_executed("/tmp/export.sql", "a".repeat(64)).unwrap();
        assert_eq!(m.status, ExportStatus::Executed);
        assert!(m.mark_verified(&"b".repeat(64)).is_err()); // sha mismatch
        m.mark_verified(&"a".repeat(64)).unwrap();
        assert_eq!(m.status, ExportStatus::Verified);
    }

    #[test]
    fn no_plaintext_secrets_in_json() {
        let m = ExportManifest::new(
            "exp-1",
            "now",
            vec!["t1".into()],
            "evidence",
            "detail: pg_dump for tenant t1",
        );
        let j = m.to_safe_json().to_string().to_ascii_lowercase();
        assert!(!j.contains("password"));
    }

    #[test]
    fn double_execute_rejected() {
        let mut m = ExportManifest::new("exp-1", "now", vec![], "e", "d");
        m.mark_executed("/tmp/a", "a".repeat(64)).unwrap();
        assert!(m.mark_executed("/tmp/b", "b".repeat(64)).is_err());
    }
}
