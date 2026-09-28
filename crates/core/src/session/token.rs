//! Opaque credential material: generation, hashing and verification
//! (TASK 7A file 10).
//!
//! Three kinds of secret exist in the control plane — session tokens, API
//! key secrets and invitation tokens — and none of them may ever be stored,
//! logged or returned after creation. This module is the ONLY place that
//! produces or checks them, so that rule is enforceable by review.
//!
//! * **Tokens** (session / API key / invite) are 256 bits of OS randomness
//!   rendered base64url without padding, carrying a human-readable prefix
//!   (`ses_`, `sk_`, `inv_`). They are high-entropy and never chosen by a
//!   user, so a single SHA-256 is the right lookup hash: it is
//!   deterministic (needed for an indexed lookup) and there is nothing to
//!   brute-force. This matches what [`crate::auth::sha256_hex`] already
//!   does for deployment keys.
//! * **Passwords** are low entropy and human-chosen, so they get PBKDF2-
//!   HMAC-SHA256 with a per-password random salt and a high iteration
//!   count, encoded as
//!   `pbkdf2-sha256$<iterations>$<salt_b64>$<hash_b64>`. Verification is
//!   constant-time.
//!
//! Comparisons use a constant-time equality so a timing side channel
//! cannot reveal how much of a hash matched.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Bytes of randomness in a generated token (256 bits).
pub const TOKEN_BYTES: usize = 32;

/// Bytes of randomness in a password salt (128 bits).
pub const SALT_BYTES: usize = 16;

/// PBKDF2 iteration count. OWASP's 2023 floor for PBKDF2-HMAC-SHA256.
pub const PBKDF2_ITERATIONS: u32 = 600_000;

/// How many characters of a token form its public, non-secret prefix
/// (`ses_ab12cd34`) — enough to correlate logs, far too little to guess.
pub const PUBLIC_PREFIX_CHARS: usize = 8;

/// A freshly generated credential: the plaintext exists only in this value
/// and is returned to the caller exactly once.
#[derive(Debug, Clone)]
pub struct GeneratedToken {
    /// The full secret, e.g. `sk_9f3a…`. Show once, never store.
    pub plaintext: String,
    /// SHA-256 hex of `plaintext`. This is what goes in the database.
    pub hash: String,
    /// Public identifier (`sk_9f3a1b2c`), safe in listings and logs.
    pub prefix: String,
}

impl GeneratedToken {
    /// Deliberately opaque: a `Debug`/log line must never leak the secret.
    pub fn redacted(&self) -> String {
        format!("{}…(redacted)", self.prefix)
    }
}

/// Generate a token with `kind` as its namespace prefix (`ses`, `sk`, `inv`).
pub fn generate_token(kind: &str) -> GeneratedToken {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let body = URL_SAFE_NO_PAD.encode(bytes);
    let kind = kind.trim().trim_end_matches('_');
    let plaintext = if kind.is_empty() {
        body
    } else {
        format!("{kind}_{body}")
    };
    let hash = hash_token(&plaintext);
    let prefix = public_prefix(&plaintext);
    GeneratedToken {
        plaintext,
        hash,
        prefix,
    }
}

/// The lookup hash of a token (SHA-256 hex). Same construction as the
/// existing deployment-key hashing, so both credential families are stored
/// the same way.
pub fn hash_token(plaintext: &str) -> String {
    hex::encode(Sha256::digest(plaintext.as_bytes()))
}

/// Constant-time check of a presented token against a stored hash.
pub fn verify_token(presented: &str, stored_hash: &str) -> bool {
    if presented.trim().is_empty() || stored_hash.trim().is_empty() {
        return false;
    }
    constant_time_eq(hash_token(presented).as_bytes(), stored_hash.as_bytes())
}

/// The public prefix of a token: the namespace plus the first
/// [`PUBLIC_PREFIX_CHARS`] characters of the random body.
pub fn public_prefix(plaintext: &str) -> String {
    match plaintext.split_once('_') {
        Some((kind, body)) => {
            let take: String = body.chars().take(PUBLIC_PREFIX_CHARS).collect();
            format!("{kind}_{take}")
        }
        None => plaintext.chars().take(PUBLIC_PREFIX_CHARS).collect(),
    }
}

/// Hash a password with PBKDF2-HMAC-SHA256 and a fresh random salt.
/// Returns the encoded string that belongs in `users.password_hash`.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; SALT_BYTES];
    rand::thread_rng().fill_bytes(&mut salt);
    hash_password_with(password, &salt, PBKDF2_ITERATIONS)
}

/// Hash with an explicit salt and iteration count (tests, re-hashing).
pub fn hash_password_with(password: &str, salt: &[u8], iterations: u32) -> String {
    let dk = pbkdf2_sha256(password.as_bytes(), salt, iterations.max(1), 32);
    format!(
        "pbkdf2-sha256${}${}${}",
        iterations.max(1),
        URL_SAFE_NO_PAD.encode(salt),
        URL_SAFE_NO_PAD.encode(dk)
    )
}

/// Verify a password against an encoded hash. Unparseable input is a
/// failure, never a pass.
pub fn verify_password(password: &str, encoded: &str) -> bool {
    let Some(parsed) = ParsedPasswordHash::parse(encoded) else {
        return false;
    };
    let dk = pbkdf2_sha256(
        password.as_bytes(),
        &parsed.salt,
        parsed.iterations,
        parsed.hash.len(),
    );
    constant_time_eq(&dk, &parsed.hash)
}

