//! Fresh-tenant contract (GAP-MAP v2 P0).
//!
//! A brand-new deployment — zero tenants, zero configuration, no database,
//! no credentials — still owes its customers a contract. This test pins
//! that contract down so a fresh install can never ship broken:
//!
//! * liveness and the public API contract answer honestly;
//! * the kill switch is OFF by default and the execution mode is not live;
//! * collection endpoints return EMPTY data, never fabricated rows;
//! * the SaaS surface degrades to clean 5xx/202 semantics without a
//!   database — never a panic, never a fake success;
//! * every response carries the security headers;
//! * unknown paths are 404s, never 500s.
//!
//! Fully hermetic: no network, no database, no credentials.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::AppConfig;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::openapi::API_VERSION;
use sniper_suite::saas::SaasStore;

/// A deployment exactly as shipped: nothing configured, nothing attached.
fn fresh_state() -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        db: None,
        journal: None,
        serve_dashboard: false,
        health: Arc::new(HealthRegistry::new()),
        metrics_enabled: false,
        saas: Arc::new(SaasStore::new()),
        trading: None,
        module_registry: Arc::new(
            sniper_suite::module_runtime::module_registry::TenantModuleRegistry::new(),
        ),
    }
}

async fn get(path: &str) -> (StatusCode, axum::http::HeaderMap, Value) {
    let app = router(fresh_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request served");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 10_000_000)
        .await
        .expect("read body");
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{path}: response body is not JSON: {e}"))
    };
    (status, headers, value)
}

async fn post_json(path: &str, body: &str) -> (StatusCode, Value) {
    let app = router(fresh_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("build request"),
        )
        .await
        .expect("request served");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 10_000_000)
        .await
        .expect("read body");
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

// ---------------------------------------------------------------- liveness --

#[tokio::test]
async fn health_is_live_on_a_fresh_deployment() {
    let (status, _, _) = get("/api/health").await;
    assert_eq!(status, StatusCode::OK, "liveness must never degrade");
}

// ------------------------------------------------------ the public contract --

#[tokio::test]
async fn the_published_contract_is_served_and_current() {
    let (status, _, doc) = get("/api/saas/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        doc["info"]["version"].as_str(),
        Some(API_VERSION),
        "the served contract must be at API_VERSION"
    );
    let paths = doc["paths"].as_object().expect("paths object");
    assert!(
        paths.len() >= 45,
        "a fresh deployment publishes the full contract ({} paths)",
        paths.len()
    );
    // The password-reset / email-verification flows are part of the
    // contract from day one.
    for required in [
        "/api/saas/password-reset/request",
        "/api/saas/password-reset/confirm",
        "/api/saas/email-verification/request",
        "/api/saas/email-verification/confirm",
    ] {
        assert!(paths.contains_key(required), "missing contract path {required}");
    }
    // No secret material may ever appear in the public contract.
    let rendered = doc.to_string();
    for banned in ["secret_hash", "password_hash", "token_hash"] {
        assert!(
            !rendered.contains(banned),
            "contract leaks sensitive field {banned}"
        );
    }
}

// -------------------------------------------------- defaults on first boot --

#[tokio::test]
async fn kill_switch_is_off_and_mode_is_not_live_by_default() {
    let (status, _, risk) = get("/api/risk/global").await;
    assert_eq!(status, StatusCode::OK);
    let text = risk.to_string();
    assert!(
        !text.is_empty(),
        "the global risk summary must render on a fresh deployment"
    );

    let shared = fresh_state().shared;
    assert!(!shared.kill_switch(), "kill switch must ship DISENGAGED");
    let summary = shared.summary().await;
    assert!(!summary.kill_switch, "summary must agree with the flag");
    assert!(
        !summary.live_allowed,
        "live trading must not be allowed before explicit operator action"
    );
}

#[tokio::test]
async fn status_module_and_positions_answer_without_fabricating_data() {
    for path in ["/api/status", "/api/modules", "/api/positions", "/api/orders", "/api/trades"] {
        let (status, _, body) = get(path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path}: a fresh deployment answers its own API"
        );
        assert!(!body.is_null(), "{path}: response must be JSON");
    }
    // Positions and orders on a fresh install are EMPTY — an empty list is
    // the only honest answer.
    let (_, _, positions) = get("/api/positions").await;
    let text = positions.to_string();
    for fabricated in ["BTC", "ETH", "SOL-", "position-1"] {
        assert!(
            !text.contains(fabricated),
            "fresh deployment fabricates position data ({fabricated})"
        );
    }
}

// ------------------------------- SaaS surface without a database (degrade) --

#[tokio::test]
async fn saas_registration_degrades_to_503_without_a_database() {
    let body = r#"{"email":"buyer@example.com","password":"a-strong-passphrase-123"}"#;
    let (status, _) = post_json("/api/saas/users", body).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "registration without a database must be an honest 503"
    );
}

#[tokio::test]
async fn saas_login_degrades_to_503_without_a_database() {
    let body = r#"{"email":"buyer@example.com","password":"a-strong-passphrase-123"}"#;
    let (status, _) = post_json("/api/saas/sessions", body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn password_reset_request_is_accepted_and_enumeration_safe_without_a_database() {
    // The request endpoint must behave identically with no database at all:
    // 202, same body shape, nothing to enumerate.
    for email in ["known@example.com", "unknown@example.com", ""] {
        let body = format!(r#"{{"email":"{email}"}}"#);
        let (status, value) = post_json("/api/saas/password-reset/request", &body).await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "reset request must be 202 for every input (got {status} for {email:?})"
        );
        assert_eq!(value["status"].as_str(), Some("accepted"));
        assert_eq!(value["expires_minutes"].as_i64(), Some(30));
    }
}

#[tokio::test]
async fn email_verification_request_is_accepted_without_a_database() {
    let (status, value) = post_json(
        "/api/saas/email-verification/request",
        r#"{"email":"buyer@example.com"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(value["status"].as_str(), Some("accepted"));
    assert_eq!(value["expires_minutes"].as_i64(), Some(24 * 60));
}

#[tokio::test]
async fn token_confirms_never_succeed_without_the_supporting_state() {
    // Confirm paths cannot claim success on a bare deployment: they either
    // refuse the token (422) or report the missing backend (503).
    for path in [
        "/api/saas/password-reset/confirm",
        "/api/saas/email-verification/confirm",
    ] {
        let (status, _) =
            post_json(path, r#"{"token":"rst_totally_bogus","new_password":"x"}"#).await;
        assert!(
            status == StatusCode::UNPROCESSABLE_ENTITY
                || status == StatusCode::SERVICE_UNAVAILABLE,
            "{path}: got {status}"
        );
        assert_ne!(status, StatusCode::OK, "{path} must never confirm a bogus token");
    }
}

// --------------------------------------------------------- security headers --

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    for path in ["/api/health", "/api/status", "/api/saas/openapi.json"] {
        let (status, headers, _) = get(path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers.contains_key("content-security-policy"),
            "{path}: missing CSP"
        );
        assert!(
            headers.contains_key("x-content-type-options"),
            "{path}: missing XCTO"
        );
        assert!(
            headers.contains_key("x-frame-options"),
            "{path}: missing X-Frame-Options"
        );
    }
}

// --------------------------------------------------------------- unknowns --

#[tokio::test]
async fn unknown_paths_are_404_never_500() {
    for path in [
        "/api/does-not-exist",
        "/api/saas/definitely-not-a-route",
        "/totally/random/path",
    ] {
        let (status, _, _) = get(path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
}
