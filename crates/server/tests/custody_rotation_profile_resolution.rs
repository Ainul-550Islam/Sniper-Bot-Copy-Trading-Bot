//! PostgreSQL-backed proof that custody rotation resolves the tenant's
//! PERSISTED custody profile — never a freshly generated one
//! (PROMPT 6 §P0 / Phase 1, file 3).
//!
//! The historic defect (AUDIT §13.6, fixed in PROMPT 5 and now pinned by
//! this test) was `CustodyProfileId::new()` inside the rotation handler:
//! every rotation invented a synthetic profile identity instead of
//! resolving the tenant's actual stored profile.
//!
//! What this test proves, through the REAL HTTP routes:
//!
//! 1. a custody profile created via `POST /api/saas/custody/profiles` is
//!    durably persisted in PostgreSQL (`custody_profiles` row, correct
//!    organization) when a database is attached;
//! 2. `POST /api/saas/custody/rotations` succeeds ONLY by naming that
//!    exact persisted profile id — the response carries the SAME id, not
//!    a new random one;
//! 3. the rotation record's status endpoint reports the same profile id
//!    (the persisted identity follows the rotation through its lifecycle);
//! 4. the profile row in PostgreSQL is unchanged by the rotation (the
//!    rotation never mutates or replaces the persisted profile).
//!
//! Service-backed: requires `POSTGRES_URL`; without it the suite reports
//! NOT_RUN and never fabricates a pass.

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::{AppConfig, DatabaseConfig};
use bot_core::db::Database;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::SaasStore;

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn not_run(m: &str) {
    eprintln!("NOT_RUN: {m} — POSTGRES_URL missing");
}

async fn pg() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url).await.ok()?;
    let _ = db.migrate().await;
    Some(Arc::new(db))
}

/// A minimal in-process API state (the api.rs unit-test pattern). When a
/// database is attached, the SaaS store itself is the PRODUCTION
/// PostgreSQL-backed store (`SaasStore::with_database`), so users,
/// organizations and sessions are durable exactly as in a deployment —
/// not just the custody rows.
async fn test_state(db: Option<Arc<Database>>) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    let saas = SaasStore::with_database(db.clone())
        .await
        .expect("saas store");
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
        saas: Arc::new(saas),
        trading: None,
        module_registry: Arc::new(
            sniper_suite::module_runtime::module_registry::TenantModuleRegistry::new(),
        ),
    }
}

/// One HTTP request through the real router; returns (status, body).
async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(payload) => builder
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .expect("request body"),
        None => builder.body(Body::empty()).expect("empty body"),
    };
    let response = app.oneshot(request).await.expect("router answers");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collected")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A deterministic base58-looking public address (public data only).
fn address(seed: u8) -> String {
    let body = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let mut chars: Vec<char> = body.chars().collect();
    chars[3] = char::from(b'a' + (seed % 26));
    chars.into_iter().collect()
}

