//! Tenant and identity isolation for invitation acceptance and email
//! verification (P1 `tenant_auth_isolation`).
//!
//! 1. **Hermetic (always runs):** without an attached database, invitation
//!    acceptance and email-verification confirmation answer an honest 503 and
//!    never a success shape.
//! 2. **Durable (`POSTGRES_URL` required, else NOT_RUN):** wrong, expired,
//!    superseded, replayed and cross-user tokens are refused, and a refused
//!    token leaves no side effect (`email_verified` and membership unchanged).
//!    Tokens are read exactly as the real delivery channels expose them: the
//!    verification link from the durable outbox, and the invitation token from
//!    the invite-creation response.

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

const PASSWORD: &str = "correct-horse-battery";

fn state_with(saas: Arc<SaasStore>, db: Option<Arc<Database>>) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        trusted_proxies: Default::default(),
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
async fn invite_accept_without_a_database_is_unavailable_never_success() {
    let state = state_with(Arc::new(SaasStore::new()), None);
    let (status, body) = request(
        state,
        "POST",
        "/api/saas/team/invites/accept",
        Some(json!({ "token": "inv_not_a_real_token", "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "identity_storage_unavailable", "{body}");
}

#[tokio::test]
async fn email_confirm_without_a_database_is_unavailable_never_success() {
    let state = state_with(Arc::new(SaasStore::new()), None);
    let (status, body) = request(
        state,
        "POST",
        "/api/saas/email-verification/confirm",
        Some(json!({ "token": "ver_not_a_real_token" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "email_verification_unavailable", "{body}");
}

// ---------------------------------------------------------------------
// Durable flows (POSTGRES_URL required)
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

async fn durable_store(db: &Arc<Database>) -> Arc<SaasStore> {
    Arc::new(
        SaasStore::with_database(Some(db.clone()))
            .await
            .expect("durable store"),
    )
}

/// A fresh, lowercase address so reruns against one database never collide.
fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@example.com", Uuid::new_v4().simple())
}

async fn register(saas: Arc<SaasStore>, db: &Arc<Database>, email: &str) {
    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/users",
        Some(json!({ "email": email, "password": PASSWORD, "display_name": "Identity" })),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "registration of {email}: {body}"
    );
}

async fn request_verification(saas: Arc<SaasStore>, db: &Arc<Database>, email: &str) {
    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/email-verification/request",
        Some(json!({ "email": email })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "verification request: {body}");
}

/// The newest verification token for `email`, read from the durable outbox.
async fn latest_verification_token(db: &Arc<Database>, email: &str) -> String {
    let row: (String,) = sqlx::query_as(
        "SELECT body_text FROM email_outbox
          WHERE recipient = $1 AND body_text LIKE '%verify-email?token=%'
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(email)
    .fetch_one(db.pool())
    .await
    .expect("verification email in outbox");
    let after = row.0.split("token=").nth(1).expect("token in link");
    after
        .split(|c: char| c.is_whitespace())
        .next()
        .expect("token value")
        .to_string()
}

async fn confirm_verification(
    saas: Arc<SaasStore>,
    db: &Arc<Database>,
    token: &str,
) -> (StatusCode, Value) {
    request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/email-verification/confirm",
        Some(json!({ "token": token })),
        None,
    )
    .await
}

async fn email_verified(db: &Arc<Database>, email: &str) -> bool {
    let row: (bool,) = sqlx::query_as("SELECT email_verified FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(db.pool())
        .await
        .expect("user row");
    row.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_verification_token_is_refused_with_no_side_effect() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: wrong verification token — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = unique_email("wrong");
    register(saas.clone(), &db, &email).await;
    request_verification(saas.clone(), &db, &email).await;

    let (status, body) = confirm_verification(saas, &db, "ver_not_a_real_token").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "invalid_or_expired_token", "{body}");
    assert!(
        !email_verified(&db, &email).await,
        "a wrong token must not verify"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verification_token_verifies_only_its_own_user_and_is_single_use() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: cross-user verification — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let first = unique_email("own-a");
    let other = unique_email("own-b");
    register(saas.clone(), &db, &first).await;
    register(saas.clone(), &db, &other).await;
    request_verification(saas.clone(), &db, &first).await;
    let token = latest_verification_token(&db, &first).await;

    let (status, body) = confirm_verification(saas.clone(), &db, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        email_verified(&db, &first).await,
        "the token's own user is verified"
    );
    assert!(
        !email_verified(&db, &other).await,
        "a token for one user must never verify another user"
    );

    let (status, body) = confirm_verification(saas, &db, &token).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "replay must fail: {body}"
    );
    assert_eq!(body["error"], "invalid_or_expired_token", "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_verification_token_is_refused() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: expired verification token — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = unique_email("expired");
    register(saas.clone(), &db, &email).await;
    request_verification(saas.clone(), &db, &email).await;
    let token = latest_verification_token(&db, &email).await;

    sqlx::query(
        "UPDATE email_verifications SET created_at = now() - interval '2 minutes', expires_at = now() - interval '1 minute'
          WHERE user_id = (SELECT id FROM users WHERE email = $1)",
    )
    .bind(&email)
    .execute(db.pool())
    .await
    .expect("expire the token");

    let (status, body) = confirm_verification(saas, &db, &token).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        !email_verified(&db, &email).await,
        "an expired token must not verify"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseded_verification_token_is_revoked_by_a_newer_request() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: superseded verification token — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = unique_email("supersede");
    register(saas.clone(), &db, &email).await;

    request_verification(saas.clone(), &db, &email).await;
    let older = latest_verification_token(&db, &email).await;
    request_verification(saas.clone(), &db, &email).await;
    let newer = latest_verification_token(&db, &email).await;
    assert_ne!(older, newer, "the second request must mint a new token");

    let (status, body) = confirm_verification(saas.clone(), &db, &older).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "older token: {body}"
    );
    assert!(!email_verified(&db, &email).await);

    let (status, body) = confirm_verification(saas, &db, &newer).await;
    assert_eq!(status, StatusCode::OK, "newer token: {body}");
    assert!(email_verified(&db, &email).await);
}

/// Create an owner, an organization, and return a session SCOPED to it.
async fn owner_scoped_session(saas: Arc<SaasStore>, db: &Arc<Database>) -> String {
    let owner = unique_email("owner");
    register(saas.clone(), db, &owner).await;
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/sessions",
        Some(json!({ "email": owner, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner login: {body}");
    let unscoped = body["token"].as_str().expect("token").to_string();

    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/organizations",
        Some(json!({ "name": "Isolation Org" })),
        Some(&unscoped),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "org creation: {body}");
    let slug = body["organization"]["slug"]
        .as_str()
        .expect("slug")
        .to_string();

    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/sessions",
        Some(json!({ "email": owner, "password": PASSWORD, "organization": slug })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "scoped login: {body}");
    body["token"].as_str().expect("token").to_string()
}

/// Create an invitation for `invitee` and return the one-time token.
async fn create_invitation(
    saas: Arc<SaasStore>,
    db: &Arc<Database>,
    scoped: &str,
    invitee: &str,
) -> String {
    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/team/invites",
        Some(json!({ "email": invitee, "role": "viewer" })),
        Some(scoped),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite creation: {body}");
    body["invitation_token"]
        .as_str()
        .expect("invitation token is returned once, at creation")
        .to_string()
}

async fn accept_invitation(
    saas: Arc<SaasStore>,
    db: &Arc<Database>,
    token: &str,
) -> (StatusCode, Value) {
    request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/team/invites/accept",
        Some(json!({ "token": token, "password": PASSWORD })),
        None,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invitation_token_is_single_use_and_refused_on_replay() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: invitation replay — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let scoped = owner_scoped_session(saas.clone(), &db).await;
    let invitee = unique_email("invitee");
    let token = create_invitation(saas.clone(), &db, &scoped, &invitee).await;

    let (status, body) = accept_invitation(saas.clone(), &db, &token).await;
    assert!(
        status.is_success(),
        "first acceptance must succeed: {status} {body}"
    );

    let (status, body) = accept_invitation(saas, &db, &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "replay must fail: {body}");
    assert_eq!(body["error"], "invitation_not_found", "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_invitation_token_is_refused() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: expired invitation — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let scoped = owner_scoped_session(saas.clone(), &db).await;
    let invitee = unique_email("expired-invitee");
    let token = create_invitation(saas.clone(), &db, &scoped, &invitee).await;

    sqlx::query("UPDATE invites SET expires_at = now() - interval '1 minute' WHERE email = $1")
        .bind(&invitee)
        .execute(db.pool())
        .await
        .expect("expire the invitation");

    let (status, body) = accept_invitation(saas, &db, &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "invitation_not_found", "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_invitation_token_is_refused() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: wrong invitation token — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let (status, body) = accept_invitation(saas, &db, "inv_not_a_real_token").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "invitation_not_found", "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_verification_confirms_have_exactly_one_success() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: concurrent verification — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = unique_email("verify-race");
    register(saas.clone(), &db, &email).await;
    request_verification(saas.clone(), &db, &email).await;
    let token = latest_verification_token(&db, &email).await;
    let (first, second) = tokio::join!(
        confirm_verification(saas.clone(), &db, &token),
        confirm_verification(saas.clone(), &db, &token),
    );
    let mut statuses = [first.0.as_u16(), second.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 422]);
    assert!(email_verified(&db, &email).await);
    assert!(
        saas.user_by_email(&email)
            .await
            .expect("canonical user")
            .expect("user")
            .email_verified
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verification_projection_failure_rolls_back_token_and_canonical_user() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: verification rollback — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = unique_email("verify-rollback");
    register(saas.clone(), &db, &email).await;
    request_verification(saas.clone(), &db, &email).await;
    let token = latest_verification_token(&db, &email).await;
    let user = saas
        .user_by_email(&email)
        .await
        .expect("canonical lookup")
        .expect("user");
    let function = format!("verify_fault_{}", Uuid::new_v4().simple());
    sqlx::query(&format!(
        "CREATE FUNCTION {function}() RETURNS trigger AS $$
         BEGIN RAISE EXCEPTION 'injected verification projection failure'; END $$ LANGUAGE plpgsql"
    ))
    .execute(db.pool())
    .await
    .expect("fault function");
    sqlx::query(&format!(
        "CREATE TRIGGER {function}_trg BEFORE UPDATE ON users
         FOR EACH ROW WHEN (OLD.id = '{}'::uuid) EXECUTE FUNCTION {function}()",
        user.id,
    ))
    .execute(db.pool())
    .await
    .expect("fault trigger");
    let (status, body) = confirm_verification(saas.clone(), &db, &token).await;
    // Remove the fault before assertions so an unexpected result still leaves
    // the shared database usable by the remaining integration targets.
    sqlx::query(&format!("DROP TRIGGER {function}_trg ON users"))
        .execute(db.pool())
        .await
        .expect("remove fault trigger");
    sqlx::query(&format!("DROP FUNCTION {function}()"))
        .execute(db.pool())
        .await
        .expect("remove fault function");
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(!email_verified(&db, &email).await);
    assert!(
        !saas
            .user_by_email(&email)
            .await
            .expect("canonical user")
            .expect("user")
            .email_verified
    );
    let (status, body) = confirm_verification(saas.clone(), &db, &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the token must survive rollback: {body}"
    );
    assert!(
        saas.user_by_email(&email)
            .await
            .expect("canonical user")
            .expect("user")
            .email_verified
    );
}
