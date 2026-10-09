//! Builder-code attribution for Polymarket orders (GAP-MAP v2, P2).
//!
//! ## Verified against the CURRENT CLOB docs (2026-10, V2)
//! Polymarket retired the V1 builder flow (the `POLY_BUILDER_*` HMAC
//! headers + `@polymarket/builder-signing-sdk`) at the CLOB V2 cutover
//! (2026-04-28). Attribution is now NATIVE to the order: the signed
//! EIP-712 `Order` struct carries a `builder` field — a **bytes32**
//! identifier from the operator's Builder Profile:
//!
//! ```text
//! Order(salt, maker, signer, taker, tokenId, makerAmount, takerAmount,
//!       expiration, nonce, feeRateBps, side, signatureType,
//!       timestamp, metadata, builder)          // <- bytes32 builder
//! ```
//!
//! Consequences encoded here:
//! * there is NO header flow to configure — nothing HMAC, nothing env
//!   header list; the builder code is attached PER ORDER (or inherited
//!   from client construction), so this module's only job is to produce
//!   the exact bytes32 value that goes into the order struct;
//! * builder codes are PUBLIC identifiers (they appear on-chain in every
//!   attributed order), so carrying them in config/env is not a secret
//!   handling problem;
//! * `0x00…00` (32 zero bytes) is the "no builder" sentinel; an order
//!   without attribution sends zeros.
//!
//! This module is pure and dependency-free: validation + encoding only.
//! The venue client ([`crate::venue`]) attaches the returned bytes32 when
//! building V2 orders; wiring the field through `eip712.rs`/`venue.rs` is
//! a deliberate follow-up (the V2 order struct migration touches those
//! files as a unit, see the migration checklist in
//! `docs/archive/GAP-MISSING-FILES-2026-10-06.md` (historical snapshot).
//!
//! Sources (fetched 2026-10-08):
//! * docs.polymarket.com/v2-migration — "A single builderCode field on the
//!   order … replacing the old HMAC-header flow"; zero = no attribution.
//! * docs.polymarket.com/developers/CLOB/clients/methods-builder —
//!   builder-attributed `getOpenOrders` / builder trades methods.

use crate::error::{PolyError, PolyResult};

/// The bytes32 "no builder" sentinel: order carries no attribution.
pub const ZERO_BUILDER: [u8; 32] = [0u8; 32];

/// Environment variable carrying the operator's builder code. Accepted
/// forms: a 32-byte `0x`-hex string (canonical), or a short ASCII code
/// (right-NUL-padded to 32 bytes, matching bytes32 ABI encoding).
pub const BUILDER_CODE_ENV: &str = "POLY_BUILDER_CODE";

/// A validated builder code, ready to be written into the V2 order struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderCode {
    bytes: [u8; 32],
}

impl BuilderCode {
    /// Parse and validate a builder code from config/env.
    ///
    /// Accepted:
    /// * `0x` + 64 hex chars — used verbatim as the bytes32 value;
    /// * 1–32 printable ASCII chars — right-NUL-padded to 32 bytes
    ///   (standard bytes32 string encoding).
    ///
    /// Everything else fails closed. The empty string is NOT a valid code
    /// — "no attribution" must be expressed by [`Self::disabled`], never by
    /// an empty config value sneaking through validation.
    pub fn parse(raw: &str) -> PolyResult<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(PolyError::invalid(
                "builder code: empty value — use disabled() for no attribution",
            ));
        }
        if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
            if hex.len() != 64 {
                return Err(PolyError::invalid(format!(
                    "builder code: 0x form must be exactly 32 bytes (64 hex chars), got {}",
                    hex.len()
                )));
            }
            if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(PolyError::invalid("builder code: invalid hex digit"));
            }
            let mut bytes = [0u8; 32];
            for (i, slot) in bytes.iter_mut().enumerate() {
                *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                    .map_err(|e| PolyError::invalid(format!("builder code: {e}")))?;
            }
            if bytes == ZERO_BUILDER {
                return Err(PolyError::invalid(
                    "builder code: all-zero value is the no-attribution sentinel, not a code",
                ));
            }
            return Ok(Self { bytes });
        }
        // Short ASCII code form.
        if trimmed.len() > 32 {
            return Err(PolyError::invalid(format!(
                "builder code: ASCII form must be at most 32 chars, got {}",
                trimmed.len()
            )));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_graphic())
        {
            return Err(PolyError::invalid(
                "builder code: ASCII form must be printable, non-space characters",
            ));
        }
        let mut bytes = [0u8; 32];
        bytes[..trimmed.len()].copy_from_slice(trimmed.as_bytes());
        Ok(Self { bytes })
    }

    /// Explicitly NO builder attribution: orders carry the zero sentinel.
    /// Kept as a distinct constructor so "off" is a visible, intentional
    /// state in config — never an accident of parsing.
    pub fn disabled() -> Self {
        Self { bytes: ZERO_BUILDER }
    }

    /// Load from the environment: unset/blank → [`Self::disabled`];
    /// present-but-invalid → error (fail closed; a typo in a builder code
    /// must not silently drop attribution on revenue-bearing orders).
    pub fn from_env() -> PolyResult<Self> {
        match std::env::var(BUILDER_CODE_ENV) {
            Ok(raw) if !raw.trim().is_empty() => Self::parse(&raw),
            _ => Ok(Self::disabled()),
        }
    }

    /// True when attribution is OFF (orders carry the zero sentinel).
    pub fn is_disabled(&self) -> bool {
        self.bytes == ZERO_BUILDER
    }

    /// The exact 32 bytes to write into the V2 order struct's `builder`
    /// field.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Canonical wire form: `0x` + 64 lowercase hex.
    pub fn to_hex(&self) -> String {
        format!("0x{}", hex_encode(&self.bytes))
    }
}

