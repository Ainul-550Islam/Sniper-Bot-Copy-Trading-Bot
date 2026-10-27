//! SOLE owner of the dataset hash chain (BATCH 02).
//!
//! Reuses the project's battle-tested SHA-256 primitive
//! (`bot_core::auth::sha256_hex`, which is `hex(Sha256(bytes))`) rather than
//! re-implementing crypto. This module owns the CHAIN rules only:
//!
//! * `sha256` of a chunk = SHA-256 of that chunk's exact bytes;
//! * `prev_sha256` of chunk *i* = `sha256` of chunk *i-1*; the first chunk
//!   links to [`GENESIS_PREV_SHA256`] (64 zero hex chars);
//! * `sha256_root` = SHA-256 over the domain-separated, ordered list of all
//!   chunk hashes (see [`sha256_root`]);
//! * comparisons use [`constant_time_eq`] so a digest comparison cannot leak
//!   its position through timing.
//!
//! All dataset content is UTF-8 NDJSON / hex / RFC3339 text, so hashing a
//! `&str` hashes the exact file bytes. A chunk whose bytes are not valid
//! UTF-8 is corruption: the verifier treats `String::from_utf8` failure as a
//! hash-chain failure (never a silent pass).

use bot_core::auth::sha256_hex;

/// `prev_sha256` for the first chunk (a fixed 64-char genesis digest).
pub const GENESIS_PREV_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// SHA-256 (hex) of one chunk's exact content.
pub fn hash_chunk(content: &str) -> String {
    sha256_hex(content)
}

/// The chain link for chunk `index` given the previous chunk's hash.
/// Chunk 1 (index 1) links to genesis; every later chunk links to the
/// immediately preceding chunk's `sha256`.
pub fn prev_link_for(index: u64, previous_hash: Option<&str>) -> String {
    if index <= 1 {
        GENESIS_PREV_SHA256.to_string()
    } else {
        previous_hash.unwrap_or(GENESIS_PREV_SHA256).to_string()
    }
}

/// SHA-256 root over the ordered chunk hashes. Domain separation plus an
/// explicit count/length framing makes the root unambiguous for any number
/// of chunks and resistant to trivial concatenation collisions.
pub fn sha256_root(chunk_hashes: &[String]) -> String {
    let mut acc = String::with_capacity(32 + chunk_hashes.len() * 65);
    acc.push_str("dataset-root/v1\n");
    acc.push_str(&format!("{}\n", chunk_hashes.len()));
    for h in chunk_hashes {
        acc.push_str(h);
        acc.push('\n');
    }
    sha256_hex(&acc)
}

/// Constant-time equality for hex digests (and any short byte strings).
/// Returns `false` on a length mismatch without leaking which byte differed.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut diff = a.len() ^ b.len();
    let n = a.len().min(b.len());
    for i in 0..n {
        diff |= (a[i] ^ b[i]) as usize;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_matches_known_sha256_vector() {
        // SHA-256("abc") — pins the reused bot-core primitive.
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn root_is_order_sensitive_and_deterministic() {
        let a = vec!["a".repeat(64), "b".repeat(64)];
        let b = vec!["b".repeat(64), "a".repeat(64)];
        assert_eq!(sha256_root(&a), sha256_root(&a));
        assert_ne!(sha256_root(&a), sha256_root(&b));
        assert_ne!(sha256_root(&a), sha256_root(&a[..1]));
    }

    #[test]
    fn constant_time_eq_is_exact() {
        let h = "deadbeef".repeat(8);
        assert!(constant_time_eq(&h, &"deadbeef".repeat(8)));
        assert!(!constant_time_eq(&h, &"deadbeef".repeat(7)));
        let mut flipped = h.clone();
        flipped.replace_range(0..1, "0");
        // only assert when it actually differs
        if flipped != h {
            assert!(!constant_time_eq(&h, &flipped));
        }
    }

    #[test]
    fn genesis_is_64_zeros() {
        assert_eq!(GENESIS_PREV_SHA256.len(), 64);
        assert!(GENESIS_PREV_SHA256.bytes().all(|b| b == b'0'));
        assert_eq!(prev_link_for(1, Some("anything")), GENESIS_PREV_SHA256);
        let prev = "ab".repeat(32);
        assert_eq!(prev_link_for(2, Some(&prev)), prev);
    }
}
