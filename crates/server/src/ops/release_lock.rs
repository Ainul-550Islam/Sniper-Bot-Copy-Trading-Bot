//! Release lockfile — deterministic lock of release package contents (Batch 5).
//! Records version, git rev if any, locked_at, pinned entries (file/sha256/size/built_at).
//! SHA256 computed independently per file; never reused across artifacts.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedEntry {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub built_at: String,
}

impl LockedEntry {
    pub fn new(
        path: impl Into<String>,
        sha256: impl Into<String>,
        size_bytes: u64,
        built_at: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            sha256: sha256.into(),
            size_bytes,
            built_at: built_at.into(),
        }
    }

    pub fn from_bytes(path: impl Into<String>, bytes: &[u8], built_at: impl Into<String>) -> Self {
        let mut h = Sha256::new();
        h.update(bytes);
        let sha = hex::encode(h.finalize());
        Self::new(path, sha, bytes.len() as u64, built_at)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseLock {
    pub release_version: String,
    pub git_revision: Option<String>,
    pub locked_at: String,
    pub entries: Vec<LockedEntry>,
}

impl ReleaseLock {
    pub fn new(
        release_version: impl Into<String>,
        git_revision: Option<String>,
        locked_at: impl Into<String>,
        mut entries: Vec<LockedEntry>,
    ) -> Self {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            release_version: release_version.into(),
            git_revision,
            locked_at: locked_at.into(),
            entries,
        }
    }

    pub fn verify_sorted(&self) -> Result<(), String> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        for (a, b) in self.entries.iter().zip(sorted.iter()) {
            if a.path != b.path {
                return Err(format!("entries not sorted: {} before {}", a.path, b.path));
            }
        }
        Ok(())
    }

    pub fn validate_hashes_sorted(&self) -> Result<(), String> {
        // Each entry hash is independent — just validate format here
        for e in &self.entries {
            if e.sha256.len() != 64 || !e.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("invalid sha256 for {}: {}", e.path, e.sha256));
            }
        }
        Ok(())
    }

    pub fn to_canonical_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    pub fn canonical_hash(&self) -> Result<String, String> {
        let s = self.to_canonical_json()?;
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        Ok(hex::encode(h.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_computed_independently_per_file() {
        let e1 = LockedEntry::from_bytes("a.txt", b"hello", "2026-09-24T00:00:00Z");
        let e2 = LockedEntry::from_bytes("b.txt", b"hello", "2026-09-24T00:00:00Z");
        assert_eq!(e1.sha256, e2.sha256); // same content same hash
        let e3 = LockedEntry::from_bytes("c.txt", b"world", "2026-09-24T00:00:00Z");
        assert_ne!(e1.sha256, e3.sha256);
    }

    #[test]
    fn entries_sorted_deterministically() {
        let l = ReleaseLock::new(
            "0.1.0",
            None,
            "2026-09-24T00:00:00Z",
            vec![
                LockedEntry::new("z.txt", "a".repeat(64), 10, "now"),
                LockedEntry::new("a.txt", "b".repeat(64), 10, "now"),
            ],
        );
        assert_eq!(l.entries[0].path, "a.txt");
        assert!(l.verify_sorted().is_ok());
    }

    #[test]
    fn invalid_hash_rejected() {
        let l = ReleaseLock::new(
            "0.1.0",
            None,
            "now",
            vec![LockedEntry::new("a.txt", "not-a-hash", 10, "now")],
        );
        assert!(l.validate_hashes_sorted().is_err());
    }

    #[test]
    fn canonical_json_is_deterministic() {
        let l1 = ReleaseLock::new(
            "0.1.0",
            Some("abc".into()),
            "now",
            vec![LockedEntry::new("a.txt", "a".repeat(64), 1, "now")],
        );
        let l2 = ReleaseLock::new(
            "0.1.0",
            Some("abc".into()),
            "now",
            vec![LockedEntry::new("a.txt", "a".repeat(64), 1, "now")],
        );
        assert_eq!(
            l1.to_canonical_json().unwrap(),
            l2.to_canonical_json().unwrap()
        );
        assert_eq!(l1.canonical_hash().unwrap(), l2.canonical_hash().unwrap());
    }
}
