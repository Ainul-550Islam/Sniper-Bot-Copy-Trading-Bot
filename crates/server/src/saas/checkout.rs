//! HTTP handler for tenant checkout initiation (BATCH file 12).
//!
//! Uses existing SaaS authentication/RBAC/tenant context. Accepts only server-known plan code.
//! Requires idempotency. Returns provider-neutral checkout response. Never returns provider secret.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use bot_core::authorization::AccessRequest;
use bot_core::billing::plan::PlanCode;
use bot_core::billing::provider::BillingProviderKind;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

/// Checkout request body — server-known plan_code only, no amount.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckoutRequest {
    pub plan_code: String,
    pub provider: Option<String>,
    pub idempotency_key: String,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckoutResponse {
    pub id: String,
    pub organization_id: String,
    pub plan_code: String,
    pub provider: String,
    pub status: String,
    pub checkout_url: Option<String>,
    pub instructions: Option<String>,
    pub expires_at: Option<String>,
}

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/checkout", axum::routing::post(create_checkout))
}

pub async fn create_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CheckoutRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::billing(Permission::BillingRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(decision) => return deny_response(&state, &decision).await,
    };

    // Only server-known plan codes
    let plan_code = match PlanCode::parse(&body.plan_code) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":"invalid_plan","reason":"unknown plan_code"})),
            )
                .into_response()
        }
    };

    // Provider: explicit parse, no silent fallback to Manual when client supplied unknown provider
    let provider = match &body.provider {
        None => BillingProviderKind::Manual,
        Some(p) => match BillingProviderKind::parse(p) {
            Some(k) => k,
            None => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error":"unsupported_provider","reason": format!("unknown provider '{}'", p)})),
                )
                    .into_response()
            }
        },
    };

    // Require idempotency key
    if body.idempotency_key.trim().is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"missing_idempotency_key","reason":"idempotency_key is required"}))).into_response();
    }
    if body.idempotency_key.len() > 128 {
        return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid_idempotency_key","reason":"idempotency_key too long"}))).into_response();
    }

    let now = Utc::now();
    // Idempotency: check if already exists for this org+key (durable when DB attached)
    if let Some(existing) = crate::saas::billing::BillingService::find_checkout(
        &state,
        ctx.organization.id,
        &body.idempotency_key,
    )
    .await
    {
        let resp = CheckoutResponse {
            id: existing.id.to_string(),
            organization_id: existing.organization_id.to_string(),
            plan_code: existing.plan_code.as_str().into(),
            provider: existing.provider.as_str().into(),
            status: existing.status.as_str().into(),
            checkout_url: existing.checkout_url.clone(),
            instructions: Some(format!(
                "manual provider: assign the {} plan via operator flow; no hosted checkout",
                existing.plan_code.as_str()
            )),
            expires_at: existing.expires_at.map(|t| t.to_rfc3339()),
        };
        return (axum::http::StatusCode::OK, Json(serde_json::json!(resp))).into_response();
    }

    match crate::saas::billing::BillingService::create_checkout(
        &state,
        ctx.organization.id,
        plan_code,
        provider,
        body.idempotency_key.clone(),
        body.success_url.clone(),
        body.cancel_url.clone(),
        now,
    )
    .await
    {
        Ok(rec) => {
            let resp = CheckoutResponse {
                id: rec.id.to_string(),
                organization_id: rec.organization_id.to_string(),
                plan_code: rec.plan_code.as_str().into(),
                provider: rec.provider.as_str().into(),
                status: rec.status.as_str().into(),
                checkout_url: rec.checkout_url.clone(),
                instructions: Some(format!(
                    "checkout created for plan {}; provider {}; use success_url for completion",
                    rec.plan_code.as_str(),
                    rec.provider.as_str()
                )),
                expires_at: rec.expires_at.map(|t| t.to_rfc3339()),
            };
            (
                axum::http::StatusCode::CREATED,
                Json(serde_json::json!(resp)),
            )
                .into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            // Typed errors: not configured, live gating, DB, provider failure — no silent fallback
            let status = if msg.contains("not configured")
                || msg.contains("missing") && (msg.contains("STRIPE") || msg.contains("PADDLE"))
                || msg.contains("PROVIDER_NOT_CONFIGURED")
            {
                axum::http::StatusCode::NOT_IMPLEMENTED
            } else if msg.contains("NOT_RUN")
                || msg.contains("EXTERNAL_REQUIRED")
                || msg.contains("LIVE_BILLING")
                || msg.contains("reconciliation required")
                || msg.contains("database unavailable")
                || msg.contains("durable update failed")
            {
                // Live opt-in missing, external provider required, or the durable write
                // could not be completed — retryable/infra, not a client fault.
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            } else {
                axum::http::StatusCode::BAD_REQUEST
            };
            (
                status,
                Json(serde_json::json!({"error":"checkout_failed","reason": msg})),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_authority_is_server_side() {
        let json = r#"{"plan_code":"pro","idempotency_key":"k1"}"#;
        let req: CheckoutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.plan_code, "pro");
        let json_extra =
            r#"{"plan_code":"pro","idempotency_key":"k1","amount_cents":99999,"currency":"usd"}"#;
        let req2: CheckoutRequest = serde_json::from_str(json_extra).unwrap();
        assert_eq!(req2.plan_code, "pro");
        let serialized = serde_json::to_string(&req2).unwrap();
        assert!(!serialized.contains("amount"));
    }

    #[test]
    fn idempotency_key_required() {
        let req = CheckoutRequest {
            plan_code: "starter".into(),
            provider: None,
            idempotency_key: "   ".into(),
            success_url: None,
            cancel_url: None,
        };
        assert!(req.idempotency_key.trim().is_empty());
    }

    #[test]
    fn provider_parse_is_strict() {
        assert_eq!(
            BillingProviderKind::parse("stripe"),
            Some(BillingProviderKind::Stripe)
        );
        assert_eq!(
            BillingProviderKind::parse("manual"),
            Some(BillingProviderKind::Manual)
        );
        assert_eq!(BillingProviderKind::parse("unknown_provider"), None);
        // checkout handler must reject unknown provider rather than silently falling back to manual
        let unknown = "unknown_provider";
        assert!(BillingProviderKind::parse(unknown).is_none());
    }
}
