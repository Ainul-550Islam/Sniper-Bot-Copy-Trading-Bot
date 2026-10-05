//! Durable security posture and tenant policy enforcement.
//!
//! Organization policy is stored in PostgreSQL. MFA enrollment is reported
//! from `user_mfa_devices`; enrollment and code verification are refused until
//! the deployment has a configured encrypted MFA implementation rather than
//! returning a test secret or claiming a verification succeeded.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine as _,
};
use chrono::Utc;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::hmac::{self, HMAC_SHA1_FOR_LEGACY_USE_ONLY};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/security/status", axum::routing::get(get_status))
        .route(
            "/api/saas/security/rotate-tokens",
            axum::routing::post(rotate_tokens),
        )
        .route(
            "/api/saas/security/mfa-enforce",
            axum::routing::post(enforce_mfa),
        )
        .route(
            "/api/saas/security/totp/setup",
            axum::routing::post(setup_totp),
        )
        .route(
            "/api/saas/security/totp/verify",
            axum::routing::post(verify_totp),
        )
        .route(
            "/api/saas/security/ip-allowlist",
            axum::routing::post(update_ip_allowlist),
        )
}

#[derive(Debug, Deserialize)]
pub struct EnforceMfaBody {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct VerifyTotpBody {
    pub device_id: Option<uuid::Uuid>,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct IpAllowlistBody {
    pub cidrs: Vec<String>,
}

fn valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.trim().split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    prefix <= if address.is_ipv4() { 32 } else { 128 }
}

const MFA_KEY_ENV: &str = "MFA_ENCRYPTION_KEY";
const MFA_PREFIX: &str = "enc:v1:";
const MFA_NONCE_BYTES: usize = 12;

fn mfa_encryption_key() -> Result<[u8; 32], String> {
    let encoded = std::env::var(MFA_KEY_ENV)
        .map_err(|_| format!("{MFA_KEY_ENV} must be configured as a base64-encoded 32-byte key"))?;
    let bytes = STANDARD_NO_PAD
        .decode(encoded.trim())
        .or_else(|_| STANDARD.decode(encoded.trim()))
        .map_err(|_| format!("{MFA_KEY_ENV} is not valid base64"))?;
    bytes
        .try_into()
        .map_err(|_| format!("{MFA_KEY_ENV} must decode to exactly 32 bytes"))
}

fn encrypt_mfa_secret(secret: &str) -> Result<String, String> {
    let key = mfa_encryption_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "MFA encryption key could not be initialized".to_string())?;
    let sealing_key = LessSafeKey::new(unbound);
    let mut nonce_bytes = [0u8; MFA_NONCE_BYTES];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| "secure randomness for MFA encryption is unavailable".to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut ciphertext = secret.as_bytes().to_vec();
    sealing_key
        .seal_in_place_append_tag(nonce, Aad::empty(), &mut ciphertext)
        .map_err(|_| "MFA secret encryption failed".to_string())?;
    let mut encoded = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&ciphertext);
    Ok(format!("{MFA_PREFIX}{}", STANDARD_NO_PAD.encode(encoded)))
}

fn decrypt_mfa_secret(stored: &str) -> Result<String, String> {
    let encoded = stored
        .strip_prefix(MFA_PREFIX)
        .ok_or_else(|| "stored MFA secret is not encrypted".to_string())?;
    let decoded = STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| "stored MFA ciphertext is not valid base64".to_string())?;
    if decoded.len() <= MFA_NONCE_BYTES {
        return Err("stored MFA ciphertext is truncated".to_string());
    }
    let key = mfa_encryption_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "MFA encryption key could not be initialized".to_string())?;
    let opening_key = LessSafeKey::new(unbound);
    let nonce_bytes: [u8; MFA_NONCE_BYTES] = decoded[..MFA_NONCE_BYTES]
        .try_into()
        .map_err(|_| "stored MFA nonce is invalid".to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut plaintext = decoded[MFA_NONCE_BYTES..].to_vec();
    let plaintext = opening_key
        .open_in_place(nonce, Aad::empty(), &mut plaintext)
        .map_err(|_| "stored MFA secret could not be decrypted".to_string())?;
    String::from_utf8(plaintext.to_vec())
        .map_err(|_| "stored MFA secret is not valid UTF-8".to_string())
}

fn encode_base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    output
}

