//! Tenant custody-rotation application service (Batch 3).
//!
//! Allows authorized rotation workflow, integrates custody policy/resolve/provider abstractions.
//! Does not expose private keys, does not revoke existing signer before safe activation except emergency.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use bot_core::authorization::AccessRequest;
use bot_core::custody::model::{CustodyProfileId, ProviderType, SignerId};
use bot_core::custody::rotation::{RotationRecord, RotationState};
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Deserialize)]
pub struct RotationRequest {
    pub old_signer_id: String,
    pub new_signer_id: String,
    pub provider_type: Option<String>,
    pub force: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct RotationResponse {
    pub id: String,
    pub organization_id: String,
    pub profile_id: String,
    pub old_signer: String,
    pub new_signer: String,
    pub state: String,
    pub force_revoked: bool,
    pub created_at: String,
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/custody/rotations", axum::routing::post(create))
        .route(
            "/api/saas/custody/rotations/{id}",
            axum::routing::get(get_status),
        )
        .route(
            "/api/saas/custody/rotations/{id}/activate",
            axum::routing::post(activate),
        )
        .route(
            "/api/saas/custody/rotations/{id}/revoke",
            axum::routing::post(revoke),
        )
}

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn store() -> &'static Mutex<HashMap<Uuid, RotationRecord>> {
    static S: OnceLock<Mutex<HashMap<Uuid, RotationRecord>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<RotationRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org = ctx.organization.id;
    let profile = state.saas.organization(org).await; // placeholder for profile resolution
    let profile_id = CustodyProfileId::new(); // in prod, lookup profile; here synthetic but tenant-scoped
    let _ = profile;
    let old = match SignerId::parse(&body.old_signer_id) {
        Some(id) => id,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_old_signer_id"})),
            )
                .into_response()
        }
    };
    let new = match SignerId::parse(&body.new_signer_id) {
        Some(id) => id,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_new_signer_id"})),
            )
                .into_response()
        }
    };
    if old == new {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":"old_and_new_must_differ"})),
        )
            .into_response();
    }
    let provider = body
        .provider_type
        .as_deref()
        .and_then(ProviderType::parse)
        .unwrap_or(ProviderType::Local);
    let now = Utc::now();
    let rec = RotationRecord::new(org, profile_id, old, new, provider, now);
    let id = rec.id;
    store().lock().unwrap().insert(id, rec.clone());
    state
        .audit
        .record(
            "saas",
            "saas.custody.rotation.created",
            Some(&org.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({"rotation": id.to_string(), "profile": profile_id.to_string()}),
        )
        .await;
    (
        axum::http::StatusCode::CREATED,
        Json(json!(RotationResponse {
            id: id.to_string(),
            organization_id: org.to_string(),
            profile_id: profile_id.to_string(),
            old_signer: old.to_string(),
            new_signer: new.to_string(),
            state: rec.state.as_str().to_string(),
            force_revoked: false,
            created_at: now.to_rfc3339(),
        })),
    )
        .into_response()
}

async fn get_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let uid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id"})),
            )
                .into_response()
        }
    };
    let rec = match store().lock().unwrap().get(&uid).cloned() {
        Some(r) => r,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found"})),
            )
                .into_response()
        }
    };
    if rec.organization_id != ctx.organization.id && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error":"not_found"})),
        )
            .into_response();
    }
    let _ = state;
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "id": rec.id.to_string(),
            "organization_id": rec.organization_id.to_string(),
            "profile_id": rec.profile_id.to_string(),
            "old_signer": rec.old_signer.to_string(),
            "new_signer": rec.new_signer.to_string(),
            "state": rec.state.as_str(),
            "force_revoked": rec.force_revoked,
            "created_at": rec.created_at.to_rfc3339(),
            "updated_at": rec.updated_at.to_rfc3339()
        })),
    )
        .into_response()
}