/// The parts of an encoded PBKDF2 string.
struct ParsedPasswordHash {
    iterations: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

impl ParsedPasswordHash {
    fn parse(encoded: &str) -> Option<Self> {
        let mut parts = encoded.trim().split('$');
        if parts.next()? != "pbkdf2-sha256" {
            return None;
        }
        let iterations: u32 = parts.next()?.parse().ok()?;
        if iterations == 0 {
            return None;
        }
        let salt = URL_SAFE_NO_PAD.decode(parts.next()?).ok()?;
        let hash = URL_SAFE_NO_PAD.decode(parts.next()?).ok()?;
        if parts.next().is_some() || salt.is_empty() || hash.is_empty() {
            return None;
        }
        Some(ParsedPasswordHash {
            iterations,
            salt,
            hash,
        })
    }
}

/// PBKDF2-HMAC-SHA256 (RFC 8018). Implemented on the `hmac`/`sha2` crates
/// the workspace already depends on, so TASK 7A adds no new dependency.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, out_len: usize) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    const HASH_LEN: usize = 32;

    let mut out = Vec::with_capacity(out_len);
    let blocks = out_len.div_ceil(HASH_LEN);
    for block in 1..=blocks {
        // U1 = PRF(password, salt || INT_32_BE(block))
        let mut mac = HmacSha256::new_from_slice(password).expect("hmac accepts any key length");
        mac.update(salt);
        mac.update(&(block as u32).to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        let mut t = u;
        // U2..Uc
        for _ in 1..iterations {
            let mut mac =
                HmacSha256::new_from_slice(password).expect("hmac accepts any key length");
            mac.update(&u);
            u = mac.finalize().into_bytes();
            for (t_byte, u_byte) in t.iter_mut().zip(u.iter()) {
                *t_byte ^= *u_byte;
            }
        }
        out.extend_from_slice(&t);
    }
    out.truncate(out_len);
    out
}

/// Constant-time byte comparison: the running time depends only on the
/// length, never on where the first difference is.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_unique_high_entropy_and_prefixed() {
        let a = generate_token("ses");
        let b = generate_token("ses");
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.hash, b.hash);
        assert!(a.plaintext.starts_with("ses_"));
        assert!(a.prefix.starts_with("ses_"));
        assert_eq!(a.prefix.len(), "ses_".len() + PUBLIC_PREFIX_CHARS);
        // 32 random bytes base64url → 43 characters.
        assert_eq!(a.plaintext.len(), "ses_".len() + 43);
        assert_eq!(a.hash.len(), 64, "sha-256 hex");
        // The prefix alone must not authenticate.
        assert!(!verify_token(&a.prefix, &a.hash));
        // A redacted line never contains the secret body.
        assert!(!a.redacted().contains(&a.plaintext[8..]));
    }

    #[test]
    fn token_verification_is_exact() {
        let t = generate_token("sk");
        assert!(verify_token(&t.plaintext, &t.hash));
        assert!(!verify_token(&format!("{}x", t.plaintext), &t.hash));
        assert!(!verify_token(
            &t.plaintext[..t.plaintext.len() - 1],
            &t.hash
        ));
        assert!(!verify_token("", &t.hash));
        assert!(!verify_token(&t.plaintext, ""));
        // A different token never matches.
        let other = generate_token("sk");
        assert!(!verify_token(&other.plaintext, &t.hash));
    }

    #[test]
    fn passwords_are_salted_and_verify() {
        let encoded = hash_password("correct horse battery staple");
        assert!(encoded.starts_with("pbkdf2-sha256$600000$"));
        assert!(verify_password("correct horse battery staple", &encoded));
        assert!(!verify_password("wrong password", &encoded));
        // The plaintext never appears in the stored form.
        assert!(!encoded.contains("correct"));
        // The same password hashed twice differs (random salt).
        let again = hash_password("correct horse battery staple");
        assert_ne!(encoded, again);
        assert!(verify_password("correct horse battery staple", &again));
    }

    #[test]
    fn malformed_password_hashes_never_verify() {
        for bad in [
            "",
            "not-a-hash",
            "pbkdf2-sha256$0$c2FsdA$aGFzaA",
            "pbkdf2-sha256$600000$$aGFzaA",
            "pbkdf2-sha256$600000$c2FsdA$",
            "argon2id$v=19$m=1,t=1,p=1$c2FsdA$aGFzaA",
            "pbkdf2-sha256$600000$c2FsdA$aGFzaA$extra",
        ] {
            assert!(!verify_password("anything", bad), "{bad:?} must not verify");
        }
    }

    #[test]
    fn pbkdf2_matches_the_rfc_6070_style_vector() {
        // RFC 6070 defines vectors for SHA-1; for SHA-256 the widely used
        // equivalent is: P="password", S="salt", c=1, dkLen=32 →
        // 120fb6cffcf8b32c43e7225256c4f837a86548c9 2ccc35480805987cb70be17b
        let dk = pbkdf2_sha256(b"password", b"salt", 1, 32);
        assert_eq!(
            hex::encode(dk),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        // c=2 vector.
        let dk = pbkdf2_sha256(b"password", b"salt", 2, 32);
        assert_eq!(
            hex::encode(dk),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
        // Output longer than one block exercises the block loop.
        let dk = pbkdf2_sha256(b"passwd", b"salt", 1, 64);
        assert_eq!(dk.len(), 64);
    }

    #[test]
    fn constant_time_eq_is_correct() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn public_prefix_handles_every_shape() {
        assert_eq!(public_prefix("ses_abcdefghijkl"), "ses_abcdefgh");
        assert_eq!(public_prefix("short"), "short");
        assert_eq!(public_prefix("ses_ab"), "ses_ab");
        assert_eq!(public_prefix(""), "");
        // A token generated without a namespace still yields a prefix.
        let t = generate_token("");
        assert!(!t.prefix.is_empty());
        assert!(!t.plaintext.starts_with('_'));
    }
}
