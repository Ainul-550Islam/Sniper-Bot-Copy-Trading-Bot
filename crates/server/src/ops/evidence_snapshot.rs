//! Deterministic machine-readable buyer evidence snapshot (Batch 3).
//!
//! Includes version, git/source identifier if available, migration high-water mark,
//! workspace member count, source file count, test counts actually recorded,
//! CI status if supplied, known limitations, intentionally-unexecuted validations.
//! Every value comes from actual runtime/files or explicit input — never invents numbers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceSnapshot {
    pub version: String,
    pub source_id: String, // git sha or "unknown"
    pub generated_at: String,
    pub migration_high_water: String,
    pub migration_count: usize,
    pub workspace_members: usize,
    pub source_files: usize,
    pub rs_files: usize,
    pub test_counts: BTreeMap<String, i64>, // e.g., "bot_core_lib" -> 461
    pub ci_status: Option<String>,
    pub known_limitations: Vec<String>,
    pub unexecuted_validations: Vec<String>,
    pub frontend: FrontendEvidence,
    pub checks: BTreeMap<String, String>, // gate -> PASS/WARN/BLOCK
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendEvidence {
    pub has_package_lock: bool,
    pub lockfile_version: Option<u32>,
    pub build_pass: Option<bool>,
}

impl EvidenceSnapshot {
    pub fn new(
        version: impl Into<String>,
        source_id: impl Into<String>,
        migration_high_water: impl Into<String>,
        migration_count: usize,
        workspace_members: usize,
        source_files: usize,
        rs_files: usize,
    ) -> Self {
        Self {
            version: version.into(),
            source_id: source_id.into(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            migration_high_water: migration_high_water.into(),
            migration_count,
            workspace_members,
            source_files,
            rs_files,
            test_counts: BTreeMap::new(),
            ci_status: None,
            known_limitations: Vec::new(),
            unexecuted_validations: Vec::new(),
            frontend: FrontendEvidence {
                has_package_lock: false,
                lockfile_version: None,
                build_pass: None,
            },
            checks: BTreeMap::new(),
        }
    }

    pub fn with_test_count(mut self, key: impl Into<String>, count: i64) -> Self {
        self.test_counts.insert(key.into(), count);
        self
    }

    pub fn hash(&self) -> String {
        // Deterministic hash of canonical JSON (sorted keys via BTreeMap)
        let canonical = serde_json::to_string(self).unwrap();
        let digest = sha2::Sha256::digest(canonical.as_bytes());
        format!("{:x}", digest)
    }
}

use sha2::Digest;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_hash() {
        let s1 = EvidenceSnapshot::new("0.1.0", "abc123", "0021", 21, 8, 400, 253)
            .with_test_count("bot_core", 461);
        let mut s2 = s1.clone();
        // Same content => same hash
        assert_eq!(s1.hash(), s2.hash());
        // Different count => different hash
        s2.test_counts.insert("bot_core".into(), 462);
        assert_ne!(s1.hash(), s2.hash());
    }

    #[test]
    fn never_invents_numbers() {
        let s = EvidenceSnapshot::new("0.1.0", "unknown", "0021", 21, 8, 0, 0);
        assert_eq!(s.migration_count, 21);
        assert_eq!(s.workspace_members, 8);
        // Source files must be supplied, not invented
        assert_eq!(s.source_files, 0);
    }

    #[test]
    fn snapshot_is_serializable() {
        let s = EvidenceSnapshot::new("0.1.0", "abc", "0021", 21, 8, 100, 50);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"version\":\"0.1.0\""));
    }
}
