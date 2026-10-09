//! Release artifact descriptor (Batch 4).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseArtifact {
    pub version: String,
    pub source_identifier: String, // commit short or "workspace"
    pub filename: String,
    pub sha256: String, // hex
    pub size_bytes: u64,
    pub built_at: DateTime<Utc>,
    pub target: String,          // e.g. "x86_64-unknown-linux-gnu"
    pub reproducibility: String, // e.g. "reproducible: cargo 1.98.1, rust-toolchain.toml"
}

impl ReleaseArtifact {
    pub fn new(
        version: impl Into<String>,
        source: impl Into<String>,
        filename: impl Into<String>,
        sha256: impl Into<String>,
        size: u64,
        target: impl Into<String>,
    ) -> Self {
        Self {
            version: version.into(),
            source_identifier: source.into(),
            filename: filename.into(),
            sha256: sha256.into(),
            size_bytes: size,
            built_at: Utc::now(),
            target: target.into(),
            reproducibility: format!("cargo {}", env!("CARGO_PKG_VERSION")),
        }
    }
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}
pub fn sha256_file_hex(path: &std::path::Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(sha256_hex(&data))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_known_vector() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
    #[test]
    fn deterministic_canonical() {
        let a = ReleaseArtifact::new(
            "0.1.0",
            "abc123",
            "sniper-suite.tar.gz",
            "deadbeef",
            123,
            "x86_64-unknown-linux-gnu",
        );
        let b = ReleaseArtifact::new(
            "0.1.0",
            "abc123",
            "sniper-suite.tar.gz",
            "deadbeef",
            123,
            "x86_64-unknown-linux-gnu",
        );
        // canonical_json differs only in built_at, so pin it
        let mut a2 = a.clone();
        let b2 = b.clone();
        a2.built_at = b2.built_at;
        assert_eq!(a2.canonical_json(), b2.canonical_json());
    }
    #[test]
    fn verify_helpers() {
        let h = sha256_hex(b"");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
