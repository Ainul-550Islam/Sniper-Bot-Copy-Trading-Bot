//! Production-safe CORS policy resolution (BATCH file 17).
//!
//! Default must NOT be unrestricted wildcard for authenticated SaaS APIs.
//! Support explicit configured origins. Reject invalid origin config at startup.

use std::fmt;

/// Resolved CORS policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsPolicy {
    /// Allowed origins — empty means no CORS; single "*" handled separately as wildcard but not default for SaaS.
    pub allowed_origins: Vec<String>,
    /// Whether wildcard is allowed (only when explicitly configured).
    pub allow_wildcard: bool,
    /// Whether credentials/headers are allowed (for authenticated APIs this is restricted).
    pub allow_credentials: bool,
}

impl CorsPolicy {
    /// Resolve from configured origins. Returns error if config is invalid and should fail startup.
    pub fn from_config(origins: &[String]) -> Result<Self, CorsConfigError> {
        // Check for wildcard: if any origin is "*", it must be the ONLY origin and explicitly allowed.
        let has_wildcard = origins.iter().any(|o| o.trim() == "*");
        if has_wildcard {
            if origins.len() != 1 {
                return Err(CorsConfigError::MixedWildcard(
                    "wildcard '*' cannot be combined with explicit origins".into(),
                ));
            }
            // Wildcard is technically valid when explicitly configured, but for authenticated SaaS it is NOT default.
            // We allow it only when explicitly set; the caller can decide to reject it for SaaS.
            return Ok(CorsPolicy {
                allowed_origins: vec!["*".into()],
                allow_wildcard: true,
                allow_credentials: false,
            });
        }

        // Empty list → no CORS (fail-closed for SaaS). Deployment-level may choose to allow all via explicit "*".
        if origins.is_empty() {
            return Ok(CorsPolicy {
                allowed_origins: Vec::new(),
                allow_wildcard: false,
                allow_credentials: false,
            });
        }

        // Validate each origin
        let mut validated = Vec::new();
        for raw in origins {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let origin = validate_origin(trimmed)?;
            validated.push(origin);
        }

        if validated.is_empty() {
            return Ok(CorsPolicy {
                allowed_origins: Vec::new(),
                allow_wildcard: false,
                allow_credentials: false,
            });
        }

        Ok(CorsPolicy {
            allowed_origins: validated,
            allow_wildcard: false,
            allow_credentials: true,
        })
    }

    /// Is wildcard allowed for SaaS authenticated APIs? Default is NO — must be explicit.
    pub fn is_wildcard_allowed_for_saas(&self) -> bool {
        self.allow_wildcard
    }

    /// Should we apply CORS for SaaS routes? Only if explicit origins configured.
    pub fn allows_any_origin(&self) -> bool {
        self.allow_wildcard || !self.allowed_origins.is_empty()
    }

    /// Check if a request origin is allowed.
    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        if self.allow_wildcard {
            return true;
        }
        self.allowed_origins.iter().any(|o| o == origin)
    }

    /// Build the Axum CorsLayer for this policy. For SaaS authenticated routes, wildcard is not used.
    #[allow(clippy::wrong_self_convention)]
    pub fn into_layer(&self) -> tower_http::cors::CorsLayer {
        use axum::http::HeaderValue;
        use tower_http::cors::{Any, CorsLayer};
        if self.allow_wildcard {
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        } else if self.allowed_origins.is_empty() {
            // No CORS — empty layer (no allow_origin)
            CorsLayer::new().allow_methods(Any).allow_headers(Any)
        } else {
            let parsed: Vec<HeaderValue> = self
                .allowed_origins
                .iter()
                .filter_map(|o| o.parse::<HeaderValue>().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(parsed)
                .allow_methods(Any)
                .allow_headers(Any)
        }
    }
}