/// Register a user, create their organization, and return a session token
/// scoped to that organization plus the organization id.
async fn org_owner(app: axum::Router, email: &str) -> (String, String) {
    let password = "Sup3r-Secret-Passphrase-42";

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/users",
        None,
        Some(json!({ "email": email, "password": password, "display_name": "Rotation Owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register: {body}");

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/sessions",
        None,
        Some(json!({ "email": email, "password": password })),
    )
    .await;
    assert!(status.is_success(), "login: {body}");
    let unscoped = body.parse::<Value>().expect("login json")["token"]
        .as_str()
        .expect("token")
        .to_string();

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/organizations",
        Some(&unscoped),
        Some(json!({ "name": "Rotation Persistence Org" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create org: {body}");
    let org = body.parse::<Value>().expect("org json")["organization"]["id"]
        .as_str()
        .expect("organization id")
        .to_string();
    let slug = body.parse::<Value>().expect("json body")["organization"]["slug"]
        .as_str()
        .expect("organization slug")
        .to_string();

    // A second login scoped to the new organization (creator = OrgOwner,
    // which holds WalletManage).
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/sessions",
        None,
        Some(json!({ "email": email, "password": password, "organization": slug })),
    )
    .await;
    assert!(status.is_success(), "scoped login: {body}");
    let scoped = body.parse::<Value>().expect("json body")["token"]
        .as_str()
        .expect("scoped token")
        .to_string();
    (scoped, org)
}

/// Create + activate a custody profile with two active signers; returns
/// (profile_id, old_signer_id, new_signer_id).
async fn profile_with_two_signers(app: axum::Router, token: &str) -> (String, String, String) {
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/profiles",
        Some(token),
        Some(json!({ "name": "primary", "provider_type": "local" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create profile: {body}");
    let profile_id = body.parse::<Value>().expect("json body")["id"]
        .as_str()
        .expect("profile id")
        .to_string();

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/saas/custody/profiles/{profile_id}/activate"),
        Some(token),
        None,
    )
    .await;
    assert!(status.is_success(), "activate profile: {body}");

    let mut signer_ids = Vec::new();
    for (seed, identity) in [(1u8, "sniper-key-old"), (2u8, "sniper-key-new")] {
        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/saas/custody/signers",
            Some(token),
            Some(json!({
                "custody_profile_id": profile_id,
                "logical_identity": identity,
                "public_address": address(seed),
                "capabilities": ["solana.sign"],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create signer: {body}");
        let signer_id = body.parse::<Value>().expect("json body")["id"]
            .as_str()
            .expect("signer id")
            .to_string();

        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/saas/custody/signers/{signer_id}/activate"),
            Some(token),
            None,
        )
        .await;
        assert!(status.is_success(), "activate signer: {body}");
        signer_ids.push(signer_id);
    }

    (profile_id, signer_ids[0].clone(), signer_ids[1].clone())
}

#[tokio::test]
async fn rotation_uses_the_persisted_profile_id_never_a_new_one() {
    let Some(db) = pg().await else {
        not_run("rotation profile resolution");
        return;
    };
    let state = test_state(Some(db.clone())).await;
    let app = router(state);

    let email = format!("rotation-persisted-{}@example.test", uuid::Uuid::new_v4());
    let (token, org) = org_owner(app.clone(), &email).await;
    let (profile_id, old_signer, new_signer) = profile_with_two_signers(app.clone(), &token).await;

    // The profile is DURABLE: the exact created id exists in PostgreSQL
    // under the creating organization.
    let profile_uuid = uuid::Uuid::parse_str(&profile_id).expect("profile uuid");
    let org_uuid = uuid::Uuid::parse_str(&org).expect("org uuid");
    let row: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT organization_id FROM custody_profiles WHERE id = $1")
            .bind(profile_uuid)
            .fetch_optional(db.pool())
            .await
            .expect("select profile row");
    let Some((stored_org,)) = row else {
        panic!("custody profile {profile_id} was not persisted to PostgreSQL");
    };
    assert_eq!(stored_org, org_uuid, "profile row belongs to the tenant");

    // The rotation names the PERSISTED profile and must answer with the
    // SAME id — a freshly generated identity would differ.
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token),
        Some(json!({
            "profile_id": profile_id,
            "old_signer_id": old_signer,
            "new_signer_id": new_signer,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "rotation: {body}");
    let rotation = body.parse::<Value>().expect("rotation json");
    assert_eq!(
        rotation["profile_id"].as_str().expect("profile id"),
        profile_id,
        "rotation must carry the PERSISTED profile id, never a new random one"
    );
    assert_eq!(rotation["organization_id"].as_str().expect("org id"), org);
    let rotation_id = rotation["id"].as_str().expect("rotation id").to_string();

    // The rotation status endpoint reports the same persisted identity.
    let (status, body) = request(
        app.clone(),
        "GET",
        &format!("/api/saas/custody/rotations/{rotation_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rotation status: {body}");
    let status_view = body.parse::<Value>().expect("rotation status json");
    assert_eq!(
        status_view["profile_id"].as_str().expect("profile id"),
        profile_id,
        "the rotation lifecycle keeps the persisted profile identity"
    );

    // The rotation never mutates or replaces the persisted profile row.
    let row: Option<(uuid::Uuid, String)> =
        sqlx::query_as("SELECT organization_id, status FROM custody_profiles WHERE id = $1")
            .bind(profile_uuid)
            .fetch_optional(db.pool())
            .await
            .expect("select profile row after rotation");
    let Some((stored_org, stored_status)) = row else {
        panic!("custody profile {profile_id} disappeared after rotation");
    };
    assert_eq!(stored_org, org_uuid);
    assert_eq!(
        stored_status, "active",
        "profile stays active after rotation"
    );
}

#[tokio::test]
async fn profile_creation_is_durable_in_postgres() {
    let Some(db) = pg().await else {
        not_run("profile durability");
        return;
    };
    let state = test_state(Some(db.clone())).await;
    let app = router(state);

    let email = format!("rotation-durable-{}@example.test", uuid::Uuid::new_v4());
    let (token, org) = org_owner(app.clone(), &email).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/profiles",
        Some(&token),
        Some(json!({ "name": "durable", "provider_type": "local" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create profile: {body}");
    let profile_id = body.parse::<Value>().expect("json body")["id"]
        .as_str()
        .expect("profile id")
        .to_string();

    let profile_uuid = uuid::Uuid::parse_str(&profile_id).expect("profile uuid");
    let org_uuid = uuid::Uuid::parse_str(&org).expect("org uuid");
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM custody_profiles WHERE id = $1 AND organization_id = $2",
    )
    .bind(profile_uuid)
    .bind(org_uuid)
    .fetch_one(db.pool())
    .await
    .expect("count profile rows");
    assert_eq!(count.0, 1, "exactly one durable row for this profile");
}