fn decode_base32(value: &str) -> Result<Vec<u8>, String> {
    let mut buffer = 0u32;
    let mut bits = 0u8;
    let mut output = Vec::new();
    for character in value.trim().trim_end_matches('=').bytes() {
        let upper = character.to_ascii_uppercase();
        let digit = match upper {
            b'A'..=b'Z' => upper - b'A',
            b'2'..=b'7' => upper - b'2' + 26,
            _ => return Err("MFA secret is not valid base32".to_string()),
        };
        buffer = (buffer << 5) | u32::from(digit);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    if output.is_empty() {
        return Err("MFA secret is empty".to_string());
    }
    Ok(output)
}

fn totp_code(secret: &[u8], counter: u64) -> String {
    let key = hmac::Key::new(HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let bytes = tag.as_ref();
    let offset = usize::from(bytes[bytes.len() - 1] & 0x0f);
    let value = (u32::from(bytes[offset]) & 0x7f) << 24
        | u32::from(bytes[offset + 1]) << 16
        | u32::from(bytes[offset + 2]) << 8
        | u32::from(bytes[offset + 3]);
    format!("{:06}", value % 1_000_000)
}

fn verify_totp_code(secret: &[u8], code: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let counter = now.timestamp().div_euclid(30) as u64;
    (-1i64..=1).any(|offset| {
        let candidate = if offset.is_negative() {
            counter.saturating_sub(offset.unsigned_abs())
        } else {
            counter.saturating_add(offset as u64)
        };
        let expected = totp_code(secret, candidate);
        ring::constant_time::verify_slices_are_equal(expected.as_bytes(), code.as_bytes()).is_ok()
    })
}

pub(crate) async fn session_mfa_is_current(
    state: &ApiState,
    organization_id: bot_core::tenant::OrganizationId,
    session_created_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let Some(db) = state.db.as_deref() else {
        // MFA enforcement is only writable in the durable PostgreSQL path;
        // the database-disabled in-memory fixture has no policy to enforce.
        return Ok(());
    };
    let policy = sqlx::query(
        "SELECT mfa_enforced, updated_at FROM tenant_security_policies WHERE organization_id = $1",
    )
    .bind(organization_id.as_uuid())
    .fetch_optional(db.pool())
    .await
    .map_err(|error| format!("MFA policy could not be loaded: {error}"))?;
    let Some(row) = policy else {
        return Ok(());
    };
    let enforced = row.try_get::<bool, _>("mfa_enforced").unwrap_or(false);
    let updated_at = row
        .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
        .map_err(|error| format!("MFA policy timestamp could not be decoded: {error}"))?;
    if enforced && session_created_at < updated_at {
        return Err("mfa_reauthentication_required".to_string());
    }
    Ok(())
}

pub(crate) async fn verify_login_mfa(
    state: &ApiState,
    user_id: bot_core::tenant::UserId,
    organization_id: bot_core::tenant::OrganizationId,
    code: Option<&str>,
) -> Result<(), String> {
    let Some(db) = state.db.as_deref() else {
        return Err("MFA policy requires an attached PostgreSQL database".to_string());
    };
    let policy =
        sqlx::query("SELECT mfa_enforced FROM tenant_security_policies WHERE organization_id = $1")
            .bind(organization_id.as_uuid())
            .fetch_optional(db.pool())
            .await
            .map_err(|error| format!("MFA policy could not be loaded: {error}"))?;
    let enforced = policy
        .as_ref()
        .and_then(|row| row.try_get::<bool, _>("mfa_enforced").ok())
        .unwrap_or(false);
    if !enforced {
        return Ok(());
    }
    let code = code
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "mfa_challenge_required".to_string())?;
    let rows = sqlx::query(
        "SELECT id, secret_encrypted FROM user_mfa_devices WHERE user_id = $1 AND organization_id = $2 AND device_type = 'totp' AND verified = true",
    )
    .bind(user_id.as_uuid())
    .bind(organization_id.as_uuid())
    .fetch_all(db.pool())
    .await
    .map_err(|error| format!("MFA devices could not be loaded: {error}"))?;
    for row in rows {
        let device_id: uuid::Uuid = row
            .try_get("id")
            .map_err(|error| format!("MFA device id could not be decoded: {error}"))?;
        let encrypted: String = row
            .try_get("secret_encrypted")
            .map_err(|error| format!("MFA secret could not be decoded: {error}"))?;
        let secret = decrypt_mfa_secret(&encrypted)
            .and_then(|value| decode_base32(&value))
            .map_err(|error| format!("MFA secret could not be decrypted: {error}"))?;
        if verify_totp_code(&secret, code, Utc::now()) {
            let claimed = sqlx::query(
                "UPDATE user_mfa_devices SET last_used_at = now() WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '30 seconds')",
            )
            .bind(device_id)
            .execute(db.pool())
            .await
            .map_err(|error| format!("MFA use timestamp could not be persisted: {error}"))?;
            if claimed.rows_affected() > 0 {
                return Ok(());
            }
        }
    }
    Err("invalid_totp_code".to_string())
}

async fn policy_row(
    state: &ApiState,
    organization_id: uuid::Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, Response> {
    let Some(db) = state.db.as_deref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "security policy state requires an attached PostgreSQL database"
            })),
        )
            .into_response());
    };
    sqlx::query(
        "SELECT mfa_enforced, ip_allowlist, session_duration_hours, require_signed_commits FROM tenant_security_policies WHERE organization_id = $1",
    )
    .bind(organization_id)
    .fetch_optional(db.pool())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, organization = %organization_id, "security policy query failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "security policy could not be loaded"
            })),
        )
            .into_response()
    })
}

