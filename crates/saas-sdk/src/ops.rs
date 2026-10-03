//! Public SDK types for safe customer/operator status (Batch 4). No secrets.

use crate::client::SaasClient;
use crate::error::SdkError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySummary {
    pub organization_id: String,
    pub mfa_status: String,
    pub session_status: String,
    pub api_key_status: String,
    pub websocket_auth: String,
    pub custody_mode: String,
    pub audit_available: bool,
    pub lifecycle_state: String,
    pub as_of: String,
}

/// Off-site redundancy, reported on its own axis by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsiteStatus {
    /// `not_configured` | `current` | `stale` | `failing`.
    pub state: String,
    pub configured: bool,
    pub last_sync_at: Option<String>,
    /// Whether the last successful sync also carried the PITR artefacts
    /// (base backups + WAL archive). `false` while `pitr.state` is
    /// `current` means the fast recovery path is local-only.
    pub includes_pitr: bool,
}

/// Point-in-time recovery, reported on its own axis by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PitrStatus {
    /// `not_configured` | `failing` | `stale` | `unverified` | `current`.
    pub state: String,
    pub last_wal_archive_at: Option<String>,
    pub last_base_backup_at: Option<String>,
    pub last_verified_pitr: Option<String>,
}

/// Mirrors `/api/saas/backup/status`.
///
/// The old shape (`retention_configured: bool`, `protection: String`)
/// was removed in the same change that made the endpoint derive its
/// answer from the backup ledger instead of returning literals. Those
/// fields could not be kept as aliases: there was no honest value to
/// map them to for a deployment that is not backing anything up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStatus {
    pub organization_id: String,
    /// `verified` | `unverified` | `stale` | `failing` | `not_configured`.
    pub state: String,
    /// True only for `state == "verified"`. It means "recoverable, proven
    /// by a restore drill" — NOT "survives losing this host"; that is
    /// what [`BackupStatus::offsite`] answers.
    pub protected: bool,
    pub last_backup_at: Option<String>,
    pub last_verified_restore: Option<String>,
    pub retention_days: Option<i64>,
    pub encrypted: Option<bool>,
    pub backup_count: u64,
    pub offsite: OffsiteStatus,
    pub pitr: PitrStatus,
    /// Worst-case data loss in seconds. `None` means the deployment
    /// cannot evidence a recovery point at all — read it as "unknown",
    /// never as "zero".
    pub rpo_estimate_seconds: Option<i64>,
    /// `wal_archive` | `last_backup` | `unknown`. Always read this
    /// before quoting the number above: the same figure means minutes
    /// of exposure on one basis and hours on the other.
    pub rpo_basis: String,
    pub summary: String,
    pub as_of: String,
}

