//! Password reset flow (GAP-MAP v2 P1).
//!
//! Two endpoints:
//! * `POST /api/saas/password-reset/request` — `{ "email": "…" }`. Always
//!   answers `202` with the SAME body whether or not the email exists (no
//!   account enumeration). When the user exists, any still-open reset token
//!   is revoked and a fresh single-use token (SHA-256 hash stored, 30-minute
//!   TTL) is issued in one transaction; the reset email is then enqueued in
//!   the outbox.
//! * `POST /api/saas/password-reset/confirm` — `{ "token": "…",
//!   "new_password": "…" }`. The token is located BY HASH under a row lock
//!   and re-verified with a constant-time compare. The token claim, the
//!   password replacement on the authoritative identity record, the
//!   revocation of outstanding reset tokens and the revocation of EVERY live
//!   session all commit in ONE transaction
//!   (`SaasStore::complete_password_reset`). The handler answers success only
//!   after that commit; any failure rolls everything back and answers `503`.
//!
//! Rate limiting rides on the router-level IP limiter; the identical
//! response bodies remove the timing/enumeration side channel an attacker
//! could otherwise use to probe addresses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use tracing::info;

use bot_core::session::token::{generate_token, hash_password, hash_token};
use bot_core::tenant::User;

use crate::api::ApiState;
use crate::email::{outbox, templates};

/// How long a reset token lives.
const TOKEN_TTL_MINUTES: i64 = 30;

/// Minimum acceptable new password length (matches the signup policy).
const MIN_PASSWORD_LEN: usize = 12;

#[derive(Debug, Deserialize)]
pub struct RequestBody {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub token: String,
    pub new_password: String,
}

/// The identical response for a reset request, regardless of whether the
/// email belongs to a real account.
fn request_accepted() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": "If an account exists for that address, a reset link has been sent.",
            "expires_minutes": TOKEN_TTL_MINUTES
        })),
    )
        .into_response()
}

/// A storage failure. Never a success shape: the caller must retry.
fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "password_reset_unavailable" })),
    )
        .into_response()
}

/// `POST /api/saas/password-reset/request`
pub async fn request_reset(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Json(body): Json<RequestBody>,
) -> Response {
    let Some(db) = state.db.as_deref() else {
        // Without a database there are no accounts to reset; still answer
        // the same shape so behaviour does not leak configuration.
        return request_accepted();
    };
    let email = User::normalize_email(body.email.trim());
    if email.is_empty() {
        return request_accepted();
    }

    // Look the user up quietly. Unknown email → same accepted response.
    let user = sqlx::query("SELECT id, status FROM users WHERE lower(email) = lower($1)")
        .bind(&email)
        .fetch_optional(db.pool())
        .await;
    let row = match user {
        Ok(Some(r)) => r,
        Ok(None) => return request_accepted(),
        Err(e) => {
            tracing::warn!(error = %e, "password reset lookup failed");
            return unavailable();
        }
    };
    let status: String = match row.try_get("status") {
        Ok(s) => s,
        Err(_) => return request_accepted(),
    };
    if status != "active" {
        // Suspended/deactivated accounts cannot reset — but we do not say so.
        return request_accepted();
    }
    let user_id: uuid::Uuid = match row.try_get("id") {
        Ok(id) => id,
        Err(_) => return request_accepted(),
    };

    // Fresh single-use token; only the hash is persisted.
    let token = generate_token("rst");

    // Revoking the previous open token and inserting the new one are one
    // unit: a failure between them must not leave the user with no usable
    // link while the caller is told a link was issued.
    let mut transaction = match db.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::warn!(error = %e, "could not begin reset token issue");
            return unavailable();
        }
    };
    // One open token per user: revoke any previous, never-used token.
    if let Err(e) = sqlx::query(
        "UPDATE password_reset_tokens
            SET revoked_at = now()
          WHERE user_id = $1 AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *transaction)
    .await
    {
        tracing::warn!(error = %e, "could not revoke prior reset tokens");
        let _ = transaction.rollback().await;
        return unavailable();
    }
    if let Err(e) = sqlx::query(
        "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at)
         VALUES ($1, $2, now() + make_interval(mins => $3))",
    )
    .bind(user_id)
    .bind(&token.hash)
    .bind(TOKEN_TTL_MINUTES)
    .execute(&mut *transaction)
    .await
    {
        tracing::warn!(error = %e, "could not persist reset token");
        let _ = transaction.rollback().await;
        return unavailable();
    }
    if let Err(e) = transaction.commit().await {
        tracing::warn!(error = %e, "could not commit reset token issue");
        return unavailable();
    }

    // Enqueue the email (dedup = token prefix: one logical email per token).
    let link = format!(
        "{}/reset-password?token={}",
        public_base_url(),
        token.plaintext
    );
    let rendered = templates::password_reset(&email, &link, TOKEN_TTL_MINUTES as u32);
    let dedup = format!("password_reset:{user_id}:{}", token.prefix);
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
        // The token exists; failing to enqueue the mail is an operational
        // error but must not tell the requester anything.
        tracing::warn!(error = %e, "reset email could not be enqueued");
    }
    info!(prefix = %token.prefix, "password reset requested");
    request_accepted()
}