pub async fn get_status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "security posture requires an attached PostgreSQL database"
            })),
        )
            .into_response();
    };
    let policy = match policy_row(&state, ctx.organization.id.as_uuid()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mfa_device = sqlx::query(
        "SELECT EXISTS (SELECT 1 FROM user_mfa_devices WHERE organization_id = $1 AND device_type = 'totp' AND verified = true) AS configured",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_one(db.pool())
    .await;
    let has_totp_configured = match mfa_device {
        Ok(row) => row.try_get::<bool, _>("configured").unwrap_or(false),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "MFA device query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "security_storage_unavailable",
                    "reason": "MFA enrollment state could not be loaded"
                })),
            )
                .into_response();
        }
    };

    let (
        mfa_policy_requested,
        ip_allowlist,
        session_duration_hours,
        require_signed_commits,
        policy_configured,
    ) = match policy {
        Some(row) => (
            row.try_get::<bool, _>("mfa_enforced").unwrap_or(false),
            row.try_get::<Vec<String>, _>("ip_allowlist")
                .unwrap_or_default(),
            row.try_get::<i32, _>("session_duration_hours")
                .unwrap_or(12),
            row.try_get::<bool, _>("require_signed_commits")
                .unwrap_or(false),
            true,
        ),
        None => (false, Vec::new(), 12, false, false),
    };

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "policy_configured": policy_configured,
            "mfa_enforced": mfa_policy_requested,
            "mfa_policy_requested": mfa_policy_requested,
            "mfa_enforcement_status": if mfa_policy_requested { "active_for_new_sessions" } else { "disabled" },
            "has_totp_configured": has_totp_configured,
            "ip_allowlist_count": ip_allowlist.len(),
            "ip_allowlist": ip_allowlist,
            "session_duration_hours": session_duration_hours,
            "require_signed_commits": require_signed_commits,
        })),
    )
        .into_response()
}

pub async fn rotate_tokens(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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

    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "credential_rotation_unavailable",
                "reason": "credential rotation requires an attached PostgreSQL database"
            })),
        )
            .into_response();
    };
    let mut transaction = match db.pool().begin().await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "credential rotation transaction could not start");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "credential_rotation_unavailable" })),
            )
                .into_response();
        }
    };
    let sessions = sqlx::query(
        "UPDATE saas_runtime_records SET record = jsonb_set(jsonb_set(record, '{revoked_at}', to_jsonb(now()), true), '{revoke_reason}', to_jsonb('tenant_credential_rotation'::text), true), updated_at = now() WHERE kind = 'session' AND (record->>'revoked_at' IS NULL OR record->>'revoked_at' = '') AND (organization_id = $1 OR (organization_id IS NULL AND user_id IN (SELECT user_id FROM organization_members WHERE organization_id = $1 AND status = 'active')))"
    )
    .bind(ctx.organization.id.as_uuid())
    .execute(&mut *transaction)
    .await;
    let sessions = match sessions {
        Ok(value) => value.rows_affected(),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "session credential rotation failed");
            let _ = transaction.rollback().await;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "credential_rotation_unavailable", "reason": "session credentials could not be revoked" })),
            )
                .into_response();
        }
    };
    let api_keys = sqlx::query(
        "UPDATE saas_runtime_records SET record = jsonb_set(jsonb_set(record, '{revoked_at}', to_jsonb(now()), true), '{revoke_reason}', to_jsonb('tenant_credential_rotation'::text), true), updated_at = now() WHERE kind = 'api_key' AND organization_id = $1 AND (record->>'revoked_at' IS NULL OR record->>'revoked_at' = '')",
    )
    .bind(ctx.organization.id.as_uuid())
    .execute(&mut *transaction)
    .await;
    let api_keys = match api_keys {
        Ok(value) => value.rows_affected(),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "API key credential rotation failed");
            let _ = transaction.rollback().await;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "credential_rotation_unavailable", "reason": "API key credentials could not be revoked" })),
            )
                .into_response();
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::error!(error = %error, organization = %ctx.organization.id, "credential rotation transaction could not commit");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "credential_rotation_unavailable", "reason": "credential revocations could not be committed" })),
        )
            .into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.tokens_rotated",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "sessions_revoked": sessions,
            "api_keys_revoked": api_keys,
            "message": format!("Revoked {sessions} sessions and {api_keys} API keys for this organization"),
        })),
    )
        .into_response()
}

