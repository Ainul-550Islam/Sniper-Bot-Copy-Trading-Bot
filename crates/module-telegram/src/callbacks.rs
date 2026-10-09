//! Inline-keyboard callback handling for trade confirmations
//! (GAP-MAP v2, P2).
//!
//! Money-moving Telegram commands are NEVER executed on the first tap:
//! `/buy` and `/sell` reply with an inline keyboard (Confirm / Cancel) and
//! only the CONFIRM tap — issued by the SAME user, within the session TTL,
//! carrying a valid signature — releases the trade (see
//! [`crate::trade_session::TradeSession::confirm`]).
//!
//! ## Why the payload is signed
//! `callback_data` round-trips through Telegram's servers and is
//! client-controlled: without a signature, anyone who sees (or guesses) a
//! keyboard could replay, forge or race confirmations. Every button carries
//!
//! ```text
//! sc:v1:<nonce>:<sig>    confirm     (sx = cancel)
//! sig = hex(HMAC-SHA256(secret, "sc:v1:<nonce>"))[..16 bytes]
//! ```
//!
//! * the secret is process-local (env `TELEGRAM_TRADE_SECRET`, or random —
//!   a restart also drops the in-memory pending map, so a random secret
//!   never invalidates a still-valid confirmation);
//! * total payload length is 55 chars — safely under Telegram's 64-byte
//!   `callback_data` cap;
//! * verification is fail-closed: malformed, wrong-version or unsigned
//!   payloads are rejected with a typed error and the tap answered with an
//!   explanatory toast (the user is never left staring at a spinner).

use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

/// Environment variable carrying the callback signing secret (hex, 32
/// bytes). Optional: unset → a random per-process secret is generated.
pub const TRADE_SECRET_ENV: &str = "TELEGRAM_TRADE_SECRET";

type HmacSha256 = Hmac<Sha256>;

/// Callback payload protocol version.
const VERSION: &str = "v1";
/// Confirm-action prefix.
const CONFIRM_PREFIX: &str = "sc";
/// Cancel-action prefix.
const CANCEL_PREFIX: &str = "sx";

/// What a verified callback asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackAction {
    /// The requester tapped ✅ Confirm.
    Confirm { nonce: String },
    /// The requester tapped ❌ Cancel.
    Cancel { nonce: String },
}

impl CallbackAction {
    /// The confirmation nonce this tap refers to.
    pub fn nonce(&self) -> &str {
        match self {
            CallbackAction::Confirm { nonce } | CallbackAction::Cancel { nonce } => nonce,
        }
    }
}

/// Why a callback payload was rejected. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackDeny {
    /// Not our format at all (another bot's keyboard, garbage).
    NotOurs,
    /// Unknown protocol version.
    BadVersion,
    /// Structurally malformed (wrong field count, non-hex parts).
    Malformed,
    /// Signature mismatch — forged or corrupted.
    BadSignature,
}

impl CallbackDeny {
    /// Toast shown to the tapper (never reveals which check failed).
    pub fn toast(&self) -> &'static str {
        match self {
            CallbackDeny::NotOurs => "Not a trade confirmation.",
            CallbackDeny::BadVersion | CallbackDeny::Malformed => {
                "This confirmation is malformed — send the command again."
            }
            CallbackDeny::BadSignature => "This confirmation failed verification.",
        }
    }
}

/// The signing secret for callback payloads.
#[derive(Clone)]
pub struct CallbackSecret([u8; 32]);

impl CallbackSecret {
    /// Load from the environment (hex). Unset/blank → random. Invalid hex
    /// fails closed (a typo'd secret must not silently downgrade to
    /// random and invalidate a multi-replica setup without notice).
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(TRADE_SECRET_ENV) {
            Ok(raw) if !raw.trim().is_empty() => {
                let bytes = hex::decode(raw.trim())
                    .map_err(|e| format!("telegram trade secret is not hex: {e}"))?;
                if bytes.len() != 32 {
                    return Err(format!(
                        "telegram trade secret must be 32 bytes (64 hex chars), got {}",
                        bytes.len()
                    ));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                Ok(Self(arr))
            }
            _ => Ok(Self::random()),
        }
    }

