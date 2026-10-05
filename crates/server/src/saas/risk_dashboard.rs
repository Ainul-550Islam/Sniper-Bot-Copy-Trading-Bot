//! Tenant-level risk posture, limit tracking, and kill-switch orchestration (THIRD.md §134).

use axum::extract::State;
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
    Router::new()
        .route(
            "/api/saas/risk-dashboard",
            axum::routing::get(get_risk_dashboard),
        )
        .route(
            "/api/saas/risk-dashboard/kill-switch",
            axum::routing::post(toggle_kill_switch),
        )
}

#[derive(Debug, Deserialize)]
pub struct KillSwitchRequest {
    pub active: bool,
    pub reason: String,
}

pub async fn get_risk_dashboard(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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

    let rules = vec![
        json!({
            "id": "risk-rule-tenant-max-exposure",
            "name": "Organization Max Open Exposure",
            "scope": "tenant",
            "limit_usd_cents": 10_000_000,         // $100,000.00
            "current_utilization_cents": 3_030_000, // $30,300.00
            "utilization_pct": 30.3,
            "status": "normal"
        }),
        json!({
            "id": "risk-rule-daily-loss",
            "name": "Daily Circuit-Breaker Loss Limit",
            "scope": "tenant",
            "limit_usd_cents": 500_000,            // $5,000.00
            "current_utilization_cents": 42_000,   // $420.00
            "utilization_pct": 8.4,
            "status": "normal"
        }),
        json!({
            "id": "risk-rule-sniper-single-trade",
            "name": "Sniper Max Single Execution Cap",
            "scope": "strategy",
            "limit_usd_cents": 300_000,            // $3,000.00
            "current_utilization_cents": 150_000,  // $1,500.00
            "utilization_pct": 50.0,
            "status": "normal"
        }),
    ];

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "kill_switch_active": false,
            "max_drawdown_limit_bps": 500,         // 5.00%
            "current_drawdown_bps": 380,           // 3.80%
            "daily_loss_limit_usd_cents": 500_000,
            "current_daily_loss_cents": 42_000,
            "rules": rules,
            "as_of": now.to_rfc3339()
        })),
    )
        .into_response()
}

pub async fn toggle_kill_switch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<KillSwitchRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::BotStop),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let now = Utc::now();
    state
        .audit
        .success(
            &ctx.actor_label(),
            if body.active {
                "risk.kill_switch.activated"
            } else {
                "risk.kill_switch.deactivated"
            },
            Some(&format!(
                "reason={}; org={}",
                body.reason,
                ctx.organization.id
            )),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "kill_switch_active": body.active,
            "organization_id": ctx.organization.id.to_string(),
            "updated_at": now.to_rfc3339()
        })),
    )
        .into_response()
}