pub async fn setup_totp(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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
    let Some(user_id) = ctx.authorization.user_id else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "mfa_requires_user_session" })),
        )
            .into_response();
    };
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "mfa_storage_unavailable",
                "reason": "MFA enrollment requires an attached PostgreSQL database"
            })),
        )
            .into_response();
    };
    let mut secret_bytes = [0u8; 20];
    if let Err(error) = SystemRandom::new().fill(&mut secret_bytes) {
        tracing::error!(?error, "MFA secret generation failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mfa_randomness_unavailable" })),
        )
            .into_response();
    }
    let secret = encode_base32(&secret_bytes);
    let encrypted = match encrypt_mfa_secret(&secret) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "MFA secret encryption is unavailable");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "mfa_encryption_not_configured",
                    "reason": "MFA_ENCRYPTION_KEY is not configured for this deployment"
                })),
            )
                .into_response();
        }
    };
    let device_id = uuid::Uuid::new_v4();
    let deleted = sqlx::query(
        "DELETE FROM user_mfa_devices WHERE user_id = $1 AND organization_id = $2 AND device_type = 'totp' AND verified = false",
    )
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    if let Err(error) = deleted {
        tracing::error!(error = %error, "old MFA enrollment cleanup failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mfa_storage_unavailable", "reason": "MFA enrollment could not be prepared" })),
        )
            .into_response();
    }
    let inserted = sqlx::query(
        "INSERT INTO user_mfa_devices (id, user_id, organization_id, device_type, name, secret_encrypted, verified) VALUES ($1, $2, $3, 'totp', $4, $5, false)",
    )
    .bind(device_id)
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .bind("Authenticator app")
    .bind(encrypted)
    .execute(db.pool())
    .await;
    if let Err(error) = inserted {
        tracing::error!(error = %error, "MFA enrollment insert failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mfa_storage_unavailable", "reason": "MFA enrollment could not be persisted" })),
        )
            .into_response();
    }
    let account = ctx
        .user
        .as_ref()
        .map(|user| user.email.as_str())
        .unwrap_or("tenant-user");
    let otpauth_url = format!(
        "otpauth://totp/SniperSuite:{}?secret={}&issuer=SniperSuite",
        account, secret
    );
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.totp_setup_created",
            Some(&device_id.to_string()),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "device_id": device_id.to_string(),
            "secret": secret,
            "otpauth_url": otpauth_url,
            "verified": false,
            "backup_codes": null,
            "backup_codes_status": "not_configured",
        })),
    )
        .into_response()
}

