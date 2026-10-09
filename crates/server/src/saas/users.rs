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
use sqlx::Row;

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
/// Bound PBKDF2 input work and keep login, signup, and invitation policy aligned.
pub const MAX_PASSWORD_LEN: usize = 512;
/// Prevent unbounded user-controlled profile payloads from entering storage.
pub const MAX_DISPLAY_NAME_LEN: usize = 256;
const DUMMY_PASSWORD_HASH: &str =
    "pbkdf2-sha256$600000$pcTXkx8rboBanELX4R9rgA$mrXKihu3Ze1lxX5c7tiPcM8oJKSx-t6QB222NARelhk";

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
    /// Sign-up consent record (version + timestamp), captured by the
    /// control-plane `ConsentCheckbox`. Optional for API compatibility:
    /// SDK-driven signups may not carry it, and its absence is recorded as
    /// such rather than treated as consent.
    #[serde(default)]
    pub consent: Option<SignupConsent>,
}

/// The consent a signing-up user gives to the legal documents.
///
/// Stored in the durable, hash-chained audit trail (NOT invented columns):
/// the record is exactly what the UI captured — which document version was
/// accepted, and when. See `saas.user.consent_recorded` in the audit log.
#[derive(Debug, Deserialize)]
pub struct SignupConsent {
    /// Version string of the accepted legal bundle (e.g. "2026-10-07.1").
    pub version: String,
    /// ISO-8601 timestamp captured by the client when the box was ticked.
    pub accepted_at: String,
}

