//! Missing-profile custody rotation must fail closed and must not mutate
//! rotation state (PROMPT 6 §P0 / Phase 1, file 4).
//!
//! The historic defect (AUDIT §13.6, fixed in PROMPT 5 and now pinned by
//! this test) invented a synthetic `CustodyProfileId` for every rotation,
//! which meant a request naming a NONEXISTENT profile could never be
//! distinguished from a valid one. After the fix the contract is:
//!
//! * a well-formed but unknown `profile_id` → `404 profile_not_found`
//!   (the same answer as a profile owned by another tenant — no oracle);
//! * no rotation record is created (the rotation id space is untouched);
//! * no synthetic custody profile appears in the tenant's profile list;
//! * a subsequent VALID rotation still works exactly as before.
//!
//! Hermetic: no database required; the in-process stores carry the truth.

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{json, Value};
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

fn address(seed: u8) -> String {
    let body = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let mut chars: Vec<char> = body.chars().collect();
    chars[3] = char::from(b'a' + (seed % 26));
    chars.into_iter().collect()
}

async fn org_owner(app: axum::Router, email: &str) -> String {
    let password = "Sup3r-Secret-Passphrase-42";

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/users",
        None,
        Some(json!({ "email": email, "password": password, "display_name": "Missing Profile Owner" })),
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
    let unscoped = body.parse::<Value>().expect("json body")["token"]
        .as_str()
        .expect("token")
        .to_string();

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/organizations",
        Some(&unscoped),
        Some(json!({ "name": "Missing Profile Org" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create org: {body}");
    let slug = body.parse::<Value>().expect("json body")["organization"]["slug"]
        .as_str()
        .expect("organization slug")
        .to_string();

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/sessions",
        None,
        Some(json!({ "email": email, "password": password, "organization": slug })),
    )
    .await;
    assert!(status.is_success(), "scoped login: {body}");
    body.parse::<Value>().expect("json body")["token"]
        .as_str()
        .expect("scoped token")
        .to_string()
}

/// Create + activate a profile with two active signers.
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
async fn missing_profile_fails_closed_without_mutating_state() {
    let state = test_state();
    let app = router(state);

    let email = format!("rotation-missing-{}@example.test", uuid::Uuid::new_v4());
    let token = org_owner(app.clone(), &email).await;

    // A well-formed uuid that names NO profile of this tenant.
    let ghost_profile = uuid::Uuid::new_v4().to_string();
    let any_signer = uuid::Uuid::new_v4().to_string();

    // Before: a random rotation id is unknown.
    let (status, _) = request(
        app.clone(),
        "GET",
        &format!("/api/saas/custody/rotations/{}", uuid::Uuid::new_v4()),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown rotation id");

    // The missing-profile rotation attempt: typed fail-closed refusal.
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token),
        Some(json!({
            "profile_id": ghost_profile,
            "old_signer_id": any_signer,
            "new_signer_id": any_signer,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "missing profile: {body}");
    assert_eq!(
        body.parse::<Value>().expect("json body")["error"]
            .as_str()
            .expect("error code"),
        "profile_not_found",
        "typed, fail-closed error — no invented profile"
    );
    // The response carries NO rotation identity (nothing was created).
    let parsed = body.parse::<Value>().expect("refusal json");
    assert!(
        parsed["id"].is_null() && parsed["rotation"].is_null(),
        "refusal must not return a rotation id: {body}"
    );

    // After: the same random rotation id is STILL unknown — and the
    // tenant's profile list carries no synthetic profile from the attempt.
    let (status, _) = request(
        app.clone(),
        "GET",
        &format!("/api/saas/custody/rotations/{}", uuid::Uuid::new_v4()),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "rotation store untouched");

    let (status, body) = request(
        app.clone(),
        "GET",
        "/api/saas/custody/profiles",
        Some(&token),
        None,
    )
    .await;
    assert!(status.is_success(), "list profiles: {body}");
    let profiles = body
        .parse::<Value>()
        .expect("profiles json")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let matching: Vec<&Value> = profiles
        .iter()
        .filter(|p| p["id"] == json!(ghost_profile))
        .collect();
    assert!(
        matching.is_empty(),
        "no synthetic profile may appear from a refused rotation"
    );

    // A subsequent VALID rotation still works: the refusal was not sticky.
    let (profile_id, old_signer, new_signer) = profile_with_two_signers(app.clone(), &token).await;
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
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid rotation after refusal: {body}"
    );
    assert_eq!(
        body.parse::<Value>().expect("json body")["profile_id"]
            .as_str()
            .expect("profile id"),
        profile_id
    );
}

#[tokio::test]
async fn malformed_profile_id_is_a_bad_request_not_a_lookup() {
    let state = test_state();
    let app = router(state);

    let email = format!("rotation-malformed-{}@example.test", uuid::Uuid::new_v4());
    let token = org_owner(app.clone(), &email).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token),
        Some(json!({
            "profile_id": "not-a-uuid",
            "old_signer_id": uuid::Uuid::new_v4().to_string(),
            "new_signer_id": uuid::Uuid::new_v4().to_string(),
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "malformed profile id: {body}"
    );
    assert_eq!(
        body.parse::<Value>().expect("json body")["error"]
            .as_str()
            .expect("error code"),
        "invalid_profile_id"
    );
}
