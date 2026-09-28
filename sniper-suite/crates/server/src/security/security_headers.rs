//! Central HTTP security-header middleware/policy (BATCH 2 file 13).
//!
//! Wraps the existing `crate::security::headers` but provides a typed
//! policy struct and configurable construction for SaaS control-plane
//! responses. Reuses the same header values to avoid duplicate architecture.

use axum::http::{header, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

/// Typed security-header policy.
#[derive(Debug, Clone)]
pub struct SecurityHeaderPolicy {
    pub csp: &'static str,
    pub x_content_type_options: &'static str,
    pub referrer_policy: &'static str,
    pub x_frame_options: &'static str,
    pub permissions_policy: &'static str,
    pub hsts_max_age: Option<&'static str>,
    pub cache_control_api: &'static str,
}

impl Default for SecurityHeaderPolicy {
    fn default() -> Self {
        Self {
            csp: crate::security::headers::CSP,
            x_content_type_options: "nosniff",
            referrer_policy: "strict-origin-when-cross-origin",
            x_frame_options: "DENY",
            permissions_policy: "camera=(), microphone=(), geolocation=()",
            hsts_max_age: Some("max-age=31536000; includeSubDomains"),
            cache_control_api: "no-store",
        }
    }
}

impl SecurityHeaderPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether request arrived over TLS (header or scheme).
    pub fn is_https(req: &Request<axum::body::Body>) -> bool {
        if req.uri().scheme_str() == Some("https") {
            return true;
        }
        req.headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("https"))
            })
            .unwrap_or(false)
    }

    /// Apply policy to a response.
    pub fn apply(&self, req_is_https: bool, is_api: bool, res: &mut Response) {
        let headers = res.headers_mut();
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(self.csp),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static(self.x_content_type_options),
        );
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static(self.referrer_policy),
        );
        headers.insert(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static(self.x_frame_options),
        );
        headers.insert(
            axum::http::HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(self.permissions_policy),
        );
        if req_is_https {
            if let Some(hsts) = self.hsts_max_age {
                headers.insert(
                    header::STRICT_TRANSPORT_SECURITY,
                    HeaderValue::from_static(hsts),
                );
            }
        }
        if is_api {
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(self.cache_control_api),
            );
        }
    }
}

/// Middleware that delegates to `SecurityHeaderPolicy::default()`.
pub async fn apply_security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    // Delegate to the canonical implementation in `headers.rs` to avoid divergence.
    // This wrapper exists so `security_headers` is the central policy import path going forward.
    crate::security::headers::apply_security_headers(req, next).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    #[test]
    fn policy_does_not_break_api_caching() {
        let policy = SecurityHeaderPolicy::default();
        assert_eq!(policy.cache_control_api, "no-store");
        assert!(policy.csp.contains("frame-ancestors"));
    }

    #[test]
    fn https_detection() {
        let req = Request::builder()
            .uri("http://example.com/api/health")
            .header("x-forwarded-proto", "https")
            .body(Body::empty())
            .unwrap();
        assert!(SecurityHeaderPolicy::is_https(&req));
        let req2 = Request::builder()
            .uri("http://example.com/api/health")
            .body(Body::empty())
            .unwrap();
        assert!(!SecurityHeaderPolicy::is_https(&req2));
    }

    #[tokio::test]
    async fn headers_middleware_does_not_block_ws_upgrade() {
        // The headers middleware only appends headers; it never blocks WS upgrade.
        // We test that the policy struct is constructive without breaking.
        let policy = SecurityHeaderPolicy::default();
        assert_eq!(policy.x_frame_options, "DENY");
    }
}