/// Build a user record from a registration request.
pub fn build_user(email: &str, password: &str, display_name: &str) -> Result<User, String> {
    if !valid_email(email) {
        return Err("invalid email address".into());
    }
    if password.len() > MAX_PASSWORD_LEN * 4 || password.chars().count() > MAX_PASSWORD_LEN {
        return Err(format!(
            "password must be at most {MAX_PASSWORD_LEN} characters"
        ));
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    if display_name.chars().count() > MAX_DISPLAY_NAME_LEN {
        return Err(format!(
            "display name must be at most {MAX_DISPLAY_NAME_LEN} characters"
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
    if !valid_email(&body.email) {
        return (StatusCode::BAD_REQUEST, "invalid email address").into_response();
    }
    if let Some(response) = crate::saas::rate_limit::reject_sensitive_attempt(
        &state,
        "registration",
        &User::normalize_email(&body.email),
    )
    .await
    {
        return response;
    }
    let user = match build_user(&body.email, &body.password, &body.display_name) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    if let Err(error) = state.saas.create_user(&user).await {
        if matches!(&error, bot_core::error::BotError::InvalidArgument(_)) {
            // Keep duplicate-account responses indistinguishable from other
            // registration conflicts; never return the repository detail.
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "registration_conflict",
                    "reason": "an account could not be created with these details",
                })),
            )
                .into_response();
        }
        tracing::error!(error = %error, "user registration storage failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "identity_storage_unavailable",
                "reason": "account records could not be saved",
            })),
        )
            .into_response();
    }
    state
        .audit
        .success("saas", "saas.user.registered", Some(&user.id.to_string()))
        .await;
    // Record sign-up consent (version + timestamp) in the same durable,
    // hash-chained audit trail. A missing consent field is logged as such so
    // the absence is auditable too — consent is never implied.
    let consent_detail = match &body.consent {
        Some(consent) => json!({
            "consent_version": consent.version,
            "consent_accepted_at": consent.accepted_at,
            "captured": true,
        }),
        None => json!({ "captured": false }),
    };
    state
        .audit
        .record(
            "saas",
            "saas.user.consent_recorded",
            Some(&user.id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            consent_detail,
        )
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
    /// TOTP code when the selected organization requires MFA.
    #[serde(default)]
    pub mfa_code: Option<String>,
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
    if body.email.trim().is_empty()
        || body.email.trim().len() > 320
        || body.password.len() > MAX_PASSWORD_LEN * 4
        || body.password.chars().count() > MAX_PASSWORD_LEN
        || body.organization.as_deref().is_some_and(|value| {
            value.trim().is_empty() || value.trim().len() > 128
        })
        || body
            .mfa_code
            .as_deref()
            .is_some_and(|value| value.len() > 16)
    {
        return unauthorized();
    }
    if let Some(response) = crate::saas::rate_limit::reject_sensitive_attempt(
        &state,
        "login",
        &User::normalize_email(&body.email),
    )
    .await
    {
        return response;
    }
    let mut user = match state.saas.user_by_email(&body.email).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            // Hash anyway so a missing account and a wrong password take a
            // similar amount of work.
            let _ = verify_password(&body.password, DUMMY_PASSWORD_HASH);
            return unauthorized();
        }
        Err(error) => {
            tracing::error!(error = %error, "login user lookup failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "identity_storage_unavailable",
                    "reason": "account records could not be loaded",
                })),
            )
                .into_response();
        }
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

    // An MFA-protected organization must be selected at login so the code
    // can be verified before a session token is issued. An unscoped session
    // must never become a bypass around an organization policy.
    if body.organization.is_none() {
        if let Some(db) = state.db.as_deref() {
            // `saas_runtime_records` is the authoritative membership store.
            // The normalized `organization_members` table is not written by
            // the current SaaS repository and must not decide whether MFA is
            // required for an unscoped login.
            let protected = sqlx::query(
                "SELECT EXISTS (SELECT 1 FROM saas_runtime_records m JOIN tenant_security_policies tsp ON tsp.organization_id = m.organization_id WHERE m.kind = 'membership' AND m.user_id = $1 AND m.record->>'status' IS DISTINCT FROM 'removed' AND tsp.mfa_enforced = true) AS protected",
            )
            .bind(user.id.as_uuid())
            .fetch_one(db.pool())
            .await;
            let protected = match protected {
                Ok(row) => match row.try_get::<bool, _>("protected") {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::error!(error = %error, user = %user.id, "MFA policy result during login could not be decoded");
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({
                                "error": "identity_storage_unavailable",
                                "reason": "MFA policy records could not be decoded"
                            })),
                        )
                            .into_response();
                    }
                },
                Err(error) => {
                    tracing::error!(error = %error, user = %user.id, "MFA policy lookup during login failed");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": "identity_storage_unavailable",
                            "reason": "MFA policy records could not be loaded"
                        })),
                    )
                        .into_response();
                }
            };
            if protected {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "organization_required_for_mfa",
                        "mfa_required": true,
                        "reason": "select the MFA-protected organization and provide mfa_code"
                    })),
                )
                    .into_response();
            }
        }
    }

    // Optional immediate tenant scoping — only into an active membership.
    let mut organization_id = None;
    let mut mfa_policy_version = None;
    if let Some(slug) = body.organization.as_deref() {
        let org = match state.saas.organization_by_slug(slug).await {
            Ok(Some(value)) => value,
            Ok(None) => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "no_membership" })),
                )
                    .into_response();
            }
            Err(error) => {
                tracing::error!(error = %error, "login organization lookup failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "identity_storage_unavailable",
                        "reason": "organization records could not be loaded",
                    })),
                )
                    .into_response();
            }
        };
        match state.saas.membership(org.id, user.id).await {
            Ok(Some(membership)) if membership.status.is_active() => {}
            Ok(Some(_)) | Ok(None) => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "no_membership" })),
                )
                    .into_response();
            }
            Err(error) => {
                tracing::error!(error = %error, organization = %org.id, user = %user.id, "login membership lookup failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "identity_storage_unavailable",
                        "reason": "membership records could not be loaded",
                    })),
                )
                    .into_response();
            }
        }
        let mfa_subject = format!("{}:{}", user.id, org.id);
        if let Some(response) = crate::saas::rate_limit::reject_sensitive_attempt(
            &state,
            "login_totp",
            &mfa_subject,
        )
        .await
        {
            return response;
        }
        match crate::saas::security::verify_login_mfa(
            &state,
            user.id,
            org.id,
            body.mfa_code.as_deref(),
        )
        .await
        {
            Ok(policy_version) => mfa_policy_version = policy_version,
            Err(reason) if reason == "mfa_enrollment_required" => {
                if let Err(key_reason) = crate::saas::security::mfa_enrollment_ready() {
                    tracing::error!(organization = %org.id, %key_reason, "MFA enrollment key is unavailable for login onboarding");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": "mfa_setup_unavailable",
                            "reason": "the organization requires MFA but secure enrollment is not configured"
                        })),
                    )
                        .into_response();
                }
                let enrollment_now = Utc::now();
                let enrollment_token = generate_token("ses");
                let mut enrollment_session = SessionRecord::new(
                    user.id,
                    Some(org.id),
                    enrollment_token.hash.clone(),
                    enrollment_token.prefix.clone(),
                    Duration::minutes(15),
                    enrollment_now,
                );
                enrollment_session.mfa_enrollment_only = true;
                if let Err(error) = state.saas.create_session(&enrollment_session).await {
                    tracing::error!(error = %error, user = %user.id, organization = %org.id, "MFA enrollment session could not be created");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "identity_storage_unavailable", "reason": "an MFA enrollment session could not be created" })),
                    )
                        .into_response();
                }
                user.last_login_at = Some(enrollment_now);
                user.updated_at = enrollment_now;
                if let Err(error) = state.saas.update_user(&user).await {
                    tracing::warn!(error = %error, user = %user.id, "MFA enrollment last-login timestamp could not be persisted");
                }
                state
                    .audit
                    .success(
                        "saas",
                        "saas.session.mfa_enrollment_started",
                        Some(&enrollment_session.token_prefix),
                    )
                    .await;
                return Json(json!({
                    "user": user.profile(),
                    "session": {
                        "id": enrollment_session.id,
                        "prefix": enrollment_session.token_prefix,
                        "expires_at": enrollment_session.expires_at,
                        "organization_id": enrollment_session.organization_id,
                    },
                    "token": enrollment_token.plaintext,
                    "mfa_enrollment_required": true,
                    "organization_id": org.id,
                    "organization_slug": org.slug,
                }))
                .into_response();
            }
            Err(reason) => {
                let status = if matches!(
                    reason.as_str(),
                    "mfa_challenge_required" | "invalid_totp_code"
                ) {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                return (
                    status,
                    Json(json!({
                        "error": reason,
                        "mfa_required": matches!(reason.as_str(), "mfa_challenge_required" | "invalid_totp_code"),
                        "organization_id": org.id,
                    })),
                )
                    .into_response();
            }
        }
        organization_id = Some(org.id);
    }

    let now = Utc::now();
    let token = generate_token("ses");
    let mut session = SessionRecord::new(
        user.id,
        organization_id,
        token.hash.clone(),
        token.prefix.clone(),
        Duration::hours(DEFAULT_SESSION_TTL_HOURS),
        now,
    );
    session.mfa_policy_updated_at = mfa_policy_version;
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
        Some(u) => match state.saas.memberships_of_user(u.id).await {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, user = %u.id, "user memberships could not be loaded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "membership_storage_unavailable",
                        "reason": "authoritative membership records could not be loaded",
                    })),
                )
                    .into_response();
            }
        },
        None => Vec::new(),
    };
    let orgs: Vec<serde_json::Value> = {
        let mut v = Vec::new();
        for m in &memberships {
            let o = match state.saas.organization(m.organization_id).await {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(error) => {
                    tracing::error!(error = %error, organization = %m.organization_id, "user organization could not be loaded");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": "organization_storage_unavailable",
                            "reason": "authoritative organization records could not be loaded",
                        })),
                    )
                        .into_response();
                }
            };
            v.push(json!({
                "organization_id": o.id,
                "slug": o.slug,
                "name": o.name,
                "status": o.status.as_str(),
                "role": m.role.as_str(),
                "membership_status": m.status.as_str(),
            }));
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
        if current.len() > MAX_PASSWORD_LEN * 4
            || current.chars().count() > MAX_PASSWORD_LEN
            || new_password.len() > MAX_PASSWORD_LEN * 4
            || new_password.chars().count() > MAX_PASSWORD_LEN
        {
            return (
                StatusCode::BAD_REQUEST,
                format!("password must be at most {MAX_PASSWORD_LEN} characters"),
            )
                .into_response();
        }
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
        if name.chars().count() > MAX_DISPLAY_NAME_LEN {
            return (
                StatusCode::BAD_REQUEST,
                format!("display name must be at most {MAX_DISPLAY_NAME_LEN} characters"),
            )
                .into_response();
        }
        user.display_name = name.trim().to_string();
    }
    user.updated_at = now;

    // Persist the password and revoke all existing sessions in one durable
    // transaction. A failed revocation must never leave the new password
    // committed while old credentials remain usable.
    let revoked = if password_changed {
        let count = match state
            .saas
            .update_user_and_revoke_sessions(&user, "password changed", now)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, user = %user.id, "password change and session revocation could not be committed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "password_change_unavailable",
                        "reason": "the password change and session revocation could not be committed",
                    })),
                )
                    .into_response();
            }
        };
        state
            .audit
            .success(
                &ctx.actor_label(),
                "saas.user.password_changed",
                Some(&user.id.to_string()),
            )
            .await;
        count
    } else {
        if let Err(error) = state.saas.update_user(&user).await {
            tracing::error!(error = %error, user = %user.id, "profile update could not be committed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "profile_storage_unavailable" })),
            )
                .into_response();
        }
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
    let sessions = match state.saas.sessions_of_user(user.id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, user = %user.id, "user sessions could not be loaded for logout");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "session_storage_unavailable",
                    "reason": "authoritative session records could not be loaded",
                })),
            )
                .into_response();
        }
    };
    let mut revoked = false;
    for s in sessions {
        if s.id.to_string() == *session_id {
            let mut s = s;
            if s.revoke("logout", now) {
                if let Err(error) = state.saas.update_session(&s).await {
                    tracing::error!(error = %error, session = %s.id, "session revocation failed");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": "session_storage_unavailable",
                            "reason": "session could not be revoked",
                        })),
                    )
                        .into_response();
                }
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
