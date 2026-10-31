//! Authenticated session inventory and per-user revocation contract.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::AppConfig;
use bot_core::db::Database;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::SaasStore;

const PASSWORD: &str = "correct-horse-battery";

fn state_with(saas: Arc<SaasStore>) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        trusted_proxies: Default::default(),
        db: None::<Arc<Database>>,
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

async fn request(
    saas: Arc<SaasStore>,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let app = router(state_with(saas));
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(
            body.map(|value| value.to_string()).unwrap_or_default(),
        ))
        .expect("build request");
    let response = app.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("response body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON response")
    };
    (status, body)
}

async fn register_user(saas: Arc<SaasStore>, email: &str) {
    let (status, _) = request(
        saas,
        "POST",
        "/api/saas/users",
        Some(serde_json::json!({
            "email": email,
            "password": PASSWORD,
            "display_name": "Session Test User"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

async fn login(saas: Arc<SaasStore>, email: &str, organization: Option<&str>) -> (String, String) {
    let mut body = serde_json::json!({ "email": email, "password": PASSWORD });
    if let Some(organization) = organization {
        body["organization"] = Value::String(organization.to_string());
    }
    let (status, response) = request(saas, "POST", "/api/saas/sessions", Some(body), None).await;
    assert_eq!(status, StatusCode::OK, "login response: {response}");
    (
        response["token"]
            .as_str()
            .expect("session token")
            .to_string(),
        response["session"]["id"]
            .as_str()
            .expect("session id")
            .to_string(),
    )
}

async fn create_scoped_user(saas: Arc<SaasStore>, email: &str) -> (String, String, String) {
    register_user(saas.clone(), email).await;
    let (unscoped_token, _) = login(saas.clone(), email, None).await;
    let name = format!("Session Management {}", Uuid::new_v4());
    let (status, organization) = request(
        saas.clone(),
        "POST",
        "/api/saas/organizations",
        Some(serde_json::json!({ "name": name })),
        Some(&unscoped_token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "organization response: {organization}"
    );
    let slug = organization["organization"]["slug"]
        .as_str()
        .expect("organization slug")
        .to_string();
    let (token, session_id) = login(saas, email, Some(&slug)).await;
    (slug, token, session_id)
}

#[tokio::test]
async fn a_user_can_list_and_revoke_only_their_own_sessions_without_exposing_secrets() {
    let saas = Arc::new(SaasStore::new_in_memory_identity());
    let (org_a, token_a, session_a) =
        create_scoped_user(saas.clone(), "session-a@example.test").await;
    let (token_a2, session_a2) = login(saas.clone(), "session-a@example.test", Some(&org_a)).await;
    let (_org_b, _token_b, session_b) =
        create_scoped_user(saas.clone(), "session-b@example.test").await;

    let (status, body) = request(
        saas.clone(),
        "GET",
        "/api/saas/users/me/sessions",
        None,
        Some(&token_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "session list: {body}");
    // The original unscoped session used to create the organization remains
    // in the account inventory alongside the two tenant-scoped sessions.
    assert_eq!(body["count"], 3);
    let items = body["items"].as_array().expect("session items");
    assert!(items.iter().any(|item| {
        item["id"] == session_a && item["current"] == true && item["state"] == "active"
    }));
    assert!(items.iter().any(|item| {
        item["id"] == session_a2 && item["current"] == false && item["state"] == "active"
    }));
    let serialized = body.to_string();
    assert!(!serialized.contains("token_hash"));
    assert!(!serialized.contains("revoke_reason"));

    // Another account's session identifier is indistinguishable from a missing one.
    let (status, body) = request(
        saas.clone(),
        "DELETE",
        &format!("/api/saas/users/me/sessions/{session_b}"),
        None,
        Some(&token_a),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "cross-user revoke: {body}");

    let (status, body) = request(
        saas.clone(),
        "DELETE",
        &format!("/api/saas/users/me/sessions/{session_a2}"),
        None,
        Some(&token_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revoke response: {body}");
    assert_eq!(body["revoked"], true);
    assert_eq!(body["session_id"], session_a2);

    let (status, body) = request(
        saas.clone(),
        "GET",
        "/api/saas/users/me/sessions",
        None,
        Some(&token_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "refreshed session list: {body}");
    let revoked = body["items"]
        .as_array()
        .expect("session items")
        .iter()
        .find(|item| item["id"] == session_a2)
        .expect("revoked session remains visible");
    assert_eq!(revoked["state"], "revoked");
    assert!(revoked["revoked_at"].is_string());

    let (status, _) = request(
        saas,
        "GET",
        "/api/saas/users/me/sessions",
        None,
        Some(&token_a2),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
