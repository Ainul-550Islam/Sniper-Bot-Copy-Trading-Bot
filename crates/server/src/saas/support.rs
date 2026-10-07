//! Durable tenant support-ticket service.

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{routing::get, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new().route(
        "/api/saas/support/tickets",
        get(list_tickets).post(create_ticket),
    )
}

#[derive(Debug, Deserialize)]
pub struct CreateTicketBody {
    pub subject: String,
    pub priority: String,
    pub description: String,
}

fn priority_is_valid(priority: &str) -> bool {
    matches!(priority, "low" | "normal" | "high" | "urgent")
}

pub async fn list_tickets(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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
                "error": "support_storage_unavailable",
                "reason": "support tickets require an attached PostgreSQL database"
            })),
        )
            .into_response();
    };

    let rows = match sqlx::query(
        "SELECT id, organization_id, created_by, subject, priority, description, status, created_at, updated_at FROM support_tickets WHERE organization_id = $1 ORDER BY created_at DESC, id DESC LIMIT 200",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "support ticket query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "support_storage_unavailable",
                    "reason": "support tickets could not be loaded"
                })),
            )
                .into_response();
        }
    };

    let row_count = rows.len();
    let mut items = Vec::with_capacity(row_count);
    for row in rows {
        let item = match (
            row.try_get::<uuid::Uuid, _>("id"),
            row.try_get::<uuid::Uuid, _>("organization_id"),
            row.try_get::<Option<uuid::Uuid>, _>("created_by"),
            row.try_get::<String, _>("subject"),
            row.try_get::<String, _>("priority"),
            row.try_get::<String, _>("description"),
            row.try_get::<String, _>("status"),
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
        ) {
            (
                Ok(id),
                Ok(organization_id),
                Ok(created_by),
                Ok(subject),
                Ok(priority),
                Ok(description),
                Ok(status),
                Ok(created_at),
                Ok(updated_at),
            ) => json!({
                "id": id.to_string(),
                "organization_id": organization_id.to_string(),
                "created_by": created_by.map(|value| value.to_string()),
                "subject": subject,
                "priority": priority,
                "description": description,
                "status": status,
                "created_at": created_at.to_rfc3339(),
                "updated_at": updated_at.to_rfc3339(),
            }),
            _ => {
                tracing::error!(organization = %ctx.organization.id, "support ticket row could not be decoded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "support_storage_unavailable",
                        "reason": "support tickets could not be decoded"
                    })),
                )
                    .into_response();
            }
        };
        items.push(item);
    }

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "items": items,
            "count": row_count
        })),
    )
        .into_response()
}

pub async fn create_ticket(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateTicketBody>,
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
    let subject = body.subject.trim();
    let description = body.description.trim();
    let priority = body.priority.trim().to_ascii_lowercase();
    if subject.is_empty() || description.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "subject_and_description_required" })),
        )
            .into_response();
    }
    if subject.len() > 240 || description.len() > 20_000 || !priority_is_valid(&priority) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_ticket",
                "reason": "subject, description, or priority is invalid"
            })),
        )
            .into_response();
    }
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "support_storage_unavailable",
                "reason": "support tickets require an attached PostgreSQL database"
            })),
        )
            .into_response();
    };

    let id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        "INSERT INTO support_tickets (id, organization_id, created_by, subject, priority, description) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(ctx.organization.id.as_uuid())
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .bind(subject)
    .bind(&priority)
    .bind(description)
    .execute(db.pool())
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, organization = %ctx.organization.id, "support ticket insert failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "support_storage_unavailable",
                "reason": "support ticket could not be persisted"
            })),
        )
            .into_response();
    }

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.support.ticket_created",
            Some(&id.to_string()),
        )
        .await;
    (
        StatusCode::CREATED,
        Json(json!({
            "id": id.to_string(),
            "organization_id": ctx.organization.id.to_string(),
            "subject": subject,
            "priority": priority,
            "description": description,
            "status": "open",
            "notification_status": "not_configured",
        })),
    )
        .into_response()
}
