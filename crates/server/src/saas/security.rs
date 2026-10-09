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

/// Validate that an invitation can complete mandatory TOTP enrollment before
/// the invite is consumed. This prevents accepting an invite into a tenant
/// whose encryption configuration would make secure onboarding impossible.
pub(crate) fn mfa_enrollment_ready() -> Result<(), String> {
    // Validate both configured key material and the OS CSPRNG before a flow
    // consumes a one-time invitation or issues a restricted session.
    let _key = mfa_encryption_key()?;
    let mut nonce_probe = [0u8; MFA_NONCE_BYTES];
    SystemRandom::new()
        .fill(&mut nonce_probe)
        .map_err(|_| "secure randomness for MFA enrollment is unavailable".to_string())
}

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

#[allow(deprecated)]
fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    ring::constant_time::verify_slices_are_equal(left, right).is_ok()
}

fn matching_totp_counter(
    secret: &[u8],
    code: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let base = now.timestamp().div_euclid(30);
    // Prefer the current step when two six-digit HMAC outputs happen to
    // collide; adjacent steps are accepted only to tolerate small clock skew.
    for offset in [0i64, -1, 1] {
        let candidate = base.saturating_add(offset);
        if candidate < 0 {
            continue;
        }
        let candidate_u64 = match u64::try_from(candidate) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let expected = totp_code(secret, candidate_u64);
        if constant_time_equal(expected.as_bytes(), code.as_bytes()) {
            return Some(candidate);
        }
    }
    None
}

pub(crate) async fn session_mfa_is_current(
    state: &ApiState,
    organization_id: bot_core::tenant::OrganizationId,
    session_policy_version: Option<chrono::DateTime<chrono::Utc>>,
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
    let enforced = row
        .try_get::<bool, _>("mfa_enforced")
        .map_err(|error| format!("MFA policy flag could not be decoded: {error}"))?;
    let updated_at = row
        .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
        .map_err(|error| format!("MFA policy timestamp could not be decoded: {error}"))?;
    if enforced && session_policy_version != Some(updated_at) {
        return Err("mfa_reauthentication_required".to_string());
    }
    Ok(())
}

