//! Restore manifest (Batch 5). Strict DOCUMENTED→EXECUTED→VERIFIED. Requires ExportManifest reference.
//! Drill/restore never auto-promotes.

use super::export_manifest::ExportStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RestoreStatus {
    Documented,
    Executed,
    Verified,
    NotExecuted,
}

impl RestoreStatus {
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
pub struct RestoreManifest {
    pub restore_id: String,
    pub export_id: String,
    pub status: RestoreStatus,
    pub created_at: String,
    pub verified_at: Option<String>,
    pub artifact_sha256: String,
    pub target: String,
    pub evidence_ref: String,
    pub detail: String,
}

impl RestoreManifest {
    pub fn new(
        restore_id: impl Into<String>,
        export_id: impl Into<String>,
        artifact_sha256: impl Into<String>,
        target: impl Into<String>,
        created_at: impl Into<String>,
        evidence_ref: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            restore_id: restore_id.into(),
            export_id: export_id.into(),
            status: RestoreStatus::Documented,
            created_at: created_at.into(),
            verified_at: None,
            artifact_sha256: artifact_sha256.into(),
            target: target.into(),
            evidence_ref: evidence_ref.into(),
            detail: detail.into(),
        }
    }

    pub fn mark_executed(&mut self) -> Result<(), String> {
        if self.status != RestoreStatus::Documented {
            return Err(format!(
                "restore {} must be DOCUMENTED before EXECUTED, got {}",
                self.restore_id,
                self.status.as_str()
            ));
        }
        self.status = RestoreStatus::Executed;
        Ok(())
    }

    pub fn mark_verified(
        &mut self,
        export_status: ExportStatus,
        verified_at: impl Into<String>,
    ) -> Result<(), String> {
        if self.status != RestoreStatus::Executed {
            return Err(format!(
                "restore {} must be EXECUTED before VERIFIED, got {}",
                self.restore_id,
                self.status.as_str()
            ));
        }
        if export_status != ExportStatus::Verified {
            return Err(format!(
                "cannot VERIFY restore {} when export {} is not VERIFIED (is {})",
                self.restore_id,
                self.export_id,
                export_status.as_str()
            ));
        }
        self.status = RestoreStatus::Verified;
        self.verified_at = Some(verified_at.into());
        Ok(())
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "restore_id": self.restore_id,
            "export_id": self.export_id,
            "status": self.status.as_str(),
            "created_at": self.created_at,
            "verified_at": self.verified_at,
            "artifact_sha256": self.artifact_sha256,
            "target": self.target,
            "evidence_ref": self.evidence_ref,
            "detail": self.detail
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_export_verified() {
        let mut r = RestoreManifest::new(
            "res-1",
            "exp-1",
            "a".repeat(64),
            "staging",
            "now",
            "docs/BACKUP-RESTORE.md",
            "test",
        );
        r.mark_executed().unwrap();
        assert!(r.mark_verified(ExportStatus::Executed, "now").is_err());
        assert!(r.mark_verified(ExportStatus::Documented, "now").is_err());
        r.mark_verified(ExportStatus::Verified, "2026-09-24T00:00:00Z")
            .unwrap();
        assert_eq!(r.status, RestoreStatus::Verified);
    }

    #[test]
    fn strict_order() {
        let mut r = RestoreManifest::new("res-1", "exp-1", "a".repeat(64), "t", "now", "e", "d");
        assert!(r.mark_verified(ExportStatus::Verified, "now").is_err());
        r.mark_executed().unwrap();
        assert!(r.mark_executed().is_err());
    }

    #[test]
    fn cannot_double_verify() {
        let mut r = RestoreManifest::new("res-1", "exp-1", "a".repeat(64), "t", "now", "e", "d");
        r.mark_executed().unwrap();
        r.mark_verified(ExportStatus::Verified, "now").unwrap();
        assert!(r.mark_verified(ExportStatus::Verified, "now").is_err());
    }
}
