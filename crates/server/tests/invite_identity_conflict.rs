//! Invitation acceptance — duplicate-email conflict contract (P2-DUPEMAIL).
//!
//! When accepting an invitation would create a second account for an address
//! that already has one at the database level, the endpoint must answer a
//! stable `409 registration_conflict` (the same generic contract registration
//! uses), never a `500`/`503` and never another account's detail.
//!
//! Layers:
//! 1. **Hermetic (always runs)** — only unique violations on the two named
//!    identity constraints are conflicts; every other database error stays a
//!    server error (see the unit tests in `auth_flows.rs`).
//! 2. **Durable (`POSTGRES_URL` required, else NOT_RUN)** — preflight join of
//!    an existing account, and the lower-level race: the user row exists in
//!    the projection but NOT in the runtime lookup, so the preflight misses and
//!    the insert collides on `users_email_key`. This is deterministic, not a
//!    timing race.

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

async fn register_and_login(saas: Arc<SaasStore>, db: &Arc<Database>, email: &str) -> String {
    let (status, _) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/users",
        Some(json!({ "email": email, "password": PASSWORD, "display_name": "Owner" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "registration of {email}");
    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/sessions",
        Some(json!({ "email": email, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login of {email}");
    body["token"].as_str().expect("token").to_string()
}

async fn org_scoped_session(
    saas: Arc<SaasStore>,
    db: &Arc<Database>,
    owner: &str,
    name: &str,
) -> String {
    let token = register_and_login(saas.clone(), db, owner).await;
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/organizations",
        Some(json!({ "name": name })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "org creation: {body}");
    let slug = body["organization"]["slug"].as_str().expect("slug").to_string();
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

/// Issue an invitation and return its one-time plaintext token.
async fn invite(saas: Arc<SaasStore>, db: &Arc<Database>, scoped: &str, email: &str) -> String {
    let (status, body) = request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/team/invites",
        Some(json!({ "email": email, "role": "member" })),
        Some(scoped),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite create: {body}");
    body["invitation_token"]
        .as_str()
        .expect("invitation_token")
        .to_string()
}

async fn accept(
    saas: Arc<SaasStore>,
    db: &Arc<Database>,
    token: &str,
    password: Option<&str>,
) -> (StatusCode, Value) {
    request(
        state_with(saas, Some(db.clone())),
        "POST",
        "/api/saas/team/invites/accept",
        Some(match password {
            Some(p) => json!({ "token": token, "password": p }),
            None => json!({ "token": token }),
        }),
        None,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepting_for_an_existing_account_joins_it_without_a_second_account() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: invite join of existing account — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let tag = Uuid::new_v4().simple().to_string();
    let owner = format!("owner-{tag}@example.com");
    let member = format!("member-{tag}@example.com");
    let scoped = org_scoped_session(saas.clone(), &db, &owner, &format!("Org {tag}")).await;
    // The invitee already has an account (preflight will find it).
    register_and_login(saas.clone(), &db, &member).await;

    let token = invite(saas.clone(), &db, &scoped, &member).await;
    let (status, body) = accept(saas.clone(), &db, &token, None).await;
    assert!(status.is_success(), "existing account must join: {status} {body}");

    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM users WHERE lower(email) = lower($1)")
        .bind(&member)
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(count, 1, "no second account may be created");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projection_collision_on_email_is_a_409_not_a_500() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: duplicate-email conflict — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let tag = Uuid::new_v4().simple().to_string();
    let owner = format!("owner-{tag}@example.com");
    let scoped = org_scoped_session(saas.clone(), &db, &owner, &format!("Org {tag}")).await;

    // ASCII-only fixture, so the outcome does not depend on DB locale or
    // collation. The projection-only row carries the exact address being
    // invited. The runtime lookup misses it (no runtime record exists), and
    // the users_email_key unique index collides on insert.
    let address = format!("dup-{tag}@example.com");
    let existing = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'pbkdf2-sha256$1$AAAA$AAAA')",
    )
        .bind(existing)
        .bind(&address)
        .execute(db.pool())
        .await
        .expect("seed projection-only account");

    let token = invite(saas.clone(), &db, &scoped, &address).await;
    let (status, body) = accept(saas.clone(), &db, &token, Some("accept-horse-battery")).await;
    assert_eq!(status, StatusCode::CONFLICT, "collision must be a 409: {body}");
    assert_eq!(body["error"], "registration_conflict");
    let text = body.to_string();
    assert!(!text.contains("users_email_key"), "no constraint names: {text}");
    assert!(!text.contains(&existing.to_string()), "no other account id: {text}");

    // Nothing was created by the failed accept.
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM saas_runtime_records WHERE kind = 'user' AND lookup_key = $1",
    )
        .bind(&address)
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(count, 0, "the failed accept must roll back its runtime user record");
}
