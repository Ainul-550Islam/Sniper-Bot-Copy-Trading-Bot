//! Custody sign request — the server-side value object that crosses the
//! custody boundary (§F of the enterprise program, Batch 8).
//!
//! A `CustodySignRequest` carries everything the boundary needs to decide
//! whether a digest may be signed, and nothing it must not:
//!
//! * organization + custody profile + signer identifiers (all UUID-backed,
//!   ownership-checked again at the boundary — a request is a claim, not
//!   proof);
//! * the 32-byte message digest as lowercase hex (already a hash — the
//!   pre-image never has to enter the custody layer for Solana message
//!   signing);
//! * the module and capability the caller believes it is exercising, so the
//!   policy gate can deny a module that is not entitled to the signer;
//! * an explicit provider expectation so a Vault-bound signer can never be
//!   silently satisfied by a local wallet (or the reverse).
//!
//! What a `CustodySignRequest` can NEVER carry:
//!
//! * private keys or key material of any kind — the boundary resolves keys
//!   on the provider side only;
//! * secrets, tokens, or credential references — provider configuration is
//!   resolved from the operator environment, never from tenant input;
//! * a "make it succeed" flag — the boundary has no override.

use bot_core::custody::{CustodyProfileId, ProviderType, SignerId};
use bot_core::tenant::OrganizationId;

/// Why a sign request is malformed before any policy is consulted.
///
/// Machine-readable, secret-free, and safe to return to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidSignRequest {
    /// Digest must be exactly 64 lowercase-or-uppercase hex chars (32 bytes).
    DigestMalformed { len: usize },
    /// A Solana message digest is 32 bytes; other lengths are rejected.
    DigestWrongLength { len: usize },
    /// The purpose string is empty or whitespace.
    PurposeEmpty,
    /// The purpose string is only used for the audit trail; keep it bounded.
    PurposeTooLong { len: usize },
    /// The module identifier is empty.
    ModuleEmpty,
    /// The capability identifier is empty.
    CapabilityEmpty,
}

impl InvalidSignRequest {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvalidSignRequest::DigestMalformed { .. } => "digest_malformed",
            InvalidSignRequest::DigestWrongLength { .. } => "digest_wrong_length",
            InvalidSignRequest::PurposeEmpty => "purpose_empty",
            InvalidSignRequest::PurposeTooLong { .. } => "purpose_too_long",
            InvalidSignRequest::ModuleEmpty => "module_empty",
            InvalidSignRequest::CapabilityEmpty => "capability_empty",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            InvalidSignRequest::DigestMalformed { len } => {
                format!("digest must be hex, got {len} non-hex-or-wrong-length chars")
            }
            InvalidSignRequest::DigestWrongLength { len } => {
                format!("digest must encode 32 bytes (64 hex chars), got {len} chars")
            }
            InvalidSignRequest::PurposeEmpty => "purpose must not be empty".to_string(),
            InvalidSignRequest::PurposeTooLong { len } => {
                format!("purpose must be at most 128 chars, got {len}")
            }
            InvalidSignRequest::ModuleEmpty => "module must not be empty".to_string(),
            InvalidSignRequest::CapabilityEmpty => "capability must not be empty".to_string(),
        }
    }
}

impl std::fmt::Display for InvalidSignRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for InvalidSignRequest {}

/// A validated request to sign one 32-byte digest through the custody boundary.
///
/// Build via [`CustodySignRequest::new`] (which validates) — the struct is
/// not constructible with an unvalidated digest through the public API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodySignRequest {
    organization_id: OrganizationId,
    custody_profile_id: CustodyProfileId,
    signer_id: SignerId,
    /// 32-byte digest, lowercase hex (64 chars).
    digest_hex: String,
    /// Module the caller is exercising (e.g. "module-sniper").
    module: String,
    /// Capability the signer must hold (e.g. "solana:sign").
    capability: String,
    /// Provider the caller expects to satisfy this request, if pinned.
    expected_provider: Option<ProviderType>,
    /// Free-form, bounded audit purpose (e.g. "order-signing").
    purpose: String,
}