impl SaasClient {
    pub async fn security_summary(&self) -> Result<SecuritySummary, SdkError> {
        self.get("/api/saas/security/summary").await
    }
    pub async fn backup_status(&self) -> Result<BackupStatus, SdkError> {
        self.get("/api/saas/backup/status").await
    }
    pub async fn operator_runtime_config(&self) -> Result<serde_json::Value, SdkError> {
        self.get("/api/ops/runtime-config").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SdkErrorKind;
    #[test]
    fn security_summary_deserialize() {
        let j = serde_json::json!({"organization_id":"org","mfa_status":"not_supported","session_status":"session","api_key_status":"in_use","websocket_auth":"header_or_first_frame","custody_mode":"local","audit_available":true,"lifecycle_state":"active","as_of":"2026-09-23"});
        let v: SecuritySummary = serde_json::from_value(j).unwrap();
        assert_eq!(v.websocket_auth, "header_or_first_frame");
    }
    #[test]
    fn backup_status_distinguishes() {
        let j = serde_json::json!({
            "organization_id":"org","state":"unverified","protected":false,
            "last_backup_at":"2026-10-02T02:00:00Z","last_verified_restore":null,
            "retention_days":30,"encrypted":true,"backup_count":7,
            "offsite":{"state":"not_configured","configured":false,"last_sync_at":null,"includes_pitr":false},
            "pitr":{"state":"not_configured","last_wal_archive_at":null,"last_base_backup_at":null,"last_verified_pitr":null},
            "rpo_estimate_seconds":36000,"rpo_basis":"last_backup",
            "summary":"backups are current, but no restore has EVER been verified; copies are LOCAL ONLY",
            "as_of":"now"
        });
        let v: BackupStatus = serde_json::from_value(j).unwrap();
        // The point of the type: "a backup exists" and "the data is
        // protected" are different questions, and the SDK must not let a
        // caller collapse them.
        assert_eq!(v.backup_count, 7);
        assert!(!v.protected);
        assert!(v.last_verified_restore.is_none());
        assert!(!v.offsite.configured);
    }

    /// A deployment with no ledger must still deserialize — that is the
    /// state a buyer evaluating the system sees first.
    #[test]
    fn backup_status_handles_a_deployment_with_no_backups() {
        let j = serde_json::json!({
            "organization_id":"org","state":"not_configured","protected":false,
            "last_backup_at":null,"last_verified_restore":null,
            "retention_days":null,"encrypted":null,"backup_count":0,
            "offsite":{"state":"not_configured","configured":false,"last_sync_at":null,"includes_pitr":false},
            "pitr":{"state":"not_configured","last_wal_archive_at":null,"last_base_backup_at":null,"last_verified_pitr":null},
            "rpo_estimate_seconds":null,"rpo_basis":"unknown",
            "summary":"no backup ledger is present in this deployment","as_of":"now"
        });
        let v: BackupStatus = serde_json::from_value(j).unwrap();
        assert_eq!(v.state, "not_configured");
        assert!(!v.protected);
        assert_eq!(v.encrypted, None);
    }
    #[test]
    fn no_secrets_in_url() {
        let path = "/api/saas/security/summary";
        assert!(!path.contains("secret"));
    }
    #[test]
    fn error_mapping() {
        let e = crate::error::SdkError::new(SdkErrorKind::Unauthorized, "unauthorized");
        assert_eq!(e.kind, SdkErrorKind::Unauthorized);
    }
    /// `rpo_estimate_seconds: null` must survive the round trip as
    /// `None`. A client that defaulted it to 0 would display "no data
    /// loss" for a deployment that cannot evidence any recovery point.
    #[test]
    fn an_unknown_rpo_is_none_not_zero() {
        let j = serde_json::json!({
            "organization_id":"org","state":"not_configured","protected":false,
            "last_backup_at":null,"last_verified_restore":null,
            "retention_days":null,"encrypted":null,"backup_count":0,
            "offsite":{"state":"not_configured","configured":false,"last_sync_at":null,"includes_pitr":false},
            "pitr":{"state":"not_configured","last_wal_archive_at":null,"last_base_backup_at":null,"last_verified_pitr":null},
            "rpo_estimate_seconds":null,"rpo_basis":"unknown",
            "summary":"nothing recorded","as_of":"now"
        });
        let v: BackupStatus = serde_json::from_value(j).unwrap();
        assert_eq!(v.rpo_estimate_seconds, None);
        assert_ne!(v.rpo_estimate_seconds, Some(0));
        assert_eq!(v.rpo_basis, "unknown");
    }

    #[test]
    fn backup_status_high_level_only() {
        let j = serde_json::to_string(&BackupStatus {
            organization_id: "org".into(),
            state: "unverified".into(),
            protected: false,
            last_backup_at: Some("2026-09-23".into()),
            last_verified_restore: None,
            retention_days: Some(30),
            encrypted: Some(true),
            backup_count: 3,
            offsite: OffsiteStatus {
                state: "not_configured".into(),
                configured: false,
                last_sync_at: None,
                includes_pitr: false,
            },
            pitr: PitrStatus {
                state: "not_configured".into(),
                last_wal_archive_at: None,
                last_base_backup_at: None,
                last_verified_pitr: None,
            },
            rpo_estimate_seconds: Some(36_000),
            rpo_basis: "last_backup".into(),
            summary: "backups are current, but no restore has EVER been verified".into(),
            as_of: "now".into(),
        })
        .unwrap();
        // Nothing in this type may carry a location: not a connection
        // string, not a bucket, not a path on the backup host.
        for banned in ["postgres://", "s3://", "gs://", "/app/data", "/var/"] {
            assert!(
                !j.contains(banned),
                "banned {banned} leaked into the SDK type"
            );
        }
    }
}
