//! Custody sign response — the only shape a custody-boundary sign attempt
//! may return (§F, Batch 8).
//!
//! Two honest outcomes exist:
//!
//! * `Signed` — a provider really produced an Ed25519 signature over the
//!   requested digest. This can only happen through a live, configured
//!   provider handle; there is no code path that fabricates one.
//! * `Refused` — the boundary said no, with a machine-readable
//!   [`RefusalReason`] and a secret-free human detail.
//!
//! There is deliberately no "maybe" and no `ok: bool`. Callers match on the
//! enum; a refusal is a first-class outcome, not an error string that could
//! be mistaken for a bug.

use bot_core::custody::{CustodyDenyReason, ProviderType, ResolveDenyReason};
use serde::Serialize;

/// Why a sign attempt was refused.
///
/// Every variant maps to a real guard that ran — none of them is a
/// catch-all "error" placeholder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefusalReason {
    /// The request itself was malformed (digest, purpose, module,
    /// capability) — nothing was attempted.
    InvalidRequest { code: String },
    /// The custody policy denied the attempt (tenant state, ownership,
    /// capability, module entitlement). Carries the policy engine's own
    /// dynamic detail (e.g. WHICH capability is missing).
    Policy {
        deny: CustodyDenyReason,
        detail: String,
    },
    /// Resolution of the signer through its provider was denied
    /// (ownership mismatch, provider mismatch, capability, health gate).
    Resolve { deny: String },
    /// The provider backing this signer is not implemented / not configured
    /// in this deployment. Carries the EXACT dependency the operator must
    /// provide. This is the honest OPTION-B posture: refuse and say what is
    /// missing instead of pretending to sign.
    ProviderUnsupported {
        provider: ProviderType,
        dependency: String,
    },
    /// A configured, implemented provider failed at runtime (transport,
    /// timeout, upstream refusal). Signature was NOT produced.
    ProviderFailure {
        provider: ProviderType,
        detail: String,
    },
}

impl RefusalReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefusalReason::InvalidRequest { .. } => "invalid_request",
            RefusalReason::Policy { .. } => "policy_denied",
            RefusalReason::Resolve { .. } => "resolve_denied",
            RefusalReason::ProviderUnsupported { .. } => "provider_unsupported",
            RefusalReason::ProviderFailure { .. } => "provider_failure",
        }
    }

    /// Machine-readable code suitable for audit rows and API error bodies.
    pub fn code(&self) -> String {
        match self {
            RefusalReason::InvalidRequest { code } => format!("invalid_request.{code}"),
            RefusalReason::Policy { deny, .. } => format!("policy.{}", deny.as_str()),
            RefusalReason::Resolve { deny } => format!("resolve.{}", deny),
            RefusalReason::ProviderUnsupported { provider, .. } => {
                format!("provider_unsupported.{}", provider.as_str())
            }
            RefusalReason::ProviderFailure { provider, .. } => {
                format!("provider_failure.{}", provider.as_str())
            }
        }
    }

    /// Secret-free human detail.
    pub fn detail(&self) -> String {
        match self {
            RefusalReason::InvalidRequest { code } => format!("malformed sign request: {code}"),
            RefusalReason::Policy { deny, detail } => {
                if detail.is_empty() {
                    deny.detail().to_string()
                } else {
                    detail.clone()
                }
            }
            RefusalReason::Resolve { deny } => deny.clone(),
            RefusalReason::ProviderUnsupported { provider, dependency } => format!(
                "custody provider {} is not implemented in this deployment; required dependency: {}",
                provider.as_str(),
                dependency
            ),
            RefusalReason::ProviderFailure { provider, detail } => format!(
                "custody provider {} failed: {}",
                provider.as_str(),
                detail
            ),
        }
    }

    /// Build the refusal from a core resolution denial.
    pub fn from_resolve(deny: &ResolveDenyReason) -> Self {
        RefusalReason::Resolve {
            deny: deny.to_string(),
        }
    }
}

/// The result of one custody sign attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodySignResponse {
    /// The provider produced this signature over the requested digest.
    Signed {
        /// Ed25519 signature bytes (64).
        signature: Vec<u8>,
        /// Hex-encoded signature.
        signature_hex: String,
        /// Provider that produced it.
        provider: ProviderType,
    },
    /// The boundary refused; nothing was signed.
    Refused { reason: RefusalReason },
}

impl CustodySignResponse {
    pub fn signed(signature: Vec<u8>, provider: ProviderType) -> Self {
        let signature_hex = hex::encode(&signature);
        CustodySignResponse::Signed {
            signature,
            signature_hex,
            provider,
        }
    }