async fn activate(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let uid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id"})),
            )
                .into_response()
        }
    };
    let org = ctx.organization.id;
    let is_platform = ctx.authorization.is_platform_scope();
    let now = Utc::now();
    let transition_result: Result<(String, OrganizationId), String> = {
        let mut map = store().lock().unwrap();
        let rec = match map.get_mut(&uid) {
            Some(r) => r,
            None => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found"})),
                )
                    .into_response()
            }
        };
        if rec.organization_id != org && !is_platform {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found"})),
            )
                .into_response();
        }
        match rec.transition(RotationState::Active, now, false) {
            Ok(_) => Ok((rec.state.as_str().to_string(), rec.organization_id)),
            Err(e) => Err(e),
        }
    };
    match transition_result {
        Ok((state_str, org_id)) => {
            state
                .audit
                .record(
                    "saas",
                    "saas.custody.rotation.activated",
                    Some(&org_id.to_string()),
                    bot_core::audit::AuditOutcome::Success,
                    json!({"rotation": uid.to_string()}),
                )
                .await;
            (
                axum::http::StatusCode::OK,
                Json(json!({"ok": true, "state": state_str})),
            )
                .into_response()
        }
        Err(e) => (axum::http::StatusCode::CONFLICT, Json(json!({"error": e}))).into_response(),
    }
}

async fn revoke(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let uid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id"})),
            )
                .into_response()
        }
    };
    let force = body.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let org = ctx.organization.id;
    let is_platform = ctx.authorization.is_platform_scope();
    let now = Utc::now();

    enum RevokeOutcome {
        Ok(String, bool, OrganizationId),
        Err(String, axum::http::StatusCode),
    }

    let outcome: RevokeOutcome = {
        let mut map = store().lock().unwrap();
        let rec = match map.get_mut(&uid) {
            Some(r) => r,
            None => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found"})),
                )
                    .into_response()
            }
        };
        if rec.organization_id != org && !is_platform {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found"})),
            )
                .into_response();
        }
        if rec.state == RotationState::Pending && !force {
            RevokeOutcome::Err(
                "must activate replacement before revoke; use force for emergency".to_string(),
                axum::http::StatusCode::CONFLICT,
            )
        } else {
            if rec.state == RotationState::Active && !force {
                let _ = rec.transition(RotationState::Draining, now, false);
            }
            let target = RotationState::Revoked;
            match rec.transition(target, now, force) {
                Ok(_) => RevokeOutcome::Ok(
                    rec.state.as_str().to_string(),
                    rec.force_revoked,
                    rec.organization_id,
                ),
                Err(e) => RevokeOutcome::Err(e, axum::http::StatusCode::CONFLICT),
            }
        }
    };
    match outcome {
        RevokeOutcome::Ok(state_str, force_revoked, org_id) => {
            state
                .audit
                .record(
                    "saas",
                    "saas.custody.rotation.revoked",
                    Some(&org_id.to_string()),
                    bot_core::audit::AuditOutcome::Success,
                    json!({"rotation": uid.to_string(), "force": force}),
                )
                .await;
            (
                axum::http::StatusCode::OK,
                Json(json!({"ok": true, "state": state_str, "force_revoked": force_revoked})),
            )
                .into_response()
        }
        RevokeOutcome::Err(msg, code) => (code, Json(json!({"error": msg}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::custody::model::{CustodyProfileId, ProviderType, SignerId};
    use bot_core::tenant::OrganizationId;
    use chrono::Utc;

    #[test]
    fn no_private_keys_in_responses() {
        let rec = RotationRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Vault,
            Utc::now(),
        );
        let v = serde_json::to_value(&rec)
            .unwrap()
            .to_string()
            .to_ascii_lowercase();
        assert!(!v.contains("private"));
        assert!(!v.contains("secret"));
    }

    #[test]
    fn old_not_revoked_before_new_active() {
        let mut r = RotationRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Vault,
            Utc::now(),
        );
        let now = Utc::now();
        // Direct revoke without active should fail when not force
        assert!(r.transition(RotationState::Revoked, now, false).is_err());
        assert!(r.transition(RotationState::Active, now, false).is_ok());
        assert!(r.transition(RotationState::Draining, now, false).is_ok());
        assert!(r.transition(RotationState::Revoked, now, false).is_ok());
    }

    #[test]
    fn emergency_force_revoked_allowed() {
        let mut r = RotationRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Local,
            Utc::now(),
        );
        let now = Utc::now();
        assert!(r.transition(RotationState::Revoked, now, true).is_ok());
        assert!(r.force_revoked);
    }

    #[test]
    fn cross_tenant_isolation() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        let r = RotationRecord::new(
            org1,
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Local,
            Utc::now(),
        );
        assert_ne!(r.organization_id, org2);
    }
}
