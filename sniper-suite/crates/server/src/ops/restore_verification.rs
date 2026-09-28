//! Restore verification model (Batch 4). Must explicitly represent NOT_EXECUTED.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreStatus {
    NotExecuted,
    Succeeded,
    Failed,
    InProgress,
}

impl RestoreStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotExecuted => "not_executed",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::InProgress => "in_progress",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreRecord {
    pub restore_id: String,
    pub source_backup_id: String,
    pub target_database: String, // redacted identifier, not connection string
    pub migration_high_water: Option<String>,
    pub row_verification: Option<bool>,
    pub table_verification: Option<bool>,
    pub integrity: RestoreStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub operator: String, // not secret
    pub detail: String,
}

impl RestoreRecord {
    pub fn not_executed(backup_id: impl Into<String>) -> Self {
        Self { restore_id: format!("restore-{}", uuid::Uuid::new_v4()), source_backup_id: backup_id.into(), target_database: "<redacted>".into(), migration_high_water: None, row_verification: None, table_verification: None, integrity: RestoreStatus::NotExecuted, started_at: None, completed_at: None, operator: "unknown".into(), detail: "restore not executed — documentation only; run pg_restore and verification to produce evidence".into() }
    }
    pub fn succeeded(backup_id: impl Into<String>, high_water: impl Into<String>) -> Self {
        Self {
            restore_id: format!("restore-{}", uuid::Uuid::new_v4()),
            source_backup_id: backup_id.into(),
            target_database: "<redacted>".into(),
            migration_high_water: Some(high_water.into()),
            row_verification: Some(true),
            table_verification: Some(true),
            integrity: RestoreStatus::Succeeded,
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
            operator: "operator".into(),
            detail: "restore verified: row/table counts match".into(),
        }
    }
    pub fn is_executed(&self) -> bool {
        self.integrity != RestoreStatus::NotExecuted
    }
}

pub fn verify_row_counts(expected: Option<u64>, actual: Option<u64>) -> Option<bool> {
    match (expected, actual) {
        (Some(e), Some(a)) => Some(e == a),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn not_executed_is_default() {
        let r = RestoreRecord::not_executed("b1");
        assert_eq!(r.integrity, RestoreStatus::NotExecuted);
        assert!(!r.is_executed());
        assert!(r.detail.contains("not executed"));
    }
    #[test]
    fn succeeded_is_executed() {
        let r = RestoreRecord::succeeded("b1", "0021");
        assert!(r.is_executed());
        assert_eq!(r.integrity, RestoreStatus::Succeeded);
    }
    #[test]
    fn do_not_fabricate_restore() {
        let r = RestoreRecord::not_executed("b1");
        assert_eq!(r.integrity, RestoreStatus::NotExecuted);
        // must not claim succeeded
        assert_ne!(r.integrity, RestoreStatus::Succeeded);
    }
    #[test]
    fn row_verification_logic() {
        assert_eq!(verify_row_counts(Some(100), Some(100)), Some(true));
        assert_eq!(verify_row_counts(Some(100), Some(99)), Some(false));
        assert_eq!(verify_row_counts(None, Some(100)), None);
    }
}
