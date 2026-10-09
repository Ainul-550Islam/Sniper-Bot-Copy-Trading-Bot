//! Cryptographic attestation record for release and buyer evidence.
//!
//! Attestations identify their signature scheme explicitly. This module
//! implements standard HMAC-SHA256 attestations; detached asymmetric
//! signatures are represented as a distinct scheme and are not accepted by
//! the HMAC verifier.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignatureScheme {
    HmacSha256,
    Detached { algorithm: String, key_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attestation {
    pub version: String,
    pub snapshot_hash: String,
    pub timestamp: DateTime<Utc>,
    pub scope: String,
    pub evidence_ids: Vec<String>,
    pub attestor: String,
    pub signature_scheme: SignatureScheme,
    pub signature: String,
}

impl Attestation {
    pub fn new(
        version: impl Into<String>,
        snapshot_hash: impl Into<String>,
        scope: impl Into<String>,
        evidence_ids: Vec<String>,
        attestor: impl Into<String>,
    ) -> Self {
        Self {
            version: version.into(),
            snapshot_hash: snapshot_hash.into(),
            timestamp: Utc::now(),
            scope: scope.into(),
            evidence_ids,
            attestor: attestor.into(),
            signature_scheme: SignatureScheme::HmacSha256,
            signature: String::new(),
        }
    }

    /// Serialize every attestation field except the signature itself.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature.clear();
        serde_json::to_vec(&unsigned).expect("attestation serialization")
    }

    /// Sign with a standard HMAC-SHA256 key. The scheme is set explicitly so
    /// a verifier cannot mistake this evidence for an asymmetric signature.
    pub fn sign(&mut self, key: &[u8]) {
        self.signature_scheme = SignatureScheme::HmacSha256;
        self.signature = hex::encode(hmac_sha256(key, &self.canonical_bytes()));
    }

    /// Verify only HMAC-SHA256 attestations. Detached signatures require the
    /// corresponding external public-key verifier and are not accepted here.
    pub fn verify(&self, key: &[u8]) -> bool {
        if !matches!(self.signature_scheme, SignatureScheme::HmacSha256)
            || self.signature.is_empty()
        {
            return false;
        }
        let expected = hex::encode(hmac_sha256(key, &self.canonical_bytes()));
        constant_time_eq(self.signature.as_bytes(), expected.as_bytes())
    }

    pub fn summary(&self) -> String {
        let snapshot_prefix: String = self.snapshot_hash.chars().take(8).collect();
        format!(
            "attestation {} scope={} snapshot={} evidence={}",
            self.version,
            self.scope,
            snapshot_prefix,
            self.evidence_ids.join(",")
        )
    }
}

/// Standard HMAC construction with SHA-256 and its 64-byte compression block.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut normalized_key = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        normalized_key[..digest.len()].copy_from_slice(&digest);
    } else {
        normalized_key[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36_u8; BLOCK_SIZE];
    let mut opad = [0x5c_u8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        ipad[index] ^= normalized_key[index];
        opad[index] ^= normalized_key[index];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in a.iter().zip(b.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_standard_hmac() {
        let mut attestation = Attestation::new(
            "0.1.0",
            "abc123deadbeef",
            "release-0.1.0",
            vec!["evidence-001".into()],
            "ci",
        );
        attestation.sign(b"test-key");
        assert!(matches!(
            attestation.signature_scheme,
            SignatureScheme::HmacSha256
        ));
        assert!(attestation.verify(b"test-key"));
        assert!(!attestation.verify(b"wrong-key"));
    }

    #[test]
    fn tamper_detected() {
        let mut attestation =
            Attestation::new("0.1.0", "abc123", "release", vec!["e1".into()], "operator");
        attestation.sign(b"key");
        let mut tampered = attestation.clone();
        tampered.version = "0.2.0".into();
        assert!(!tampered.verify(b"key"));
    }

    #[test]
    fn detached_scheme_is_not_claimed_to_be_hmac() {
        let mut attestation = Attestation::new("0.1.0", "hash", "scope", vec![], "operator");
        attestation.signature_scheme = SignatureScheme::Detached {
            algorithm: "ed25519".into(),
            key_id: "release-key".into(),
        };
        attestation.signature = "external-signature".into();
        assert!(!attestation.verify(b"key"));
    }

    #[test]
    fn deterministic_canonical_bytes_when_timestamp_is_fixed() {
        let mut first = Attestation::new("0.1.0", "hash", "scope", vec!["e1".into()], "op");
        let mut second = first.clone();
        let fixed = chrono::DateTime::parse_from_rfc3339("2026-09-24T04:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        first.timestamp = fixed;
        second.timestamp = fixed;
        assert_eq!(first.canonical_bytes(), second.canonical_bytes());
    }

    #[test]
    fn summary_does_not_claim_external_validation() {
        let attestation = Attestation::new("0.1.0", "hash", "buyer-evidence", vec![], "ci");
        let serialized = serde_json::to_string(&attestation)
            .expect("serialization")
            .to_ascii_lowercase();
        assert!(!serialized.contains("external audit"));
        assert!(!serialized.contains("production deployment"));
    }
}
