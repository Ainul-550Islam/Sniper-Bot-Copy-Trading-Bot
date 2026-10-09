//! Verify backup artifacts and metadata (Batch 4). No plaintext secrets.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub backup_id: String,
    pub database: String,
    pub created_at: DateTime<Utc>,
    pub checksum_sha256: String, // hex
    pub size_bytes: u64,
    pub encrypted: bool,
    pub retention_until: Option<DateTime<Utc>>,
    pub restore_tested: Option<bool>, // None = not tested
    pub location_redacted: String,    // e.g. "s3://bucket/<redacted>" never raw creds
}

impl BackupMetadata {
    pub fn new(
        backup_id: impl Into<String>,
        database: impl Into<String>,
        checksum: impl Into<String>,
        size: u64,
    ) -> Self {
        Self {
            backup_id: backup_id.into(),
            database: database.into(),
            created_at: Utc::now(),
            checksum_sha256: checksum.into(),
            size_bytes: size,
            encrypted: true,
            retention_until: None,
            restore_tested: None,
            location_redacted: "<redacted>".into(),
        }
    }
    pub fn is_expired(&self) -> bool {
        self.retention_until
            .map(|d| Utc::now() > d)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupVerification {
    pub backup_id: String,
    pub checksum_ok: bool,
    pub size_ok: bool,
    pub encrypted_ok: bool,
    pub retention_ok: bool,
    pub overall: bool,
    pub detail: String,
}

pub fn verify(
    backup: &BackupMetadata,
    expected_checksum: Option<&str>,
    expected_size: Option<u64>,
) -> BackupVerification {
    let checksum_ok = expected_checksum
        .map(|e| e.eq_ignore_ascii_case(&backup.checksum_sha256))
        .unwrap_or(true);
    let size_ok = expected_size
        .map(|s| s == backup.size_bytes)
        .unwrap_or(true);
    let encrypted_ok = backup.encrypted;
    let retention_ok = !backup.is_expired();
    let overall = checksum_ok && size_ok && encrypted_ok && retention_ok;
    let detail = if overall {
        "ok".into()
    } else {
        format!("checksum_ok={checksum_ok} size_ok={size_ok} encrypted={encrypted_ok} retention_ok={retention_ok}")
    };
    BackupVerification {
        backup_id: backup.backup_id.clone(),
        checksum_ok,
        size_ok,
        encrypted_ok,
        retention_ok,
        overall,
        detail: redact(&detail),
    }
}

fn redact(s: &str) -> String {
    let l = s.to_ascii_lowercase();
    if l.contains("secret") || l.contains("password") || l.contains("token") {
        "<redacted>".into()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_fails() {
        let mut b = BackupMetadata::new("b1", "postgres", "abc", 100);
        b.retention_until = Some(Utc::now() - chrono::Duration::days(1));
        let v = verify(&b, Some("abc"), Some(100));
        assert!(!v.retention_ok);
        assert!(!v.overall);
    }
    #[test]
    fn checksum_mismatch() {
        let b = BackupMetadata::new("b1", "postgres", "abc", 100);
        let v = verify(&b, Some("wrong"), Some(100));
        assert!(!v.checksum_ok);
    }
    #[test]
    fn encrypted_required() {
        let mut b = BackupMetadata::new("b1", "postgres", "abc", 100);
        b.encrypted = false;
        let v = verify(&b, Some("abc"), Some(100));
        assert!(!v.encrypted_ok);
    }
    #[test]
    fn passes_when_ok() {
        let b = BackupMetadata::new("b1", "postgres", "abc", 100);
        let v = verify(&b, Some("abc"), Some(100));
        assert!(v.overall);
    }
    #[test]
    fn redacts_location() {
        let b = BackupMetadata::new("b1", "postgres", "abc", 100);
        assert_eq!(b.location_redacted, "<redacted>");
    }
}
