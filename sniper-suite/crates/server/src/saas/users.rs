//! SaaS user lifecycle (TASK 7A file 23).
//!
//! Registration, login, the current-user view and safe profile updates.
//! The pre-TASK-7A control plane had no user concept at all — only
//! deployment keys — so everything here is new surface rather than a
//! rewrite of existing behaviour.
//!
//! Security rules this module enforces:
//!
//! * a password is hashed with PBKDF2 before it touches storage, and the
//!   hash is never serialised (handlers return
//!   [`bot_core::tenant::UserProfile`], which has no hash field);
//! * a session token is returned exactly once, at login;
//! * a caller may only update their own safe profile fields — display name
//!   and password — never their status, never `platform_admin`, never a
//!   role;
//! * changing a password revokes every existing session of that user.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::session::token::{generate_token, hash_password, verify_password};
use bot_core::session::{SessionRecord, DEFAULT_SESSION_TTL_HOURS};
use bot_core::tenant::{User, UserId, UserStatus};

use super::middleware::{authorize_request, deny_response};
use crate::api::ApiState;

/// Minimum password length. Deliberately a floor, not a complexity ritual:
/// length is what resists guessing.
pub const MIN_PASSWORD_LEN: usize = 12;

/// Validate an email well enough to reject obvious nonsense without
/// pretending to implement RFC 5322.
pub fn valid_email(email: &str) -> bool {
    let e = email.trim();
    if e.len() < 3 || e.len() > 320 || e.contains(char::is_whitespace) {
        return false;
    }
    match e.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

/// Registration body.
#[derive(Debug, Deserialize)]
pub struct RegisterBody {
    /// Login address.
    pub email: String,
    /// Plaintext password (hashed immediately, never stored).
    pub password: String,
    /// Display name.
    #[serde(default)]
    pub display_name: String,
}

/// Build a user record from a registration request.
pub fn build_user(email: &str, password: &str, display_name: &str) -> Result<User, String> {
    if !valid_email(email) {
        return Err("invalid email address".into());
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    let now = Utc::now();
    Ok(User {
        id: UserId::new(),
        email: User::normalize_email(email),
        email_verified: false,
        display_name: display_name.trim().to_string(),
        password_hash: hash_password(password),
        status: UserStatus::Active,
        platform_admin: false,
        created_at: now,
        updated_at: now,
        last_login_at: None,
    })
}

/// `POST /api/saas/users` — register. Public by design (it is how a
/// customer signs up); provisioning turns the account into a tenant.
pub async fn register(State(state): State<ApiState>, Json(body): Json<RegisterBody>) -> Response {
    let user = match build_user(&body.email, &body.password, &body.display_name) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    if let Err(e) = state.saas.create_user(&user).await {
        // Do not distinguish "taken" from other failures in a way that
        // enumerates accounts; the message is generic.
        return (StatusCode::CONFLICT, e.to_string()).into_response();
    }
    state
        .audit
        .success("saas", "saas.user.registered", Some(&user.id.to_string()))
        .await;
    (StatusCode::CREATED, Json(json!({ "user": user.profile() }))).into_response()
}

/// Login body.
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    /// Login address.
    pub email: String,
    /// Plaintext password.
    pub password: String,
    /// Optional organization slug to scope the session to immediately.
    #[serde(default)]
    pub organization: Option<String>,
}

/// `POST /api/saas/sessions` — log in and receive a session token ONCE.
pub async fn login(State(state): State<ApiState>, Json(body): Json<LoginBody>) -> Response {
    let unauthorized = || {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_credentials" })),
        )
            .into_response()
    };
    let Some(mut user) = state.saas.user_by_email(&body.email).await else {
        // Hash anyway so a missing account and a wrong password take a
        // similar amount of work.
        let _ = verify_password(&body.password, "pbkdf2-sha256$600000$c2FsdA$aGFzaA");
        return unauthorized();
    };
    if !verify_password(&body.password, &user.password_hash) {
        return unauthorized();
    }
    if !user.can_authenticate() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "account_not_active" })),
        )
            .into_response();
    }

    // Optional immediate tenant scoping — only into an organization the
    // user is actually a member of.
    let mut organization_id = None;
    if let Some(slug) = body.organization.as_deref() {
        let Some(org) = state.saas.organization_by_slug(slug).await else {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "no_membership" })),
            )
                .into_response();
        };
        if state.saas.membership(org.id, user.id).await.is_none() {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "no_membership" })),
            )
                .into_response();
        }
        organization_id = Some(org.id);
    }

    let now = Utc::now();
    let token = generate_token("ses");
    let session = SessionRecord::new(
        user.id,
        organization_id,
        token.hash.clone(),
        token.prefix.clone(),
        Duration::hours(DEFAULT_SESSION_TTL_HOURS),
        now,
    );
    if let Err(e) = state.saas.create_session(&session).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    user.last_login_at = Some(now);
    user.updated_at = now;
    let _ = state.saas.update_user(&user).await;
    state
        .audit
        .success("saas", "saas.session.created", Some(&session.token_prefix))
        .await;

    Json(json!({
        "user": user.profile(),
        "session": {
            "id": session.id,
            "prefix": session.token_prefix,
            "expires_at": session.expires_at,
            "organization_id": session.organization_id,
        },
        // The ONLY time the session token is ever returned.
        "token": token.plaintext,
    }))
    .into_response()
}

