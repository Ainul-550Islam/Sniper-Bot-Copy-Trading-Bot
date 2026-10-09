//! Tenant unified activity timeline assembled from durable audit events.

use axum::extract::{Query, State};
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
    Router::new().route("/api/saas/activity", axum::routing::get(list_activity))
}

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<usize>,
    pub cursor: Option<i64>,
    pub actor: Option<String>,
    pub action: Option<String>,
}

fn row_to_activity(row: &PgRow) -> Result<serde_json::Value, String> {
    let id: i64 = row.try_get("id").map_err(|error| error.to_string())?;
    let ts: chrono::DateTime<chrono::Utc> = row.try_get("ts").map_err(|error| error.to_string())?;
    let actor: String = row.try_get("actor").map_err(|error| error.to_string())?;
    let action: String = row.try_get("action").map_err(|error| error.to_string())?;
    let target: Option<String> = row.try_get("target").map_err(|error| error.to_string())?;
    let outcome: String = row.try_get("outcome").map_err(|error| error.to_string())?;
    let detail: serde_json::Value = row.try_get("detail").map_err(|error| error.to_string())?;

    Ok(json!({
        "id": id.to_string(),
        "organization_id": row
            .try_get::<uuid::Uuid, _>("organization_id")
            .map_err(|error| error.to_string())?
            .to_string(),
        "actor": actor,
        "action": action,
        "target": target,
        "outcome": outcome,
        "detail": detail,
        "timestamp": ts.to_rfc3339(),
    }))
}

pub async fn list_activity(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
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
                "error": "activity_storage_unavailable",
                "reason": "activity history requires an attached PostgreSQL database"
            })),
        )
            .into_response();
    };

    let limit = query.limit.unwrap_or(50).clamp(1, 200) as i64;
    let mut statement = QueryBuilder::<Postgres>::new(
        "SELECT id, organization_id, actor, action, target, outcome, detail, ts FROM audit_events WHERE organization_id = ",
    );
    statement.push_bind(ctx.organization.id.as_uuid());
    if let Some(cursor) = query.cursor {
        statement.push(" AND id < ");
        statement.push_bind(cursor);
    }
    if let Some(actor) = query
        .actor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        statement.push(" AND actor ILIKE ");
        statement.push_bind(format!("%{actor}%"));
    }
    if let Some(action) = query
        .action
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        statement.push(" AND action = ");
        statement.push_bind(action.to_string());
    }
    statement.push(" ORDER BY id DESC LIMIT ");
    statement.push_bind(limit);

    let rows = match statement.build().fetch_all(db.pool()).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "tenant activity query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "activity_storage_unavailable",
                    "reason": "activity records could not be loaded"
                })),
            )
                .into_response();
        }
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        match row_to_activity(row) {
            Ok(value) => items.push(value),
            Err(error) => {
                tracing::error!(error = %error, organization = %ctx.organization.id, "tenant activity row could not be decoded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "activity_storage_unavailable",
                        "reason": "activity records could not be decoded"
                    })),
                )
                    .into_response();
            }
        }
    }

    let next_cursor = rows.last().and_then(|row| row.try_get::<i64, _>("id").ok());
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "items": items,
            "count": rows.len(),
            "next_cursor": next_cursor.map(|value| value.to_string()),
        })),
    )
        .into_response()
}
