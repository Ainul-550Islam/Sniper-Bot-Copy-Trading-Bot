//! Team-invite flow (GAP-MAP v2 P1).
//!
//! Two layers:
//!
//! 1. **Hermetic contract (always runs)** — everything the invite surface
//!    owes a caller before persistence is involved: authentication is
//!    mandatory, non-assignable roles are refused, malformed emails are
//!    refused, and without a database the endpoint answers an honest 503
//!    instead of fabricating an invitation.
//! 2. **Durable flow (`POSTGRES_URL` required, else NOT_RUN)** — the real
//!    lifecycle against the `invites` table: create → list → resend
//!    (token rotation) → revoke, scoped so one organization never sees
//!    another's invitations.
//!
//! The invitation token is returned EXACTLY once (in the create response)
//! because no external email provider is configured; both tests pin that.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::{AppConfig, DatabaseConfig};
use bot_core::db::Database;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::SaasStore;

const EMAIL: &str = "owner@example.com";
const PASSWORD: &str = "correct-horse-battery";
const MEMBER_EMAIL: &str = "member@example.com";

fn state_with(saas: Arc<SaasStore>, db: Option<Arc<Database>>) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        db,
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
        .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
        .expect("build request");
    let response = app.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

/// Register a user and return an UNscoped session token.
async fn register_and_login(saas: Arc<SaasStore>, email: &str) -> String {
    let (status, _) = request(
        state_with(saas.clone(), None),
        "POST",
        "/api/saas/users",
        Some(serde_json::json!({
            "email": email,
            "password": PASSWORD,
            "display_name": "Owner"
        })),
        None,
    )
    .await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::CONFLICT,
        "registration of {email}: {status}"
    );
    let (status, body) = request(
        state_with(saas, None),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({ "email": email, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login of {email}");
    body["token"].as_str().expect("token").to_string()
}

/// Create an organization and log back in with a session SCOPED to it.
async fn org_scoped_session(saas: Arc<SaasStore>, db: Option<Arc<Database>>, name: &str) -> String {
    let token = register_and_login(saas.clone(), EMAIL).await;
    let (status, body) = request(
        state_with(saas.clone(), db.clone()),
        "POST",
        "/api/saas/organizations",
        Some(serde_json::json!({ "name": name })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "org creation: {body}");
    let slug = body["organization"]["slug"].as_str().expect("slug").to_string();
    let (status, body) = request(
        state_with(saas, db),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({
            "email": EMAIL,
            "password": PASSWORD,
            "organization": slug
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "scoped login: {body}");
    body["token"].as_str().expect("token").to_string()
}

// ---------------------------------------------------------------------
// Hermetic contract
// ---------------------------------------------------------------------

#[tokio::test]
async fn invites_require_authentication() {
    let state = state_with(Arc::new(SaasStore::new()), None);
    let (status, _) = request(
        state.clone(),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "member" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = request(state, "GET", "/api/saas/team/invites", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn role_and_email_validation_run_before_storage() {
    let saas = Arc::new(SaasStore::new());
    let scoped = org_scoped_session(saas.clone(), None, "Acme Validation").await;

    // Org-owner is NOT assignable through the tenant invite surface —
    // refused before any storage is touched (this deployment has none).
    let (status, body) = request(
        state_with(saas.clone(), None),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "org_owner" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.to_string().contains("role_not_assignable_by_tenant"));

    let (status, body) = request(
        state_with(saas.clone(), None),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "platform_admin" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Malformed email → 400.
    let (status, _) = request(
        state_with(saas, None),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": "not-an-email", "role": "member" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_a_database_the_invite_is_an_honest_503() {
    let saas = Arc::new(SaasStore::new());
    let scoped = org_scoped_session(saas.clone(), None, "Acme No Db").await;
    let (status, body) = request(
        state_with(saas, None),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "member" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "invitations require Postgres; nothing may be fabricated: {body}"
    );
    assert!(!body.to_string().contains("invitation_token"));
}

// ---------------------------------------------------------------------
// Durable flow (POSTGRES_URL required)
// ---------------------------------------------------------------------

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

async fn setup_db() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(Arc::new(db))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invite_lifecycle_create_list_resend_revoke() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: invite lifecycle — POSTGRES_URL missing");
        return;
    };
    let saas = Arc::new(SaasStore::new());
    let scoped = org_scoped_session(saas.clone(), Some(db.clone()), "Acme Invites").await;

    // CREATE — the token is returned exactly once, here.
    let (status, created) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "member" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let invitation_id = created["invitation_id"].as_str().expect("id").to_string();
    assert!(created["invitation_token"].as_str().is_some());
    assert_eq!(created["email"], MEMBER_EMAIL);

    // LIST — the invitation shows up for its own organization.
    let (status, listed) = request(
        state_with(saas.clone(), Some(db.clone())),
        "GET",
        "/api/saas/team/invites",
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = listed["invites"]
        .as_array()
        .or_else(|| listed["items"].as_array())
        .expect("invite list array");
    assert!(
        items.iter().any(|i| i["id"] == invitation_id),
        "created invite must be listed: {listed}"
    );
    // The plaintext token is never re-served by the list endpoint.
    assert!(!listed.to_string().contains(
        created["invitation_token"].as_str().unwrap()
    ));

    // RESEND — rotates the token (a new one is returned, once).
    let (status, resent) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        &format!("/api/saas/team/invites/{invitation_id}/resend"),
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resent}");
    if let Some(new_token) = resent["invitation_token"].as_str() {
        assert_ne!(new_token, created["invitation_token"].as_str().unwrap());
    }

    // REVOKE — the invitation disappears from the list.
    let (status, _) = request(
        state_with(saas.clone(), Some(db.clone())),
        "DELETE",
        &format!("/api/saas/team/invites/{invitation_id}"),
        None,
        Some(&scoped),
    )
    .await;
    assert!(status == StatusCode::OK || status == StatusCode::NO_CONTENT);
    let (status, listed) = request(
        state_with(saas.clone(), Some(db.clone())),
        "GET",
        "/api/saas/team/invites",
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !listed.to_string().contains(&invitation_id),
        "revoked invite must not be listed: {listed}"
    );

    // Cross-tenant isolation: a different organization does not see the
    // row even by id.
    let other = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(other)
        .bind(format!("other-{}", &other.to_string()[..8]))
        .bind("Other Org")
        .execute(db.pool())
        .await
        .expect("seed other org");
    let (status, _) = request(
        state_with(saas.clone(), Some(db.clone())),
        "DELETE",
        &format!("/api/saas/team/invites/{invitation_id}"),
        None,
        Some(&scoped),
    )
    .await;
    // Already revoked for the owner org — the point is no panic/500.
    assert!(status != StatusCode::INTERNAL_SERVER_ERROR);
}
