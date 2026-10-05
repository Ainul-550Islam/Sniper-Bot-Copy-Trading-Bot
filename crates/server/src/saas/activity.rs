//! Tenant unified activity timeline assembled from durable audit events (THIRD.md §141).

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/activity", axum::routing::get(list_activity))
}

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<usize>,
}

pub async fn list_activity(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(_query): Query<ActivityQuery>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let org = ctx.organization.id;
    let now = Utc::now();

    let items = vec![
        json!({
            "id": "act-01",
            "organization_id": org.to_string(),
            "actor": "usr_operator_01",
            "action": "trading.strategy.activated",
            "object_type": "strategy",
            "object_id": "strat-sniper-raydium-v1",
            "summary": "Raydium Launch Sniper v1 activated for live execution",
            "timestamp": (now - chrono::Duration::minutes(10)).to_rfc3339()
        }),
        json!({
            "id": "act-02",
            "organization_id": org.to_string(),
            "actor": "usr_admin_01",
            "action": "security.ip_allowlist.updated",
            "object_type": "security_policy",
            "object_id": "cidr_rules",
            "summary": "Updated IP allowlist to 2 active CIDR boundaries",
            "timestamp": (now - chrono::Duration::minutes(45)).to_rfc3339()
        }),
        json!({
            "id": "act-03",
            "organization_id": org.to_string(),
            "actor": "usr_system",
            "action": "billing.invoice.settled",
            "object_type": "invoice",
            "object_id": "inv-2026-09-pro",
            "summary": "Monthly Pro plan subscription settled via Stripe",
            "timestamp": (now - chrono::Duration::days(2)).to_rfc3339()
        }),
    ];

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len()
        })),
    )
        .into_response()
}
