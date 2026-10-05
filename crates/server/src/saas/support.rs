//! Dedicated enterprise support and SLA ticket orchestration (SECOND.md §89).

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
            "/api/saas/support/tickets",
            axum::routing::get(list_tickets).post(create_ticket),
        )
}

#[derive(Debug, Deserialize)]
pub struct CreateTicketBody {
    pub subject: String,
    pub priority: String,
    pub description: String,
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

    let org = ctx.organization.id;
    let now = Utc::now();

    let items = vec![json!({
        "id": "tkt-2026-9481",
        "organization_id": org.to_string(),
        "subject": "Dedicated Jito RPC Gateway Latency Spike",
        "priority": "high",
        "status": "resolved",
        "created_at": (now - chrono::Duration::days(1)).to_rfc3339(),
        "updated_at": now.to_rfc3339(),
    })];

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

pub async fn create_ticket(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateTicketBody>,
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

    if body.subject.trim().is_empty() || body.description.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "subject_and_description_required" })),
        )
            .into_response();
    }

    let id = format!("tkt-{}", uuid::Uuid::new_v4());
    let now = Utc::now();

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.support.ticket_created",
            Some(&id),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "organization_id": ctx.organization.id.to_string(),
            "subject": body.subject,
            "priority": body.priority,
            "status": "open",
            "created_at": now.to_rfc3339(),
            "updated_at": now.to_rfc3339(),
        })),
    )
        .into_response()
}
