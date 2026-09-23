//! Security response headers for every HTTP response (TASK 7B file 16).
//!
//! One middleware, mounted in the api router's `route_layer` chain, sets:
//!
//! | Header | Value | Why |
//! |---|---|---|
//! | `Content-Security-Policy` | `default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'` | no external origins, no wildcard hosts; `'unsafe-inline'` exists ONLY because the embedded dashboard (TASK 4) ships exactly one inline `<script>`, so the dashboard keeps working unchanged |
//! | `X-Content-Type-Options` | `nosniff` | no MIME confusion on API JSON |
//! | `Referrer-Policy` | `strict-origin-when-cross-origin` | never leak paths (and any accidental query strings) cross-origin |
//! | `X-Frame-Options` | `DENY` | belt-and-braces alongside `frame-ancestors 'none'` |
//! | `Permissions-Policy` | `camera=(), microphone=(), geolocation=()` | the UI needs no device capabilities |
//! | `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` | ONLY when the request arrived over TLS (directly or via `x-forwarded-proto: https`) — plain-HTTP local deployments stay clean |
//! | `Cache-Control` (`/api/*` only) | `no-store` | API responses (profiles, keys, exports) are never cached |
//!
//! The middleware never rewrites bodies and never blocks the WebSocket
//! upgrade: it only appends headers.

use axum::http::{header, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

/// The Content-Security-Policy applied to every response.
pub const CSP: &str = "default-src 'self'; \
script-src 'self' 'unsafe-inline'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
connect-src 'self'; \
font-src 'self'; \
object-src 'none'; \
base-uri 'self'; \
form-action 'self'; \
frame-ancestors 'none'";

/// Did this request arrive over TLS? Either the connection itself or the
/// proxy's `x-forwarded-proto: https` counts.
fn request_is_https(req: &Request<axum::body::Body>) -> bool {
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

/// The middleware. Composes into the existing `route_layer` chain after
/// request-context and rate-limiting, before `with_state`.
pub async fn apply_security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let https = request_is_https(&req);
    let is_api = req.uri().path().starts_with("/api");
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        axum::http::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/api/saas/exports", get(|| async { "json" }))
            .route(
                "/dashboard",
                get(|| async { "<html><script>1</script></html>" }),
            )
            .layer(middleware::from_fn(apply_security_headers))
    }

    #[tokio::test]
    async fn every_response_carries_the_full_header_set() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/api/saas/exports")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let h = res.headers();
        assert_eq!(h.get(header::CONTENT_SECURITY_POLICY).unwrap(), CSP);
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(
            h.get(header::REFERRER_POLICY).unwrap(),
            "strict-origin-when-cross-origin"
        );
        assert_eq!(h.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(
            h.get("permissions-policy").unwrap(),
            "camera=(), microphone=(), geolocation=()"
        );
        // API responses are never cacheable…
        assert_eq!(h.get(header::CACHE_CONTROL).unwrap(), "no-store");
        // …and plain HTTP requests get NO HSTS header.
        assert!(h.get(header::STRICT_TRANSPORT_SECURITY).is_none());
    }

    #[tokio::test]
    async fn tls_arrivals_get_hsts_and_dashboard_pages_are_not_no_store() {
        // Behind a TLS-terminating proxy.
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/api/saas/exports")
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .unwrap(),
            "max-age=31536000; includeSubDomains"
        );

        // The dashboard is NOT under /api, so only the security headers apply.
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let h = res.headers();
        assert!(h.get(header::CACHE_CONTROL).is_none());
        assert!(h.get(header::CONTENT_SECURITY_POLICY).is_some());
        // The one inline dashboard script must keep running: 'unsafe-inline'
        // is present in script-src by documented necessity.
        let csp = h
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self' 'unsafe-inline'"));
        assert!(!csp.contains('*'), "no wildcard sources");
    }

    #[tokio::test]
    async fn bodies_pass_through_untouched() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = http_body_util::BodyExt::collect(res.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&bytes[..], b"<html><script>1</script></html>");
    }
}
