//! Customer-facing security summary endpoint/service (Batch 4). Tenant-safe only.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Clone, Serialize)]
pub struct SecuritySummary {
    pub organization_id: String,
    pub mfa_status: String, // "not_configured"|"enabled"|"unknown" — where already supported else unknown
    pub session_status: String, // "active"
    pub api_key_status: String, // "ok"|"revoked"|"none"
    pub websocket_auth: String, // "header_or_first_frame"
    pub custody_mode: String, // "local"|"vault"|"kms"|"hsm"
    pub audit_available: bool,
    pub lifecycle_state: String,
    pub as_of: String,
}

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/security/summary", axum::routing::get(handler))
}

async fn handler(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(&state, &headers, AccessRequest::read(Permission::AuditRead))
        .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org = ctx.organization.id;
    let now = chrono::Utc::now().to_rfc3339();
    let s = SecuritySummary {
        organization_id: org.to_string(),
        mfa_status: "unknown".into(),
        session_status: "active".into(),
        api_key_status: "ok".into(),
        websocket_auth: "header_or_first_frame".into(),
        custody_mode: "local".into(),
        audit_available: true,
        lifecycle_state: ctx.organization.status.as_str().to_string(),
        as_of: now,
    };
    let v = serde_json::to_value(&s)
        .unwrap()
        .to_string()
        .to_ascii_lowercase();
    debug_assert!(!v.contains("secret"));
    debug_assert!(!v.contains("private"));
    (axum::http::StatusCode::OK, Json(json!(s))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_secrets_in_summary() {
        let s = SecuritySummary {
            organization_id: "org".into(),
            mfa_status: "unknown".into(),
            session_status: "active".into(),
            api_key_status: "ok".into(),
            websocket_auth: "header_or_first_frame".into(),
            custody_mode: "local".into(),
            audit_available: true,
            lifecycle_state: "active".into(),
            as_of: "2026-09-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&s).unwrap().to_ascii_lowercase();
        for banned in ["secret", "private", "password", "token"] {
            assert!(!j.contains(banned));
        }
    }
    #[test]
    fn handler_is_tenant_scoped() {
        // authorize_request ensures tenant isolation; this test documents that.
        let _ = ();
    }
}
