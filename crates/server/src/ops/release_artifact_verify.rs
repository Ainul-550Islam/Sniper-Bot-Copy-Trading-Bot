//! Verify release artifact SHA256 and metadata (Batch 4). Never trust client checksum without computing.

use super::release_artifact::{sha256_file_hex, sha256_hex, ReleaseArtifact};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub filename: String,
    pub expected_sha256: String,
    pub computed_sha256: String,
    pub matches: bool,
    pub size_matches: bool,
    pub detail: String,
}

pub fn verify_bytes(
    artifact: &ReleaseArtifact,
    bytes: &[u8],
    check_size: bool,
) -> VerificationResult {
    let computed = sha256_hex(bytes);
    let matches = computed.eq_ignore_ascii_case(&artifact.sha256);
    let size_matches = !check_size || bytes.len() as u64 == artifact.size_bytes;
    let detail = if matches && size_matches {
        "ok".into()
    } else if !matches {
        format!(
            "sha256 mismatch expected {} got {computed}",
            artifact.sha256
        )
    } else {
        format!(
            "size mismatch expected {} got {}",
            artifact.size_bytes,
            bytes.len()
        )
    };
    VerificationResult {
        filename: artifact.filename.clone(),
        expected_sha256: artifact.sha256.clone(),
        computed_sha256: computed,
        matches: matches && size_matches,
        size_matches,
        detail,
    }
}

pub fn verify_file(
    artifact: &ReleaseArtifact,
    path: &std::path::Path,
) -> Result<VerificationResult, String> {
    let computed = sha256_file_hex(path)?;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size_matches = meta.len() == artifact.size_bytes;
    let matches = computed.eq_ignore_ascii_case(&artifact.sha256);
    let detail = if matches && size_matches {
        "ok".into()
    } else {
        "mismatch".to_string()
    };
    Ok(VerificationResult {
        filename: artifact.filename.clone(),
        expected_sha256: artifact.sha256.clone(),
        computed_sha256: computed,
        matches: matches && size_matches,
        size_matches,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::super::release_artifact::ReleaseArtifact;
    use super::*;
    #[test]
    fn matches_when_correct() {
        let bytes = b"hello world";
        let sha = super::super::release_artifact::sha256_hex(bytes);
        let art = ReleaseArtifact::new(
            "0.1.0",
            "src",
            "file.bin",
            sha.clone(),
            bytes.len() as u64,
            "x86_64",
        );
        let r = verify_bytes(&art, bytes, true);
        assert!(r.matches);
        assert!(r.size_matches);
    }
    #[test]
    fn fails_on_tamper() {
        let bytes = b"hello";
        let sha = super::super::release_artifact::sha256_hex(bytes);
        let art = ReleaseArtifact::new("0.1.0", "src", "file.bin", sha, 5, "x86_64");
        let r = verify_bytes(&art, b"hell0", true);
        assert!(!r.matches);
    }
    #[test]
    fn fails_on_size_mismatch() {
        let bytes = b"hello";
        let sha = super::super::release_artifact::sha256_hex(bytes);
        let mut art = ReleaseArtifact::new("0.1.0", "src", "file.bin", sha, 5, "x86_64");
        art.size_bytes = 999;
        let r = verify_bytes(&art, bytes, true);
        assert!(!r.matches);
        assert!(!r.size_matches);
    }
    #[test]
    fn never_trust_client_without_compute() {
        let art = ReleaseArtifact::new(
            "0.1.0",
            "src",
            "file.bin",
            "client_claimed_deadbeef",
            5,
            "x86_64",
        );
        let r = verify_bytes(&art, b"hello", false);
        // Must compute, not trust client sha
        assert!(!r.matches);
    }
}