    /// Random per-process secret (safe default for single-process bots).
    pub fn random() -> Self {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        Self(rng.gen())
    }

    /// Build from raw bytes (server wiring: shared across replicas so a
    /// confirmation issued by one replica verifies on another).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Sign a `<prefix>:<version>:<nonce>` payload.
    fn sign(&self, body: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.0)
            .expect("hmac accepts any key length");
        mac.update(body.as_bytes());
        let full = mac.finalize().into_bytes();
        hex::encode(&full[..16])
    }

    /// Encode the callback_data for one button.
    pub fn encode(&self, confirm: bool, nonce: &str) -> String {
        let prefix = if confirm { CONFIRM_PREFIX } else { CANCEL_PREFIX };
        let body = format!("{prefix}:{VERSION}:{nonce}");
        let sig = self.sign(&body);
        format!("{body}:{sig}")
    }

    /// Verify and decode a callback_data payload. Anything not signed by
    /// this secret (including other bots' keyboards) fails with a typed
    /// deny — the caller answers the tap with the deny's toast and drops
    /// it.
    pub fn verify(&self, data: &str) -> Result<CallbackAction, CallbackDeny> {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() != 4 {
            // Foreign keyboards: Telegram delivers ANY bot's callbacks we
            // were attached to; four fields is ours, anything else is not.
            return Err(if parts.len() >= 2 && (parts[0] == CONFIRM_PREFIX || parts[0] == CANCEL_PREFIX) {
                CallbackDeny::Malformed
            } else {
                CallbackDeny::NotOurs
            });
        }
        let (prefix, version, nonce, sig) = (parts[0], parts[1], parts[2], parts[3]);
        if prefix != CONFIRM_PREFIX && prefix != CANCEL_PREFIX {
            return Err(CallbackDeny::NotOurs);
        }
        if version != VERSION {
            return Err(CallbackDeny::BadVersion);
        }
        if nonce.len() != 16 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CallbackDeny::Malformed);
        }
        if sig.len() != 32 || !sig.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CallbackDeny::Malformed);
        }
        let body = format!("{prefix}:{version}:{nonce}");
        let expected = self.sign(&body);
        // Constant-time compare (both are fixed-width hex).
        let ok = expected
            .bytes()
            .zip(sig.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
        if !ok {
            return Err(CallbackDeny::BadSignature);
        }
        Ok(if prefix == CONFIRM_PREFIX {
            CallbackAction::Confirm { nonce: nonce.to_string() }
        } else {
            CallbackAction::Cancel { nonce: nonce.to_string() }
        })
    }

    /// Build the reply_markup JSON for a confirmation keyboard.
    pub fn confirm_keyboard(&self, nonce: &str) -> String {
        json!({
            "inline_keyboard": [[
                { "text": "✅ Confirm", "callback_data": self.encode(true, nonce) },
                { "text": "❌ Cancel", "callback_data": self.encode(false, nonce) }
            ]]
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> CallbackSecret {
        CallbackSecret::from_bytes([7u8; 32])
    }

    #[test]
    fn payload_round_trips_and_fits_telegram_cap() {
        let s = secret();
        let data = s.encode(true, "0123456789abcdef");
        assert!(data.len() <= 64, "Telegram caps callback_data at 64 bytes: {}", data.len());
        assert_eq!(data.len(), 55);
        match s.verify(&data).unwrap() {
            CallbackAction::Confirm { nonce } => assert_eq!(nonce, "0123456789abcdef"),
            other => panic!("expected Confirm, got {other:?}"),
        }
        let cancel = s.encode(false, "0123456789abcdef");
        assert!(matches!(
            s.verify(&cancel).unwrap(),
            CallbackAction::Cancel { .. }
        ));
    }

    #[test]
    fn tampered_payloads_fail_closed() {
        let s = secret();
        let data = s.encode(true, "0123456789abcdef");
        // Flip one char of the nonce (index 7 sits inside the nonce part of
        // "sc:v1:<nonce>:<sig>").
        let mut bytes = data.as_bytes().to_vec();
        bytes[7] ^= 0x01;
        let tampered = String::from_utf8(bytes).unwrap();
        assert_eq!(s.verify(&tampered).unwrap_err(), CallbackDeny::BadSignature);
        // Flip the last signature char to a DIFFERENT hex digit, so the
        // tampered value still has valid shape and reaches the MAC check.
        // (A raw bit flip can turn 'a'/'f' into a non-hex char -> Malformed.)
        let mut bytes = data.as_bytes().to_vec();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        let tampered_sig = String::from_utf8(bytes).unwrap();
        assert_eq!(s.verify(&tampered_sig).unwrap_err(), CallbackDeny::BadSignature);
    }

    #[test]
    fn wrong_secret_never_verifies() {
        let a = secret();
        let b = CallbackSecret::from_bytes([9u8; 32]);
        let data = a.encode(true, "0123456789abcdef");
        assert_eq!(b.verify(&data).unwrap_err(), CallbackDeny::BadSignature);
    }

    #[test]
    fn foreign_and_malformed_payloads_are_distinguished() {
        let s = secret();
        assert_eq!(s.verify("menu:open:42").unwrap_err(), CallbackDeny::NotOurs);
        assert_eq!(s.verify("").unwrap_err(), CallbackDeny::NotOurs);
        // Right prefix, wrong shape -> malformed, not foreign.
        assert_eq!(s.verify("sc:only-two").unwrap_err(), CallbackDeny::Malformed);
        assert_eq!(
            s.verify("sc:v9:0123456789abcdef:00000000000000000000000000000000").unwrap_err(),
            CallbackDeny::BadVersion
        );
        assert_eq!(
            s.verify("sc:v1:zzzz:00000000000000000000000000000000").unwrap_err(),
            CallbackDeny::Malformed
        );
    }

    #[test]
    fn swap_confirm_cancel_prefix_breaks_signature() {
        let s = secret();
        let confirm = s.encode(true, "0123456789abcdef");
        // Rewrite the prefix to cancel, keep the signature.
        let forged = confirm.replacen("sc:", "sx:", 1);
        assert_eq!(s.verify(&forged).unwrap_err(), CallbackDeny::BadSignature);
    }

    #[test]
    fn keyboard_json_carries_both_buttons() {
        let s = secret();
        let kb = s.confirm_keyboard("0123456789abcdef");
        let v: serde_json::Value = serde_json::from_str(&kb).unwrap();
        let row = &v["inline_keyboard"][0];
        assert_eq!(row.as_array().unwrap().len(), 2);
        assert!(row[0]["text"].as_str().unwrap().contains("Confirm"));
        assert!(row[1]["text"].as_str().unwrap().contains("Cancel"));
        // Both payloads verify under the same secret.
        assert!(s.verify(row[0]["callback_data"].as_str().unwrap()).is_ok());
        assert!(s.verify(row[1]["callback_data"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn env_loading_accepts_hex_and_rejects_junk() {
        std::env::remove_var(TRADE_SECRET_ENV);
        assert!(CallbackSecret::from_env().is_ok(), "unset -> random");
        std::env::set_var(TRADE_SECRET_ENV, "00ff".repeat(16).as_str());
        assert!(CallbackSecret::from_env().is_ok());
        std::env::set_var(TRADE_SECRET_ENV, "zzzz");
        assert!(CallbackSecret::from_env().is_err(), "non-hex fails closed");
        std::env::set_var(TRADE_SECRET_ENV, "00ff");
        assert!(CallbackSecret::from_env().is_err(), "short fails closed");
        std::env::remove_var(TRADE_SECRET_ENV);
    }
}
