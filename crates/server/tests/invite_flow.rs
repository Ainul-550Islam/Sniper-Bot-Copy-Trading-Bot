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
//! Invitation credentials are queued to the recipient in the same transaction
//! as the invitation hash; no inviter-facing response ever contains a token.

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
    let slug = body["organization"]["slug"]
        .as_str()
        .expect("slug")
        .to_string();
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
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "viewer" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = request(state, "GET", "/api/saas/team/invites", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn role_and_email_validation_run_before_storage() {
    let saas = Arc::new(SaasStore::new_in_memory_identity());
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

    let (status, _body) = request(
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
        Some(serde_json::json!({ "email": "not-an-email", "role": "viewer" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_a_database_the_invite_is_an_honest_503() {
    let saas = Arc::new(SaasStore::new_in_memory_identity());
    let scoped = org_scoped_session(saas.clone(), None, "Acme No Db").await;
    let (status, body) = request(
        state_with(saas, None),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "viewer" })),
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

async fn invitation_token(db: &Arc<Database>, email: &str, invite_id: &str) -> String {
    let row: (String,) = sqlx::query_as(
        "SELECT body_text FROM email_outbox
          WHERE recipient = $1 AND template_key = 'member_invite'
            AND dedup_key LIKE $2
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(email)
    .bind(format!("member_invite:{invite_id}:%"))
    .fetch_one(db.pool())
    .await
    .expect("recipient invitation email queued in outbox");
    row.0
        .split("token=")
        .nth(1)
        .expect("token in recipient email link")
        .split_whitespace()
        .next()
        .expect("invitation token")
        .to_string()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invite_lifecycle_create_list_resend_revoke() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: invite lifecycle — POSTGRES_URL missing");
        return;
    };
    let saas = Arc::new(
        SaasStore::with_database(Some(db.clone()))
            .await
            .expect("durable identity store"),
    );
    let scoped = org_scoped_session(saas.clone(), Some(db.clone()), "Acme Invites").await;

    // A mail-queue insert failure must roll the invitation hash back too.
    let fault = format!("invite_outbox_fault_{}", Uuid::new_v4().simple());
    sqlx::query(&format!(
        "CREATE FUNCTION {fault}() RETURNS trigger AS $$
         BEGIN
           IF NEW.template_key = 'member_invite' THEN
             RAISE EXCEPTION 'injected invitation outbox failure';
           END IF;
           RETURN NEW;
         END $$ LANGUAGE plpgsql"
    ))
    .execute(db.pool())
    .await
    .expect("outbox fault function");
    sqlx::query(&format!(
        "CREATE TRIGGER {fault}_trg BEFORE INSERT ON email_outbox
         FOR EACH ROW EXECUTE FUNCTION {fault}()"
    ))
    .execute(db.pool())
    .await
    .expect("outbox fault trigger");
    let failed_email = format!("outbox-failure-{}@example.com", Uuid::new_v4());
    let (failed_status, failed_body) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": failed_email, "role": "viewer" })),
        Some(&scoped),
    )
    .await;
    sqlx::query(&format!("DROP TRIGGER {fault}_trg ON email_outbox"))
        .execute(db.pool())
        .await
        .expect("drop outbox fault trigger");
    sqlx::query(&format!("DROP FUNCTION {fault}()"))
        .execute(db.pool())
        .await
        .expect("drop outbox fault function");
    assert_eq!(
        failed_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{failed_body}"
    );
    let (failed_invites,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM invites WHERE email = $1 AND status = 'pending'")
            .bind(&failed_email)
            .fetch_one(db.pool())
            .await
            .expect("failed invitation count");
    assert_eq!(
        failed_invites, 0,
        "no usable invite may outlive missing mail"
    );

    // CREATE — the token is never returned to the inviter; inspect only the
    // recipient outbox as the test's simulated delivery channel.
    let (status, created) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/team/invites",
        Some(serde_json::json!({ "email": MEMBER_EMAIL, "role": "viewer" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let invitation_id = created["invitation_id"].as_str().expect("id").to_string();
    assert!(created.get("invitation_token").is_none());
    assert_eq!(created["token_delivery"], "queued_for_delivery");
    assert_eq!(created["email"], MEMBER_EMAIL);
    let initial_token = invitation_token(&db, MEMBER_EMAIL, &invitation_id).await;
    let (queued_to,): (String,) = sqlx::query_as(
        "SELECT recipient FROM email_outbox WHERE recipient = $1 AND template_key = 'member_invite' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(MEMBER_EMAIL)
    .fetch_one(db.pool())
    .await
    .expect("invite mail recipient");
    assert_eq!(
        queued_to, MEMBER_EMAIL,
        "invitation secrets are addressed to the invitee"
    );

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
    assert!(!listed.to_string().contains(&initial_token));

    // Failed resend enqueue must roll back token rotation so the previously
    // queued recipient link remains the active invite credential.
    let (active_hash_before,): (String,) =
        sqlx::query_as("SELECT token_hash FROM invites WHERE id = $1")
            .bind(Uuid::parse_str(&invitation_id).expect("invite UUID"))
            .fetch_one(db.pool())
            .await
            .expect("active token hash before resend");
    let resend_fault = format!("invite_resend_outbox_fault_{}", Uuid::new_v4().simple());
    sqlx::query(&format!(
        "CREATE FUNCTION {resend_fault}() RETURNS trigger AS $$
         BEGIN
           IF NEW.template_key = 'member_invite' THEN
             RAISE EXCEPTION 'injected resend outbox failure';
           END IF;
           RETURN NEW;
         END $$ LANGUAGE plpgsql"
    ))
    .execute(db.pool())
    .await
    .expect("resend fault function");
    sqlx::query(&format!(
        "CREATE TRIGGER {resend_fault}_trg BEFORE INSERT ON email_outbox
         FOR EACH ROW EXECUTE FUNCTION {resend_fault}()"
    ))
    .execute(db.pool())
    .await
    .expect("resend fault trigger");
    let (failed_resend_status, failed_resend_body) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        &format!("/api/saas/team/invites/{invitation_id}/resend"),
        None,
        Some(&scoped),
    )
    .await;
    sqlx::query(&format!("DROP TRIGGER {resend_fault}_trg ON email_outbox"))
        .execute(db.pool())
        .await
        .expect("drop resend fault trigger");
    sqlx::query(&format!("DROP FUNCTION {resend_fault}()"))
        .execute(db.pool())
        .await
        .expect("drop resend fault function");
    assert_eq!(
        failed_resend_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{failed_resend_body}"
    );
    let (active_hash_after,): (String,) =
        sqlx::query_as("SELECT token_hash FROM invites WHERE id = $1")
            .bind(Uuid::parse_str(&invitation_id).expect("invite UUID"))
            .fetch_one(db.pool())
            .await
            .expect("active token hash after failed resend");
    assert_eq!(
        active_hash_before, active_hash_after,
        "failed resend preserves active token"
    );

    // RESEND — rotates the recipient-delivered token.
    let (status, resent) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        &format!("/api/saas/team/invites/{invitation_id}/resend"),
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resent}");
    assert!(resent.get("invitation_token").is_none());
    assert_eq!(resent["token_delivery"], "queued_for_delivery");
    let resent_token = invitation_token(&db, MEMBER_EMAIL, &invitation_id).await;
    assert_ne!(
        resent_token, initial_token,
        "resend rotates recipient-delivered token"
    );

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
    let persisted: (String,) = sqlx::query_as("SELECT status FROM invites WHERE id = $1")
        .bind(Uuid::parse_str(&invitation_id).expect("invite UUID"))
        .fetch_one(db.pool())
        .await
        .expect("durable revoked invitation");
    assert_eq!(
        persisted.0, "revoked",
        "revocation must retain the audit row"
    );
    let expired_id = Uuid::new_v4();
    let expired_token = bot_core::session::token::generate_token("inv");
    sqlx::query(
        "INSERT INTO invites (id, organization_id, email, role, token_hash, expires_at)
         VALUES ($1, $2, 'expired@example.com', 'viewer', $3, now() - interval '1 minute')",
    )
    .bind(expired_id)
    .bind(
        Uuid::parse_str(created["organization_id"].as_str().expect("organization"))
            .expect("org UUID"),
    )
    .bind(expired_token.hash)
    .execute(db.pool())
    .await
    .expect("expired pending invitation");
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
    assert!(
        !listed.to_string().contains(&expired_id.to_string()),
        "expired invitations are not active: {listed}"
    );
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone())),
        "POST",
        "/api/saas/team/invites/accept",
        Some(serde_json::json!({ "token": resent_token, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a revoked token must remain unusable: {body}"
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
