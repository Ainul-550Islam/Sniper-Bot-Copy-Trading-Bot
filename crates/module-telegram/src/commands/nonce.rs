//! The **only** owner of confirmation-nonce handling.
//!
//! A confirmation nonce is the bearer token that turns a *request* into an
//! *execution*. It is therefore treated like a secret:
//!
//! * generated from the operating system CSPRNG ([`rand::rngs::OsRng`]) as 16
//!   random bytes, rendered as 32 lowercase hex characters (≥ 8 chars, no
//!   ambiguity, safe to type into a chat);
//! * only its SHA-256 digest is ever persisted (migration 0067 stores
//!   `nonce_sha256 bytea`) — the plaintext exists exactly once, inside the
//!   chat reply that delivered it;
//! * comparison is constant-time ([`verify`]) so a wrong nonce cannot be
//!   recovered by timing;
//! * single-use is enforced by the pending-command state machine
//!   ([`crate::commands::pending`]), not by this module.
//!
//! Nothing here logs, formats or debugs a nonce: [`Nonce`] deliberately has no
//! `Display`, `Serialize` or `Debug` implementation that would reveal it.

use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Random bytes behind one nonce.
pub const NONCE_BYTES: usize = 16;

/// Length of the rendered nonce (hex of [`NONCE_BYTES`]).
pub const NONCE_LEN: usize = NONCE_BYTES * 2;

/// Shortest nonce a deployment may ever accept (spec floor).
pub const MIN_NONCE_LEN: usize = 8;

/// A freshly generated confirmation nonce.
///
/// The inner string is intentionally private and there is no `Debug` impl that
/// prints it: a nonce only ever leaves this module through [`Nonce::expose`],
/// which the reply builder calls exactly once per command.
pub struct Nonce(String);

impl Nonce {
    /// Generate a new nonce from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Self(hex::encode(bytes))
    }

    /// Re-hydrate a nonce received from a chat (used only to verify it; never
    /// to store it).
    pub fn from_input(raw: &str) -> Option<Self> {
        let text = raw.trim();
        if text.len() < MIN_NONCE_LEN {
            return None;
        }
        if !text.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        Some(Self(text.to_ascii_lowercase()))
    }

    /// The plaintext nonce. Call only when composing the single reply that
    /// delivers it, or when verifying a candidate against its digest.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// SHA-256 of the plaintext — the only form that may be stored or logged.
    pub fn sha256(&self) -> [u8; 32] {
        hash(self.expose())
    }
}

impl std::fmt::Debug for Nonce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the value, even in a panic message or a test failure.
        f.write_str("Nonce(<redacted>)")
    }
}

/// SHA-256 of a nonce plaintext.
pub fn hash(nonce: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(nonce.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Constant-time comparison of a candidate nonce against a stored digest.
///
/// Returns `false` for anything malformed, so a caller can treat "wrong" and
/// "garbage" identically.
pub fn verify(candidate: &str, expected_digest: &[u8; 32]) -> bool {
    let candidate = candidate.trim();
    if candidate.len() < MIN_NONCE_LEN {
        return false;
    }
    let actual = hash(candidate);
    // Fold the comparison: no early exit, no branch on the data.
    let mut diff = 0u8;
    for (a, b) in actual.iter().zip(expected_digest.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Hex rendering of a digest — for test assertions and operator-facing
/// diagnostics only (a digest is not secret material, but it is still not
/// shown in chat).
pub fn digest_hex(digest: &[u8; 32]) -> String {
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_nonces_are_hex_and_long_enough() {
        for _ in 0..64 {
            let nonce = Nonce::generate();
            assert_eq!(nonce.expose().len(), NONCE_LEN);
            assert!(nonce.expose().len() >= MIN_NONCE_LEN);
            assert!(nonce.expose().chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn generated_nonces_do_not_repeat() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(Nonce::generate().expose().to_string()));
        }
    }

    #[test]
    fn nonce_debug_is_redacted() {
        let nonce = Nonce::generate();
        let rendered = format!("{nonce:?}");
        assert!(!rendered.contains(nonce.expose()));
        assert_eq!(rendered, "Nonce(<redacted>)");
    }

    #[test]
    fn verification_matches_only_the_same_nonce() {
        let nonce = Nonce::generate();
        let digest = nonce.sha256();
        assert!(verify(nonce.expose(), &digest));
        assert!(!verify(&nonce.expose().to_ascii_uppercase(), &digest));
        assert!(!verify("deadbeefdeadbeef", &digest));
        assert!(!verify("", &digest));
        assert!(!verify("short", &digest));
        assert_eq!(digest.len(), 32);
        assert_eq!(digest_hex(&digest).len(), 64);
    }

    #[test]
    fn input_rehydration_is_strict() {
        assert_eq!(
            Nonce::from_input("ABCDEF12").map(|n| n.expose().to_string()),
            Some("abcdef12".to_string())
        );
        assert!(Nonce::from_input("abc").is_none());
        assert!(Nonce::from_input("has space 12").is_none());
        assert!(Nonce::from_input("").is_none());
        assert!(Nonce::from_input("dashes-not-allowed").is_none());
    }

    #[test]
    fn hashing_is_stable_and_distinct() {
        assert_eq!(hash("abc"), hash("abc"));
        assert_ne!(hash("abc"), hash("abd"));
        assert_eq!(digest_hex(&hash("abc")).len(), 64);
    }
}
