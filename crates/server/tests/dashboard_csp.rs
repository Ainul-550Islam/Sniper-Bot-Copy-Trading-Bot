//! End-to-end route and CSP regression test for the public operations dashboard.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::AppConfig;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;
use sniper_suite::api::{router, ApiState};
use sniper_suite::dashboard::{DASHBOARD_HTML, DASHBOARD_JS};
use sniper_suite::saas::SaasStore;

fn app() -> axum::Router {
    let shared = AppState::new(AppConfig::from_defaults());
    let state = ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        trusted_proxies: Default::default(),
        db: None,
        journal: None,
        serve_dashboard: true,
        health: Arc::new(HealthRegistry::new()),
        metrics_enabled: false,
        saas: Arc::new(SaasStore::new()),
        trading: None,
        module_registry: Arc::new(
            sniper_suite::module_runtime::module_registry::TenantModuleRegistry::new(),
        ),
    };
    router(state)
}

async fn get(app: axum::Router, path: &str) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("GET request"),
    )
    .await
    .expect("router response")
}

#[tokio::test]
async fn public_dashboard_html_and_javascript_share_a_strict_csp() {
    let html = get(app(), "/dashboard").await;
    assert_eq!(html.status(), StatusCode::OK);
    let html_headers = html.headers().clone();
    let html_body = html
        .into_body()
        .collect()
        .await
        .expect("collect HTML")
        .to_bytes();
    let html_body = String::from_utf8_lossy(&html_body);
    assert_eq!(html_body.as_ref(), DASHBOARD_HTML);
    assert!(html_body.contains("<script src=\"/dashboard/app.js\"></script>"));
    assert!(
        !html_body.contains("<script>"),
        "dashboard HTML contains no inline script"
    );

    let script = get(app(), "/dashboard/app.js").await;
    assert_eq!(script.status(), StatusCode::OK);
    let script_headers = script.headers().clone();
    let content_type = script_headers
        .get(header::CONTENT_TYPE)
        .expect("JavaScript content type")
        .to_str()
        .expect("valid content type");
    assert!(content_type.starts_with("application/javascript"));
    let script_body = script
        .into_body()
        .collect()
        .await
        .expect("collect JavaScript")
        .to_bytes();
    assert_eq!(script_body.as_ref(), DASHBOARD_JS.as_bytes());

    for headers in [&html_headers, &script_headers] {
        let csp = headers
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("CSP on dashboard asset")
            .to_str()
            .expect("valid CSP");
        let script_policy = csp
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("script-src "))
            .expect("script-src directive");
        assert_eq!(script_policy, "script-src 'self'");
        assert!(!script_policy.contains("unsafe-inline"));
        assert!(
            !headers.contains_key(header::CACHE_CONTROL),
            "dashboard cache policy is unchanged"
        );
    }
}
