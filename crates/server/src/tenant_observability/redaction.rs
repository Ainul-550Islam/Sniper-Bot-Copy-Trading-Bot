//! Tenant-safe redaction rules (tenant-isolation file 61).
//!
//! [`Redaction`] is the single source of truth for what may appear in
//! tenant-facing logs, error messages and diagnostics: key NAMES that
//! smell like secrets are masked wholesale; key names that are safe
//! pass through; and nothing about the rules depends on the caller
//! remembering to apply them — the sweep helpers exist so generic
//! maps and error strings go through ONE place.
//!
//! Masked value: `[REDACTED]` (stable, greppable, obviously not data).

use std::borrow::Cow;

/// The redaction engine (stateless rules).
pub struct Redaction;

/// Key fragments that mark a key as secret-shaped. Matched
/// case-insensitively against the whole key name.
const SECRET_KEY_FRAGMENTS: [&str; 16] = [
    "seed",
    "mnemonic",
    "private_key",
    "privatekey",
    "secret",
    "password",
    "passwd",
    "token",
    "bearer",
    "authorization",
    "credential",
    "api_key",
    "apikey",
    "key_material",
    "keymaterial",
    "signer_payload",
];

/// The masked value substituted for secret content.
pub const REDACTED: &str = "[REDACTED]";

impl Redaction {
    /// Is this key name secret-shaped?
    pub fn is_secret_key(key: &str) -> bool {
        let normalized = key
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '.'], "_");
        SECRET_KEY_FRAGMENTS
            .iter()
            .any(|fragment| normalized.contains(fragment))
    }

    /// Redact a value under a key: secret-shaped keys yield
    /// `[REDACTED]` regardless of the value; safe keys pass through.
    pub fn redact_value<'a>(key: &str, value: &'a str) -> Cow<'a, str> {
        if Self::is_secret_key(key) {
            Cow::Borrowed(REDACTED)
        } else {
            Cow::Borrowed(value)
        }
    }

    /// Sweep a set of string pairs (a generic field map before it
    /// reaches a log line or a diagnostic body).
    pub fn sanitize_pairs(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
        pairs
            .into_iter()
            .map(|(key, value)| {
                if Self::is_secret_key(&key) {
                    (key, REDACTED.to_string())
                } else {
                    (key, value)
                }
            })
            .collect()
    }

    /// Redact secret-shaped assignments out of a free-form diagnostic
    /// string: `token=abc`, `"private_key": "abc"`, `seed: abc` all
    /// become `…=[REDACTED]` / `…": "[REDACTED]"` / `…: [REDACTED]`.
    /// Text without secret-shaped keys is returned unchanged.
    pub fn redact_text(text: &str) -> String {
        let mut out = text.to_string();
        for fragment in SECRET_KEY_FRAGMENTS {
            // `key=value` / `key": "value"` / `key: value` forms.
            for pattern in [
                // kebab/snake forms followed by '=' (no spaces)
                format!("{fragment}="),
                format!("{fragment} ="),
                // JSON-ish forms
                format!("\"{fragment}\":"),
                // colon forms
                format!("{fragment}:"),
            ] {
                let mut search_from = 0;
                loop {
                    // Rebuild the haystack after every replacement so
                    // offsets can never go stale.
                    let lowercase_hay = out.to_ascii_lowercase();
                    let Some(rel) = lowercase_hay[search_from..].find(&pattern) else {
                        break;
                    };
                    let start = search_from + rel;
                    // Only redact when the fragment starts a key: the
                    // preceding character must not be alphanumeric or
                    // '_' (so "seedling" does not match "seed", while
                    // "wallet_seed" correctly does).
                    let starts_key = start == 0
                        || !out[..start]
                            .chars()
                            .next_back()
                            .map(|c| c.is_alphanumeric() || c == '_')
                            .unwrap_or(false);
                    if starts_key {
                        let value_start = start + pattern.len();
                        // Skip whitespace and an optional opening quote.
                        let mut vs = value_start;
                        let bytes = out.as_bytes();
                        while vs < bytes.len() && (bytes[vs] as char).is_whitespace() {
                            vs += 1;
                        }
                        if vs < bytes.len() && bytes[vs] == b'"' {
                            vs += 1;
                        }
                        // Consume the value run (to whitespace, quote
                        // or end).
                        let mut ve = vs;
                        while ve < bytes.len()
                            && !matches!(bytes[ve] as char, ' ' | '"' | ',' | '}' | '\n' | '\t')
                        {
                            ve += 1;
                        }
                        if ve > vs {
                            out.replace_range(vs..ve, REDACTED);
                        }
                    }
                    search_from = start + pattern.len().max(1);
                    if search_from >= out.len() {
                        break;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_shaped_keys_are_detected() {
        for key in [
            "seed",
            "SEED",
            "private_key",
            "wallet.seed",
            "api_key",
            "apiKey",
            "bearer_token",
            "authorization",
            "provider_credentials",
            "signer_payload",
            "mnemonic_phrase",
        ] {
            assert!(Redaction::is_secret_key(key), "{key} must be secret-shaped");
        }
    }

    #[test]
    fn safe_keys_are_not_detected() {
        for key in [
            "organization_id",
            "runtime_id",
            "module",
            "request_id",
            "execution_id",
            "order_id",
            "correlation_id",
            "principal",
            "origin",
            "mode",
            "decision",
            "worker_id",
        ] {
            assert!(!Redaction::is_secret_key(key), "{key} must stay safe");
        }
    }

    #[test]
    fn redact_value_masks_secrets_and_passes_safe_values() {
        assert_eq!(Redaction::redact_value("token", "abc123"), REDACTED);
        assert_eq!(Redaction::redact_value("seed", "word word word"), REDACTED);
        assert_eq!(Redaction::redact_value("module", "copy"), "copy");
    }

    #[test]
    fn sanitize_pairs_sweeps_a_generic_map() {
        let swept = Redaction::sanitize_pairs(vec![
            ("organization_id".to_string(), "org-1".to_string()),
            ("private_key".to_string(), "[base58blob]".to_string()),
            ("order_id".to_string(), "ord-9".to_string()),
        ]);
        assert_eq!(swept[0].1, "org-1");
        assert_eq!(swept[1].1, REDACTED);
        assert_eq!(swept[2].1, "ord-9");
    }

    #[test]
    fn redact_text_masks_free_form_assignments() {
        let diagnostic = "order ord-1 failed: token=abc123 for org org-9 seed: word1 word2";
        let redacted = Redaction::redact_text(diagnostic);
        assert!(redacted.contains("token=[REDACTED]"), "{redacted}");
        assert!(redacted.contains("seed: [REDACTED]"), "{redacted}");
        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("ord-1"));
        // A safe word containing no secret fragment is untouched.
        let clean = "organization_id=org-9 module=copy";
        assert_eq!(Redaction::redact_text(clean), clean);
    }

    #[test]
    fn redact_text_masks_json_forms() {
        let json = r#"{"private_key": "abc", "module": "copy"}"#;
        let redacted = Redaction::redact_text(json);
        assert!(
            redacted.contains(r#""private_key": "[REDACTED]""#),
            "{redacted}"
        );
        assert!(redacted.contains(r#""module": "copy""#));
    }
}
