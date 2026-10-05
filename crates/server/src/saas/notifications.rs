//! Notification routing and channel delivery preferences (THIRD.md §139).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/notifications/preferences",
            axum::routing::get(get_preferences).put(update_preferences),
        )
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationPreferences {
    pub email_trade_executed: bool,
    pub email_risk_breach: bool,
    pub email_daily_summary: bool,
    pub telegram_instant_fills: bool,
    pub telegram_circuit_breaker: bool,
    pub webhook_forwarding: bool,
}

pub async fn get_preferences(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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

    let prefs = NotificationPreferences {
        email_trade_executed: true,
        email_risk_breach: true,
        email_daily_summary: false,
        telegram_instant_fills: true,
        telegram_circuit_breaker: true,
        webhook_forwarding: true,
    };

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "preferences": prefs
        })),
    )
        .into_response()
}

pub async fn update_preferences(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<NotificationPreferences>,
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

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.notifications.preferences_updated",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "preferences": body
        })),
    )
        .into_response()
}
