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
    request_reset_for(
        saas.clone(),
        &db,
        &format!("unknown-{}@example.com", Uuid::new_v4()),
    )
    .await;

    request_reset_for(saas.clone(), &db, &email).await;
    let token = latest_reset_token(&db, &email).await;

    // Wrong token: opaque denial, nothing changes.
    let (status, _) = confirm(&db, saas.clone(), "rst_not_a_real_token", NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "old password still valid after denial"
    );

    let (status, body) = confirm(&db, saas.clone(), &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "reset: {body}");
    assert_eq!(body["status"], "reset_complete");

    // The password login checks really changed (authoritative record).
    let (status, _) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "old password must stop working"
    );
    let (status, _) = login(saas.clone(), &db, &email, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "new password must work");

    // Every pre-existing session is revoked.
    assert_eq!(
        me(saas.clone(), &db, &session_a).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        me(saas.clone(), &db, &session_b).await,
        StatusCode::UNAUTHORIZED
    );

    // The token cannot be reused.
    let (status, _) = confirm(&db, saas.clone(), &token, "third-horse-battery-x").await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "token reuse must fail"
    );
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

    // Both requests race for the same token; the transactional lock/consume
    // permits one winner and an opaque denial for the other request.
    let (first, second) = tokio::join!(
        confirm(&db, saas.clone(), &token, NEW_PASSWORD),
        confirm(&db, saas.clone(), &token, "other-horse-battery-y"),
    );
    let statuses = [first.0, second.0];
    let successes = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(
        successes, 1,
        "exactly one confirm may succeed: {statuses:?}"
    );
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
    sqlx::query(&format!(
        "DROP TRIGGER IF EXISTS {trigger_fn}_trg ON saas_runtime_records"
    ))
    .execute(db.pool())
    .await
    .expect("drop trigger");
    let (status, _) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "old password must still work after rollback"
    );
    assert_eq!(
        me(saas.clone(), &db, &session).await,
        StatusCode::OK,
        "session must survive rollback"
    );

    let (status, body) = confirm(&db, saas.clone(), &token, NEW_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the unconsumed token must still work: {body}"
    );
    let (status, _) = login(saas.clone(), &db, &email, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query(&format!("DROP FUNCTION IF EXISTS {trigger_fn}()"))
        .execute(db.pool())
        .await
        .expect("drop fault function");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_and_expired_reset_tokens_leave_password_and_session_unchanged() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: reset expiry and revocation — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = format!("reset-expiry-{}@example.com", Uuid::new_v4());
    register(saas.clone(), &db, &email).await;
    let (status, body) = login(saas.clone(), &db, &email, OLD_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    let session = body["token"].as_str().expect("session").to_string();
    request_reset_for(saas.clone(), &db, &email).await;
    let older = latest_reset_token(&db, &email).await;
    request_reset_for(saas.clone(), &db, &email).await;
    let newer = latest_reset_token(&db, &email).await;
    assert_ne!(older, newer);
    assert_eq!(
        confirm(&db, saas.clone(), &older, NEW_PASSWORD).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    sqlx::query("UPDATE password_reset_tokens SET created_at = now() - interval '2 minutes', expires_at = now() - interval '1 minute' WHERE token_hash = $1")
        .bind(bot_core::session::token::hash_token(&newer))
        .execute(db.pool()).await.expect("expire this token only");
    assert_eq!(
        confirm(&db, saas.clone(), &newer, NEW_PASSWORD).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        login(saas.clone(), &db, &email, OLD_PASSWORD).await.0,
        StatusCode::OK
    );
    assert_eq!(me(saas.clone(), &db, &session).await, StatusCode::OK);
    assert_eq!(
        login(saas, &db, &email, NEW_PASSWORD).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlong_reset_password_does_not_consume_the_token() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: reset password upper bound — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = format!("reset-length-{}@example.com", Uuid::new_v4());
    register(saas.clone(), &db, &email).await;
    request_reset_for(saas.clone(), &db, &email).await;
    let token = latest_reset_token(&db, &email).await;
    let password = "x".repeat(sniper_suite::saas::users::MAX_PASSWORD_LEN + 1);
    let (status, body) = confirm(&db, saas.clone(), &token, &password).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "password_too_long");
    let (status, body) = confirm(&db, saas.clone(), &token, NEW_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "valid retry must still work: {body}"
    );
    assert_eq!(
        login(saas, &db, &email, NEW_PASSWORD).await.0,
        StatusCode::OK
    );
}

/// A busy token is an opaque denial, not a statement-timeout outage. The
/// competing transaction deliberately keeps the row locked until the HTTP
/// request returns; no scheduler timing or slow password hash is required.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locked_reset_token_is_denied_without_waiting_or_consuming_it() {
    let Some(db) = setup_db().await else {
        assert!(
            std::env::var_os("REQUIRE_POSTGRES").is_none(),
            "POSTGRES_URL is required for the reset lock regression"
        );
        eprintln!("NOT_RUN: reset lock contention — POSTGRES_URL missing");
        return;
    };
    let saas = durable_store(&db).await;
    let email = format!("reset-locked-{}@example.com", Uuid::new_v4());
    register(saas.clone(), &db, &email).await;
    request_reset_for(saas.clone(), &db, &email).await;
    let token = latest_reset_token(&db, &email).await;

    let mut holder = db.pool().begin().await.expect("lock transaction");
    let token_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM password_reset_tokens WHERE token_hash = $1 FOR UPDATE")
            .bind(bot_core::session::token::hash_token(&token))
            .fetch_one(&mut *holder)
            .await
            .expect("hold this token row");

    // A separate pool makes the deadline apply to the HTTP transaction's
    // connection, rather than depending on which pooled connection it gets.
    let contender = Arc::new(
        Database::connect(
            &DatabaseConfig {
                enabled: true,
                statement_timeout_ms: 100,
                ..Default::default()
            },
            &pg_url().expect("Postgres already required above"),
        )
        .await
        .expect("contending pool"),
    );
    let denied = confirm(&contender, saas.clone(), &token, NEW_PASSWORD).await;
    holder.rollback().await.expect("release token lock");
    let used: (Option<chrono::DateTime<chrono::Utc>>,) =
        sqlx::query_as("SELECT used_at FROM password_reset_tokens WHERE id = $1")
            .bind(token_id.0)
            .fetch_one(db.pool())
            .await
            .expect("token state after contention");
    assert_eq!(
        used.0, None,
        "the losing request must not consume the token"
    );
    assert_eq!(denied.0, StatusCode::UNPROCESSABLE_ENTITY, "{denied:?}");
    assert_eq!(denied.1["error"], "invalid_or_expired_token");
    assert_eq!(
        confirm(&db, saas, &token, NEW_PASSWORD).await.0,
        StatusCode::OK,
        "the token must remain usable once the unrelated holder releases it"
    );
}

/// A login cannot create a session after password reset and then write its
/// stale user snapshot back over the reset. Two database gates force the
/// vulnerable ordering without relying on scheduler timing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_interleaved_with_password_reset_never_resurrects_stale_credentials() {
    let Some(db) = setup_db().await else {
        assert!(
            std::env::var_os("REQUIRE_POSTGRES").is_none(),
            "POSTGRES_URL is required for the login/reset interleaving regression"
        );
        eprintln!("NOT_RUN: login/reset interleaving — POSTGRES_URL missing");
        return;
    };
    let email = format!("login-reset-race-{}@example.com", Uuid::new_v4());
    let saas = durable_store(&db).await;
    register(saas.clone(), &db, &email).await;
    let original_hash: (String,) = sqlx::query_as(
        "SELECT record->>'password_hash' FROM saas_runtime_records
          WHERE kind='user' AND record->>'email'=$1",
    )
    .bind(&email)
    .fetch_one(db.pool())
    .await
    .expect("original canonical password hash");
    request_reset_for(saas.clone(), &db, &email).await;
    let reset_token = latest_reset_token(&db, &email).await;
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE lower(email)=lower($1)")
        .bind(&email)
        .fetch_one(db.pool())
        .await
        .expect("canonical test user");
    let token_hash = bot_core::session::token::hash_token(&reset_token);

    let session_gate_fn = format!("login_reset_session_gate_{}", Uuid::new_v4().simple());
    let reset_gate_fn = format!("login_reset_token_gate_{}", Uuid::new_v4().simple());
    let session_key_a: i32 = 781_240_031;
    let session_key_b: i32 = 1_942_703_811;
    let reset_key_a: i32 = 781_240_032;
    let reset_key_b: i32 = 1_942_703_812;
    sqlx::query(&format!(
        "CREATE FUNCTION {session_gate_fn}() RETURNS trigger AS $$
         BEGIN
           PERFORM pg_advisory_xact_lock({session_key_a}, {session_key_b});
           RETURN NEW;
         END $$ LANGUAGE plpgsql"
    ))
    .execute(db.pool())
    .await
    .expect("create session-insert gate");
    sqlx::query(&format!(
        "CREATE TRIGGER {session_gate_fn}_trg BEFORE INSERT ON saas_runtime_records
         FOR EACH ROW WHEN (NEW.kind = 'session') EXECUTE FUNCTION {session_gate_fn}()"
    ))
    .execute(db.pool())
    .await
    .expect("attach session-insert gate");
    sqlx::query(&format!(
        "CREATE FUNCTION {reset_gate_fn}() RETURNS trigger AS $$
         BEGIN
           PERFORM pg_advisory_xact_lock({reset_key_a}, {reset_key_b});
           RETURN NEW;
         END $$ LANGUAGE plpgsql"
    ))
    .execute(db.pool())
    .await
    .expect("create reset-token gate");
    sqlx::query(&format!(
        "CREATE TRIGGER {reset_gate_fn}_trg BEFORE UPDATE ON password_reset_tokens
         FOR EACH ROW WHEN (OLD.token_hash = '{token_hash}' AND NEW.used_at IS NOT NULL)
         EXECUTE FUNCTION {reset_gate_fn}()"
    ))
    .execute(db.pool())
    .await
    .expect("attach reset-token gate");

    let mut session_gate_holder = db.pool().begin().await.expect("session gate transaction");
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(session_key_a)
        .bind(session_key_b)
        .execute(&mut *session_gate_holder)
        .await
        .expect("hold session-insert gate");
    let mut reset_gate_holder = db.pool().begin().await.expect("reset gate transaction");
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(reset_key_a)
        .bind(reset_key_b)
        .execute(&mut *reset_gate_holder)
        .await
        .expect("hold reset-token gate");

    let login_saas = saas.clone();
    let login_db = db.clone();
    let login_email = email.clone();
    let login_task =
        tokio::spawn(async move { login(login_saas, &login_db, &login_email, OLD_PASSWORD).await });
    let session_gate_reached = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: (bool,) = sqlx::query_as(
                "SELECT EXISTS (
                    SELECT 1 FROM pg_stat_activity
                     WHERE datname = current_database()
                       AND wait_event_type = 'Lock' AND wait_event = 'advisory'
                       AND query ILIKE 'INSERT INTO saas_runtime_records%'
                 )",
            )
            .fetch_one(db.pool())
            .await
            .expect("observe login paused before session insert");
            if waiting.0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        session_gate_reached.is_ok(),
        "login did not reach the controlled session insert"
    );

    let reset_db = db.clone();
    let reset_saas = saas.clone();
    let reset_token_for_task = reset_token.clone();
    let reset_task = tokio::spawn(async move {
        confirm(&reset_db, reset_saas, &reset_token_for_task, NEW_PASSWORD).await
    });
    // The buggy implementation reaches token consumption while login's
    // session insert is paused. The atomic implementation first locks the
    // user row, so reset must wait there. Observe the actual SQL lock point.
    let phase = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let (phase,): (String,) = sqlx::query_as(
                "SELECT CASE
                    WHEN EXISTS (
                      SELECT 1 FROM pg_stat_activity
                       WHERE datname=current_database()
                         AND wait_event_type='Lock' AND wait_event='advisory'
                         AND query ILIKE 'UPDATE password_reset_tokens SET used_at%'
                    ) THEN 'reset_gate'
                    WHEN EXISTS (
                      SELECT 1 FROM pg_stat_activity
                       WHERE datname=current_database()
                         AND wait_event_type='Lock'
                         AND query ILIKE 'SELECT record FROM saas_runtime_records%'
                    ) THEN 'user_row_lock'
                    ELSE 'pending' END",
            )
            .fetch_one(db.pool())
            .await
            .expect("observe reset lock point");
            if phase != "pending" {
                break phase;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reset did not reach either controlled transaction gate");

    let reset_can_commit_before_login_release = phase == "reset_gate";
    let ((login_status, login_body), (reset_status, reset_body)) =
        if reset_can_commit_before_login_release {
            // Reproduce the vulnerable order exactly: reset commits while login
            // has already created a session but has not yet written last_login.
            reset_gate_holder
                .commit()
                .await
                .expect("release reset token gate");
            let reset_result = reset_task.await.expect("join early password reset");
            session_gate_holder
                .commit()
                .await
                .expect("release late login session insert");
            let login_result = login_task.await.expect("join late login");
            (login_result, reset_result)
        } else {
            // Correct implementation: login owns the user row lock before it
            // reaches the session-insert trigger, so reset waits until login
            // commits. Reset then revokes the just-created session.
            session_gate_holder
                .commit()
                .await
                .expect("release atomic login transaction");
            let login_result = login_task.await.expect("join login");
            reset_gate_holder
                .commit()
                .await
                .expect("release reset token gate after user lock");
            let reset_result = reset_task.await.expect("join password reset");
            (login_result, reset_result)
        };
    assert_eq!(login_status, StatusCode::OK, "racing login: {login_body}");
    assert_eq!(reset_status, StatusCode::OK, "password reset: {reset_body}");

    sqlx::query(&format!(
        "DROP TRIGGER IF EXISTS {session_gate_fn}_trg ON saas_runtime_records"
    ))
    .execute(db.pool())
    .await
    .expect("drop session-insert gate");
    sqlx::query(&format!("DROP FUNCTION IF EXISTS {session_gate_fn}()"))
        .execute(db.pool())
        .await
        .expect("drop session-insert gate function");
    sqlx::query(&format!(
        "DROP TRIGGER IF EXISTS {reset_gate_fn}_trg ON password_reset_tokens"
    ))
    .execute(db.pool())
    .await
    .expect("drop reset-token gate");
    sqlx::query(&format!("DROP FUNCTION IF EXISTS {reset_gate_fn}()"))
        .execute(db.pool())
        .await
        .expect("drop reset-token gate function");

    assert!(
        phase == "reset_gate" || phase == "user_row_lock",
        "the race must be serialized at one of the intended gates: {phase}"
    );
    let (active_sessions,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM saas_runtime_records
          WHERE kind='session' AND user_id=$1 AND record->>'revoked_at' IS NULL",
    )
    .bind(user_id.0)
    .fetch_one(db.pool())
    .await
    .expect("count surviving sessions");
    assert_eq!(
        active_sessions, 0,
        "no pre-reset or racing session may survive reset"
    );
    let (persisted_hash,): (String,) = sqlx::query_as(
        "SELECT record->>'password_hash' FROM saas_runtime_records
          WHERE kind='user' AND id=$1",
    )
    .bind(user_id.0.to_string())
    .fetch_one(db.pool())
    .await
    .expect("final canonical password hash");
    assert_ne!(
        persisted_hash, original_hash.0,
        "late login's stale snapshot must not overwrite reset password"
    );
    assert!(
        bot_core::session::token::verify_password(NEW_PASSWORD, &persisted_hash),
        "the confirmed reset password must remain authoritative"
    );
}
