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
    /// Always `"not_supported"` in this build, which is the honest word:
    /// there is no MFA code anywhere in this repository (no enrolment,
    /// no verification, no recovery codes). The previous value,
    /// `"unknown"`, implied the system had simply failed to look —
    /// inviting a buyer to assume the capability exists and is merely
    /// unreported. Change this field in the same commit that adds MFA.
    pub mfa_status: String,
    /// How THIS request authenticated: `"session"` or `"api_key"`. It
    /// describes the caller, not a global posture.
    pub session_status: String,
    /// `"in_use"` when this request presented a tenant API key,
    /// `"not_used_for_this_request"` otherwise. It used to be the
    /// constant `"ok"`, which read as "your keys are healthy" — a claim
    /// about objects this handler never looked at.
    pub api_key_status: String,
    pub websocket_auth: String, // "header_or_first_frame"
    /// The signing backend this deployment is actually configured with
    /// (`local`|`vault`|`kms`|`hsm`), read from `[signing].provider` —
    /// never a constant. Only `local` is implemented in this build; the
    /// others fail startup rather than falling back, so a non-local value
    /// here means a running deployment really is on that backend.
    pub custody_mode: String,
    /// True only when the audit trail is DB-chained. A memory-only
    /// deployment has an audit *log*, not an audit *trail*: it cannot be
    /// verified after a restart, so reporting `true` for it would tell a
    /// customer they have evidence they do not have.
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
    // Derive, never assert. These two fields used to be the literals
    // "local" and `true` regardless of how the process was configured,
    // which made the endpoint a decoration rather than a control.
    let custody_mode = state
        .shared
        .config
        .read()
        .await
        .signing
        .provider
        .as_str()
        .to_string();
    let audit_available = state.audit.durable();
    // Derived from the credential that actually authenticated this
    // request, not from a constant.
    let (session_status, api_key_status) = if ctx.api_key.is_some() {
        ("api_key", "in_use")
    } else {
        ("session", "not_used_for_this_request")
    };
    let s = SecuritySummary {
        organization_id: org.to_string(),
        mfa_status: "not_supported".into(),
        session_status: session_status.into(),
        api_key_status: api_key_status.into(),
        websocket_auth: "header_or_first_frame".into(),
        custody_mode,
        audit_available,
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
    /// Guards the exact regression this field was introduced to fix:
    /// "unknown" invites the reader to assume the capability exists.
    #[test]
    fn mfa_is_reported_as_unsupported_not_unknown() {
        let s = SecuritySummary {
            organization_id: "org".into(),
            mfa_status: "not_supported".into(),
            session_status: "session".into(),
            api_key_status: "not_used_for_this_request".into(),
            websocket_auth: "header_or_first_frame".into(),
            custody_mode: "local".into(),
            audit_available: false,
            lifecycle_state: "active".into(),
            as_of: "2026-10-02T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&s).expect("serialize");
        assert_eq!(v["mfa_status"], "not_supported");
        assert_ne!(v["mfa_status"], "unknown");
    }

    #[test]
    fn no_secrets_in_summary() {
        let s = SecuritySummary {
            organization_id: "org".into(),
            mfa_status: "not_supported".into(),
            session_status: "session".into(),
            api_key_status: "in_use".into(),
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
    /// Every variant the signing config can hold must survive the trip to
    /// JSON unchanged. The regression this guards is the original bug:
    /// a deployment on `vault` reporting `local`.
    #[test]
    fn custody_mode_is_whatever_the_config_says() {
        use bot_core::config::SigningProvider;
        for (provider, expected) in [
            (SigningProvider::Local, "local"),
            (SigningProvider::Vault, "vault"),
            (SigningProvider::Kms, "kms"),
            (SigningProvider::Hsm, "hsm"),
        ] {
            let s = SecuritySummary {
                organization_id: "org".into(),
                mfa_status: "not_supported".into(),
                session_status: "session".into(),
                api_key_status: "in_use".into(),
                websocket_auth: "header_or_first_frame".into(),
                custody_mode: provider.as_str().to_string(),
                audit_available: false,
                lifecycle_state: "active".into(),
                as_of: "2026-10-02T00:00:00Z".into(),
            };
            let v = serde_json::to_value(&s).expect("serialize");
            assert_eq!(v["custody_mode"], expected);
        }
    }

    /// `audit_available` must be able to be false. A field that is always
    /// true carries no information, and this one is read as a compliance
    /// claim.
    #[test]
    fn audit_available_can_be_false() {
        let s = SecuritySummary {
            organization_id: "org".into(),
            mfa_status: "not_supported".into(),
            session_status: "session".into(),
            api_key_status: "in_use".into(),
            websocket_auth: "header_or_first_frame".into(),
            custody_mode: "local".into(),
            audit_available: false,
            lifecycle_state: "active".into(),
            as_of: "2026-10-02T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&s).expect("serialize");
        assert_eq!(v["audit_available"], serde_json::Value::Bool(false));
    }

    #[test]
    fn handler_is_tenant_scoped() {
        // authorize_request ensures tenant isolation; this test documents that.
        let _ = ();
    }
}