fn validate_origin(origin: &str) -> Result<String, CorsConfigError> {
    if origin == "*" {
        return Ok("*".into());
    }
    // Must be valid URL with scheme https or http, no path/query/fragment
    let url = url::Url::parse(origin).map_err(|e| {
        CorsConfigError::InvalidOrigin(format!("invalid origin '{}': {}", origin, e))
    })?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(CorsConfigError::InvalidOrigin(format!(
                "origin must be http or https: {}",
                origin
            )))
        }
    }
    if url.username() != "" || url.password().is_some() {
        return Err(CorsConfigError::InvalidOrigin(format!(
            "origin must not contain credentials: {}",
            origin
        )));
    }
    if url.path() != "/" && url.path() != "" {
        // Some browsers send origin with no path, but config should be bare origin
        // We allow origin with path only if it's exactly "/" — otherwise warn
        if url.path() != "/" {
            return Err(CorsConfigError::InvalidOrigin(format!(
                "origin must not contain path: {}",
                origin
            )));
        }
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CorsConfigError::InvalidOrigin(format!(
            "origin must not contain query/fragment: {}",
            origin
        )));
    }
    // Rebuild canonical origin (scheme + host + port)
    let host = url.host_str().ok_or_else(|| {
        CorsConfigError::InvalidOrigin(format!("origin must have host: {}", origin))
    })?;
    let port_part = url.port().map(|p| format!(":{}", p)).unwrap_or_default();
    Ok(format!("{}://{}{}", url.scheme(), host, port_part))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsConfigError {
    InvalidOrigin(String),
    MixedWildcard(String),
}

impl fmt::Display for CorsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CorsConfigError::InvalidOrigin(m) => write!(f, "invalid CORS origin: {}", m),
            CorsConfigError::MixedWildcard(m) => write!(f, "CORS config error: {}", m),
        }
    }
}
impl std::error::Error for CorsConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_fail_closed_not_wildcard() {
        let policy = CorsPolicy::from_config(&[]).unwrap();
        assert!(
            !policy.allow_wildcard,
            "default must not be wildcard for SaaS"
        );
        assert!(!policy.is_wildcard_allowed_for_saas());
        assert!(!policy.allows_any_origin(), "empty means no CORS");
        assert!(!policy.is_origin_allowed("https://example.com"));
    }

    #[test]
    fn explicit_origin_is_allowed() {
        let policy = CorsPolicy::from_config(&[
            "https://app.example.com".into(),
            "https://admin.example.com".into(),
        ])
        .unwrap();
        assert!(!policy.allow_wildcard);
        assert!(policy.is_origin_allowed("https://app.example.com"));
        assert!(policy.is_origin_allowed("https://admin.example.com"));
        assert!(!policy.is_origin_allowed("https://evil.com"));
        assert!(policy.allows_any_origin());
    }

    #[test]
    fn wildcard_must_be_explicit_singleton() {
        let policy = CorsPolicy::from_config(&["*".into()]).unwrap();
        assert!(policy.allow_wildcard);
        assert!(policy.is_origin_allowed("https://anything.com"));
        // Mixed wildcard rejected at startup
        let err = CorsPolicy::from_config(&["*".into(), "https://example.com".into()]).unwrap_err();
        assert!(matches!(err, CorsConfigError::MixedWildcard(_)));
    }

    #[test]
    fn invalid_origin_rejected_at_startup() {
        let err = CorsPolicy::from_config(&["https://example.com/path".into()]).unwrap_err();
        assert!(matches!(err, CorsConfigError::InvalidOrigin(_)));
        let err2 = CorsPolicy::from_config(&["ftp://example.com".into()]).unwrap_err();
        assert!(matches!(err2, CorsConfigError::InvalidOrigin(_)));
        let err3 = CorsPolicy::from_config(&["https://user:pass@example.com".into()]).unwrap_err();
        assert!(matches!(err3, CorsConfigError::InvalidOrigin(_)));
    }

    #[test]
    fn whitespace_and_case_are_handled() {
        let policy = CorsPolicy::from_config(&["  https://example.com  ".into()]).unwrap();
        assert!(policy.is_origin_allowed("https://example.com"));
    }

    #[test]
    fn wildcard_is_not_default_for_authenticated_saas() {
        // This is the critical security property: without explicit config, SaaS APIs do NOT allow wildcard.
        let default_policy = CorsPolicy::from_config(&[]).unwrap();
        assert!(
            !default_policy.allow_wildcard,
            "wildcard must never be default"
        );
        // Explicit wildcard is allowed only when operator explicitly sets it — not by accident
        let explicit = CorsPolicy::from_config(&["*".into()]).unwrap();
        assert!(explicit.allow_wildcard);
    }
}