/// `GET /api/saas/users/me` — the authenticated user and their tenants.
pub async fn current_user(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let memberships = match &ctx.user {
        Some(u) => state.saas.memberships_of_user(u.id).await,
        None => Vec::new(),
    };
    let orgs: Vec<serde_json::Value> = {
        let mut v = Vec::new();
        for m in &memberships {
            if let Some(o) = state.saas.organization(m.organization_id).await {
                v.push(json!({
                    "organization_id": o.id,
                    "slug": o.slug,
                    "name": o.name,
                    "status": o.status.as_str(),
                    "role": m.role.as_str(),
                    "membership_status": m.status.as_str(),
                }));
            }
        }
        v
    };
    Json(json!({
        "user": ctx.user.as_ref().map(|u| u.profile()),
        "principal": ctx.authorization.principal,
        "active_organization": {
            "organization_id": ctx.organization.id,
            "slug": ctx.organization.slug,
            "status": ctx.organization.status.as_str(),
            "role": ctx.authorization.role.as_str(),
        },
        "permissions": ctx.authorization.permissions.to_strings(),
        "organizations": orgs,
    }))
    .into_response()
}

/// Safe profile update body. Deliberately narrow: no status, no
/// `platform_admin`, no role — those are not self-service fields.
#[derive(Debug, Deserialize)]
pub struct UpdateProfileBody {
    /// New display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Current password, required to change the password.
    #[serde(default)]
    pub current_password: Option<String>,
    /// New password.
    #[serde(default)]
    pub new_password: Option<String>,
}

/// `PATCH /api/saas/users/me` — update display name and/or password.
pub async fn update_profile(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<UpdateProfileBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(mut user) = ctx.user.clone() else {
        return (
            StatusCode::FORBIDDEN,
            "this credential is not tied to a user account",
        )
            .into_response();
    };

    let now = Utc::now();
    let mut password_changed = false;
    if let Some(new_password) = body.new_password.as_deref() {
        let Some(current) = body.current_password.as_deref() else {
            return (
                StatusCode::BAD_REQUEST,
                "current_password is required to change the password",
            )
                .into_response();
        };
        if !verify_password(current, &user.password_hash) {
            return (StatusCode::FORBIDDEN, "current password is incorrect").into_response();
        }
        if new_password.chars().count() < MIN_PASSWORD_LEN {
            return (
                StatusCode::BAD_REQUEST,
                format!("password must be at least {MIN_PASSWORD_LEN} characters"),
            )
                .into_response();
        }
        user.password_hash = hash_password(new_password);
        password_changed = true;
    }
    if let Some(name) = body.display_name.as_deref() {
        user.display_name = name.trim().to_string();
    }
    user.updated_at = now;
    if let Err(e) = state.saas.update_user(&user).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // A password change invalidates every existing session.
    let revoked = if password_changed {
        let n = state
            .saas
            .revoke_user_sessions(user.id, "password changed", now)
            .await;
        state
            .audit
            .success(
                &ctx.actor_label(),
                "saas.user.password_changed",
                Some(&user.id.to_string()),
            )
            .await;
        n
    } else {
        0
    };

    Json(json!({
        "user": user.profile(),
        "sessions_revoked": revoked,
    }))
    .into_response()
}

/// `POST /api/saas/users/me/logout` — revoke the presented session.
pub async fn logout(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let bot_core::authorization::Principal::UserSession { session_id } =
        &ctx.authorization.principal
    else {
        return (
            StatusCode::BAD_REQUEST,
            "only a user session can be logged out",
        )
            .into_response();
    };
    let Some(user) = ctx.user.as_ref() else {
        return (StatusCode::BAD_REQUEST, "no user on this session").into_response();
    };
    // Revoke by looking the session up through the user's own list, so a
    // caller cannot revoke somebody else's session id.
    let now = Utc::now();
    let mut revoked = false;
    for s in state.saas.sessions_of_user(user.id).await {
        if s.id.to_string() == *session_id {
            let mut s = s;
            if s.revoke("logout", now) {
                let _ = state.saas.update_session(&s).await;
                revoked = true;
            }
        }
    }
    Json(json!({ "ok": true, "revoked": revoked })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation_rejects_nonsense() {
        for good in [
            "a@b.co",
            "person.name+tag@example.com",
            "  spaced@example.com  ",
        ] {
            assert!(valid_email(good), "{good} should be valid");
        }
        for bad in [
            "",
            "no-at-sign",
            "@example.com",
            "a@nodot",
            "a@.com",
            "a@com.",
            "has space@example.com",
        ] {
            assert!(!valid_email(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn building_a_user_hashes_the_password_and_normalises_the_email() {
        let u = build_user("Person@Example.COM", "correct horse battery", "Person").unwrap();
        assert_eq!(u.email, "person@example.com");
        assert!(u.password_hash.starts_with("pbkdf2-sha256$"));
        assert!(!u.password_hash.contains("correct"));
        assert!(verify_password("correct horse battery", &u.password_hash));
        assert!(!u.email_verified);
        assert!(
            !u.platform_admin,
            "registration never grants platform scope"
        );
        assert_eq!(u.status, UserStatus::Active);
        // The serialisable profile has no hash.
        let json = serde_json::to_string(&u.profile()).unwrap();
        assert!(!json.contains("pbkdf2"));
    }

    #[test]
    fn weak_inputs_are_refused() {
        assert!(build_user("bad-email", "correct horse battery", "x").is_err());
        let short = build_user("a@b.co", "short", "x").unwrap_err();
        assert!(short.contains("at least"));
        assert!(build_user("a@b.co", &"x".repeat(MIN_PASSWORD_LEN), "x").is_ok());
    }
}