/// Attribution policy applied to one order submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuilderAttribution {
    /// Attach this code to the order's `builder` field.
    Attach(BuilderCode),
    /// No attribution (zero sentinel). Distinct from "attach" so the
    /// venue layer can assert it never sends a garbage non-zero value.
    None,
}

impl BuilderAttribution {
    /// Build the policy from an optional code: `None`/disabled → no
    /// attribution.
    pub fn from_code(code: Option<BuilderCode>) -> Self {
        match code {
            Some(c) if !c.is_disabled() => Self::Attach(c),
            _ => Self::None,
        }
    }

    /// The bytes32 value for the order struct — zeros when unattributed.
    pub fn field_bytes(&self) -> [u8; 32] {
        match self {
            Self::Attach(code) => *code.as_bytes(),
            Self::None => ZERO_BUILDER,
        }
    }

    /// True when orders will carry a builder code.
    pub fn is_attributed(&self) -> bool {
        matches!(self, Self::Attach(_))
    }
}

/// Minimal hex encoder (no dependency for one call).
fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_hex_code() {
        let hex = "0x1111111111111111111111111111111111111111111111111111111111111111";
        let code = BuilderCode::parse(hex).unwrap();
        assert_eq!(code.as_bytes(), &[0x11; 32]);
        assert_eq!(code.to_hex(), hex);
        assert!(!code.is_disabled());
    }

    #[test]
    fn parses_ascii_code_with_nul_padding() {
        let code = BuilderCode::parse("sniper").unwrap();
        let mut expected = [0u8; 32];
        expected[..6].copy_from_slice(b"sniper");
        assert_eq!(code.as_bytes(), &expected);
    }

    #[test]
    fn rejects_malformed_inputs() {
        assert!(BuilderCode::parse("").is_err(), "empty");
        assert!(BuilderCode::parse("0x1234").is_err(), "short hex");
        assert!(BuilderCode::parse("0xzz111111111111111111111111111111111111111111111111111111111111").is_err(), "non-hex");
        assert!(BuilderCode::parse("0x0000000000000000000000000000000000000000000000000000000000000000").is_err(), "zero sentinel as code");
        let too_long = "a".repeat(33);
        assert!(BuilderCode::parse(&too_long).is_err(), "33-char ascii");
        assert!(BuilderCode::parse("has space").is_err(), "space in ascii form");
    }

    #[test]
    fn disabled_is_the_zero_sentinel() {
        let code = BuilderCode::disabled();
        assert!(code.is_disabled());
        assert_eq!(code.as_bytes(), &ZERO_BUILDER);
        assert_eq!(
            code.to_hex(),
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn attribution_policy_never_emits_garbage() {
        assert_eq!(BuilderAttribution::from_code(None).field_bytes(), ZERO_BUILDER);
        assert_eq!(
            BuilderAttribution::from_code(Some(BuilderCode::disabled())).field_bytes(),
            ZERO_BUILDER
        );
        let code = BuilderCode::parse("myapp").unwrap();
        let policy = BuilderAttribution::from_code(Some(code.clone()));
        assert!(policy.is_attributed());
        assert_eq!(policy.field_bytes(), *code.as_bytes());
        assert!(!BuilderAttribution::from_code(None).is_attributed());
    }

    #[test]
    fn env_loading_is_optional_and_validated() {
        // Unset → disabled. (Serial because tests share the process env.)
        std::env::remove_var(BUILDER_CODE_ENV);
        assert!(BuilderCode::from_env().unwrap().is_disabled());
        std::env::set_var(BUILDER_CODE_ENV, "arena");
        let code = BuilderCode::from_env().unwrap();
        assert_eq!(code.as_bytes()[..5], *b"arena");
        std::env::set_var(BUILDER_CODE_ENV, "0x1234");
        assert!(BuilderCode::from_env().is_err(), "typo must fail closed");
        std::env::remove_var(BUILDER_CODE_ENV);
    }
}
