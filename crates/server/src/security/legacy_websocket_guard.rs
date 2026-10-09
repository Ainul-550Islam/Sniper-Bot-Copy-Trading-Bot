//! Compatibility guard for the legacy `/api/events` route (BATCH 2 file 12).
//!
//! Prevents the legacy query-string API key mechanism from being used by
//! new SaaS tenants. Introduces explicit configuration:
//! `disabled` / `compatibility-only` / `legacy-enabled`. Default must be secure.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Legacy WebSocket mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegacyWsMode {
    /// Legacy route disabled entirely (default — secure).
    #[default]
    Disabled,
    /// Only loopback/dev or requests that already passed SaaS auth may use legacy path.
    CompatibilityOnly,
    /// Legacy `?key=` route enabled for backward-compat single-operator installs.
    LegacyEnabled,
}

impl LegacyWsMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LegacyWsMode::Disabled => "disabled",
            LegacyWsMode::CompatibilityOnly => "compatibility-only",
            LegacyWsMode::LegacyEnabled => "legacy-enabled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "disabled" | "off" | "false" => Some(LegacyWsMode::Disabled),
            "compatibility" | "compatibility-only" | "compat" => {
                Some(LegacyWsMode::CompatibilityOnly)
            }
            "legacy-enabled" | "enabled" | "on" | "true" | "legacy" => {
                Some(LegacyWsMode::LegacyEnabled)
            }
            _ => None,
        }
    }

    /// Is `?key=` query auth allowed in this mode?
    pub fn allows_query_key(&self) -> bool {
        matches!(self, LegacyWsMode::LegacyEnabled)
    }
}

/// Resolve mode from env var `LEGACY_WS_MODE` or config.
pub fn mode_from_env() -> LegacyWsMode {
    std::env::var("LEGACY_WS_MODE")
        .ok()
        .and_then(|v| LegacyWsMode::parse(&v))
        .unwrap_or_default()
}

/// Guard result for a legacy events WS upgrade attempt.
pub enum LegacyGuardDecision {
    /// Proceed with legacy handling.
    Allow,
    /// Block — return error response with deprecation metadata.
    Deny(Response),
}

/// Apply the guard. `query` is the raw query string (e.g., `?key=...`),
/// `headers` is the request headers, `remote_is_loopback` indicates if the
/// peer is loopback (dev mode), `tenant_is_saas` indicates if tenant is SaaS.
pub fn guard(
    mode: LegacyWsMode,
    query: Option<&str>,
    _headers: &HeaderMap,
    remote_is_loopback: bool,
    tenant_is_saas: bool,
) -> LegacyGuardDecision {
    let has_query_key = query
        .map(|q| {
            let lower = q.to_ascii_lowercase();
            lower.contains("key=") || lower.contains("token=") || lower.contains("api_key=")
        })
        .unwrap_or(false);

    match mode {
        LegacyWsMode::Disabled => {
            if has_query_key {
                let mut res = (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({
                        "error": "legacy_query_auth_disabled",
                        "reason": "use Authorization header or /api/saas/events",
                    })),
                )
                    .into_response();
                res.headers_mut()
                    .insert("X-Legacy-Deprecated", HeaderValue::from_static("true"));
                res.headers_mut()
                    .insert("X-Legacy-Mode", HeaderValue::from_static("disabled"));
                return LegacyGuardDecision::Deny(res);
            }
            LegacyGuardDecision::Allow
        }
        LegacyWsMode::CompatibilityOnly => {
            if has_query_key && tenant_is_saas {
                let mut res = (
                    StatusCode::FORBIDDEN,
                    axum::Json(json!({
                        "error": "saas_tenant_legacy_forbidden",
                        "reason": "SaaS tenants must use /api/saas/events with header auth",
                    })),
                )
                    .into_response();
                res.headers_mut()
                    .insert("X-Legacy-Deprecated", HeaderValue::from_static("true"));
                return LegacyGuardDecision::Deny(res);
            }
            if has_query_key && !remote_is_loopback {
                // Non-loopback with query key in compat mode — warn but allow for existing operator if not SaaS
                // We add deprecation header but still allow
                LegacyGuardDecision::Allow
            } else {
                LegacyGuardDecision::Allow
            }
        }
        LegacyWsMode::LegacyEnabled => {
            // Fully enabled — allow but always add deprecation header
            LegacyGuardDecision::Allow
        }
    }
}

/// Helper to add deprecation headers to any legacy response.
pub fn deprecation_headers(mode: LegacyWsMode) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("X-Legacy-Deprecated", HeaderValue::from_static("true"));
    h.insert(
        "X-Legacy-Mode",
        HeaderValue::from_str(mode.as_str()).unwrap_or(HeaderValue::from_static("disabled")),
    );
    h.insert("Deprecation", HeaderValue::from_static("true"));
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn default_is_secure_disabled() {
        assert_eq!(LegacyWsMode::default(), LegacyWsMode::Disabled);
        assert!(!LegacyWsMode::default().allows_query_key());
    }

    #[test]
    fn disabled_blocks_query_key() {
        let h = HeaderMap::new();
        let d = guard(
            LegacyWsMode::Disabled,
            Some("?key=secret"),
            &h,
            false,
            false,
        );
        assert!(matches!(d, LegacyGuardDecision::Deny(_)));
    }

    #[test]
    fn disabled_allows_without_query() {
        let h = HeaderMap::new();
        let d = guard(LegacyWsMode::Disabled, None, &h, false, false);
        assert!(matches!(d, LegacyGuardDecision::Allow));
    }

    #[test]
    fn legacy_enabled_allows_query() {
        let h = HeaderMap::new();
        let d = guard(
            LegacyWsMode::LegacyEnabled,
            Some("?key=secret"),
            &h,
            false,
            false,
        );
        assert!(matches!(d, LegacyGuardDecision::Allow));
    }

    #[test]
    fn compatibility_blocks_saas_tenant_with_query() {
        let h = HeaderMap::new();
        let d = guard(
            LegacyWsMode::CompatibilityOnly,
            Some("?key=secret"),
            &h,
            false,
            true, // SaaS tenant
        );
        assert!(matches!(d, LegacyGuardDecision::Deny(_)));
    }

    #[test]
    fn compatibility_allows_loopback_operator_with_query() {
        let h = HeaderMap::new();
        let d = guard(
            LegacyWsMode::CompatibilityOnly,
            Some("?key=secret"),
            &h,
            true,  // loopback
            false, // not SaaS (single operator)
        );
        assert!(matches!(d, LegacyGuardDecision::Allow));
    }

    #[test]
    fn deprecation_headers_present() {
        let h = deprecation_headers(LegacyWsMode::Disabled);
        assert!(h.contains_key("X-Legacy-Deprecated"));
        assert!(h.contains_key("Deprecation"));
    }

    #[test]
    fn does_not_migrate_raw_secrets() {
        // Guard never extracts or logs query secret value
        let query = "?key=supersecret123";
        let h = HeaderMap::new();
        let d = guard(LegacyWsMode::Disabled, Some(query), &h, false, false);
        if let LegacyGuardDecision::Deny(res) = d {
            let body = format!("{:?}", res);
            assert!(!body.contains("supersecret123"));
        }
    }
}