pub(crate) async fn verify_login_mfa(
    state: &ApiState,
    user_id: bot_core::tenant::UserId,
    organization_id: bot_core::tenant::OrganizationId,
    code: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let Some(db) = state.db.as_deref() else {
        return Err("MFA policy requires an attached PostgreSQL database".to_string());
    };
    let policy = sqlx::query(
        "SELECT mfa_enforced, updated_at FROM tenant_security_policies WHERE organization_id = $1",
    )
    .bind(organization_id.as_uuid())
    .fetch_optional(db.pool())
    .await
    .map_err(|error| format!("MFA policy could not be loaded: {error}"))?;
    let Some(policy) = policy else {
        return Ok(None);
    };
    let enforced = policy
        .try_get::<bool, _>("mfa_enforced")
        .map_err(|error| format!("MFA policy flag could not be decoded: {error}"))?;
    if !enforced {
        return Ok(None);
    }
    let policy_version = policy
        .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
        .map_err(|error| format!("MFA policy timestamp could not be decoded: {error}"))?;
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
    if rows.is_empty() {
        return Err("mfa_enrollment_required".to_string());
    }
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
        if let Some(counter) = matching_totp_counter(&secret, code, Utc::now()) {
            let claimed = sqlx::query(
                "UPDATE user_mfa_devices SET last_used_at = now(), counter = $2 WHERE id = $1 AND counter < $2",
            )
            .bind(device_id)
            .bind(counter)
            .execute(db.pool())
            .await
            .map_err(|error| format!("MFA use counter could not be persisted: {error}"))?;
            if claimed.rows_affected() > 0 {
                return Ok(Some(policy_version));
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
        Ok(row) => match row.try_get::<bool, _>("configured") {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, organization = %ctx.organization.id, "MFA enrollment state could not be decoded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "security_storage_unavailable",
                        "reason": "MFA enrollment state could not be loaded"
                    })),
                )
                    .into_response();
            }
        },
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
        Some(row) => {
            let decoded = (
                row.try_get::<bool, _>("mfa_enforced"),
                row.try_get::<Vec<String>, _>("ip_allowlist"),
                row.try_get::<i32, _>("session_duration_hours"),
                row.try_get::<bool, _>("require_signed_commits"),
            );
            match decoded {
                (Ok(mfa), Ok(allowlist), Ok(duration), Ok(signed_commits)) => {
                    (mfa, allowlist, duration, signed_commits, true)
                }
                (mfa, allowlist, duration, signed_commits) => {
                    let error = mfa
                        .err()
                        .or_else(|| allowlist.err())
                        .or_else(|| duration.err())
                        .or_else(|| signed_commits.err());
                    tracing::error!(?error, organization = %ctx.organization.id, "security policy row could not be decoded");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": "security_storage_unavailable",
                            "reason": "security policy could not be decoded"
                        })),
                    )
                        .into_response();
                }
            }
        }
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
    let session_counts = sqlx::query(
        r#"WITH revoked AS (
               UPDATE saas_runtime_records
                  SET record = jsonb_set(
                                  jsonb_set(record, '{revoked_at}', to_jsonb(now()), true),
                                  '{revoke_reason}',
                                  to_jsonb('tenant_credential_rotation'::text),
                                  true
                              ),
                      updated_at = now()
                WHERE kind = 'session'
                  AND (record->>'revoked_at' IS NULL OR record->>'revoked_at' = '')
                  AND (
                      organization_id = $1
                      OR (organization_id IS NULL AND user_id IN (
                          SELECT user_id
                            FROM saas_runtime_records
                           WHERE kind = 'membership'
                             AND organization_id = $1
                             AND record->>'status' = 'active'
                      ))
                  )
               RETURNING id, record->>'revoked_at' AS revoked_at,
                         record->>'revoke_reason' AS revoke_reason
           ), normalized AS (
               UPDATE sessions AS s
                  SET revoked_at = revoked.revoked_at::timestamptz,
                      revoke_reason = COALESCE(revoked.revoke_reason, '')
                 FROM revoked
                WHERE s.id = revoked.id::uuid
               RETURNING s.id
           )
           SELECT (SELECT COUNT(*) FROM revoked)::bigint AS runtime_count,
                  (SELECT COUNT(*) FROM normalized)::bigint AS normalized_count"#,
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_one(&mut *transaction)
    .await;
    let counts = match session_counts {
        Ok(row) => {
            let runtime = row.try_get::<i64, _>("runtime_count");
            let normalized = row.try_get::<i64, _>("normalized_count");
            match (runtime, normalized) {
                (Ok(runtime), Ok(normalized)) if runtime == normalized => (runtime, normalized),
                (Ok(runtime), Ok(normalized)) => {
                    tracing::error!(organization = %ctx.organization.id, runtime, normalized, "session projection is incomplete during credential rotation");
                    let _ = transaction.rollback().await;
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "credential_rotation_unavailable", "reason": "session revocations could not be synchronized" })),
                    )
                        .into_response();
                }
                (runtime, normalized) => {
                    tracing::error!(?runtime, ?normalized, organization = %ctx.organization.id, "session revocation counts could not be decoded");
                    let _ = transaction.rollback().await;
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "credential_rotation_unavailable", "reason": "session revocations could not be verified" })),
                    )
                        .into_response();
                }
            }
        }
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
    let sessions = counts.0;
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
    let ctx = match crate::saas::middleware::authorize_mfa_enrollment_request(&state, &headers)
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
    if let Some(response) = crate::saas::rate_limit::reject_sensitive_attempt(
        &state,
        "totp_setup",
        &format!("{}:{}", user_id, ctx.organization.id),
    )
    .await
    {
        return response;
    }
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
    let mut enrollment_tx = match db.pool().begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::error!(error = %error, "MFA enrollment transaction could not start");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "mfa_storage_unavailable", "reason": "MFA enrollment could not be prepared" })),
            )
                .into_response();
        }
    };
    if let Err(error) = sqlx::query(
        "DELETE FROM user_mfa_devices WHERE user_id = $1 AND organization_id = $2 AND device_type = 'totp' AND verified = false",
    )
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .execute(&mut *enrollment_tx)
    .await
    {
        tracing::error!(error = %error, "old MFA enrollment cleanup failed");
        let _ = enrollment_tx.rollback().await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mfa_storage_unavailable", "reason": "MFA enrollment could not be prepared" })),
        )
            .into_response();
    }
    if let Err(error) = sqlx::query(
        "INSERT INTO user_mfa_devices (id, user_id, organization_id, device_type, name, secret_encrypted, verified) VALUES ($1, $2, $3, 'totp', $4, $5, false)",
    )
    .bind(device_id)
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .bind("Authenticator app")
    .bind(encrypted)
    .execute(&mut *enrollment_tx)
    .await
    {
        tracing::error!(error = %error, "MFA enrollment insert failed");
        let _ = enrollment_tx.rollback().await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "mfa_storage_unavailable", "reason": "MFA enrollment could not be persisted" })),
        )
            .into_response();
    }
    if let Err(error) = enrollment_tx.commit().await {
        tracing::error!(error = %error, "MFA enrollment transaction could not commit");
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

async fn promote_verified_enrollment_session(
    state: &ApiState,
    ctx: &crate::saas::middleware::SaasContext,
    user_id: bot_core::tenant::UserId,
    organization_id: bot_core::tenant::OrganizationId,
) -> Result<bool, String> {
    let bot_core::authorization::Principal::UserSession { session_id } =
        &ctx.authorization.principal
    else {
        return Ok(false);
    };
    let session_id = bot_core::session::SessionId::parse(session_id)
        .ok_or_else(|| "MFA enrollment session id is invalid".to_string())?;
    let mut session = state
        .saas
        .session(session_id)
        .await
        .map_err(|error| format!("MFA enrollment session could not be loaded: {error}"))?
        .ok_or_else(|| "MFA enrollment session no longer exists".to_string())?;
    if session.user_id != user_id || session.organization_id != Some(organization_id) {
        return Err("MFA enrollment session scope changed unexpectedly".to_string());
    }
    let Some(db) = state.db.as_deref() else {
        return Err("MFA policy requires an attached PostgreSQL database".to_string());
    };
    let policy = sqlx::query(
        "SELECT mfa_enforced, updated_at FROM tenant_security_policies WHERE organization_id = $1",
    )
    .bind(organization_id.as_uuid())
    .fetch_optional(db.pool())
    .await
    .map_err(|error| format!("MFA policy could not be loaded: {error}"))?;
    let (enforced, policy_version) = match policy {
        Some(row) => {
            let enforced = row
                .try_get::<bool, _>("mfa_enforced")
                .map_err(|error| format!("MFA policy flag could not be decoded: {error}"))?;
            let version = row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                .map_err(|error| format!("MFA policy timestamp could not be decoded: {error}"))?;
            (enforced, if enforced { Some(version) } else { None })
        }
        None => (false, None),
    };
    if !session.mfa_enrollment_only
        && (!enforced || session.mfa_policy_updated_at == policy_version)
    {
        return Ok(false);
    }
    session.mfa_policy_updated_at = policy_version;
    session.mfa_enrollment_only = false;
    state
        .saas
        .update_session(&session)
        .await
        .map_err(|error| format!("MFA enrollment session could not be promoted: {error}"))?;
    Ok(true)
}

pub async fn verify_totp(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<VerifyTotpBody>,
) -> Response {
    let ctx = match crate::saas::middleware::authorize_mfa_enrollment_request(&state, &headers)
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
    if let Some(response) = crate::saas::rate_limit::reject_sensitive_attempt(
        &state,
        "totp_verify",
        &format!("{}:{}", user_id, ctx.organization.id),
    )
    .await
    {
        return response;
    }
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
    let Some(counter) = matching_totp_counter(&secret, &body.code, Utc::now()) else {
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
    };
    let updated = sqlx::query(
        "UPDATE user_mfa_devices SET verified = true, last_used_at = now(), counter = $4 WHERE id = $1 AND user_id = $2 AND organization_id = $3 AND counter < $4",
    )
    .bind(device_id)
    .bind(user_id.as_uuid())
    .bind(ctx.organization.id.as_uuid())
    .bind(counter)
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
    let session_promoted = match promote_verified_enrollment_session(
        &state,
        &ctx,
        user_id,
        ctx.organization.id,
    )
    .await
    {
        Ok(promoted) => promoted,
        Err(reason) => {
            tracing::error!(organization = %ctx.organization.id, user = %user_id, %reason, "verified MFA session could not be promoted");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "mfa_storage_unavailable",
                    "reason": "the authenticator was verified but the onboarding session could not be activated"
                })),
            )
                .into_response();
        }
    };
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
        Json(json!({
            "success": true,
            "device_id": device_id.to_string(),
            "verified": true,
            "session_promoted": session_promoted
        })),
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
            Ok(row) => match row.try_get::<bool, _>("enrolled") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, "MFA enrollment result could not be decoded");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "mfa_storage_unavailable" })),
                    )
                        .into_response();
                }
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn totp_matches_rfc_6238_sha1_vector_and_returns_replay_counter() {
        let secret = b"12345678901234567890";
        let now = Utc
            .timestamp_opt(59, 0)
            .single()
            .expect("the RFC test timestamp is valid");

        assert_eq!(totp_code(secret, 1), "287082");
        assert_eq!(matching_totp_counter(secret, "287082", now), Some(1));
        assert_eq!(matching_totp_counter(secret, "28708x", now), None);
    }

    #[test]
    fn base32_encoding_round_trips_without_padding() {
        let bytes = [0u8, 1, 2, 3, 255];
        let encoded = encode_base32(&bytes);
        assert_eq!(decode_base32(&encoded).as_deref(), Ok(bytes.as_slice()));
        assert!(decode_base32("").is_err());
    }

    /// Every SHA-1 vector from RFC 6238 appendix B, truncated to the six
    /// digits this deployment uses. The seed is the RFC's ASCII key.
    #[test]
    fn totp_matches_every_rfc_6238_sha1_vector() {
        let secret = b"12345678901234567890";
        let vectors = [
            (59u64, 1u64, "287082"),
            (1_111_111_109, 37_037_036, "081804"),
            (1_111_111_111, 37_037_037, "050471"),
            (1_234_567_890, 41_152_263, "005924"),
            (2_000_000_000, 66_666_666, "279037"),
            (20_000_000_000, 666_666_666, "353130"),
        ];
        for (unix, counter, code) in vectors {
            assert_eq!(unix / 30, counter, "counter is floor(T/30)");
            assert_eq!(totp_code(secret, counter), code, "T = {unix}");
        }
    }

    /// The verifier must resolve the RFC vectors back to their counters
    /// when "now" is exactly that vector's timestamp.
    #[test]
    fn matching_counter_resolves_the_rfc_vectors() {
        let secret = b"12345678901234567890";
        for (unix, counter, code) in [
            (59i64, 1i64, "287082"),
            (1_111_111_109, 37_037_036, "081804"),
            (1_234_567_890, 41_152_263, "005924"),
            (2_000_000_000, 66_666_666, "279037"),
        ] {
            let now = Utc
                .timestamp_opt(unix, 0)
                .single()
                .expect("the RFC test timestamps are valid");
            assert_eq!(
                matching_totp_counter(secret, code, now),
                Some(counter),
                "code {code} at T = {unix}"
            );
        }
    }

    /// ±1 step of clock skew is accepted; the returned counter identifies
    /// WHICH step matched (the caller persists it to defeat replay).
    #[test]
    fn adjacent_steps_are_accepted_and_identified() {
        let secret = b"12345678901234567890";
        // now sits on step 2 of the RFC timeline.
        let now = Utc
            .timestamp_opt(60, 0)
            .single()
            .expect("valid timestamp");

        let previous = totp_code(secret, 1); // step 1: code for T = 59
        let current = totp_code(secret, 2);
        let next = totp_code(secret, 3);

        assert_eq!(matching_totp_counter(secret, &previous, now), Some(1));
        assert_eq!(matching_totp_counter(secret, &current, now), Some(2));
        assert_eq!(matching_totp_counter(secret, &next, now), Some(3));

        // Two steps away is OUTSIDE the skew window.
        let far_past = totp_code(secret, 0);
        let far_future = totp_code(secret, 4);
        if far_past != current {
            assert_eq!(matching_totp_counter(secret, &far_past, now), None);
        }
        if far_future != current {
            assert_eq!(matching_totp_counter(secret, &far_future, now), None);
        }
    }

    /// The current step is preferred when codes collide across steps: the
    /// search order is 0, -1, +1, so an ambiguous code resolves to the
    /// present, never to a past step a replay could reuse.
    #[test]
    fn current_step_wins_on_collision() {
        let secret = b"12345678901234567890";
        let now = Utc
            .timestamp_opt(1_111_111_109, 0)
            .single()
            .expect("valid timestamp");
        let base = 1_111_111_109i64.div_euclid(30);
        let current_code = totp_code(secret, base as u64);
        assert_eq!(
            matching_totp_counter(secret, &current_code, now),
            Some(base),
            "the current step must be checked first"
        );
    }

    /// Replay detection depends on the returned counter being stable and
    /// monotonic per secret: the same code at the same time always yields
    /// the same counter, so a stored "last used" comparison is sound.
    #[test]
    fn replay_counter_is_deterministic() {
        let secret = b"12345678901234567890";
        let now = Utc
            .timestamp_opt(1_234_567_890, 0)
            .single()
            .expect("valid timestamp");
        let first = matching_totp_counter(secret, "005924", now);
        let second = matching_totp_counter(secret, "005924", now);
        assert_eq!(first, Some(41_152_263));
        assert_eq!(first, second);
        // A code from a step BEFORE the last-used counter must be refused
        // by the caller; the verifier hands back exactly the step it used,
        // so the stored counter (41152263) makes 41152262 a replay.
        assert!(first > Some(41_152_262));
    }

    /// Anything that is not exactly six ASCII digits never reaches the
    /// HMAC comparison.
    #[test]
    fn malformed_codes_are_rejected_before_any_crypto() {
        let secret = b"12345678901234567890";
        let now = Utc
            .timestamp_opt(59, 0)
            .single()
            .expect("valid timestamp");
        for bad in [
            "", "28708", "2870823", "28708x", "28 708", "28708\n", "-87082",
            "２８７０８２", // fullwidth digits: bytes() sees multi-byte UTF-8
            "287082\r",
        ] {
            assert_eq!(matching_totp_counter(secret, bad, now), None, "input {bad:?}");
        }
    }

    #[test]
    fn base32_decode_rejects_non_alphabet_characters() {
        // '1', '8', '9', '0' and punctuation are not in RFC 4648 base32.
        for bad in ["1", "8", "9", "0", "ABC$", "!!!", "AB C"] {
            assert!(decode_base32(bad).is_err(), "input {bad:?}");
        }
        // Lowercase and padding are tolerated (authenticator apps vary).
        let upper = decode_base32("JBSWY3DPEHPK3PXP").expect("valid base32");
        let lower = decode_base32("jbswy3dpehpk3pxp").expect("lowercase ok");
        let padded = decode_base32("JBSWY3DPEHPK3PXP====").expect("padding ok");
        assert_eq!(upper, lower);
        assert_eq!(upper, padded);
    }

    #[test]
    fn constant_time_compare_matches_semantics() {
        assert!(constant_time_equal(b"005924", b"005924"));
        assert!(!constant_time_equal(b"005924", b"005925"));
        assert!(!constant_time_equal(b"005924", b"00592"));
    }
}
