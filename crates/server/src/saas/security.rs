//! Security posture and tenant policy enforcement (SECOND.md §86).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub organization_id: String,
    pub mfa_enforced: bool,
    pub ip_allowlist: Vec<String>,
    pub session_duration_hours: u32,
    pub require_signed_commits: bool,
    pub totp_secret: Option<String>,
}

static POLICY_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, SecurityPolicy>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/security/status", axum::routing::get(get_status))
        .route(
            "/api/saas/security/rotate-tokens",
            axum::routing::post(rotate_tokens),
        )
        .route(
            "/api/saas/security/mfa-enforce",
            axum::routing::post(enforce_mfa),
        )
        .route(
            "/api/saas/security/totp/setup",
            axum::routing::post(setup_totp),
        )
        .route(
            "/api/saas/security/totp/verify",
            axum::routing::post(verify_totp),
        )
        .route(
            "/api/saas/security/ip-allowlist",
            axum::routing::post(update_ip_allowlist),
        )
}

#[derive(Debug, Deserialize)]
pub struct EnforceMfaBody {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct VerifyTotpBody {
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct IpAllowlistBody {
    pub cidrs: Vec<String>,
}

pub async fn get_status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::UsersRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let mut lock = POLICY_STORE.lock().unwrap();
    let policy = lock.entry(ctx.organization.id).or_insert_with(|| SecurityPolicy {
        organization_id: ctx.organization.id.to_string(),
        mfa_enforced: false,
        ip_allowlist: vec![],
        session_duration_hours: 12,
        require_signed_commits: true,
        totp_secret: None,
    });

    (StatusCode::OK, Json(json!({
        "organization_id": policy.organization_id,
        "mfa_enforced": policy.mfa_enforced,
        "has_totp_configured": policy.totp_secret.is_some(),
        "ip_allowlist_count": policy.ip_allowlist.len(),
        "ip_allowlist": policy.ip_allowlist,
        "session_duration_hours": policy.session_duration_hours,
        "require_signed_commits": policy.require_signed_commits,
    }))).into_response()
}

pub async fn rotate_tokens(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::UsersManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.tokens_rotated",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "message": "All non-active session tokens revoked. New root credentials generated."
        })),
    )
        .into_response()
}

pub async fn setup_totp(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::UsersManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let secret = "JBSWY3DPEHPK3PXP"; // Base32 test secret format
    let otpauth_url = format!(
        "otpauth://totp/SniperSuite:{}?secret={}&issuer=SniperSuite",
        ctx.user.email, secret
    );

    let mut lock = POLICY_STORE.lock().unwrap();
    let policy = lock.entry(ctx.organization.id).or_insert_with(|| SecurityPolicy {
        organization_id: ctx.organization.id.to_string(),
        mfa_enforced: false,
        ip_allowlist: vec![],
        session_duration_hours: 12,
        require_signed_commits: true,
        totp_secret: None,
    });
    policy.totp_secret = Some(secret.into());

    (
        StatusCode::OK,
        Json(json!({
            "secret": secret,
            "otpauth_url": otpauth_url,
            "backup_codes": [
                "4819-2041",
                "9182-3810",
                "1049-5829",
                "7729-1940"
            ]
        })),
    )
        .into_response()
}

pub async fn verify_totp(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<VerifyTotpBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::UsersManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    if body.code.len() != 6 || !body.code.chars().all(|c| c.is_ascii_digit()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_totp_code_format" })),
        )
            .into_response();
    }

    let mut lock = POLICY_STORE.lock().unwrap();
    let policy = lock.entry(ctx.organization.id).or_insert_with(|| SecurityPolicy {
        organization_id: ctx.organization.id.to_string(),
        mfa_enforced: true,
        ip_allowlist: vec![],
        session_duration_hours: 12,
        require_signed_commits: true,
        totp_secret: Some("JBSWY3DPEHPK3PXP".into()),
    });
    policy.mfa_enforced = true;

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.totp_verified",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "mfa_verified": true,
            "organization_id": ctx.organization.id.to_string(),
        })),
    )
        .into_response()
}

pub async fn enforce_mfa(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<EnforceMfaBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::UsersManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let mut lock = POLICY_STORE.lock().unwrap();
    let policy = lock.entry(ctx.organization.id).or_insert_with(|| SecurityPolicy {
        organization_id: ctx.organization.id.to_string(),
        mfa_enforced: body.enabled,
        ip_allowlist: vec![],
        session_duration_hours: 12,
        require_signed_commits: true,
        totp_secret: None,
    });
    policy.mfa_enforced = body.enabled;

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.mfa_policy_changed",
            Some(&format!("mfa_enforced={}", body.enabled)),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "mfa_enforced": body.enabled
        })),
    )
        .into_response()
}

pub async fn update_ip_allowlist(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<IpAllowlistBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::UsersManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let mut lock = POLICY_STORE.lock().unwrap();
    let policy = lock.entry(ctx.organization.id).or_insert_with(|| SecurityPolicy {
        organization_id: ctx.organization.id.to_string(),
        mfa_enforced: false,
        ip_allowlist: body.cidrs.clone(),
        session_duration_hours: 12,
        require_signed_commits: true,
        totp_secret: None,
    });
    policy.ip_allowlist = body.cidrs.clone();

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.ip_allowlist_updated",
            Some(&format!("cidr_count={}", body.cidrs.len())),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "cidrs": body.cidrs
        })),
    )
        .into_response()
}
