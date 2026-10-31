//! Two-pool proof that custody reads use PostgreSQL as their authority.
//!
//! Writes (profile creation, signer rotation and profile revocation) go through
//! replica A's real HTTP router. Replica B owns a separate `Database` pool and
//! must observe every transition immediately. A second tenant sees no profile
//! and receives the same not-found response for an explicitly named foreign id.

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::{AppConfig, DatabaseConfig};
use bot_core::db::Database;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;
use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::SaasStore;

fn required_url(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} is required when REQUIRE_SERVICES=1"))
}

async fn replica() -> Option<Arc<Database>> {
    let url = match std::env::var("POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ if std::env::var("REQUIRE_SERVICES").as_deref() == Ok("1") => {
            panic!("POSTGRES_URL is required when REQUIRE_SERVICES=1")
        }
        _ => {
            eprintln!("NOT_RUN: custody_db_authoritative — PostgreSQL service is not configured");
            return None;
        }
    };
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..DatabaseConfig::default()
    };
    let database = Database::connect(&cfg, &url)
        .await
        .expect("connect an independent replica pool and migrate");
    database.migrate().await.expect("apply migrations");
    Some(Arc::new(database))
}

async fn test_state(db: Arc<Database>) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    let saas = SaasStore::with_database(Some(db.clone()))
        .await
        .expect("construct Postgres-backed SaaS store");
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        trusted_proxies: Default::default(),
        db: Some(db),
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
            .expect("JSON request"),
        None => builder.body(Body::empty()).expect("empty request"),
    };
    let response = app.oneshot(request).await.expect("router response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn create_owner(app: axum::Router, email: &str) -> (String, String) {
    let password = "Custody-Replica-Test-Passphrase-42";
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/users",
        None,
        Some(json!({ "email": email, "password": password, "display_name": "Replica Owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register owner: {body}");

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/sessions",
        None,
        Some(json!({ "email": email, "password": password })),
    )
    .await;
    assert!(status.is_success(), "unscoped login: {body}");
    let unscoped = body.parse::<Value>().expect("login response JSON")["token"]
        .as_str()
        .expect("unscoped token")
        .to_owned();

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/organizations",
        Some(&unscoped),
        Some(json!({
            "name": format!("Custody Replica Tenant {}", uuid::Uuid::new_v4()),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create tenant: {body}");
    let organization = body.parse::<Value>().expect("organization JSON");
    let org_id = organization["organization"]["id"]
        .as_str()
        .expect("organization id")
        .to_owned();
    let slug = organization["organization"]["slug"]
        .as_str()
        .expect("organization slug")
        .to_owned();

    let (status, body) = request(
        app,
        "POST",
        "/api/saas/sessions",
        None,
        Some(json!({ "email": email, "password": password, "organization": slug })),
    )
    .await;
    assert!(status.is_success(), "tenant-scoped login: {body}");
    let token = body.parse::<Value>().expect("scoped session JSON")["token"]
        .as_str()
        .expect("scoped token")
        .to_owned();
    (token, org_id)
}

fn public_address(seed: u8) -> String {
    let mut bytes: Vec<char> = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        .chars()
        .collect();
    bytes[3] = char::from(b'a' + seed % 26);
    bytes.into_iter().collect()
}

async fn create_signer(
    app: axum::Router,
    token: &str,
    profile_id: &str,
    identity: &str,
    seed: u8,
) -> String {
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/signers",
        Some(token),
        Some(json!({
            "custody_profile_id": profile_id,
            "logical_identity": identity,
            "public_address": public_address(seed),
            "capabilities": ["solana.sign"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create signer: {body}");
    let id = body.parse::<Value>().expect("signer JSON")["id"]
        .as_str()
        .expect("signer id")
        .to_owned();
    let (status, body) = request(
        app,
        "POST",
        &format!("/api/saas/custody/signers/{id}/activate"),
        Some(token),
        None,
    )
    .await;
    assert!(status.is_success(), "activate signer: {body}");
    id
}

#[tokio::test]
async fn custody_mutations_are_immediately_visible_across_replica_pools() {
    let Some(db_a) = replica().await else { return };
    let Some(db_b) = replica().await else { return };
    assert_ne!(db_a.pool().size(), 0);
    assert_ne!(db_b.pool().size(), 0);
    let app_a = router(test_state(db_a.clone()).await);
    let app_b = router(test_state(db_b.clone()).await);

    let email_a = format!("custody-replica-a-{}@example.test", uuid::Uuid::new_v4());
    let (token_a, org_a) = create_owner(app_a.clone(), &email_a).await;
    let (status, body) = request(
        app_a.clone(),
        "POST",
        "/api/saas/custody/profiles",
        Some(&token_a),
        Some(json!({ "name": "replica-owned", "provider_type": "local" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create profile on replica A: {body}"
    );
    let profile_id = body.parse::<Value>().expect("profile JSON")["id"]
        .as_str()
        .expect("profile id")
        .to_owned();

    // Replica B has a separate pool and a separately constructed SaaS store.
    // Its list response is loaded from the durable tables, not an A-local map.
    let (status, body) = request(
        app_b.clone(),
        "GET",
        "/api/saas/custody/profiles",
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replica B profile read: {body}");
    let profiles = body.parse::<Value>().expect("profile list JSON");
    let profile = profiles["profiles"]
        .as_array()
        .expect("profiles array")
        .iter()
        .find(|entry| entry["id"] == profile_id)
        .expect("replica B sees the newly created profile");
    assert_eq!(profile["status"], "pending");

    let (status, body) = request(
        app_a.clone(),
        "POST",
        &format!("/api/saas/custody/profiles/{profile_id}/activate"),
        Some(&token_a),
        None,
    )
    .await;
    assert!(status.is_success(), "activate on replica A: {body}");
    let old_signer = create_signer(app_a.clone(), &token_a, &profile_id, "replica-old", 1).await;
    let new_signer = create_signer(app_a.clone(), &token_a, &profile_id, "replica-new", 2).await;

    let (status, body) = request(
        app_a.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token_a),
        Some(json!({
            "profile_id": profile_id,
            "old_signer_id": old_signer,
            "new_signer_id": new_signer,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "start signer rotation: {body}");
    let rotation = body.parse::<Value>().expect("rotation JSON");
    assert_eq!(rotation["profile_id"], profile_id);
    let rotation_id = rotation["id"].as_str().expect("rotation id").to_owned();

    let (status, body) = request(
        app_b.clone(),
        "GET",
        "/api/saas/custody/profiles",
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "replica B refreshed profile read: {body}"
    );
    let profiles = body.parse::<Value>().expect("refreshed profile JSON");
    let profile = profiles["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == profile_id)
        .expect("replica B sees the created profile");
    assert_eq!(profile["status"], "active");
    assert_eq!(profile["public_signers"].as_array().unwrap().len(), 2);
    let (status, body) = request(
        app_b.clone(),
        "GET",
        &format!("/api/saas/custody/rotations/{rotation_id}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replica B rotation read: {body}");
    assert_eq!(body.parse::<Value>().unwrap()["id"], rotation_id);

    // Revoke the rotation and profile through replica A. No restart or cache
    // invalidation RPC is needed: replica B's next authenticated read is fresh.
    let (status, body) = request(
        app_a.clone(),
        "POST",
        &format!("/api/saas/custody/rotations/{rotation_id}/revoke"),
        Some(&token_a),
        Some(json!({ "force": true })),
    )
    .await;
    assert!(status.is_success(), "revoke rotation on replica A: {body}");
    let (status, body) = request(
        app_a.clone(),
        "POST",
        &format!("/api/saas/custody/profiles/{profile_id}/revoke"),
        Some(&token_a),
        None,
    )
    .await;
    assert!(status.is_success(), "revoke profile on replica A: {body}");

    let (status, body) = request(
        app_b.clone(),
        "GET",
        "/api/saas/custody/profiles",
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replica B sees revocation: {body}");
    let profiles = body.parse::<Value>().expect("revoked profile JSON");
    let profile = profiles["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == profile_id)
        .expect("replica B still sees the revoked profile");
    assert_eq!(profile["status"], "revoked");
    let (status, body) = request(
        app_b.clone(),
        "GET",
        &format!("/api/saas/custody/rotations/{rotation_id}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "replica B sees rotation revocation: {body}"
    );
    assert_eq!(body.parse::<Value>().unwrap()["state"], "revoked");

    // A second authenticated tenant cannot see A's profile through the list
    // or mutate it by its known UUID (no cross-tenant existence oracle).
    let email_b = format!("custody-replica-b-{}@example.test", uuid::Uuid::new_v4());
    let (token_b, org_b) = create_owner(app_a.clone(), &email_b).await;
    assert_ne!(
        org_a, org_b,
        "cross-tenant fixture must create distinct tenants"
    );
    let (status, body) = request(
        app_b.clone(),
        "GET",
        "/api/saas/custody/profiles",
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "cross-tenant list is scoped: {body}"
    );
    assert!(
        body.parse::<Value>().unwrap()["profiles"]
            .as_array()
            .unwrap()
            .is_empty(),
        "cross-tenant list leaked another tenant's profile: {body}"
    );
    let (status, body) = request(
        app_b.clone(),
        "POST",
        &format!("/api/saas/custody/profiles/{profile_id}/revoke"),
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-tenant mutation must not reveal or change the profile: {body}"
    );

    // Close both test tenants after the HTTP checks; cascading foreign keys
    // remove the account, session, custody, and rotation rows they created.
    let cleanup = PgPoolOptions::new()
        .max_connections(2)
        .connect(&required_url("POSTGRES_URL"))
        .await
        .expect("connect cleanup pool");
    let org_a = uuid::Uuid::parse_str(&org_a).expect("first tenant UUID");
    let org_b = uuid::Uuid::parse_str(&org_b).expect("second tenant UUID");
    let mut cleanup_tx = cleanup
        .begin()
        .await
        .expect("begin test cleanup transaction");
    for org in [org_a, org_b] {
        // Custody tables intentionally RESTRICT tenant deletion. Remove this
        // test's durable graph in dependency order rather than weakening those
        // production foreign keys.
        sqlx::query("DELETE FROM custody_rotations WHERE organization_id = $1")
            .bind(org)
            .execute(&mut *cleanup_tx)
            .await
            .expect("cleanup test rotations");
        sqlx::query("DELETE FROM custody_audit WHERE organization_id = $1")
            .bind(org)
            .execute(&mut *cleanup_tx)
            .await
            .expect("cleanup test custody audit");
        sqlx::query("DELETE FROM custody_signers WHERE organization_id = $1")
            .bind(org)
            .execute(&mut *cleanup_tx)
            .await
            .expect("cleanup test signers");
        sqlx::query("DELETE FROM custody_profiles WHERE organization_id = $1")
            .bind(org)
            .execute(&mut *cleanup_tx)
            .await
            .expect("cleanup test profiles");
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(org)
            .execute(&mut *cleanup_tx)
            .await
            .expect("cleanup test tenant");
    }
    for email in [&email_a, &email_b] {
        sqlx::query("DELETE FROM users WHERE email = $1")
            .bind(email)
            .execute(&mut *cleanup_tx)
            .await
            .expect("cleanup test user");
    }
    cleanup_tx
        .commit()
        .await
        .expect("commit test cleanup transaction");
    cleanup.close().await;
    db_a.close().await;
    db_b.close().await;
}
