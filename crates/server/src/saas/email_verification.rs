//! Email verification flow (GAP-MAP v2 P1).
//!
//! * `POST /api/saas/email-verification/request` — `{ "email": "…" }`.
//!   Always answers `202` with the same body (no account enumeration).
//!   For a real, unverified account it revokes any prior open token for
//!   that user+email, issues a fresh single-use token (hash-only, 24-hour
//!   TTL) and enqueues the verification email.
//! * `POST /api/saas/email-verification/confirm` — `{ "token": "…" }`.
//!   Locates the token by hash, re-verifies constant-time, consumes it
//!   atomically, and flips `users.email_verified`.
//!
//! Verification is an anti-abuse signal (proof the address is reachable
//! and controlled), NOT an authentication factor; login does not depend on
//! it, and this module never grants sessions.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use tracing::info;

use bot_core::session::token::{generate_token, hash_token, verify_token};
use bot_core::tenant::User;

use crate::api::ApiState;
use crate::email::{outbox, templates};

/// Verification links live longer than password resets (24 h) because they
/// are lower risk and users often sit on them.
const TOKEN_TTL_MINUTES: i64 = 24 * 60;

#[derive(Debug, Deserialize)]
pub struct RequestBody {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub token: String,
}

fn request_accepted() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": "If that address belongs to an account that still needs verification, a confirmation link has been sent.",
            "expires_minutes": TOKEN_TTL_MINUTES
        })),
    )
        .into_response()
}

/// `POST /api/saas/email-verification/request`
pub async fn request_verification(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Json(body): Json<RequestBody>,
) -> Response {
    let Some(db) = state.db.as_deref() else {
        return request_accepted();
    };
    let email = User::normalize_email(body.email.trim());
    if email.is_empty() {
        return request_accepted();
    }
    let row = match sqlx::query(
        "SELECT id, email_verified, status FROM users WHERE lower(email) = lower($1)",
    )
    .bind(&email)
    .fetch_optional(db.pool())
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return request_accepted(),
        Err(e) => {
            tracing::warn!(error = %e, "email verification lookup failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "email_verification_unavailable" })),
            )
                .into_response();
        }
    };
    let already_verified: bool = row.try_get("email_verified").unwrap_or(true);
    let status: String = row.try_get("status").unwrap_or_default();
    if already_verified || status != "active" {
        // Nothing to verify (or not an active account) — same generic answer.
        return request_accepted();
    }
    let user_id: uuid::Uuid = match row.try_get("id") {
        Ok(id) => id,
        Err(_) => return request_accepted(),
    };

    // One open token per user+email: revoke the previous one.
    let revoke = sqlx::query(
        "UPDATE email_verifications
            SET revoked_at = now()
          WHERE user_id = $1 AND lower(email) = lower($2)
            AND verified_at IS NULL AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(&email)
    .execute(db.pool())
    .await;
    if let Err(e) = revoke {
        tracing::warn!(error = %e, "could not revoke prior verification tokens");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "email_verification_unavailable" })),
        )
            .into_response();
    }

    let token = generate_token("ver");
    let insert = sqlx::query(
        "INSERT INTO email_verifications (user_id, email, token_hash, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(mins => $4))",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&token.hash)
    .bind(TOKEN_TTL_MINUTES)
    .execute(db.pool())
    .await;
    if let Err(e) = insert {
        tracing::warn!(error = %e, "could not persist verification token");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "email_verification_unavailable" })),
        )
            .into_response();
    }

    let link = format!(
        "{}/verify-email?token={}",
        public_base_url(),
        token.plaintext
    );
    let rendered = templates::email_verification(&email, &link, TOKEN_TTL_MINUTES as u32);
    let dedup = format!("email_verification:{user_id}:{}", token.prefix);
    if let Err(e) = outbox::enqueue(
        db.pool(),
        &dedup,
        rendered.template_key,
        &email,
        &rendered.subject,
        &rendered.body_text,
        Some(&rendered.body_html),
        Some(user_id),
        None,
    )
    .await
    {
        tracing::warn!(error = %e, "verification email could not be enqueued");
    }
    info!(prefix = %token.prefix, "email verification requested");
    request_accepted()
}

/// `POST /api/saas/email-verification/confirm`
pub async fn confirm_verification(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Json(body): Json<ConfirmBody>,
) -> Response {
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "email_verification_unavailable" })),
        )
            .into_response();
    };
    let presented_hash = hash_token(body.token.trim());
    let row = match sqlx::query(
        "SELECT id, user_id, token_hash
           FROM email_verifications
          WHERE token_hash = $1
            AND verified_at IS NULL
            AND revoked_at IS NULL
            AND expires_at > now()",
    )
    .bind(&presented_hash)
    .fetch_optional(db.pool())
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "verification token lookup failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "email_verification_unavailable" })),
            )
                .into_response();
        }
    };
    let Some(row) = row else {
        return invalid_token();
    };
    let stored_hash: String = match row.try_get("token_hash") {
        Ok(h) => h,
        Err(_) => return invalid_token(),
    };
    if !verify_token(body.token.trim(), &stored_hash) {
        return invalid_token();
    }
    let token_id: uuid::Uuid = match row.try_get("id") {
        Ok(id) => id,
        Err(_) => return invalid_token(),
    };
    let user_id: uuid::Uuid = match row.try_get("user_id") {
        Ok(id) => id,
        Err(_) => return invalid_token(),
    };

    // Atomic single-use claim.
    let consumed = match sqlx::query(
        "UPDATE email_verifications
            SET verified_at = now()
          WHERE id = $1 AND verified_at IS NULL AND expires_at > now()",
    )
    .bind(token_id)
    .execute(db.pool())
    .await
    {
        Ok(r) => r.rows_affected() == 1,
        Err(e) => {
            tracing::warn!(error = %e, "verification token consume failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "email_verification_unavailable" })),
            )
                .into_response();
        }
    };
    if !consumed {
        return invalid_token();
    }

    // Flip the flag (idempotent).
    let update = sqlx::query(
        "UPDATE users SET email_verified = true, updated_at = now()
          WHERE id = $1 AND email_verified = false",
    )
    .bind(user_id)
    .execute(db.pool())
    .await;
    match update {
        Ok(r) => info!(newly_verified = r.rows_affected() == 1, "email verification complete"),
        Err(e) => {
            tracing::warn!(error = %e, "email_verified flag update failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "email_verification_unavailable" })),
            )
                .into_response();
        }
    }
    (
        StatusCode::OK,
        Json(json!({
            "status": "verified",
            "message": "Your email address is confirmed."
        })),
    )
        .into_response()
}

/// Uniform denial for every failed confirm (expired/revoked/used/unknown).
fn invalid_token() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "error": "invalid_or_expired_token",
            "message": "This verification link is no longer valid. Request a new one."
        })),
    )
        .into_response()
}

fn public_base_url() -> String {
    std::env::var("PUBLIC_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/email-verification/request",
            post(request_verification),
        )
        .route(
            "/api/saas/email-verification/confirm",
            post(confirm_verification),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denials_are_uniform() {
        assert_eq!(invalid_token().status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(request_accepted().status(), StatusCode::ACCEPTED);
    }

    #[test]
    fn verification_tokens_use_the_ver_namespace_and_verify() {
        let t = generate_token("ver");
        assert!(t.plaintext.starts_with("ver_"));
        assert!(verify_token(&t.plaintext, &t.hash));
        assert!(!verify_token("ver_zzzz", &t.hash));
    }
}
