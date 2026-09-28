//! Typed errors for the SaaS SDK (BATCH 2 complete).
//!
//! Maps HTTP status + API error code into typed SDK errors. Distinguishes:
//! auth, permission, validation, conflict, rate-limit, server, network,
//! provider-unavailable, lifecycle-closed. Never exposes server secrets.

use std::fmt;

/// Error kind — machine-readable, secret-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkErrorKind {
    /// 400 — invalid request / validation
    InvalidRequest,
    /// 401 — auth missing/invalid
    Unauthorized,
    /// 403 — permission denied (role/entitlement/capability/lifecycle)
    Forbidden,
    /// Also 403 permission alias
    PermissionDenied,
    /// 404 — not found (also for cross-tenant)
    NotFound,
    /// 409 — conflict (idempotency, already exists, lifecycle phase)
    Conflict,
    /// 422 — validation
    Validation,
    /// 429 — rate limited
    RateLimited,
    /// 5xx transport/network
    Transport,
    /// Network / connection
    Network,
    /// Decode / parse error
    Decode,
    /// 500 server
    Server,
    /// Provider unavailable (Vault/KMS/HSM/billing provider)
    ProviderUnavailable,
    /// Lifecycle closed — tenant cannot operate
    LifecycleClosed,
    /// Unknown
    Unknown,
}

impl SdkErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SdkErrorKind::InvalidRequest => "invalid_request",
            SdkErrorKind::Unauthorized => "unauthorized",
            SdkErrorKind::Forbidden => "forbidden",
            SdkErrorKind::PermissionDenied => "permission_denied",
            SdkErrorKind::NotFound => "not_found",
            SdkErrorKind::Conflict => "conflict",
            SdkErrorKind::Validation => "validation",
            SdkErrorKind::RateLimited => "rate_limited",
            SdkErrorKind::Transport => "transport",
            SdkErrorKind::Network => "network",
            SdkErrorKind::Decode => "decode",
            SdkErrorKind::Server => "server",
            SdkErrorKind::ProviderUnavailable => "provider_unavailable",
            SdkErrorKind::LifecycleClosed => "lifecycle_closed",
            SdkErrorKind::Unknown => "unknown",
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            SdkErrorKind::Transport
                | SdkErrorKind::Network
                | SdkErrorKind::Server
                | SdkErrorKind::RateLimited
                | SdkErrorKind::ProviderUnavailable
        )
    }

    pub fn is_auth(&self) -> bool {
        matches!(self, SdkErrorKind::Unauthorized)
    }

    /// Map HTTP status to kind (fallback to Unknown).
    pub fn from_status(status: u16, body_code: Option<&str>) -> Self {
        if let Some(code) = body_code {
            let lower = code.to_ascii_lowercase();
            if lower.contains("auth") || lower.contains("unauthorized") {
                return SdkErrorKind::Unauthorized;
            }
            if lower.contains("permission") || lower.contains("forbidden") {
                return SdkErrorKind::PermissionDenied;
            }
            if lower.contains("validation") || lower.contains("invalid") {
                return SdkErrorKind::Validation;
            }
            if lower.contains("conflict") || lower.contains("already_exists") {
                return SdkErrorKind::Conflict;
            }
            if lower.contains("rate") {
                return SdkErrorKind::RateLimited;
            }
            if lower.contains("provider") && lower.contains("unavailable") {
                return SdkErrorKind::ProviderUnavailable;
            }
            if lower.contains("closed") || lower.contains("lifecycle") {
                return SdkErrorKind::LifecycleClosed;
            }
        }
        match status {
            400 => SdkErrorKind::InvalidRequest,
            401 => SdkErrorKind::Unauthorized,
            403 => SdkErrorKind::PermissionDenied,
            404 => SdkErrorKind::NotFound,
            409 => SdkErrorKind::Conflict,
            422 => SdkErrorKind::Validation,
            429 => SdkErrorKind::RateLimited,
            500..=599 => SdkErrorKind::Server,
            _ => SdkErrorKind::Unknown,
        }
    }
}

/// SDK error — never contains secret material.
#[derive(Debug, Clone)]
pub struct SdkError {
    pub kind: SdkErrorKind,
    pub message: String,
    pub status: Option<u16>,
    pub code: Option<String>,
}

impl SdkError {
    pub fn new(kind: SdkErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
            code: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    pub fn is_auth(&self) -> bool {
        self.kind.is_auth()
    }

    /// Redacted display — ensures no secret leaks even if message contained one (it shouldn't).
    fn redacted_message(&self) -> String {
        let lower = self.message.to_ascii_lowercase();
        if lower.contains("secret") || lower.contains("private") || lower.contains("token") {
            "**redacted**".into()
        } else {
            self.message.clone()
        }
    }
}

impl fmt::Display for SdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = self.redacted_message();
        if let Some(status) = self.status {
            write!(f, "{} ({}): {}", self.kind.as_str(), status, msg)
        } else {
            write!(f, "{}: {}", self.kind.as_str(), msg)
        }
    }
}

impl std::error::Error for SdkError {}

/// Helper to map `reqwest` / transport errors into SdkError.
pub fn from_reqwest_error(err: reqwest::Error) -> SdkError {
    if err.is_timeout() || err.is_connect() {
        SdkError::new(SdkErrorKind::Network, err.to_string())
    } else if err.is_status() {
        if let Some(status) = err.status() {
            SdkError::new(
                SdkErrorKind::from_status(status.as_u16(), None),
                err.to_string(),
            )
            .with_status(status.as_u16())
        } else {
            SdkError::new(SdkErrorKind::Transport, err.to_string())
        }
    } else {
        SdkError::new(SdkErrorKind::Transport, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_secret_free_and_retry_classified() {
        let e = SdkError::new(SdkErrorKind::Transport, "timeout");
        assert!(e.is_retryable());
        let e2 = SdkError::new(SdkErrorKind::InvalidRequest, "bad plan");
        assert!(!e2.is_retryable());
        assert!(!e.to_string().contains("secret"));
    }

    #[test]
    fn status_maps_to_typed_kind() {
        assert_eq!(
            SdkErrorKind::from_status(401, None),
            SdkErrorKind::Unauthorized
        );
        assert_eq!(
            SdkErrorKind::from_status(403, None),
            SdkErrorKind::PermissionDenied
        );
        assert_eq!(SdkErrorKind::from_status(409, None), SdkErrorKind::Conflict);
        assert_eq!(
            SdkErrorKind::from_status(429, None),
            SdkErrorKind::RateLimited
        );
        assert_eq!(SdkErrorKind::from_status(500, None), SdkErrorKind::Server);
    }

    #[test]
    fn provider_unavailable_is_retryable() {
        let e = SdkError::new(SdkErrorKind::ProviderUnavailable, "vault unavailable");
        assert!(e.is_retryable());
    }

    #[test]
    fn lifecycle_closed_not_retryable() {
        let e = SdkError::new(SdkErrorKind::LifecycleClosed, "tenant closed");
        assert!(!e.is_retryable());
    }

    #[test]
    fn body_code_overrides_status() {
        let k = SdkErrorKind::from_status(403, Some("lifecycle_closed"));
        assert_eq!(k, SdkErrorKind::LifecycleClosed);
        let k2 = SdkErrorKind::from_status(503, Some("provider_unavailable"));
        assert_eq!(k2, SdkErrorKind::ProviderUnavailable);
    }

    #[test]
    fn secret_redaction() {
        let e = SdkError::new(SdkErrorKind::Server, "secret token leaked");
        assert!(e.to_string().contains("**redacted**"));
        assert!(!e.to_string().contains("secret"));
    }
}
