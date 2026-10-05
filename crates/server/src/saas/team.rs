//! Durable organization membership and invitation administration.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::{MembershipRole, MembershipStatus, Permission};
use bot_core::session::token::generate_token;
use bot_core::tenant::MembershipId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/team/invites",
            axum::routing::get(list_invites).post(invite_member),
        )
        .route(
            "/api/saas/team/invites/:id",
            axum::routing::delete(revoke_invite),
        )
        .route(
            "/api/saas/team/invites/:id/resend",
            axum::routing::post(resend_invite),
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

fn storage_error(reason: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "team_storage_unavailable", "reason": reason })),
    )
        .into_response()
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
    let Some(db) = state.db.as_deref() else {
        return storage_error("team invitations require an attached PostgreSQL database");
    };
    let rows = match sqlx::query(
        "SELECT id, organization_id, email, role, status, created_at, expires_at FROM invites WHERE organization_id = $1 ORDER BY created_at DESC LIMIT 200",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "team invitation query failed");
            return storage_error("team invitations could not be loaded");
        }
    };
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let item = match (
            row.try_get::<uuid::Uuid, _>("id"),
            row.try_get::<uuid::Uuid, _>("organization_id"),
            row.try_get::<String, _>("email"),
            row.try_get::<String, _>("role"),
            row.try_get::<String, _>("status"),
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("expires_at"),
        ) {
            (
                Ok(id),
                Ok(organization_id),
                Ok(email),
                Ok(role),
                Ok(status),
                Ok(created_at),
                Ok(expires_at),
            ) => json!({
                "id": id.to_string(),
                "organization_id": organization_id.to_string(),
                "email": email,
                "role": role,
                "status": status,
                "created_at": created_at.to_rfc3339(),
                "expires_at": expires_at.to_rfc3339(),
            }),
            _ => return storage_error("team invitation rows could not be decoded"),
        };
        items.push(item);
    }
    let count = items.len();
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "items": items,
            "count": count,
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
    let email = body.email.trim().to_ascii_lowercase();
    let Some(role) = MembershipRole::parse(&body.role) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_role" })),
        )
            .into_response();
    };
    if matches!(
        role,
        MembershipRole::PlatformAdmin | MembershipRole::OrgOwner
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "role_not_assignable_by_tenant" })),
        )
            .into_response();
    }
    if email.is_empty() || !email.contains('@') || email.len() > 320 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_email" })),
        )
            .into_response();
    }
    let Some(db) = state.db.as_deref() else {
        return storage_error("team invitations require an attached PostgreSQL database");
    };
    let token = generate_token("inv");
    let invitation_id = uuid::Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(7);
    let result = sqlx::query(
        "INSERT INTO invites (id, organization_id, email, role, token_hash, invited_by, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(invitation_id)
    .bind(ctx.organization.id.as_uuid())
    .bind(&email)
    .bind(role.as_str())
    .bind(&token.hash)
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .bind(expires_at)
    .execute(db.pool())
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, organization = %ctx.organization.id, "team invitation insert failed");
        return storage_error("team invitation could not be persisted");
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.team.member_invited",
            Some(&invitation_id.to_string()),
        )
        .await;
    (
        StatusCode::CREATED,
        Json(json!({
            "success": true,
            "invitation_id": invitation_id.to_string(),
            "organization_id": ctx.organization.id.to_string(),
            "email": email,
            "role": role.as_str(),
            "expires_at": expires_at.to_rfc3339(),
            "invitation_token": token.plaintext,
            "token_delivery": "returned_once; external email delivery is not configured",
        })),
    )
        .into_response()
}

pub async fn revoke_invite(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(invite_id): Path<String>,
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
    let invite_id = match uuid::Uuid::parse_str(&invite_id) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "invite_not_found" })),
            )
                .into_response()
        }
    };
    let Some(db) = state.db.as_deref() else {
        return storage_error("team invitations require an attached PostgreSQL database");
    };
    let result = sqlx::query(
        "UPDATE invites SET status = 'revoked' WHERE id = $1 AND organization_id = $2 AND status = 'pending'",
    )
    .bind(invite_id)
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match result {
        Ok(done) if done.rows_affected() == 1 => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.team.invite_revoked",
                    Some(&invite_id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "success": true, "invite_id": invite_id, "status": "revoked" })),
            )
                .into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "invite_not_found" })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, invite = %invite_id, "invite revoke failed");
            storage_error("invitation could not be revoked")
        }
    }
}

