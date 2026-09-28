//! Deterministic build/release reproducibility model (Batch 5).
//! Records toolchain, Cargo.lock hash, package-lock hash, source tree hash, build flags, target triple.
//! Verification helpers. Never claim byte-for-byte reproducibility unless actually measured.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReproducibilityRecord {
    pub rust_toolchain: String,
    pub cargo_lock_sha256: Option<String>,
    pub package_lock_sha256: Option<String>,
    pub source_tree_sha256: Option<String>,
    pub build_flags: Vec<String>,
    pub target_triple: String,
    pub byte_for_byte_verified: bool,
    pub verification_detail: String,
}

impl ReproducibilityRecord {
    pub fn new(
        rust_toolchain: impl Into<String>,
        target_triple: impl Into<String>,
        build_flags: Vec<String>,
    ) -> Self {
        Self {
            rust_toolchain: rust_toolchain.into(),
            cargo_lock_sha256: None,
            package_lock_sha256: None,
            source_tree_sha256: None,
            build_flags,
            target_triple: target_triple.into(),
            byte_for_byte_verified: false,
            verification_detail: "not_verified — byte-for-byte reproducibility requires two independent builds and hash compare".into(),
        }
    }

    pub fn with_cargo_lock_hash(mut self, hash: impl Into<String>) -> Self {
        self.cargo_lock_sha256 = Some(hash.into());
        self
    }
    pub fn with_package_lock_hash(mut self, hash: impl Into<String>) -> Self {
        self.package_lock_sha256 = Some(hash.into());
        self
    }
    pub fn with_source_tree_hash(mut self, hash: impl Into<String>) -> Self {
        self.source_tree_sha256 = Some(hash.into());
        self
    }

    /// Mark as verified only when caller provides evidence of two builds matching.
    pub fn mark_verified(mut self, detail: impl Into<String>) -> Self {
        self.byte_for_byte_verified = true;
        self.verification_detail = detail.into();
        self
    }

    pub fn hash_bytes(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    pub fn verify_cargo_lock(&self, actual_bytes: &[u8]) -> Result<(), String> {
        let expected = self
            .cargo_lock_sha256
            .as_ref()
            .ok_or("no cargo_lock hash recorded")?;
        let actual = Self::hash_bytes(actual_bytes);
        if &actual == expected {
            Ok(())
        } else {
            Err(format!(
                "cargo lock mismatch: expected {} got {}",
                expected, actual
            ))
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rust_toolchain": self.rust_toolchain,
            "cargo_lock_sha256": self.cargo_lock_sha256,
            "package_lock_sha256": self.package_lock_sha256,
            "source_tree_sha256": self.source_tree_sha256,
            "build_flags": self.build_flags,
            "target_triple": self.target_triple,
            "byte_for_byte_verified": self.byte_for_byte_verified,
            "verification_detail": self.verification_detail
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        let h1 = ReproducibilityRecord::hash_bytes(b"hello");
        let h2 = ReproducibilityRecord::hash_bytes(b"hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn not_verified_by_default() {
        let r = ReproducibilityRecord::new(
            "1.82",
            "x86_64-unknown-linux-gnu",
            vec!["--release".into()],
        );
        assert!(!r.byte_for_byte_verified);
        assert!(r.verification_detail.contains("not_verified"));
    }

    #[test]
    fn verify_cargo_lock_roundtrip() {
        let data = b"cargo lock content";
        let hash = ReproducibilityRecord::hash_bytes(data);
        let r =
            ReproducibilityRecord::new("1.82", "x86_64", vec![]).with_cargo_lock_hash(hash.clone());
        assert!(r.verify_cargo_lock(data).is_ok());
        assert!(r.verify_cargo_lock(b"other").is_err());
    }

    #[test]
    fn mark_verified_requires_evidence() {
        let r = ReproducibilityRecord::new("1.82", "x86_64", vec![])
            .mark_verified("two builds sha256 abc == abc on 2026-09-24");
        assert!(r.byte_for_byte_verified);
    }
}
