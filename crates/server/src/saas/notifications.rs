//! Durable tenant notification preferences.

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new().route(
        "/api/saas/notifications/preferences",
        axum::routing::get(get_preferences).put(update_preferences),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreferences {
    pub email_trade_executed: bool,
    pub email_risk_breach: bool,
    pub email_daily_summary: bool,
    pub telegram_instant_fills: bool,
    pub telegram_circuit_breaker: bool,
    pub webhook_forwarding: bool,
}

fn preferences_from_row(row: &sqlx::postgres::PgRow) -> Result<NotificationPreferences, String> {
    Ok(NotificationPreferences {
        email_trade_executed: row
            .try_get("email_trade_executed")
            .map_err(|error| error.to_string())?,
        email_risk_breach: row
            .try_get("email_risk_breach")
            .map_err(|error| error.to_string())?,
        email_daily_summary: row
            .try_get("email_daily_summary")
            .map_err(|error| error.to_string())?,
        telegram_instant_fills: row
            .try_get("telegram_instant_fills")
            .map_err(|error| error.to_string())?,
        telegram_circuit_breaker: row
            .try_get("telegram_circuit_breaker")
            .map_err(|error| error.to_string())?,
        webhook_forwarding: row
            .try_get("webhook_forwarding")
            .map_err(|error| error.to_string())?,
    })
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
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "notification_storage_unavailable",
                "reason": "notification preferences require an attached PostgreSQL database"
            })),
        )
            .into_response();
    };

    let row = match sqlx::query(
        "SELECT email_trade_executed, email_risk_breach, email_daily_summary, telegram_instant_fills, telegram_circuit_breaker, webhook_forwarding FROM notification_preferences WHERE organization_id = $1",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "notification preferences query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "notification_storage_unavailable",
                    "reason": "notification preferences could not be loaded"
                })),
            )
                .into_response();
        }
    };
    let preferences = match row.as_ref().map(preferences_from_row).transpose() {
        Ok(Some(value)) => value,
        Ok(None) => NotificationPreferences {
            email_trade_executed: true,
            email_risk_breach: true,
            email_daily_summary: false,
            telegram_instant_fills: true,
            telegram_circuit_breaker: true,
            webhook_forwarding: true,
        },
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "notification preferences row could not be decoded");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "notification_storage_unavailable",
                    "reason": "notification preferences could not be decoded"
                })),
            )
                .into_response();
        }
    };
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "preferences": preferences
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
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "notification_storage_unavailable",
                "reason": "notification preferences require an attached PostgreSQL database"
            })),
        )
            .into_response();
    };

    let result = sqlx::query(
        "INSERT INTO notification_preferences (organization_id, email_trade_executed, email_risk_breach, email_daily_summary, telegram_instant_fills, telegram_circuit_breaker, webhook_forwarding, updated_by, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now()) ON CONFLICT (organization_id) DO UPDATE SET email_trade_executed = EXCLUDED.email_trade_executed, email_risk_breach = EXCLUDED.email_risk_breach, email_daily_summary = EXCLUDED.email_daily_summary, telegram_instant_fills = EXCLUDED.telegram_instant_fills, telegram_circuit_breaker = EXCLUDED.telegram_circuit_breaker, webhook_forwarding = EXCLUDED.webhook_forwarding, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(ctx.organization.id.as_uuid())
    .bind(body.email_trade_executed)
    .bind(body.email_risk_breach)
    .bind(body.email_daily_summary)
    .bind(body.telegram_instant_fills)
    .bind(body.telegram_circuit_breaker)
    .bind(body.webhook_forwarding)
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .execute(db.pool())
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, organization = %ctx.organization.id, "notification preferences update failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "notification_storage_unavailable",
                "reason": "notification preferences could not be persisted"
            })),
        )
            .into_response();
    }

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
