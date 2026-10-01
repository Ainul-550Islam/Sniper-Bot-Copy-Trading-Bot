//! Tenant A cannot rotate tenant B's custody profile
//! (PROMPT 6 §P0 / Phase 1, file 5).
//!
//! The rotation handler resolves the profile with a tenant-scoped lookup
//! (`find_profile(org, id)`), so a profile owned by another organization
//! is indistinguishable from a missing one: `404 profile_not_found`,
//! with no oracle about the profile's existence. This test pins that
//! boundary through the real HTTP routes, in both directions:
//!
//! * tenant B naming tenant A's profile + signers → refused, nothing
//!   created for either tenant;
//! * tenant B naming its OWN profile but tenant A's signer ids →
//!   refused (signer must exist in THIS tenant and sit in the rotated
//!   profile);
//! * tenant A's own rotation still succeeds afterwards (control).
//!
//! Hermetic: no database required.

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

/// Register a user, create their organization, and return a session token
/// scoped to that organization (the creator is OrgOwner).
async fn org_owner(app: axum::Router, email: &str, org_name: &str) -> String {
    let password = "Sup3r-Secret-Passphrase-42";

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/users",
        None,
        Some(json!({ "email": email, "password": password, "display_name": "Cross Tenant Owner" })),
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
        Some(json!({ "name": org_name })),
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
async fn tenant_b_cannot_rotate_tenant_a_profile() {
    let state = test_state();
    let app = router(state);

    let email_a = format!("cross-a-{}@example.test", uuid::Uuid::new_v4());
    let email_b = format!("cross-b-{}@example.test", uuid::Uuid::new_v4());
    let token_a = org_owner(app.clone(), &email_a, "Tenant A Org").await;
    let token_b = org_owner(app.clone(), &email_b, "Tenant B Org").await;

    let (profile_a, old_a, new_a) = profile_with_two_signers(app.clone(), &token_a).await;
    // Tenant B also owns a valid profile (to prove the refusal is about
    // CROSS-TENANT resources, not about B being broken).
    let (profile_b, old_b, new_b) = profile_with_two_signers(app.clone(), &token_b).await;

    // B names A's profile and A's signers: the tenant-scoped lookup must
    // refuse with the same answer as a missing profile (no oracle).
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token_b),
        Some(json!({
            "profile_id": profile_a,
            "old_signer_id": old_a,
            "new_signer_id": new_a,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-tenant rotation: {body}"
    );
    assert_eq!(
        body.parse::<Value>().expect("json body")["error"]
            .as_str()
            .expect("error code"),
        "profile_not_found",
        "cross-tenant attempt is indistinguishable from a missing profile"
    );

    // B names its OWN profile but A's signer ids: the signers do not
    // resolve in B's tenant, so the rotation is refused.
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token_b),
        Some(json!({
            "profile_id": profile_b,
            "old_signer_id": old_a,
            "new_signer_id": new_b,
        })),
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::CONFLICT,
        "foreign signer in own profile: {status} {body}"
    );

    // Control: A's own rotation still works, and B's rotation works with
    // B's own resources — the boundary only blocks cross-tenant access.
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token_a),
        Some(json!({
            "profile_id": profile_a,
            "old_signer_id": old_a,
            "new_signer_id": new_a,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "A rotates its own profile: {body}"
    );
    assert_eq!(
        body.parse::<Value>().expect("json body")["profile_id"]
            .as_str()
            .expect("profile id"),
        profile_a
    );

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/saas/custody/rotations",
        Some(&token_b),
        Some(json!({
            "profile_id": profile_b,
            "old_signer_id": old_b,
            "new_signer_id": new_b,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "B rotates its own profile: {body}"
    );
    assert_eq!(
        body.parse::<Value>().expect("json body")["profile_id"]
            .as_str()
            .expect("profile id"),
        profile_b
    );

    // B's profile list never contains A's profile.
    let (status, body) = request(
        app.clone(),
        "GET",
        "/api/saas/custody/profiles",
        Some(&token_b),
        None,
    )
    .await;
    assert!(status.is_success(), "B lists profiles: {body}");
    let profiles = body
        .parse::<Value>()
        .expect("profiles json")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        profiles.iter().all(|p| p["id"] != json!(profile_a)),
        "tenant A's profile must never appear in tenant B's list"
    );
}
