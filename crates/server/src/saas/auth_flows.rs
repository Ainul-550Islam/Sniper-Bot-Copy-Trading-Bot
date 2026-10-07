//! Public invitation acceptance flow.
//!
//! An invitation is a durable, tenant-owned credential. The plaintext token
//! is created by the authenticated team-invite handler, stored only as a
//! SHA-256 hash, and accepted exactly once. This endpoint creates the invited
//! user when necessary, creates the tenant membership, marks the invitation
//! accepted, and returns a normal short-lived session token once.
//!
//! Email delivery is intentionally not claimed here: the deployment may
//! deliver the one-time invitation token through its configured mailer or
//! another approved channel. The token is never logged or persisted in
//! plaintext.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Postgres, Transaction};

use bot_core::membership::{Membership, MembershipRole};
use bot_core::session::token::{generate_token, hash_password, hash_token};
use bot_core::session::{SessionRecord, DEFAULT_SESSION_TTL_HOURS};
use bot_core::tenant::{OrganizationId, User, UserId, UserStatus};

use crate::api::ApiState;
use crate::saas::users::MIN_PASSWORD_LEN;

/// Mount the public invitation acceptance endpoint.
pub fn routes() -> Router<ApiState> {
    Router::new().route(
        "/api/saas/team/invites/accept",
        axum::routing::post(accept_invite),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptInviteBody {
    /// The one-time invitation token returned by the invitation delivery
    /// channel. It is never stored by this handler.
    pub token: String,
    /// Required only when the invitation creates a new user account.
    #[serde(default)]
    pub password: Option<String>,
    /// Optional display name for a newly created account or an existing user.
    #[serde(default)]
    pub display_name: Option<String>,
}

fn response(status: StatusCode, error: &'static str, reason: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": error,
            "reason": reason.into(),
        })),
    )
        .into_response()
}

fn internal_reason(error: impl std::fmt::Display) -> Response {
    tracing::error!(error = %error, "invitation acceptance failed");
    response(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_storage_unavailable",
        "the invitation could not be accepted because authoritative identity storage was unavailable",
    )
}

