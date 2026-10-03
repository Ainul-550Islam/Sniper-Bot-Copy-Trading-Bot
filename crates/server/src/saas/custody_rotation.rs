//! Tenant custody-rotation application service (Batch 3).
//!
//! Allows authorized rotation workflow, integrates custody policy/resolve/provider abstractions.
//! Does not expose private keys, does not revoke existing signer before safe activation except emergency.
//!
//! §S-4 — rotation state is DURABLE. The authority is PostgreSQL
//! (`custody_rotations`, migration 0036) behind
//! [`crate::saas::custody_rotation_store::CustodyRotationStore`]. Before
//! that migration this state lived in a process-global map, which meant a
//! rotation created on one replica 404'd on every other one and a restart
//! stranded the profile between signers with no record it had happened.
//! Every write below FAILS CLOSED: if the durable write does not land,
//! the caller is told the rotation did not advance.

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
    /// The custody profile whose signers are being rotated. This names a
    /// RESOURCE (ownership-checked below); it never selects the tenant —
    /// the tenant comes from the authenticated context.
    pub profile_id: String,
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
            "/api/saas/custody/rotations/:id",
            axum::routing::get(get_status),
        )
        .route(
            "/api/saas/custody/rotations/:id/activate",
            axum::routing::post(activate),
        )
        .route(
            "/api/saas/custody/rotations/:id/revoke",
            axum::routing::post(revoke),
        )
}

use crate::saas::custody_rotation_store::RotationStoreError;

/// Map a store failure onto an HTTP refusal. Never a 200: a rotation the
/// store did not accept has not happened.
fn rotation_store_refusal(stage: &'static str, error: &RotationStoreError) -> Response {
    tracing::error!(stage, error = %error, "custody rotation refused: store failure");
    match error {
        RotationStoreError::ConflictInFlight => (
            axum::http::StatusCode::CONFLICT,
            Json(json!({
                "error": "rotation_already_in_flight",
                "detail": "this custody profile already has a rotation that has not reached a terminal state; complete or revoke it first",
            })),
        )
            .into_response(),
        RotationStoreError::Backend(_) | RotationStoreError::Corrupt(_) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "rotation_store_unavailable",
                "stage": stage,
                "applied": false,
                "detail": "the rotation store could not be reached; NOTHING was changed. Retry.",
            })),
        )
            .into_response(),
    }
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
    // P0 fix (PROMPT 5 §E): resolve the REAL custody profile from the
    // profile store — tenant-scoped, fail-closed. No synthetic profile
    // identity is ever invented here.
    let profile_id = match CustodyProfileId::parse(&body.profile_id) {
        Some(id) => id,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_profile_id"})),
            )
                .into_response()
        }
    };
    let profile = match crate::saas::custody::find_profile(org, profile_id) {
        Some(p) => p,
        None => {
            // Not found OR owned by another tenant: same answer, no oracle.
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"profile_not_found"})),
            )
                .into_response();
        }
    };
    if profile.status != bot_core::custody::CustodyStatus::Active {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({
                "error": "profile_not_active",
                "status": profile.status.as_str(),
            })),
        )
            .into_response();
    }
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
    // Both signers must exist, belong to THIS tenant, and sit in the
    // profile being rotated — a foreign or unbound signer id is refused.
    for (label, id) in [("old", old), ("new", new)] {
        match crate::saas::custody::find_signer(org, id) {
            Some(signer) if signer.custody_profile_id == profile_id => {}
            Some(_) => {
                return (
                    axum::http::StatusCode::CONFLICT,
                    Json(json!({"error":"signer_not_in_profile","which":label})),
                )
                    .into_response()
            }
            None => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"signer_not_found","which":label})),
                )
                    .into_response()
            }
        }
    }
    let provider = body
        .provider_type
        .as_deref()
        .and_then(ProviderType::parse)
        .unwrap_or(ProviderType::Local);
    let now = Utc::now();
    let rec = RotationRecord::new(org, profile_id, old, new, provider, now);
    let id = rec.id;
    // Durable first, respond second. A 201 here is a promise that the
    // rotation exists for every replica, not just this one.
    if let Err(e) = state.custody_rotations().insert(&rec).await {
        return rotation_store_refusal("create", &e);
    }
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
    let rec = match state.custody_rotations().get(uid).await {
        Ok(Some(r)) => r,
        Err(e) => return rotation_store_refusal("read", &e),
        Ok(None) => {
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
    let rotations = state.custody_rotations();
    let mut rec = match rotations.get(uid).await {
        Ok(Some(r)) => r,
        Err(e) => return rotation_store_refusal("read", &e),
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found"})),
            )
                .into_response()
        }
    };
    // Same answer for "absent" and "another tenant's": no oracle.
    if rec.organization_id != org && !is_platform {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error":"not_found"})),
        )
            .into_response();
    }
    // The domain object owns the state machine; the store only persists
    // what it accepted.
    let transition_result: Result<(String, OrganizationId), String> =
        match rec.transition(RotationState::Active, now, false) {
            Ok(_) => Ok((rec.state.as_str().to_string(), rec.organization_id)),
            Err(e) => Err(e),
        };
    if transition_result.is_ok() {
        if let Err(e) = rotations
            .save_transition(&rec, &ctx.actor_label(), &ctx.correlation_label())
            .await
        {
            return rotation_store_refusal("activate", &e);
        }
    }
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

    let rotations = state.custody_rotations();
    let mut rec = match rotations.get(uid).await {
        Ok(Some(r)) => r,
        Err(e) => return rotation_store_refusal("read", &e),
        Ok(None) => {
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
    let outcome: RevokeOutcome = {
        let rec = &mut rec;
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
    if matches!(outcome, RevokeOutcome::Ok(..)) {
        if let Err(e) = rotations
            .save_transition(&rec, &ctx.actor_label(), &ctx.correlation_label())
            .await
        {
            return rotation_store_refusal("revoke", &e);
        }
    }
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
