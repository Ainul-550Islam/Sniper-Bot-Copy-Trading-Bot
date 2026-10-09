//! Kill-switch flow (GAP-MAP v2 P0).
//!
//! The kill switch is the single most important control this product sells,
//! so its entire state machine is pinned down here, end to end through the
//! real router:
//!
//! * ships DISENGAGED on a fresh deployment;
//! * `POST /api/kill` engages it (global flag + halt + module disable);
//! * `POST /api/resume` releases flag AND halt;
//! * releasing the flag alone does NOT clear a halt (asymmetry is
//!   deliberate and tested);
//! * engagements are idempotent and audited;
//! * with an API key configured, the switch is unreachable without the key;
//! * the scoped venue/strategy switches engage and release through
//!   `/api/risk/kill-switch` and reject unknown scopes.
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
use sniper_suite::saas::SaasStore;

fn test_state() -> ApiState {
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

/// Same deployment, but with a legacy owner API key configured — the
/// production-shaped gate.
fn keyed_state() -> ApiState {
    let mut state = test_state();
    state.api_key = Some("owner-secret-key".to_string());
    state
}

async fn post(state: ApiState, path: &str, body: Option<&str>, api_key: Option<&str>) -> (StatusCode, Value) {
    let app = router(state);
    let mut builder = Request::builder().method("POST").uri(path);
    if let Some(key) = api_key {
        builder = builder.header("x-api-key", key);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(body.unwrap_or_default().to_string()))
        .expect("build request");
    let response = app.oneshot(request).await.expect("request served");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 10_000_000)
        .await
        .expect("read body");
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn get(state: ApiState, path: &str, api_key: Option<&str>) -> (StatusCode, Value) {
    let app = router(state);
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(key) = api_key {
        builder = builder.header("x-api-key", key);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).expect("build request"))
        .await
        .expect("request served");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 10_000_000)
        .await
        .expect("read body");
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

// ------------------------------------------------------------- fresh state --

#[tokio::test]
async fn a_fresh_deployment_ships_with_the_kill_switch_disengaged() {
    let state = test_state();
    assert!(!state.shared.kill_switch());
    let summary = state.shared.summary().await;
    assert!(!summary.kill_switch);
}

// --------------------------------------------------- the global HTTP flow --

#[tokio::test]
async fn kill_then_resume_is_a_complete_round_trip() {
    let state = test_state();
    let shared = state.shared.clone();

    // Loopback dev mode: no key configured, so the deployment answers.
    let (status, body) = post(state.clone(), "/api/kill", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kill_switch"], Value::Bool(true));
    assert!(shared.kill_switch(), "the global flag must be engaged");

    let (status, body) = post(state.clone(), "/api/resume", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kill_switch"], Value::Bool(false));
    assert!(!shared.kill_switch(), "resume must fully release");
}

#[tokio::test]
async fn engagements_are_idempotent() {
    let state = test_state();
    let shared = state.shared.clone();

    for _ in 0..3 {
        let (status, _) = post(state.clone(), "/api/kill", None, None).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert!(shared.kill_switch());

    for _ in 0..3 {
        let (status, _) = post(state.clone(), "/api/resume", None, None).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert!(!shared.kill_switch());
}

#[tokio::test]
async fn emergency_stop_survives_a_plain_release_until_resume_clears_the_halt() {
    let state = test_state();
    let shared = state.shared.clone();

    // Emergency stop engages BOTH the flag and the halt.
    shared.emergency_stop("test: trip the wire").await;
    assert!(shared.kill_switch());

    // Releasing the flag alone is NOT enough: the halt keeps the switch
    // engaged. This asymmetry is deliberate — an emergency stop must not be
    // undone by a flag flip.
    shared.set_kill_switch(false, "test: release flag only").await;
    assert!(
        shared.kill_switch(),
        "the halt must outlive the flag release"
    );

    // The resume endpoint is the only way back: flag off AND halt cleared.
    let (status, _) = post(state.clone(), "/api/resume", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!shared.kill_switch(), "resume clears the halt as well");
}

// --------------------------------------------------- the credential gate --

#[tokio::test]
async fn with_a_key_configured_the_kill_switch_is_unreachable_without_it() {
    let state = keyed_state();
    let shared = state.shared.clone();

    // No key.
    let (status, _) = post(state.clone(), "/api/kill", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!shared.kill_switch(), "a denied request must not engage the switch");

    // Wrong key.
    let (status, _) = post(state.clone(), "/api/kill", None, Some("wrong-key")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!shared.kill_switch());

    // Right key.
    let (status, _) = post(state.clone(), "/api/kill", None, Some("owner-secret-key")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(shared.kill_switch());

    // Resume is gated the same way.
    let (status, _) = post(state.clone(), "/api/resume", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(shared.kill_switch());
    let (status, _) = post(state.clone(), "/api/resume", None, Some("owner-secret-key")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!shared.kill_switch());
}

// ------------------------------------------------------------ the audit --

#[tokio::test]
async fn kill_and_resume_are_recorded_in_the_audit_trail() {
    let state = test_state();
    let (status, _) = post(state.clone(), "/api/kill", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post(state.clone(), "/api/resume", None, None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, audit) = get(state, "/api/audit", None).await;
    assert_eq!(status, StatusCode::OK);
    let text = audit.to_string();
    assert!(
        text.contains("kill_switch"),
        "the audit trail must carry the kill engagement"
    );
    assert!(
        text.contains("resume"),
        "the audit trail must carry the release"
    );
}

// ------------------------------------------------- the scoped kill switch --

#[tokio::test]
async fn scoped_switches_engage_release_and_reject_unknown_scopes() {
    let state = test_state();

    // Engage a strategy scope.
    let (status, body) = post(
        state.clone(),
        "/api/risk/kill-switch",
        Some(r#"{"scope":"strategy:high-freq","engaged":true,"reason":"test"}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outcome"].as_str(), Some("changed"));

    // Re-engaging is accepted but unchanged (idempotent).
    let (status, body) = post(
        state.clone(),
        "/api/risk/kill-switch",
        Some(r#"{"scope":"strategy:high-freq","engaged":true}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outcome"].as_str(), Some("unchanged"));

    // Release it again.
    let (status, body) = post(
        state.clone(),
        "/api/risk/kill-switch",
        Some(r#"{"scope":"strategy:high-freq","engaged":false}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outcome"].as_str(), Some("changed"));

    // Unknown scopes are rejected up front.
    let (status, _) = post(
        state.clone(),
        "/api/risk/kill-switch",
        Some(r#"{"scope":"galaxy:everything","engaged":true}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Empty scope too.
    let (status, _) = post(
        state,
        "/api/risk/kill-switch",
        Some(r#"{"scope":"","engaged":true}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ----------------------------------------------------- visibility to ops --

#[tokio::test]
async fn the_risk_summary_reflects_the_switch_state() {
    let state = test_state();
    let (status, _) = post(state.clone(), "/api/kill", None, None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, risk) = get(state.clone(), "/api/risk/global", None).await;
    assert_eq!(status, StatusCode::OK);
    let text = risk.to_string();
    assert!(
        text.contains("kill"),
        "the global risk view must mention the kill switch state"
    );

    let (status, _) = post(state.clone(), "/api/resume", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get(state, "/api/risk/global", None).await;
    assert_eq!(status, StatusCode::OK);
}