async fn insert_runtime_record<T: serde::Serialize>(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &'static str,
    id: &str,
    organization_id: Option<uuid::Uuid>,
    user_id: Option<uuid::Uuid>,
    lookup_key: Option<&str>,
    record: &T,
) -> Result<(), sqlx::Error> {
    let document = serde_json::to_value(record).map_err(|error| {
        sqlx::Error::Protocol(format!("runtime record serialization failed: {error}"))
    })?;
    sqlx::query(
        "INSERT INTO saas_runtime_records
             (kind, id, organization_id, user_id, lookup_key, record)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(kind)
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(lookup_key)
    .bind(document)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn update_runtime_record<T: serde::Serialize>(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &'static str,
    id: &str,
    organization_id: Option<uuid::Uuid>,
    user_id: Option<uuid::Uuid>,
    lookup_key: Option<&str>,
    record: &T,
) -> Result<(), sqlx::Error> {
    let document = serde_json::to_value(record).map_err(|error| {
        sqlx::Error::Protocol(format!("runtime record serialization failed: {error}"))
    })?;
    let changed = sqlx::query(
        "UPDATE saas_runtime_records
            SET organization_id = $3,
                user_id = $4,
                lookup_key = $5,
                record = $6,
                updated_at = now()
          WHERE kind = $1 AND id = $2",
    )
    .bind(kind)
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(lookup_key)
    .bind(document)
    .execute(&mut **transaction)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

/// `POST /api/saas/team/invites/accept`.
///
/// This route is intentionally public because the invitation token is the
/// bearer proof. No organization id is accepted from the caller; it is read
/// from the locked invitation row and all writes use that tenant id.
pub async fn accept_invite(
    State(state): State<ApiState>,
    Json(body): Json<AcceptInviteBody>,
) -> Response {
    let token = body.token.trim();
    if token.is_empty() || token.len() > 512 {
        return response(
            StatusCode::BAD_REQUEST,
            "invalid_invitation_token",
            "invitation token is required",
        );
    }
    if body
        .password
        .as_deref()
        .is_some_and(|password| password.chars().count() > 512)
    {
        return response(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "password is too long",
        );
    }

    let Some(db) = state.db.as_deref() else {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_storage_unavailable",
            "invitation acceptance requires an attached PostgreSQL database",
        );
    };

    let token_hash = hash_token(token);
    let mut transaction = match db.pool().begin().await {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };

    let invitation = match sqlx::query(
        "SELECT id, organization_id, email, role, invited_by
           FROM invites
          WHERE token_hash = $1
            AND status = 'pending'
            AND expires_at > now()
          FOR UPDATE",
    )
    .bind(&token_hash)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            let _ = transaction.rollback().await;
            return response(
                StatusCode::NOT_FOUND,
                "invitation_not_found",
                "invitation token is invalid, expired, or already used",
            );
        }
        Err(error) => return internal_reason(error),
    };

    let invitation_id: uuid::Uuid = match sqlx::Row::try_get(&invitation, "id") {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };
    let organization_uuid: uuid::Uuid = match sqlx::Row::try_get(&invitation, "organization_id") {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };
    let email: String = match sqlx::Row::try_get(&invitation, "email") {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };
    let role_text: String = match sqlx::Row::try_get(&invitation, "role") {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };
    let invited_by_uuid: Option<uuid::Uuid> = match sqlx::Row::try_get(&invitation, "invited_by") {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };
    let role = match MembershipRole::parse(&role_text) {
        Some(value) => value,
        None => return internal_reason(format!("unknown invitation role {role_text}")),
    };
    let organization_id = OrganizationId::from(organization_uuid);
    let normalized_email = User::normalize_email(&email);

    let existing_user = match sqlx::query(
        "SELECT id, record
           FROM saas_runtime_records
          WHERE kind = 'user' AND lookup_key = $1
          FOR UPDATE",
    )
    .bind(&normalized_email)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };

    let now = Utc::now();
    let mut user: User;
    let user_runtime_id: String;
    if let Some(row) = existing_user {
        let document: Value = match sqlx::Row::try_get(&row, "record") {
            Ok(value) => value,
            Err(error) => return internal_reason(error),
        };
        user = match serde_json::from_value(document) {
            Ok(value) => value,
            Err(error) => return internal_reason(error),
        };
        if !matches!(user.status, UserStatus::Active) {
            let _ = transaction.rollback().await;
            return response(
                StatusCode::FORBIDDEN,
                "account_not_active",
                "the invited account is not active",
            );
        }
        user.email_verified = true;
        if let Some(display_name) = body.display_name.as_deref() {
            user.display_name = display_name.trim().to_string();
        }
        user.updated_at = now;
        user_runtime_id = user.id.to_string();
        if let Err(error) = update_runtime_record(
            &mut transaction,
            "user",
            &user_runtime_id,
            None,
            Some(user.id.as_uuid()),
            Some(&normalized_email),
            &user,
        )
        .await
        {
            return internal_reason(error);
        }
    } else {
        let password = match body.password.as_deref() {
            Some(value) if value.chars().count() >= MIN_PASSWORD_LEN => value,
            Some(_) => {
                let _ = transaction.rollback().await;
                return response(
                    StatusCode::BAD_REQUEST,
                    "invalid_password",
                    format!("password must be at least {MIN_PASSWORD_LEN} characters"),
                );
            }
            None => {
                let _ = transaction.rollback().await;
                return response(
                    StatusCode::BAD_REQUEST,
                    "password_required",
                    "a password is required when the invitation creates a new account",
                );
            }
        };
        user = User {
            id: UserId::new(),
            email: normalized_email.clone(),
            email_verified: true,
            display_name: body
                .display_name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            password_hash: hash_password(password),
            status: UserStatus::Active,
            platform_admin: false,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        };
        user_runtime_id = user.id.to_string();
        if let Err(error) = insert_runtime_record(
            &mut transaction,
            "user",
            &user_runtime_id,
            None,
            Some(user.id.as_uuid()),
            Some(&normalized_email),
            &user,
        )
        .await
        {
            return internal_reason(error);
        }
    }

    let membership_lookup = format!("{}:{}", organization_id, user.id);
    let existing_membership = match sqlx::query(
        "SELECT id FROM saas_runtime_records
          WHERE kind = 'membership' AND lookup_key = $1
          FOR UPDATE",
    )
    .bind(&membership_lookup)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(value) => value,
        Err(error) => return internal_reason(error),
    };
    if existing_membership.is_some() {
        let _ = transaction.rollback().await;
        return response(
            StatusCode::CONFLICT,
            "already_a_member",
            "this account is already a member of the invited organization",
        );
    }

    let membership = Membership::new(
        organization_id,
        user.id,
        role,
        invited_by_uuid.map(UserId::from),
        now,
    );
    if let Err(error) = insert_runtime_record(
        &mut transaction,
        "membership",
        &membership.id.to_string(),
        Some(organization_uuid),
        Some(user.id.as_uuid()),
        Some(&membership_lookup),
        &membership,
    )
    .await
    {
        return internal_reason(error);
    }

    let generated_session = generate_token("ses");
    let session = SessionRecord::new(
        user.id,
        Some(organization_id),
        generated_session.hash.clone(),
        generated_session.prefix.clone(),
        Duration::hours(DEFAULT_SESSION_TTL_HOURS),
        now,
    );
    if let Err(error) = insert_runtime_record(
        &mut transaction,
        "session",
        &session.id.to_string(),
        Some(organization_uuid),
        Some(user.id.as_uuid()),
        Some(&session.token_hash),
        &session,
    )
    .await
    {
        return internal_reason(error);
    }

    let accepted = sqlx::query(
        "UPDATE invites
            SET status = 'accepted', accepted_at = now(), accepted_by = $1
          WHERE id = $2 AND status = 'pending'",
    )
    .bind(user.id.as_uuid())
    .bind(invitation_id)
    .execute(&mut *transaction)
    .await;
    match accepted {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => {
            let _ = transaction.rollback().await;
            return response(
                StatusCode::CONFLICT,
                "invitation_already_used",
                "invitation was accepted by another request",
            );
        }
        Err(error) => return internal_reason(error),
    }

    if let Err(error) = transaction.commit().await {
        return internal_reason(error);
    }

    state
        .audit
        .success(
            "saas.invitation",
            "saas.team.invite_accepted",
            Some(&invitation_id.to_string()),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "user": user.profile(),
            "organization_id": organization_id.to_string(),
            "membership": {
                "id": membership.id.to_string(),
                "role": role.as_str(),
                "status": membership.status.as_str(),
            },
            "session": {
                "id": session.id.to_string(),
                "prefix": session.token_prefix,
                "expires_at": session.expires_at,
                "organization_id": organization_id.to_string(),
            },
            "token": generated_session.plaintext,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_not_the_plaintext() {
        let token = generate_token("inv");
        assert_ne!(token.hash, token.plaintext);
        assert_eq!(hash_token(&token.plaintext), token.hash);
    }

    #[test]
    fn new_account_requires_the_existing_password_floor() {
        assert!(MIN_PASSWORD_LEN >= 12);
    }

    #[test]
    fn invitation_role_must_be_a_known_customer_role() {
        assert!(MembershipRole::parse("viewer").is_some());
        assert!(MembershipRole::parse("not-a-role").is_none());
    }
}
