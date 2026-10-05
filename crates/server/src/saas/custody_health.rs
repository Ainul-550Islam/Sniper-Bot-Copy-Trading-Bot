//! SaaS custody readiness metadata.
//!
//! This endpoint reports durable custody configuration and lifecycle state. It
//! does not claim a provider is reachable unless a separate provider health
//! probe has supplied that evidence; configured signers therefore remain
//! blocked here until a real probe is recorded by the signing boundary.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/custody/health", axum::routing::get(list_health))
        .route(
            "/api/saas/custody/health/:id",
            axum::routing::get(get_signer_health),
        )
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "custody_storage_unavailable",
            "detail": "custody health requires an attached PostgreSQL database",
        })),
    )
        .into_response()
}

fn provider_state(
    profile_count: i64,
    active_profiles: i64,
    active_signers: i64,
    revoked_profiles: i64,
) -> (&'static str, String) {
    if revoked_profiles == profile_count && profile_count > 0 {
        (
            "revoked",
            "all tenant custody profiles for this provider are revoked or closed".to_string(),
        )
    } else if active_profiles > 0 && active_signers > 0 {
        ("configured", "active custody records exist; provider reachability has not been probed by this endpoint".to_string())
    } else {
        (
            "unavailable",
            "no active custody profile and signer pair is recorded".to_string(),
        )
    }
}

async fn list_health(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(value) => value,
        Err(denial) => return deny_response(&state, &denial).await,
    };
    let Some(db) = state.db.as_deref() else {
        return unavailable();
    };
    let rows = match sqlx::query(
        "SELECT cp.provider_type,
                COUNT(DISTINCT cp.id)::bigint AS profile_count,
                COUNT(DISTINCT cp.id) FILTER (WHERE cp.status = 'active')::bigint AS active_profiles,
                COUNT(cs.id) FILTER (WHERE cs.status = 'active' AND cp.status = 'active')::bigint AS active_signers,
                COUNT(DISTINCT cp.id) FILTER (WHERE cp.status IN ('revoked', 'closed'))::bigint AS revoked_profiles
           FROM custody_profiles cp
           LEFT JOIN custody_signers cs ON cs.custody_profile_id = cp.id
          WHERE cp.organization_id = $1
          GROUP BY cp.provider_type
          ORDER BY cp.provider_type",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant custody health metadata");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "custody_storage_error", "detail": "custody health could not be loaded" })),
            )
                .into_response();
        }
    };

    let providers: Vec<_> = rows
        .into_iter()
        .map(|row| {
            let provider_type: String = row.get("provider_type");
            let profile_count = row.get::<i64, _>("profile_count");
            let active_profiles = row.get::<i64, _>("active_profiles");
            let active_signers = row.get::<i64, _>("active_signers");
            let revoked_profiles = row.get::<i64, _>("revoked_profiles");
            let (state_name, detail) = provider_state(
                profile_count,
                active_profiles,
                active_signers,
                revoked_profiles,
            );
            json!({
                "provider_type": provider_type,
                "state": state_name,
                "detail": detail,
                "signing_allowed": false,
                "checked_at": serde_json::Value::Null,
                "evidence_level": "configuration_only",
            })
        })
        .collect();

    state
        .audit
        .record(
            "saas",
            "saas.custody.health.list",
            Some(&ctx.organization.id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({ "organization": ctx.organization.id.to_string(), "providers": providers.len() }),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "providers": providers,
            "generated_at": Utc::now(),
            "evidence": "durable custody records only; provider reachability requires a separate health probe",
        })),
    )
        .into_response()
}

async fn get_signer_health(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(value) => value,
        Err(denial) => return deny_response(&state, &denial).await,
    };
    let Some(db) = state.db.as_deref() else {
        return unavailable();
    };
    let signer_id = match Uuid::parse_str(&id) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "detail": "signer was not found" })),
            )
                .into_response()
        }
    };
    let row = match sqlx::query(
        "SELECT cs.id, cs.provider_type, cs.status AS signer_status, cp.status AS profile_status
           FROM custody_signers cs
           JOIN custody_profiles cp ON cp.id = cs.custody_profile_id
          WHERE cs.id = $1 AND cs.organization_id = $2 AND cp.organization_id = $2",
    )
    .bind(signer_id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant signer health metadata");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "custody_storage_error", "detail": "signer health could not be loaded" }))).into_response();
        }
    };
    let Some(row) = row else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "detail": "signer was not found" })),
        )
            .into_response();
    };
    let signer_status: String = row.get("signer_status");
    let profile_status: String = row.get("profile_status");
    let state_name = if signer_status == "revoked"
        || signer_status == "closed"
        || profile_status == "revoked"
        || profile_status == "closed"
    {
        "revoked"
    } else if signer_status == "active" && profile_status == "active" {
        "configured"
    } else {
        "unavailable"
    };
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "signer_id": signer_id,
            "provider_type": row.get::<String, _>("provider_type"),
            "state": state_name,
            "detail": "durable signer metadata loaded; provider reachability has not been probed",
            "signing_allowed": false,
            "checked_at": serde_json::Value::Null,
            "evidence_level": "configuration_only",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_does_not_allow_signing() {
        let (state, detail) = provider_state(1, 1, 1, 0);
        assert_eq!(state, "configured");
        assert!(detail.contains("not been probed"));
    }

    #[test]
    fn absent_configuration_is_unavailable() {
        let (state, _) = provider_state(1, 0, 0, 0);
        assert_eq!(state, "unavailable");
    }
}