pub async fn verify_totp(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<VerifyTotpBody>,
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
    let Some(user_id) = ctx.authorization.user_id else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "mfa_requires_user_session" })),
        )
            .into_response();
    };
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mfa_storage_unavailable", "reason": "MFA verification requires PostgreSQL" })),
        )
            .into_response();
    };
    let device_id = match body.device_id {
        Some(value) => value,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "device_id_required" })),
            )
                .into_response()
        }
    };
    let row = match sqlx::query(
        "SELECT secret_encrypted, verified FROM user_mfa_devices WHERE id = $1 AND user_id = $2 AND organization_id = $3 AND device_type = 'totp'",
    )
    .bind(device_id)
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "mfa_device_not_found" }))).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "MFA device lookup failed");
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mfa_storage_unavailable" }))).into_response();
        }
    };
    let encrypted: String = match row.try_get("secret_encrypted") {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "MFA secret row could not be decoded");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "mfa_storage_unavailable" })),
            )
                .into_response();
        }
    };
    let secret = match decrypt_mfa_secret(&encrypted).and_then(|value| decode_base32(&value)) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "MFA secret could not be decrypted");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "mfa_encryption_unavailable" })),
            )
                .into_response();
        }
    };
    if !verify_totp_code(&secret, &body.code, Utc::now()) {
        state
            .audit
            .record(
                &ctx.actor_label(),
                "saas.security.totp_verification_failed",
                Some(&device_id.to_string()),
                bot_core::audit::AuditOutcome::Denied,
                json!({}),
            )
            .await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_totp_code" })),
        )
            .into_response();
    }
    let updated = sqlx::query(
        "UPDATE user_mfa_devices SET verified = true, last_used_at = now() WHERE id = $1 AND user_id = $2 AND organization_id = $3 AND (last_used_at IS NULL OR last_used_at < now() - interval '30 seconds')",
    )
    .bind(device_id)
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match updated {
        Ok(result) if result.rows_affected() > 0 => {}
        Ok(_) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "error": "totp_code_already_used" })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "MFA verification state update failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "mfa_storage_unavailable" })),
            )
                .into_response();
        }
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.totp_verified",
            Some(&device_id.to_string()),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({ "success": true, "device_id": device_id.to_string(), "verified": true })),
    )
        .into_response()
}

pub async fn enforce_mfa(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<EnforceMfaBody>,
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
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "security policy requires an attached PostgreSQL database"
            })),
        )
            .into_response();
    };
    if body.enabled {
        let enrolled = sqlx::query(
            "SELECT EXISTS (SELECT 1 FROM user_mfa_devices WHERE organization_id = $1 AND device_type = 'totp' AND verified = true) AS enrolled",
        )
        .bind(ctx.organization.id.as_uuid())
        .fetch_one(db.pool())
        .await;
        let enrolled = match enrolled {
            Ok(row) => row.try_get::<bool, _>("enrolled").unwrap_or(false),
            Err(error) => {
                tracing::error!(error = %error, "MFA enrollment check failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "mfa_storage_unavailable" })),
                )
                    .into_response();
            }
        };
        if !enrolled {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "mfa_device_not_verified",
                    "reason": "verify at least one authenticator device before enforcing MFA"
                })),
            )
                .into_response();
        }
    }
    let result = sqlx::query(
        "INSERT INTO tenant_security_policies (organization_id, mfa_enforced, updated_by, updated_at) VALUES ($1, $2, $3, now()) ON CONFLICT (organization_id) DO UPDATE SET mfa_enforced = EXCLUDED.mfa_enforced, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(ctx.organization.id.as_uuid())
    .bind(body.enabled)
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .execute(db.pool())
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, organization = %ctx.organization.id, "MFA policy update failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "MFA policy could not be persisted"
            })),
        )
            .into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.mfa_policy_changed",
            Some(&format!("mfa_enforced={}", body.enabled)),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "mfa_enforced": body.enabled
        })),
    )
        .into_response()
}

pub async fn update_ip_allowlist(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<IpAllowlistBody>,
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
    if body.cidrs.len() > 100 || body.cidrs.iter().any(|value| !valid_cidr(value)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_ip_allowlist",
                "reason": "each entry must be an IPv4 or IPv6 CIDR with a valid prefix"
            })),
        )
            .into_response();
    }
    let Some(db) = state.db.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "security policy requires an attached PostgreSQL database"
            })),
        )
            .into_response();
    };
    let result = sqlx::query(
        "INSERT INTO tenant_security_policies (organization_id, ip_allowlist, updated_by, updated_at) VALUES ($1, $2, $3, now()) ON CONFLICT (organization_id) DO UPDATE SET ip_allowlist = EXCLUDED.ip_allowlist, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(ctx.organization.id.as_uuid())
    .bind(&body.cidrs)
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .execute(db.pool())
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, organization = %ctx.organization.id, "IP allowlist update failed");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "security_storage_unavailable",
                "reason": "IP allowlist could not be persisted"
            })),
        )
            .into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.security.ip_allowlist_updated",
            Some(&format!("cidr_count={}", body.cidrs.len())),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "organization_id": ctx.organization.id.to_string(),
            "cidrs": body.cidrs
        })),
    )
        .into_response()
}
