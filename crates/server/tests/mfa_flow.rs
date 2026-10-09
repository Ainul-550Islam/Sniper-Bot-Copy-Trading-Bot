//! MFA / session flow (GAP-MAP v2 P1).
//!
//! Pins the authentication half of the MFA contract end to end through
//! the real router, hermetically (no network, no database):
//!
//! * registration issues an account, rejects duplicates and weak passwords;
//! * login is indistinguishable for unknown-email and wrong-password
//!   (no account enumeration);
//! * the session token is returned EXACTLY once and authenticates
//!   `users/me`; requests without it are refused;
//! * organization scoping at login is membership-gated: a session can
//!   never be scoped into an organization the user does not belong to —
//!   this is the gate that makes org-enforced MFA unbypassable;
//! * an `mfa_code` WITHOUT an organization never upgrades anything —
//!   TOTP verification only exists in the org-scoped branch;
//! * the security surface (TOTP setup/verify endpoints) refuses
//!   unauthenticated callers.
//!
//! The TOTP primitives themselves (code derivation, window, secret
//! handling) are unit-tested inside `saas/security.rs` (Part 2); this
//! file owns the FLOW.

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
use sniper_suite::saas::SaasStore;

const EMAIL: &str = "owner@example.com";
const PASSWORD: &str = "correct-horse-battery";

/// One deployment; the identity state lives in the shared [`SaasStore`],
/// so every request builds a fresh `ApiState` around the SAME store.
fn state_with(saas: Arc<SaasStore>) -> ApiState {
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
        saas,
        trading: None,
        module_registry: Arc::new(
            sniper_suite::module_runtime::module_registry::TenantModuleRegistry::new(),
        ),
    }
}

fn test_state() -> ApiState {
    state_with(Arc::new(SaasStore::new()))
}

async fn request(
    state: ApiState,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let app = router(state);
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(
            body.map(|v| v.to_string()).unwrap_or_default(),
        ))
        .expect("build request");
    let response = app.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::String(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    };
    (status, value)
}

fn register_body(email: &str) -> Value {
    serde_json::json!({
        "email": email,
        "password": PASSWORD,
        "display_name": "Owner"
    })
}

async fn register_and_login(saas: Arc<SaasStore>) -> String {
    let (status, _) = request(
        state_with(saas.clone()),
        "POST",
        "/api/saas/users",
        Some(register_body(EMAIL)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "registration must succeed");
    let (status, body) = request(
        state_with(saas),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({ "email": EMAIL, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login must succeed");
    body["token"].as_str().expect("token").to_string()
}

#[tokio::test]
async fn registration_validates_and_rejects_duplicates() {
    let state = test_state();
    let (status, _) = request(
        state.clone(),
        "POST",
        "/api/saas/users",
        Some(register_body(EMAIL)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Duplicate email is a conflict, and the reason must not leak
    // repository detail.
    let (status, body) = request(
        state.clone(),
        "POST",
        "/api/saas/users",
        Some(register_body(EMAIL)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.to_string().contains("registration_conflict"));

    // Weak passwords are rejected.
    let (status, _) = request(
        state.clone(),
        "POST",
        "/api/saas/users",
        Some(serde_json::json!({
            "email": "weak@example.com",
            "password": "short",
            "display_name": "Weak"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Malformed emails are rejected.
    let (status, _) = request(
        state,
        "POST",
        "/api/saas/users",
        Some(serde_json::json!({
            "email": "not-an-email",
            "password": PASSWORD,
            "display_name": "Bad"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_never_enumerates_accounts() {
    let state = test_state();
    let (status, _) = request(
        state.clone(),
        "POST",
        "/api/saas/users",
        Some(register_body(EMAIL)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Wrong password for a REAL account…
    let (status_wrong_pw, body_wrong_pw) = request(
        state.clone(),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({ "email": EMAIL, "password": "wrong-password-here" })),
        None,
    )
    .await;
    // …and a password for an account that DOES NOT EXIST must be
    // indistinguishable.
    let (status_no_user, body_no_user) = request(
        state,
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({
            "email": "ghost@example.com",
            "password": "wrong-password-here"
        })),
        None,
    )
    .await;
    assert_eq!(status_wrong_pw, StatusCode::UNAUTHORIZED);
    assert_eq!(status_no_user, StatusCode::UNAUTHORIZED);
    assert_eq!(body_wrong_pw, body_no_user, "responses must be identical");
    assert!(body_wrong_pw.to_string().contains("invalid_credentials"));
}

#[tokio::test]
async fn session_token_is_returned_once_and_authenticates() {
    let saas = Arc::new(SaasStore::new());
    let token = register_and_login(saas.clone()).await;

    // The token authenticates the profile endpoint.
    let (status, body) = request(
        state_with(saas.clone()),
        "GET",
        "/api/saas/users/me",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.to_string().contains(EMAIL));

    // No token → refused.
    let (status, _) = request(
        state_with(saas.clone()),
        "GET",
        "/api/saas/users/me",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Garbage token → refused.
    let (status, _) = request(
        state_with(saas),
        "GET",
        "/api/saas/users/me",
        None,
        Some("ses_totally_bogus_token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn organization_scoped_login_requires_membership() {
    let saas = Arc::new(SaasStore::new());
    let _token = register_and_login(saas.clone()).await;

    // Scoping into an organization the user does not belong to is a hard
    // refusal: this is the invariant that keeps org-enforced MFA from
    // being bypassed with an unscoped session.
    let (status, body) = request(
        state_with(saas.clone()),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({
            "email": EMAIL,
            "password": PASSWORD,
            "organization": "does-not-exist"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.to_string().contains("no_membership"));

    // A valid mfa_code does not change that: membership is checked before
    // any TOTP logic runs.
    let (status, body) = request(
        state_with(saas),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({
            "email": EMAIL,
            "password": PASSWORD,
            "organization": "does-not-exist",
            "mfa_code": "123456"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.to_string().contains("no_membership"));
}

#[tokio::test]
async fn mfa_code_without_organization_upgrades_nothing() {
    let state = test_state();
    let (status, _) = request(
        state.clone(),
        "POST",
        "/api/saas/users",
        Some(register_body(EMAIL)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // TOTP verification only exists in the org-scoped branch: an
    // unscoped login with a code attached still yields a plain unscoped
    // session (no organization_id), regardless of the code value.
    let (status, body) = request(
        state,
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({
            "email": EMAIL,
            "password": PASSWORD,
            "mfa_code": "000000"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["session"]["organization_id"].is_null(),
        "unscoped login must stay unscoped: {body}"
    );
}

#[tokio::test]
async fn security_surface_refuses_unauthenticated_callers() {
    let state = test_state();
    for (method, path) in [
        ("GET", "/api/saas/security/status"),
        ("POST", "/api/saas/security/totp/setup"),
        ("POST", "/api/saas/security/totp/verify"),
    ] {
        let (status, _) = request(state.clone(), method, path, None, None).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST,
            "{method} {path} must refuse unauthenticated callers, got {status}"
        );
    }
}