impl CustodySignRequest {
    /// Validate and build a sign request.
    ///
    /// Fails closed on malformed input: the boundary never sees an
    /// unvalidated digest.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        organization_id: OrganizationId,
        custody_profile_id: CustodyProfileId,
        signer_id: SignerId,
        digest_hex: impl Into<String>,
        module: impl Into<String>,
        capability: impl Into<String>,
        expected_provider: Option<ProviderType>,
        purpose: impl Into<String>,
    ) -> Result<Self, InvalidSignRequest> {
        let digest_hex = digest_hex.into();
        let digest_hex = digest_hex.trim().to_ascii_lowercase();
        let len = digest_hex.len();
        if len != 64 {
            return Err(InvalidSignRequest::DigestWrongLength { len });
        }
        if !digest_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(InvalidSignRequest::DigestMalformed { len });
        }
        let module = module.into();
        if module.trim().is_empty() {
            return Err(InvalidSignRequest::ModuleEmpty);
        }
        let capability = capability.into();
        if capability.trim().is_empty() {
            return Err(InvalidSignRequest::CapabilityEmpty);
        }
        let purpose = purpose.into();
        let purpose_trimmed = purpose.trim();
        if purpose_trimmed.is_empty() {
            return Err(InvalidSignRequest::PurposeEmpty);
        }
        if purpose_trimmed.len() > 128 {
            return Err(InvalidSignRequest::PurposeTooLong {
                len: purpose_trimmed.len(),
            });
        }
        Ok(Self {
            organization_id,
            custody_profile_id,
            signer_id,
            digest_hex,
            module: module.trim().to_string(),
            capability: capability.trim().to_string(),
            expected_provider,
            purpose: purpose_trimmed.to_string(),
        })
    }

    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub fn custody_profile_id(&self) -> &CustodyProfileId {
        &self.custody_profile_id
    }

    pub fn signer_id(&self) -> &SignerId {
        &self.signer_id
    }

    /// Lowercase 64-char hex encoding of the 32-byte digest.
    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }

    /// Decoded 32-byte digest.
    pub fn digest_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, chunk) in self.digest_hex.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16).unwrap_or(0) as u8;
            let lo = (chunk[1] as char).to_digit(16).unwrap_or(0) as u8;
            out[i] = (hi << 4) | lo;
        }
        out
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn capability(&self) -> &str {
        &self.capability
    }

    pub fn expected_provider(&self) -> Option<ProviderType> {
        self.expected_provider
    }

    pub fn purpose(&self) -> &str {
        &self.purpose
    }

    /// Secret-free description for logs and audit records.
    pub fn safe_summary(&self) -> String {
        format!(
            "org={} profile={} signer={} provider={} module={} capability={} purpose={} digest={}",
            self.organization_id.as_uuid(),
            self.custody_profile_id.as_uuid(),
            self.signer_id.as_uuid(),
            self.expected_provider
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "any".to_string()),
            self.module,
            self.capability,
            self.purpose,
            // The digest is a hash of the payload, not the payload; showing
            // it in audit summaries is safe and required for traceability.
            self.digest_hex
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org() -> OrganizationId {
        OrganizationId::new()
    }

    fn profile() -> CustodyProfileId {
        CustodyProfileId::new()
    }

    fn signer() -> SignerId {
        SignerId::new()
    }

    fn digest() -> String {
        "aa".repeat(32)
    }

    #[test]
    fn builds_a_validated_request() {
        let req = CustodySignRequest::new(
            org(),
            profile(),
            signer(),
            format!("{}{}", "AA".repeat(31), "bb"),
            "module-sniper",
            "solana:sign",
            Some(ProviderType::Vault),
            "  order-signing  ",
        )
        .expect("valid request");
        // Hex is normalised to lowercase.
        assert_eq!(req.digest_hex(), &format!("{}{}", "aa".repeat(31), "bb"));
        // Purpose is trimmed but preserved.
        assert_eq!(req.purpose(), "order-signing");
        assert_eq!(req.expected_provider(), Some(ProviderType::Vault));
        let bytes = req.digest_bytes();
        assert_eq!(bytes[0], 0xaa);
        assert_eq!(bytes[31], 0xbb);
        assert!(req.safe_summary().contains("digest="));
    }

    #[test]
    fn rejects_wrong_length_digests() {
        let err = CustodySignRequest::new(
            org(),
            profile(),
            signer(),
            "abcd",
            "module-sniper",
            "solana:sign",
            None,
            "order-signing",
        )
        .unwrap_err();
        assert_eq!(err, InvalidSignRequest::DigestWrongLength { len: 4 });
        assert_eq!(err.as_str(), "digest_wrong_length");
    }

    #[test]
    fn rejects_non_hex_digests() {
        let bad = format!("{}zz", "aa".repeat(31));
        let err = CustodySignRequest::new(
            org(),
            profile(),
            signer(),
            bad,
            "module-sniper",
            "solana:sign",
            None,
            "order-signing",
        )
        .unwrap_err();
        // Length is right (64) but content is not hex.
        assert_eq!(err, InvalidSignRequest::DigestMalformed { len: 64 });
    }

    #[test]
    fn rejects_empty_and_oversized_purposes() {
        for (purpose, expected) in [
            ("   ", InvalidSignRequest::PurposeEmpty),
            (
                &"x".repeat(129),
                InvalidSignRequest::PurposeTooLong { len: 129 },
            ),
        ] {
            let err = CustodySignRequest::new(
                org(),
                profile(),
                signer(),
                digest(),
                "module-sniper",
                "solana:sign",
                None,
                purpose,
            )
            .unwrap_err();
            assert_eq!(err, expected);
        }
    }

    #[test]
    fn rejects_empty_module_and_capability() {
        let err = CustodySignRequest::new(
            org(),
            profile(),
            signer(),
            digest(),
            "  ",
            "solana:sign",
            None,
            "order-signing",
        )
        .unwrap_err();
        assert_eq!(err, InvalidSignRequest::ModuleEmpty);

        let err = CustodySignRequest::new(
            org(),
            profile(),
            signer(),
            digest(),
            "module-sniper",
            "",
            None,
            "order-signing",
        )
        .unwrap_err();
        assert_eq!(err, InvalidSignRequest::CapabilityEmpty);
    }

    #[test]
    fn purpose_too_long_detail_is_human_readable() {
        let err = InvalidSignRequest::PurposeTooLong { len: 129 };
        assert!(err.detail().contains("129"));
        assert!(err.detail().contains("128"));
    }
}
