//! Password reset — durable consistency regression (P1-PWRESET).
//!
//! Layers:
//! 1. **Hermetic (always runs)** — enumeration-safe `202` for an unknown
//!    address without a database, and an honest `503` (never a success
//!    shape) for confirm without a database.
//! 2. **Durable (`POSTGRES_URL` required, else NOT_RUN)** — the real HTTP
//!    flow against an authoritative `SaasStore::with_database` store:
//!    successful reset invalidates the intended sessions and changes the
//!    password that login checks; a reused token fails; concurrent use of one
//!    token yields exactly one success; a revocation failure inside the
//!    transaction answers `503`, leaves the password and sessions unchanged,
//!    and leaves the token usable (no half-applied reset).
//!
//! The reset link is read from the durable outbox (`email_outbox`), the same
//! channel a real user receives it through.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
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

const OLD_PASSWORD: &str = "correct-horse-battery";
const NEW_PASSWORD: &str = "new-horse-battery-staple";

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

// ---------------------------------------------------------------------
// Hermetic contract (always runs)
// ---------------------------------------------------------------------

#[tokio::test]
async fn request_without_a_database_is_enumeration_safe() {
    let state = state_with(Arc::new(SaasStore::new()), None);
    let (status, body) = request(
        state,
        "POST",
        "/api/saas/password-reset/request",
        Some(json!({ "email": "nobody@example.com" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "accepted");
}

#[tokio::test]
async fn confirm_without_a_database_is_never_a_success_shape() {
    let state = state_with(Arc::new(SaasStore::new()), None);
    let (status, body) = request(
        state,
        "POST",
        "/api/saas/password-reset/confirm",
        Some(json!({ "token": "rst_anything", "new_password": NEW_PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_ne!(body["status"], "reset_complete");
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

/// An authoritative store (PostgreSQL is the source of truth for reads).
async fn durable_store(db: &Arc<Database>) -> Arc<SaasStore> {
    Arc::new(
        SaasStore::with_database(Some(db.clone()))
            .await
            .expect("durable store"),
    )
}

async fn register(saas: Arc<SaasStore>, db: &Arc<Database>, email: &str) {
    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/users",
        Some(json!({ "email": email, "password": OLD_PASSWORD, "display_name": "Reset" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "registration: {body}");
}

async fn login(
    saas: Arc<SaasStore>,
    db: &Arc<Database>,
    email: &str,
    password: &str,
) -> (StatusCode, Value) {
    request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/sessions",
        Some(json!({ "email": email, "password": password })),
        None,
    )
    .await
}

async fn request_reset_for(saas: Arc<SaasStore>, db: &Arc<Database>, email: &str) {
    let (status, _) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/password-reset/request",
        Some(json!({ "email": email })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

/// The most recent reset token for `email`, read from the durable outbox.
async fn latest_reset_token(db: &Arc<Database>, email: &str) -> String {
    let row: (String,) = sqlx::query_as(
        "SELECT body_text FROM email_outbox
          WHERE recipient = $1 AND template_key = 'password_reset'
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(email)
    .fetch_one(db.pool())
    .await
    .expect("reset email in outbox");
    let after = row.0.split("token=").nth(1).expect("token in link");
    after
        .split(|c: char| c.is_whitespace())
        .next()
        .expect("token value")
        .to_string()
}

async fn confirm(
    db: &Arc<Database>,
    saas: Arc<SaasStore>,
    token: &str,
    password: &str,
) -> (StatusCode, Value) {
    request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/password-reset/confirm",
        Some(json!({ "token": token, "new_password": password })),
        None,
    )
    .await
}

async fn me(saas: Arc<SaasStore>, db: &Arc<Database>, session: &str) -> StatusCode {
    request(
        state_with(saas, Some(db.clone())),
        "GET",
        "/api/saas/users/me",
        None,
        Some(session),
    )
    .await
    .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_changes_password_revokes_sessions_and_token_is_single_use() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: password reset durable flow — POSTGRES_URL missing");
        return;
    };
    let email = format!("reset-{}@example.com", Uuid::new_v4());
    let saas = durable_store(&db).await;
    register(saas.clone(), &db, &email).await;

    let (status, body) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "first login: {body}");
    let session_a = body["token"].as_str().expect("token").to_string();
    let (_, body) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    let session_b = body["token"].as_str().expect("token").to_string();

    // Unknown address: identical acceptance, no reset email.
    request_reset_for(saas.clone(), &db, &format!("unknown-{}@example.com", Uuid::new_v4())).await;

    request_reset_for(saas.clone(), &db, &email).await;
    let token = latest_reset_token(&db, &email).await;

    // Wrong token: opaque denial, nothing changes.
    let (status, _) = confirm(&db, saas.clone(), "rst_not_a_real_token", NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "old password still valid after denial");

    let (status, body) = confirm(&db, saas.clone(), &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "reset: {body}");
    assert_eq!(body["status"], "reset_complete");

    // The password login checks really changed (authoritative record).
    let (status, _) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "old password must stop working");
    let (status, _) = login(saas.clone(), &db, &email, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "new password must work");

    // Every pre-existing session is revoked.
    assert_eq!(me(saas.clone(), &db, &session_a).await, StatusCode::UNAUTHORIZED);
    assert_eq!(me(saas.clone(), &db, &session_b).await, StatusCode::UNAUTHORIZED);

    // The token cannot be reused.
    let (status, _) = confirm(&db, saas.clone(), &token, "third-horse-battery-x").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "token reuse must fail");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_confirms_with_one_token_yield_exactly_one_success() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: concurrent reset confirm — POSTGRES_URL missing");
        return;
    };
    let email = format!("race-{}@example.com", Uuid::new_v4());
    let saas = durable_store(&db).await;
    register(saas.clone(), &db, &email).await;
    request_reset_for(saas.clone(), &db, &email).await;
    let token = latest_reset_token(&db, &email).await;

    // Both requests race for the same token; the row lock serialises them.
    let (first, second) = tokio::join!(
        confirm(&db, saas.clone(), &token, NEW_PASSWORD),
        confirm(&db, saas.clone(), &token, "other-horse-battery-y"),
    );
    let statuses = [first.0, second.0];
    let successes = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(successes, 1, "exactly one confirm may succeed: {statuses:?}");
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::OK || *s == StatusCode::UNPROCESSABLE_ENTITY),
        "the loser must be a clean denial, not an error: {statuses:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revocation_failure_rolls_back_password_token_and_sessions() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: reset rollback under fault — POSTGRES_URL missing");
        return;
    };
    let email = format!("fault-{}@example.com", Uuid::new_v4());
    let saas = durable_store(&db).await;
    register(saas.clone(), &db, &email).await;
    let (_, body) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    let session = body["token"].as_str().expect("token").to_string();
    let user_row: (String,) =
        sqlx::query_as("SELECT id::text FROM users WHERE lower(email) = lower($1)")
        .bind(&email)
        .fetch_one(db.pool())
        .await
        .expect("user row");

    request_reset_for(saas.clone(), &db, &email).await;
    let token = latest_reset_token(&db, &email).await;

    // Fault injection, scoped to THIS user's session rows only: any session
    // revocation for this user raises, so the reset must fail as a whole.
    let trigger_fn = "password_reset_test_fail_session_revoke";
    sqlx::query(&format!(
        "CREATE OR REPLACE FUNCTION {trigger_fn}() RETURNS trigger AS $$
         BEGIN RAISE EXCEPTION 'injected session revocation failure'; END $$ LANGUAGE plpgsql"
    ))
    .execute(db.pool())
    .await
    .expect("create fault function");
    sqlx::query(&format!(
        "CREATE TRIGGER {trigger_fn}_trg BEFORE UPDATE ON saas_runtime_records
         FOR EACH ROW WHEN (OLD.kind = 'session' AND OLD.user_id = '{}'::uuid)
         EXECUTE FUNCTION {trigger_fn}()",
        user_row.0
    ))
    .execute(db.pool())
    .await
    .expect("create fault trigger");

    let (status, body) = confirm(&db, saas.clone(), &token, NEW_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a failed revocation must never report success: {body}"
    );
    assert_ne!(body["status"], "reset_complete");

    // Nothing committed: old password works, the session is still live, and
    // the token was not consumed.
    sqlx::query(&format!("DROP TRIGGER IF EXISTS {trigger_fn}_trg ON saas_runtime_records"))
        .execute(db.pool())
        .await
        .expect("drop trigger");
    let (status, _) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "old password must still work after rollback");
    assert_eq!(
        me(saas.clone(), &db, &session).await,
        StatusCode::OK,
        "session must survive rollback"
    );

    let (status, body) = confirm(&db, saas.clone(), &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "the unconsumed token must still work: {body}");
    let (status, _) = login(saas.clone(), &db, &email, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query(&format!("DROP FUNCTION IF EXISTS {trigger_fn}()"))
        .execute(db.pool())
        .await
        .expect("drop fault function");
}
