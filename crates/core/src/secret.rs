//! Secret string wrapper (File 3 §6.5).
//!
//! `SecretString` keeps credential material out of logs and debug output:
//!
//! * `Debug` and `Display` print `<redacted>`, so `{:?}` on a config struct
//!   never shows a loaded value;
//! * the buffer is wiped on drop (`zeroize`);
//! * equality is constant-time over the bytes, so comparing a presented
//!   token with a stored one does not leak the match position;
//! * the only way to read the value is [`SecretString::expose`], which makes
//!   every use site explicit and greppable.
//!
//! Deserialisation accepts a plain string, so existing TOML/env-derived
//! configuration keeps working unchanged.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

/// A credential string that never prints its value.
#[derive(Clone, Default)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    /// Wrap a value. An empty string is allowed and reports `is_empty()`.
    pub fn new(value: impl Into<String>) -> Self {
        SecretString(Zeroizing::new(value.into()))
    }

    /// Read the secret. Call sites must be explicit; never log the result.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    /// True when no value is set (or the value is the empty string).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Constant-time comparison against a candidate value.
    pub fn ct_eq_str(&self, candidate: &str) -> bool {
        ct_eq(self.0.as_bytes(), candidate.as_bytes())
    }
}

/// Constant-time byte comparison. The length check is not secret-dependent
/// in a way that matters here: the stored secret's length is not hidden by
/// this type, only its contents.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    // Keep the accumulator live so the loop is not optimised away.
    std::hint::black_box(diff) == 0
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        ct_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

impl Eq for SecretString {}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        SecretString::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        SecretString::new(value)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(SecretString::new)
    }
}

/// Serialising a secret writes the redaction marker, never the value. A
/// config round-trip therefore cannot leak credentials into a file or log.
impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_are_redacted() {
        let secret = SecretString::new("hunter2-very-secret");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert!(!format!("{secret:?}").contains("hunter2"));
    }

    #[test]
    fn struct_debug_never_shows_the_value() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Cfg {
            key: SecretString,
            name: String,
        }
        let cfg = Cfg {
            key: SecretString::new("TOKEN-XYZ-123"),
            name: "public".into(),
        };
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("TOKEN-XYZ-123"), "{rendered}");
        assert!(rendered.contains("public"));
    }

    #[test]
    fn expose_returns_the_value() {
        let secret = SecretString::new("abc");
        assert_eq!(secret.expose(), "abc");
        assert!(!secret.is_empty());
        assert!(SecretString::default().is_empty());
    }

    #[test]
    fn constant_time_equality_is_exact() {
        let secret = SecretString::new("same-value");
        assert!(secret.ct_eq_str("same-value"));
        assert!(!secret.ct_eq_str("same-valuE"));
        assert!(!secret.ct_eq_str("same-value-extra"));
        assert!(!secret.ct_eq_str(""));
        assert_eq!(SecretString::new("a"), SecretString::new("a"));
        assert_ne!(SecretString::new("a"), SecretString::new("b"));
    }

    #[test]
    fn deserialises_from_a_plain_string() {
        #[derive(Deserialize)]
        struct Holder {
            token: SecretString,
        }
        let parsed: Holder = serde_json::from_str(r#"{"token":"from-json"}"#).unwrap();
        assert_eq!(parsed.token.expose(), "from-json");
    }

    #[test]
    fn serialisation_writes_the_marker_not_the_value() {
        let json = serde_json::to_string(&SecretString::new("leak-me")).unwrap();
        assert_eq!(json, "\"<redacted>\"");
        assert!(!json.contains("leak-me"));
    }

    #[test]
    fn clone_keeps_the_value() {
        let secret = SecretString::new("cloned");
        let copy = secret.clone();
        assert_eq!(copy.expose(), "cloned");
    }
}
