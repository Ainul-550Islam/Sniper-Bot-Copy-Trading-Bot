//! Durable organization membership & team administration service (SECOND.md §85).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamInviteRecord {
    pub id: String,
    pub organization_id: String,
    pub email: String,
    pub role: String,
    pub status: String,
    pub created_at: String,
}

static INVITE_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, Vec<TeamInviteRecord>>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/team/invites",
            axum::routing::get(list_invites).post(invite_member),
        )
        .route(
            "/api/saas/team/members/:id",
            axum::routing::delete(remove_member).patch(update_member_role),
        )
}

#[derive(Debug, Deserialize)]
pub struct InviteMemberBody {
    pub email: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleBody {
    pub role: String,
}

pub async fn list_invites(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::UsersRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let mut lock = INVITE_STORE.lock().unwrap();
    let list = lock.entry(ctx.organization.id).or_default();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "items": list,
            "count": list.len(),
        })),
    )
        .into_response()
}

pub async fn invite_member(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<InviteMemberBody>,
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

    if body.email.trim().is_empty() || !body.email.contains('@') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_email" })),
        )
            .into_response();
    }

    let id = format!("inv-{}", uuid::Uuid::new_v4().simple());
    let invite = TeamInviteRecord {
        id: id.clone(),
        organization_id: ctx.organization.id.to_string(),
        email: body.email.clone(),
        role: body.role.clone(),
        status: "pending".into(),
        created_at: Utc::now().to_rfc3339(),
    };

    let mut lock = INVITE_STORE.lock().unwrap();
    lock.entry(ctx.organization.id).or_default().push(invite.clone());

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.team.member_invited",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "success": true,
            "invitation_id": id,
            "organization_id": ctx.organization.id.to_string(),
            "email": body.email,
            "role": body.role,
        })),
    )
        .into_response()
}

pub async fn remove_member(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(member_id): Path<String>,
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
            "saas.team.member_removed",
            Some(&member_id),
        )
        .await;

    (StatusCode::OK, Json(json!({ "success": true }))).into_response()
}

pub async fn update_member_role(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(member_id): Path<String>,
    Json(body): Json<UpdateRoleBody>,
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
            "saas.team.role_updated",
            Some(&format!("{}:{}", member_id, body.role)),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({ "success": true, "role": body.role })),
    )
        .into_response()
}