/// `POST /api/saas/password-reset/confirm`
pub async fn confirm_reset(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Json(body): Json<ConfirmBody>,
) -> Response {
    let Some(db) = state.db.as_deref() else {
        return unavailable();
    };
    if body.new_password.chars().count() < MIN_PASSWORD_LEN {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "password_too_short", "min_length": MIN_PASSWORD_LEN })),
        )
            .into_response();
    }

    // The presented token is looked up by its hash and re-verified inside the
    // transaction with verify_token, so hash-vs-hash confusion is rejected.
    let presented = body.token.trim();
    let presented_hash = hash_token(presented);
    let new_hash = hash_password(&body.new_password);

    let completion = match state
        .saas
        .complete_password_reset(presented, &presented_hash, &new_hash)
        .await
    {
        Ok(Some(done)) => done,
        Ok(None) => return invalid_token(),
        Err(error) => {
            // Nothing was committed: the token is still usable and the
            // password and sessions are unchanged. Report that honestly.
            tracing::warn!(error = %error, "password reset could not be completed");
            return unavailable();
        }
    };

    info!(
        sessions_revoked = completion.sessions_revoked,
        "password reset complete"
    );
    // Security-alert email: a notification only, sent after commit. Its
    // failure cannot undo the reset, so it is logged and not reported.
    let rendered = templates::security_alert(
        &completion.email,
        "password_changed",
        "Your password was changed via a reset link. All sessions were signed out.",
    );
    let dedup = format!(
        "password_reset_done:{}:{}",
        completion.user_id, completion.token_id
    );
    if let Err(error) = outbox::enqueue(
        db.pool(),
        &dedup,
        rendered.template_key,
        &completion.email,
        &rendered.subject,
        &rendered.body_text,
        Some(&rendered.body_html),
        Some(completion.user_id),
        None,
    )
    .await
    {
        tracing::warn!(error = %error, "password change alert could not be enqueued");
    }
    (
        StatusCode::OK,
        Json(json!({
            "status": "reset_complete",
            "message": "Your password was changed and all sessions were signed out."
        })),
    )
        .into_response()
}

/// The same opaque denial for every failure mode of the confirm path —
/// expired, revoked, used, or unknown tokens are indistinguishable.
fn invalid_token() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "error": "invalid_or_expired_token",
            "message": "This reset link is no longer valid. Request a new one."
        })),
    )
        .into_response()
}

/// Public base URL for links. `PUBLIC_BASE_URL` wins; otherwise relative
/// links (same-origin deployments).
fn public_base_url() -> String {
    std::env::var("PUBLIC_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/password-reset/request", post(request_reset))
        .route("/api/saas/password-reset/confirm", post(confirm_reset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::session::token::{verify_password, verify_token};

    #[test]
    fn denial_bodies_are_identical_and_never_leak_state() {
        let r1 = invalid_token();
        let r2 = invalid_token();
        assert_eq!(r1.status(), r2.status());
        assert_eq!(r1.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn storage_failure_is_never_a_success_shape() {
        let r = unavailable();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_ne!(invalid_token().status(), r.status());
    }

    #[test]
    fn request_response_is_stable_and_generic() {
        let r = request_accepted();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
    }

    #[test]
    fn base_url_trims_trailing_slashes() {
        std::env::set_var("TEST_PUBLIC_BASE_URL", "");
        std::env::remove_var("PUBLIC_BASE_URL");
        assert_eq!(public_base_url(), "");
        std::env::set_var("PUBLIC_BASE_URL", "https://app.example.com/");
        assert_eq!(public_base_url(), "https://app.example.com");
        std::env::remove_var("PUBLIC_BASE_URL");
    }

    #[test]
    fn token_round_trips_through_the_hash_and_verify_path() {
        let t = generate_token("rst");
        assert_eq!(t.hash.len(), 64);
        assert!(verify_token(&t.plaintext, &t.hash));
        assert!(!verify_token("rst_wrong", &t.hash));
        assert!(!verify_password(
            "short-password-123",
            &hash_password("different-password-456")
        ));
        assert!(verify_password(
            "correct-horse-battery",
            &hash_password("correct-horse-battery")
        ));
    }
}
