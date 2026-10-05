//! Server-authoritative product plan catalogue.
//!
//! Pricing is not synthesized here. Plan identity, status, descriptions and
//! entitlement limits come from the SaaS store. Monetary price snapshots must
//! be supplied by the configured billing provider before checkout is offered.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;

use crate::api::ApiState;

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/pricing", axum::routing::get(get_pricing_catalog))
}

pub async fn get_pricing_catalog(State(state): State<ApiState>) -> Response {
    let plans = match state.saas.plans().await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "authoritative plan catalogue could not be loaded");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "pricing_storage_unavailable",
                    "reason": "authoritative plan catalogue could not be loaded"
                })),
            )
                .into_response();
        }
    };

    let public_plans: Vec<serde_json::Value> = plans
        .into_iter()
        .filter(|plan| !matches!(plan.status, bot_core::billing::plan::PlanStatus::Private))
        .map(|plan| {
            let enabled_features = plan
                .limits
                .iter()
                .filter(|(_, limit)| limit.is_enabled())
                .map(|(feature, _)| feature.clone())
                .collect::<Vec<_>>();
            json!({
                "code": plan.code.as_str(),
                "name": plan.name,
                "description": plan.description,
                "status": plan.status.as_str(),
                "price_monthly_usd_cents": serde_json::Value::Null,
                "price_yearly_usd_cents": serde_json::Value::Null,
                "prices_available": false,
                "features": enabled_features,
                "limits": plan.limits,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "plans": public_plans,
            "currency": "USD",
            "pricing_status": "provider_not_configured"
        })),
    )
        .into_response()
}
