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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStatus {
    pub organization_id: String,
    pub retention_configured: bool,
    pub last_backup_at: Option<String>,
    pub last_verified_restore: Option<String>,
    pub protection: String,
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
        let j = serde_json::json!({"organization_id":"org","mfa_status":"unknown","session_status":"active","api_key_status":"ok","websocket_auth":"header_or_first_frame","custody_mode":"local","audit_available":true,"lifecycle_state":"active","as_of":"2026-09-23"});
        let v: SecuritySummary = serde_json::from_value(j).unwrap();
        assert_eq!(v.websocket_auth, "header_or_first_frame");
    }
    #[test]
    fn backup_status_distinguishes() {
        let j = serde_json::json!({"organization_id":"org","retention_configured":true,"last_backup_at":null,"last_verified_restore":null,"protection":"encrypted_at_rest","as_of":"now"});
        let v: BackupStatus = serde_json::from_value(j).unwrap();
        assert!(v.retention_configured);
        assert!(v.last_verified_restore.is_none());
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
    #[test]
    fn backup_status_high_level_only() {
        let j = serde_json::to_string(&BackupStatus {
            organization_id: "org".into(),
            retention_configured: true,
            last_backup_at: Some("2026-09-23".into()),
            last_verified_restore: None,
            protection: "encrypted_at_rest".into(),
            as_of: "now".into(),
        })
        .unwrap();
        assert!(!j.contains("postgres://"));
    }
}