pub async fn resend_invite(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(invite_id): Path<String>,
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
    let invite_id = match uuid::Uuid::parse_str(&invite_id) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "invite_not_found" })),
            )
                .into_response()
        }
    };
    let Some(db) = state.db.as_deref() else {
        return storage_error("team invitations require an attached PostgreSQL database");
    };
    let token = generate_token("inv");
    let expires_at = Utc::now() + Duration::days(7);
    let result = sqlx::query(
        "UPDATE invites SET token_hash = $1, expires_at = $2, status = 'pending' WHERE id = $3 AND organization_id = $4 AND status IN ('pending', 'expired') RETURNING email, role",
    )
    .bind(&token.hash)
    .bind(expires_at)
    .bind(invite_id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await;
    match result {
        Ok(Some(row)) => {
            let email: String = match row.try_get("email") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, invite = %invite_id, "resent invite row could not be decoded");
                    return storage_error("resent invitation data is invalid");
                }
            };
            let role: String = match row.try_get("role") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, invite = %invite_id, "resent invite role could not be decoded");
                    return storage_error("resent invitation data is invalid");
                }
            };
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.team.invite_resent",
                    Some(&invite_id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "invite_id": invite_id,
                    "email": email,
                    "role": role,
                    "expires_at": expires_at.to_rfc3339(),
                    "invitation_token": token.plaintext,
                    "token_delivery": "returned_once; external email delivery is not configured",
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "invite_not_found" })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, invite = %invite_id, "invite resend failed");
            storage_error("invitation could not be resent")
        }
    }
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
    let Some(id) = MembershipId::parse(&member_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "member_not_found" })),
        )
            .into_response();
    };
    let members = match state.saas.members(ctx.organization.id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "membership query failed");
            return storage_error("memberships could not be loaded");
        }
    };
    let Some(mut membership) = members.into_iter().find(|member| member.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "member_not_found" })),
        )
            .into_response();
    };
    if Some(membership.user_id) == ctx.authorization.user_id
        && membership.role == MembershipRole::OrgOwner
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "owner_cannot_remove_self" })),
        )
            .into_response();
    }
    membership.status = MembershipStatus::Removed;
    membership.updated_at = Utc::now();
    if let Err(error) = state.saas.update_membership(&membership).await {
        tracing::error!(error = %error, membership = %id, "membership removal failed");
        return storage_error("membership removal could not be persisted");
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.team.member_removed",
            Some(&member_id),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({ "success": true, "membership_id": member_id, "status": "removed" })),
    )
        .into_response()
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
    let Some(id) = MembershipId::parse(&member_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "member_not_found" })),
        )
            .into_response();
    };
    let Some(role) = MembershipRole::parse(&body.role) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_role" })),
        )
            .into_response();
    };
    if matches!(
        role,
        MembershipRole::PlatformAdmin | MembershipRole::OrgOwner
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "role_not_assignable_by_tenant" })),
        )
            .into_response();
    }
    let members = match state.saas.members(ctx.organization.id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "membership query failed");
            return storage_error("memberships could not be loaded");
        }
    };
    let Some(mut membership) = members.into_iter().find(|member| member.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "member_not_found" })),
        )
            .into_response();
    };
    membership.role = role;
    membership.updated_at = Utc::now();
    if let Err(error) = state.saas.update_membership(&membership).await {
        tracing::error!(error = %error, membership = %id, "membership role update failed");
        return storage_error("membership role could not be persisted");
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.team.role_updated",
            Some(&format!("{}:{}", member_id, role.as_str())),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({ "success": true, "membership_id": member_id, "role": role.as_str() })),
    )
        .into_response()
}