    pub fn refused(reason: RefusalReason) -> Self {
        CustodySignResponse::Refused { reason }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, CustodySignResponse::Signed { .. })
    }

    pub fn refusal(&self) -> Option<&RefusalReason> {
        match self {
            CustodySignResponse::Signed { .. } => None,
            CustodySignResponse::Refused { reason } => Some(reason),
        }
    }

    /// The provider that produced (or was asked to produce) the signature.
    pub fn provider(&self) -> Option<ProviderType> {
        match self {
            CustodySignResponse::Signed { provider, .. } => Some(*provider),
            CustodySignResponse::Refused { reason } => match reason {
                RefusalReason::ProviderUnsupported { provider, .. }
                | RefusalReason::ProviderFailure { provider, .. } => Some(*provider),
                _ => None,
            },
        }
    }

    /// Safe for logs, API bodies, and audit rows — contains no key
    /// material and no provider credentials.
    pub fn to_safe_json(&self) -> serde_json::Value {
        match self {
            CustodySignResponse::Signed {
                signature_hex,
                provider,
                ..
            } => serde_json::json!({
                "outcome": "signed",
                "provider": provider.as_str(),
                "signature": signature_hex,
            }),
            CustodySignResponse::Refused { reason } => serde_json::json!({
                "outcome": "refused",
                "code": reason.code(),
                "detail": reason.detail(),
            }),
        }
    }
}

impl std::fmt::Display for CustodySignResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CustodySignResponse::Signed { provider, .. } => {
                write!(f, "signed by {}", provider.as_str())
            }
            CustodySignResponse::Refused { reason } => {
                write!(f, "refused: {} ({})", reason.code(), reason.detail())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_response_carries_hex_and_provider() {
        let resp = CustodySignResponse::signed(vec![0xab; 64], ProviderType::Vault);
        assert!(resp.is_success());
        assert_eq!(resp.provider(), Some(ProviderType::Vault));
        let json = resp.to_safe_json();
        assert_eq!(json["outcome"], "signed");
        assert_eq!(json["provider"], "vault");
        assert_eq!(json["signature"], hex::encode([0xabu8; 64]));
        assert!(resp.to_string().contains("vault"));
    }

    #[test]
    fn refusal_is_first_class() {
        let resp = CustodySignResponse::refused(RefusalReason::Policy {
            deny: CustodyDenyReason::TenantSuspended,
            detail: CustodyDenyReason::TenantSuspended.detail().to_string(),
        });
        assert!(!resp.is_success());
        let reason = resp.refusal().unwrap();
        assert_eq!(reason.as_str(), "policy_denied");
        assert_eq!(reason.code(), "policy.tenant_suspended");
        assert!(reason.detail().contains("suspended"));
        let json = resp.to_safe_json();
        assert_eq!(json["outcome"], "refused");
        assert_eq!(json["code"], "policy.tenant_suspended");
    }

    #[test]
    fn provider_unsupported_names_the_exact_dependency() {
        let reason = RefusalReason::ProviderUnsupported {
            provider: ProviderType::Hsm,
            dependency:
                "PKCS#11 module + HSM_SLOT + HSM_PIN reference (vault/hsm integration batch)"
                    .to_string(),
        };
        let resp = CustodySignResponse::refused(reason);
        assert_eq!(resp.provider(), Some(ProviderType::Hsm));
        let json = resp.to_safe_json();
        assert_eq!(json["code"], "provider_unsupported.hsm");
        assert!(json["detail"].as_str().unwrap().contains("HSM_SLOT"));
        assert!(resp.to_string().contains("provider_unsupported.hsm"));
    }

    #[test]
    fn resolve_denials_map_to_refusals() {
        let deny = ResolveDenyReason::SignerTenantMismatch;
        let reason = RefusalReason::from_resolve(&deny);
        assert_eq!(
            reason.code(),
            "resolve.signer belongs to another organization"
        );
        assert!(matches!(reason, RefusalReason::Resolve { .. }));
    }

    #[test]
    fn provider_failure_reports_detail_without_secret() {
        let reason = RefusalReason::ProviderFailure {
            provider: ProviderType::Kms,
            detail: "connection refused to kms endpoint".to_string(),
        };
        assert_eq!(reason.as_str(), "provider_failure");
        assert_eq!(reason.code(), "provider_failure.kms");
        assert!(reason.detail().contains("connection refused"));
    }

    #[test]
    fn invalid_request_refusal_has_code() {
        let reason = RefusalReason::InvalidRequest {
            code: "digest_wrong_length".to_string(),
        };
        assert_eq!(reason.code(), "invalid_request.digest_wrong_length");
        assert!(reason.detail().contains("malformed"));
    }
}
