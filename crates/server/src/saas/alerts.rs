//! Durable tenant alert management and acknowledgement.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::{postgres::PgRow, Postgres, QueryBuilder, Row};

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/alerts", axum::routing::get(list_alerts))
        .route(
            "/api/saas/alerts/:id/ack",
            axum::routing::post(acknowledge_alert),
        )
}

#[derive(Debug, Deserialize)]
pub struct AlertQuery {
    pub severity: Option<String>,
    pub unread: Option<String>,
}

fn row_to_alert(row: &PgRow) -> Result<serde_json::Value, String> {
    let id: uuid::Uuid = row.try_get("id").map_err(|error| error.to_string())?;
    let organization_id: uuid::Uuid = row
        .try_get("organization_id")
        .map_err(|error| error.to_string())?;
    let severity: String = row.try_get("severity").map_err(|error| error.to_string())?;
    let category: String = row.try_get("category").map_err(|error| error.to_string())?;
    let title: String = row.try_get("title").map_err(|error| error.to_string())?;
    let message: String = row.try_get("message").map_err(|error| error.to_string())?;
    let acknowledged_at: Option<chrono::DateTime<chrono::Utc>> = row
        .try_get("acknowledged_at")
        .map_err(|error| error.to_string())?;
    let acknowledged_by: Option<uuid::Uuid> = row
        .try_get("acknowledged_by")
        .map_err(|error| error.to_string())?;
    let created_at: chrono::DateTime<chrono::Utc> = row
        .try_get("created_at")
        .map_err(|error| error.to_string())?;
    let updated_at: chrono::DateTime<chrono::Utc> = row
        .try_get("updated_at")
        .map_err(|error| error.to_string())?;

    Ok(json!({
        "id": id.to_string(),
        "organization_id": organization_id.to_string(),
        "severity": severity,
        "category": category,
        "title": title,
        "message": message,
        "is_acknowledged": acknowledged_at.is_some(),
        "acknowledged_at": acknowledged_at.map(|value| value.to_rfc3339()),
        "acknowledged_by": acknowledged_by.map(|value| value.to_string()),
        "created_at": created_at.to_rfc3339(),
        "updated_at": updated_at.to_rfc3339(),
    }))
}

pub async fn list_alerts(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<AlertQuery>,
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
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "alert_storage_unavailable",
                "reason": "alerts require an attached PostgreSQL database"
            })),
        )
            .into_response();
    };

    let mut statement = QueryBuilder::<Postgres>::new(
        "SELECT id, organization_id, severity, category, title, message, acknowledged_at, acknowledged_by, created_at, updated_at FROM tenant_alerts WHERE organization_id = ",
    );
    statement.push_bind(ctx.organization.id.as_uuid());
    if let Some(severity) = query
        .severity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        statement.push(" AND severity = ");
        statement.push_bind(severity.to_string());
    }
    if query.unread.as_deref() == Some("true") {
        statement.push(" AND acknowledged_at IS NULL");
    }
    statement.push(" ORDER BY created_at DESC, id DESC LIMIT 200");

    let rows = match statement.build().fetch_all(db.pool()).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "tenant alert query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "alert_storage_unavailable",
                    "reason": "alerts could not be loaded"
                })),
            )
                .into_response();
        }
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        match row_to_alert(row) {
            Ok(value) => items.push(value),
            Err(error) => {
                tracing::error!(error = %error, organization = %ctx.organization.id, "tenant alert row could not be decoded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "alert_storage_unavailable",
                        "reason": "alerts could not be decoded"
                    })),
                )
                    .into_response();
            }
        }
    }

    let unacknowledged_count = items
        .iter()
        .filter(|item| item["is_acknowledged"] == false)
        .count();
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "items": items,
            "unacknowledged_count": unacknowledged_count,
        })),
    )
        .into_response()
}

pub async fn acknowledge_alert(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
                "error": "alert_storage_unavailable",
                "reason": "alerts require an attached PostgreSQL database"
            })),
        )
            .into_response();
    };
    let alert_id = match uuid::Uuid::parse_str(&id) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "alert_not_found" })),
            )
                .into_response()
        }
    };
    let result = sqlx::query(
        "UPDATE tenant_alerts SET acknowledged_at = now(), acknowledged_by = $1, updated_at = now() WHERE id = $2 AND organization_id = $3 AND acknowledged_at IS NULL",
    )
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .bind(alert_id)
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => {
            state
                .audit
                .success(&ctx.actor_label(), "saas.alerts.acknowledged", Some(&id))
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "id": id,
                    "acknowledged": true,
                })),
            )
                .into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "alert_not_found" })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, alert = %id, "tenant alert acknowledgement failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "alert_storage_unavailable",
                    "reason": "alert acknowledgement could not be persisted"
                })),
            )
                .into_response()
        }
    }
}
