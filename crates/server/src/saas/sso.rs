//! Tenant SSO — OIDC authorization-code + PKCE (GAP-MAP v2, P2).
//!
//! Implements the OIDC relying-party side over `tenant_sso_configs`
//! (migration 0038, extended by 0053):
//!
//! ```text
//! control plane                this module                     IdP
//! GET /api/saas/sso/authorize  ─ state+PKCE, store pending ─► /authorize
//!                                                        code  │
//! POST /api/saas/sso/callback  ◄─ verify state/PKCE ──────────┘
//!   ├─ discovery + token exchange (client_secret, code_verifier)
//!   ├─ id_token: RS256 signature vs issuer JWKS, iss/aud/exp
//!   ├─ allowed_domains gate
//!   ├─ JIT provisioning (user, then membership w/ role mapping)
//!   └─ session token (returned exactly once)
//! ```
//!
//! Security rules this module enforces:
//! * **PKCE is mandatory** (S256 only) — the code alone is useless without
//!   the verifier held in the pending store;
//! * **state is one-shot**: a callback consumes its pending entry, so a
//!   replayed callback cannot re-issue a session;
//! * **the id_token signature is ALWAYS verified** against the issuer's
//!   JWKS (RS256). Unverifiable tokens are rejected — there is no
//!   "skip verification" mode;
//! * `allowed_domains` restricts JIT provisioning to the tenant's email
//!   domains (empty list = unrestricted, documented in the config);
//! * **platform admin can never be granted through SSO** — the role mapper
//!   refuses it even if the mapping json contains it (the DB trigger in
//!   0053 also excludes it);
//! * the client secret is stored ONLY as `enc:v1:` AES-256-GCM ciphertext
//!   under `SSO_CLIENT_SECRET_ENCRYPTION_KEY` and never returned by any
//!   handler;
//! * pending state lives in one in-memory map keyed by `state`: adequate
//!   for a single-process deployment (the server binary is single-process);
//!   a horizontally scaled control plane would move it to Postgres.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::{Membership, MembershipRole, Permission};
use bot_core::session::token::generate_token;
use bot_core::session::{SessionRecord, DEFAULT_SESSION_TTL_HOURS};
use bot_core::tenant::{OrganizationId, User, UserStatus, UserId};

use super::sso_state::{self, PendingAuth};

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

/// Encryption-key environment variable for the OIDC client secret.
pub const SSO_KEY_ENV: &str = "SSO_CLIENT_SECRET_ENCRYPTION_KEY";
const ENC_PREFIX: &str = "enc:v1:";
const NONCE_BYTES: usize = 12;
/// OIDC providers accepted in `provider_type`.
const OIDC_PROVIDERS: [&str; 4] = ["oidc", "google", "okta", "azure_ad"];
/// Network timeout for every IdP call.
const IDP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/sso/config", axum::routing::get(get_config))
        .route("/api/saas/sso/config", axum::routing::put(put_config))
        .route(
            "/api/saas/sso/config",
            axum::routing::delete(delete_config),
        )
        // Public: the browser redirect in and out of the IdP.
        .route("/api/saas/sso/authorize", axum::routing::get(authorize))
        .route("/api/saas/sso/callback", axum::routing::post(callback))
}

// ---------------------------------------------------------------------------
// PKCE + state (pure; RFC 7636 / RFC 6749 §10.12)
// ---------------------------------------------------------------------------

/// Generate a PKCE code verifier: 32 random bytes → base64url (43 chars,
/// inside the RFC's 43..=128 range, unreserved alphabet by construction).
pub fn pkce_verifier() -> String {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("OS RNG available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// S256 challenge for a verifier: `base64url(SHA-256(verifier))`.
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// An opaque `state` value binding authorize → callback.
pub fn state_token() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("OS RNG available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the IdP authorization URL (pure; unit-tested). Every parameter is
/// form-encoded; `code_challenge_method` is always S256.
pub fn build_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> String {
    let sep = if authorization_endpoint.contains('?') { '&' } else { '?' };
    format!(
        "{authorization_endpoint}{sep}response_type=code&client_id={}&redirect_uri={}&scope=openid%20email%20profile&state={}&code_challenge={}&code_challenge_method=S256",
        urlencode(client_id),
        urlencode(redirect_uri),
        urlencode(state),
        urlencode(challenge),
    )
}

/// Minimal percent-encoding for query values (unreserved set per RFC 3986).
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Domain gate + role mapping (pure; unit-tested)
// ---------------------------------------------------------------------------

/// Is this email allowed by the config's domain list? An EMPTY list means
/// unrestricted (the operator opted out of domain pinning); entries are
/// case-insensitive and may be given with or without a leading dot.
pub fn email_domain_allowed(email: &str, allowed_domains: &[String]) -> bool {
    if allowed_domains.is_empty() {
        return true;
    }
    let Some(domain) = email.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()) else {
        return false;
    };
    allowed_domains.iter().any(|entry| {
        let entry = entry.trim().trim_start_matches('.').to_ascii_lowercase();
        !entry.is_empty() && domain == entry
    })
}

/// Map an IdP role claim to an application role. Unknown / missing claims
/// fall back to [`MembershipRole::Viewer`] (least privilege), and
/// `platform_admin` is UNREACHABLE from this path even if the mapping
/// contains it — platform admins are appointed explicitly, never by an IdP.
pub fn map_role(claim_value: Option<&str>, mapping: &serde_json::Value) -> MembershipRole {
    let Some(claim_value) = claim_value else {
        return MembershipRole::Viewer;
    };
    let mapped = mapping
        .get(claim_value)
        .and_then(|v| v.as_str())
        .unwrap_or(claim_value);
    match mapped {
        "org_owner" => MembershipRole::OrgOwner,
        "org_admin" => MembershipRole::OrgAdmin,
        "trader" => MembershipRole::Trader,
        "security_admin" => MembershipRole::SecurityAdmin,
        "billing_admin" => MembershipRole::BillingAdmin,
        "auditor" => MembershipRole::Auditor,
        // "platform_admin" deliberately absent: falls through to Viewer.
        _ => MembershipRole::Viewer,
    }
}

// ---------------------------------------------------------------------------
// id_token verification (RS256 via ring; PKCS#1 DER built from the JWK)
// ---------------------------------------------------------------------------

/// Typed SSO failure (all user-visible via `reason()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsoError {
    /// Config missing/disabled for the org.
    NotConfigured,
    /// Unknown or expired `state`.
    InvalidState,
    /// IdP discovery failed.
    DiscoveryFailed,
    /// Token exchange failed (bad code, bad client credentials).
    TokenExchangeFailed,
    /// id_token missing, malformed or wrong algorithm.
    MalformedIdToken,
    /// Signature did not verify against the issuer JWKS.
    BadSignature,
    /// iss/aud/exp claims did not validate.
    BadClaims,
    /// No email claim (or empty).
    NoEmail,
    /// Email domain not in allowed_domains.
    DomainNotAllowed,
    /// The account exists but may not authenticate.
    UserInactive,
    /// A storage failure underneath.
    Storage,
}

impl SsoError {
    fn status(&self) -> StatusCode {
        match self {
            SsoError::InvalidState | SsoError::DomainNotAllowed | SsoError::NoEmail => {
                StatusCode::BAD_REQUEST
            }
            SsoError::NotConfigured => StatusCode::NOT_FOUND,
            SsoError::UserInactive => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_GATEWAY,
        }
    }
    fn reason(&self) -> &'static str {
        match self {
            SsoError::NotConfigured => "sso_not_configured",
            SsoError::InvalidState => "sso_state_invalid_or_expired",
            SsoError::DiscoveryFailed => "sso_discovery_failed",
            SsoError::TokenExchangeFailed => "sso_token_exchange_failed",
            SsoError::MalformedIdToken => "sso_id_token_malformed",
            SsoError::BadSignature => "sso_id_token_signature_invalid",
            SsoError::BadClaims => "sso_id_token_claims_invalid",
            SsoError::NoEmail => "sso_email_missing",
            SsoError::DomainNotAllowed => "sso_email_domain_not_allowed",
            SsoError::UserInactive => "sso_user_inactive",
            SsoError::Storage => "sso_storage_unavailable",
        }
    }
}

fn sso_error_response(err: &SsoError) -> Response {
    (
        err.status(),
        Json(json!({ "error": err.reason() })),
    )
        .into_response()
}

fn b64url_decode(segment: &str) -> Result<Vec<u8>, SsoError> {
    URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| SsoError::MalformedIdToken)
}

/// Verified claims extracted from the id_token (signature already checked).
#[derive(Debug, Clone, PartialEq)]
pub struct IdTokenClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub exp: i64,
    pub email: String,
    pub email_verified: bool,
    /// The first usable role hint: `role`, else first string of `roles`.
    pub role: Option<String>,
}

/// Split a JWT and verify alg/kid expectations without trusting anything.
fn split_jwt(id_token: &str) -> Result<(String, String, String, String), SsoError> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err(SsoError::MalformedIdToken);
    }
    let header_json = b64url_decode(parts[0])?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_json).map_err(|_| SsoError::MalformedIdToken)?;
    // RS256 ONLY. alg confusion (none/HS256) is rejected up front.
    if header.get("alg").and_then(|v| v.as_str()) != Some("RS256") {
        return Err(SsoError::MalformedIdToken);
    }
    let kid = header
        .get("kid")
        .and_then(|v| v.as_str())
        .ok_or(SsoError::MalformedIdToken)?
        .to_string();
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
        kid,
    ))
}

/// Validate the claims of an ALREADY-SIGNATURE-VERIFIED id_token.
pub fn validate_claims(
    payload_b64: &str,
    expected_iss: &str,
    expected_aud: &str,
    now: DateTime<Utc>,
) -> Result<IdTokenClaims, SsoError> {
    let payload = b64url_decode(payload_b64)?;
    let claims: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|_| SsoError::MalformedIdToken)?;
    let iss = claims
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or(SsoError::BadClaims)?;
    if iss.trim_end_matches('/') != expected_iss.trim_end_matches('/') {
        return Err(SsoError::BadClaims);
    }
    // aud may be a string or an array containing our client_id.
    let aud_ok = match claims.get("aud") {
        Some(serde_json::Value::String(a)) => a == expected_aud,
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .any(|v| v.as_str() == Some(expected_aud)),
        _ => false,
    };
    if !aud_ok {
        return Err(SsoError::BadClaims);
    }
    let exp = claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or(SsoError::BadClaims)?;
    if exp <= now.timestamp() {
        return Err(SsoError::BadClaims);
    }
    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or(SsoError::BadClaims)?
        .to_string();
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let email_verified = claims
        .get("email_verified")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let role = claims
        .get("role")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            claims.get("roles").and_then(|v| v.as_array()).and_then(|list| {
                list.iter()
                    .find_map(|v| v.as_str().map(|s| s.to_string()))
            })
        });
    Ok(IdTokenClaims {
        iss: iss.to_string(),
        aud: expected_aud.to_string(),
        sub,
        exp,
        email,
        email_verified,
        role,
    })
}

/// DER-encode a big-endian unsigned integer (RFC 8017 conventions: strip
/// leading zeros, prepend 0x00 when the top bit is set).
fn der_integer(value: &[u8]) -> Vec<u8> {
    let mut v = value;
    while v.len() > 1 && v[0] == 0 {
        v = &v[1..];
    }
    let pad = (!v.is_empty() && v[0] & 0x80 != 0) as usize;
    let mut out = vec![0x02];
    der_len(v.len() + pad, &mut out);
    if pad == 1 {
        out.push(0x00);
    }
    out.extend_from_slice(v);
    out
}

fn der_len(len: usize, out: &mut Vec<u8>) {
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = {
            let mut b = Vec::new();
            let mut l = len;
            while l > 0 {
                b.insert(0, (l & 0xff) as u8);
                l >>= 8;
            }
            b
        };
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
}

/// Build the PKCS#1 `RSAPublicKey` DER that ring's RS256 verifier expects,
/// from a JWK's `n` and `e` (both base64url big-endian).
pub fn jwk_to_rsa_der(n_b64: &str, e_b64: &str) -> Result<Vec<u8>, SsoError> {
    let n = b64url_decode(n_b64)?;
    let e = b64url_decode(e_b64)?;
    if n.len() < 256 || n.len() > 512 {
        // Accept 2048..4096-bit moduli only; smaller keys are unsafe.
        return Err(SsoError::BadSignature);
    }
    let n_enc = der_integer(&n);
    let e_enc = der_integer(&e);
    let mut seq = vec![0x30];
    der_len(n_enc.len() + e_enc.len(), &mut seq);
    seq.extend_from_slice(&n_enc);
    seq.extend_from_slice(&e_enc);
    Ok(seq)
}

/// Verify an RS256 signature with ring. `signing_input` is
/// `<header_b64>.<payload_b64>` exactly as transmitted.
pub fn verify_rs256(signing_input: &str, sig_b64: &str, public_key_der: &[u8]) -> Result<(), SsoError> {
    // An undecodable signature is a bad signature: every signature defect is
    // reported under one variant, so callers never mistake it for a token-shape error.
    let sig = b64url_decode(sig_b64).map_err(|_| SsoError::BadSignature)?;
    let key = ring::signature::UnparsedPublicKey::new(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        public_key_der,
    );
    key.verify(signing_input.as_bytes(), &sig)
        .map_err(|_| SsoError::BadSignature)
}

// ---------------------------------------------------------------------------
// IdP HTTP: discovery, JWKS, token exchange
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DiscoveryDoc {
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct JwksDoc {
    keys: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
}

fn http_client() -> Result<reqwest::Client, SsoError> {
    reqwest::Client::builder()
        .timeout(IDP_TIMEOUT)
        .build()
        .map_err(|_| SsoError::DiscoveryFailed)
}

async fn fetch_discovery(client: &reqwest::Client, issuer_url: &str) -> Result<DiscoveryDoc, SsoError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|_| SsoError::DiscoveryFailed)?;
    if !resp.status().is_success() {
        return Err(SsoError::DiscoveryFailed);
    }
    resp.json::<DiscoveryDoc>()
        .await
        .map_err(|_| SsoError::DiscoveryFailed)
}

async fn fetch_jwk_for_kid(
    client: &reqwest::Client,
    jwks_uri: &str,
    kid: &str,
) -> Result<(String, String), SsoError> {
    let resp = client
        .get(jwks_uri)
        .send()
        .await
        .map_err(|_| SsoError::DiscoveryFailed)?;
    if !resp.status().is_success() {
        return Err(SsoError::DiscoveryFailed);
    }
    let jwks = resp.json::<JwksDoc>().await.map_err(|_| SsoError::DiscoveryFailed)?;
    for key in &jwks.keys {
        if key.get("kty").and_then(|v| v.as_str()) != Some("RSA") {
            continue;
        }
        if key.get("kid").and_then(|v| v.as_str()) != Some(kid) {
            continue;
        }
        let n = key.get("n").and_then(|v| v.as_str()).ok_or(SsoError::BadSignature)?;
        let e = key.get("e").and_then(|v| v.as_str()).ok_or(SsoError::BadSignature)?;
        return Ok((n.to_string(), e.to_string()));
    }
    Err(SsoError::BadSignature)
}

async fn exchange_code(
    client: &reqwest::Client,
    token_endpoint: &str,
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code_verifier: &str,
) -> Result<TokenResponse, SsoError> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
    ];
    // Confidential clients authenticate with the secret; public clients
    // (secret not configured) rely on PKCE alone.
    let secret_owned;
    if let Some(secret) = client_secret {
        secret_owned = secret.to_string();
        form.push(("client_secret", &secret_owned));
    }
    let resp = client
        .post(token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|_| SsoError::TokenExchangeFailed)?;
    if !resp.status().is_success() {
        return Err(SsoError::TokenExchangeFailed);
    }
    let body = resp
        .json::<TokenResponse>()
        .await
        .map_err(|_| SsoError::TokenExchangeFailed)?;
    if body.id_token.is_none() {
        return Err(SsoError::MalformedIdToken);
    }
    Ok(body)
}

// ---------------------------------------------------------------------------
// Client-secret encryption (AES-256-GCM, same shape as MFA/webhook secrets)
// ---------------------------------------------------------------------------

fn sso_key() -> Result<[u8; 32], String> {
    let encoded = std::env::var(SSO_KEY_ENV).map_err(|_| {
        format!("{SSO_KEY_ENV} must be configured as a base64-encoded 32-byte key")
    })?;
    let bytes = STANDARD_NO_PAD
        .decode(encoded.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(encoded.trim()))
        .map_err(|_| format!("{SSO_KEY_ENV} is not valid base64"))?;
    bytes
        .try_into()
        .map_err(|_| format!("{SSO_KEY_ENV} must decode to exactly 32 bytes"))
}

fn encrypt_client_secret(secret: &str) -> Result<String, String> {
    let key = sso_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "SSO encryption key could not be initialized".to_string())?;
    let sealing = LessSafeKey::new(unbound);
    let mut nonce_bytes = [0u8; NONCE_BYTES];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| "secure randomness unavailable".to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut buf = secret.as_bytes().to_vec();
    sealing
        .seal_in_place_append_tag(nonce, Aad::empty(), &mut buf)
        .map_err(|_| "SSO client secret encryption failed".to_string())?;
    let mut encoded = Vec::with_capacity(nonce_bytes.len() + buf.len());
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&buf);
    Ok(format!("{ENC_PREFIX}{}", STANDARD_NO_PAD.encode(encoded)))
}

fn decrypt_client_secret(stored: &str) -> Result<String, String> {
    let encoded = stored
        .strip_prefix(ENC_PREFIX)
        .ok_or_else(|| "stored SSO client secret is not encrypted".to_string())?;
    let decoded = STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| "stored SSO ciphertext is not valid base64".to_string())?;
    if decoded.len() <= NONCE_BYTES {
        return Err("stored SSO ciphertext is truncated".to_string());
    }
    let key = sso_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "SSO encryption key could not be initialized".to_string())?;
    let opening = LessSafeKey::new(unbound);
    let nonce_bytes: [u8; NONCE_BYTES] = decoded[..NONCE_BYTES]
        .try_into()
        .map_err(|_| "stored SSO nonce is invalid".to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut buf = decoded[NONCE_BYTES..].to_vec();
    let plain = opening
        .open_in_place(nonce, Aad::empty(), &mut buf)
        .map_err(|_| "stored SSO client secret could not be decrypted".to_string())?;
    String::from_utf8(plain.to_vec()).map_err(|_| "stored SSO secret is not UTF-8".to_string())
}

// ---------------------------------------------------------------------------
// Config handlers (org-scoped)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SsoConfigBody {
    /// One of `oidc`, `google`, `okta`, `azure_ad`.
    pub provider_type: String,
    /// IdP issuer URL (discovery at `<issuer>/.well-known/openid-configuration`).
    pub issuer_url: String,
    pub client_id: String,
    /// Plaintext ONCE (encrypted at rest, never returned). Optional: public
    /// clients rely on PKCE alone.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Callback URL registered with the IdP.
    pub redirect_uri: String,
    /// Email domains allowed for JIT provisioning (empty = unrestricted).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// IdP role claim value -> application role.
    #[serde(default)]
    pub role_mapping: serde_json::Value,
    /// Require SSO for this org's members (advisory flag; the login flow
    /// consults it).
    #[serde(default)]
    pub enforce_sso: bool,
}

fn db_unavailable(what: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "sso_storage_unavailable",
            "reason": format!("{what} requires an attached PostgreSQL database"),
        })),
    )
        .into_response()
}

/// `PUT /api/saas/sso/config` — create or replace the org's SSO config.
pub async fn put_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<SsoConfigBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    if !OIDC_PROVIDERS.contains(&body.provider_type.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_sso_provider",
                "reason": "provider_type must be oidc, google, okta or azure_ad",
            })),
        )
            .into_response();
    }
    if body.issuer_url.trim().is_empty()
        || !body.issuer_url.trim().starts_with("https://")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_sso_issuer",
                "reason": "issuer_url must be an https:// URL",
            })),
        )
            .into_response();
    }
    if body.client_id.trim().is_empty() || body.redirect_uri.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_sso_config",
                "reason": "client_id and redirect_uri are required",
            })),
        )
            .into_response();
    }
    if !body.redirect_uri.trim().starts_with("https://") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_redirect_uri",
                "reason": "redirect_uri must be https",
            })),
        )
            .into_response();
    }
    if !body.role_mapping.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_role_mapping",
                "reason": "role_mapping must be a JSON object",
            })),
        )
            .into_response();
    }
    // platform_admin must never be a mapping target (DB trigger agrees).
    for value in body.role_mapping.as_object().unwrap().values() {
        if value.as_str() == Some("platform_admin") {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_role_mapping",
                    "reason": "platform_admin cannot be granted through SSO role mapping",
                })),
            )
                .into_response();
        }
    }
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("configuring SSO");
    };

    // Encrypt the client secret when supplied; keep the stored ciphertext
    // when the operator updates other fields without re-sending it.
    let encrypted_secret: Option<String> = match body.client_secret.as_deref() {
        Some(secret) if !secret.trim().is_empty() => match encrypt_client_secret(secret) {
            Ok(enc) => Some(enc),
            Err(reason) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "sso_encryption_unavailable", "reason": reason.to_string() })),
                )
                    .into_response()
            }
        },
        _ => None,
    };

    let domains: Vec<String> = body
        .allowed_domains
        .iter()
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();

    let result = sqlx::query(
        "INSERT INTO tenant_sso_configs \
            (organization_id, provider_type, issuer_url, client_id, client_secret_encrypted, \
             enforce_sso, allowed_domains, redirect_uri, role_mapping, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active') \
         ON CONFLICT (organization_id) DO UPDATE SET \
            provider_type = EXCLUDED.provider_type, \
            issuer_url = EXCLUDED.issuer_url, \
            client_id = EXCLUDED.client_id, \
            client_secret_encrypted = COALESCE(EXCLUDED.client_secret_encrypted, \
                                               tenant_sso_configs.client_secret_encrypted), \
            enforce_sso = EXCLUDED.enforce_sso, \
            allowed_domains = EXCLUDED.allowed_domains, \
            redirect_uri = EXCLUDED.redirect_uri, \
            role_mapping = EXCLUDED.role_mapping, \
            status = 'active', \
            updated_at = now() \
         RETURNING id",
    )
    .bind(ctx.organization.id.as_uuid())
    .bind(&body.provider_type)
    .bind(body.issuer_url.trim())
    .bind(body.client_id.trim())
    .bind(encrypted_secret.as_deref())
    .bind(body.enforce_sso)
    .bind(&domains)
    .bind(body.redirect_uri.trim())
    .bind(&body.role_mapping)
    .fetch_one(db.pool())
    .await;
    match result {
        Ok(row) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.sso.config_updated",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "id": row.get::<uuid::Uuid, _>("id"),
                    "organization_id": ctx.organization.id.to_string(),
                    "provider_type": body.provider_type,
                    "issuer_url": body.issuer_url.trim(),
                    "client_id": body.client_id.trim(),
                    "client_secret_set": encrypted_secret.is_some(),
                    "redirect_uri": body.redirect_uri.trim(),
                    "allowed_domains": domains,
                    "role_mapping": body.role_mapping,
                    "enforce_sso": body.enforce_sso,
                })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "sso config upsert failed");
            db_unavailable("configuring SSO")
        }
    }
}

/// `GET /api/saas/sso/config` — the config WITHOUT secrets.
pub async fn get_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("reading SSO config");
    };
    let row = sqlx::query(
        "SELECT id, provider_type, issuer_url, client_id, client_secret_encrypted, \
                enforce_sso, allowed_domains, redirect_uri, role_mapping, status \
         FROM tenant_sso_configs WHERE organization_id = $1",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await;
    match row {
        Ok(Some(row)) => {
            let secret_set = row
                .try_get::<Option<String>, _>("client_secret_encrypted")
                .ok()
                .flatten()
                .is_some();
            let domains: Vec<String> = row.get("allowed_domains");
            let role_mapping: serde_json::Value = row.get("role_mapping");
            (
                StatusCode::OK,
                Json(json!({
                    "id": row.get::<uuid::Uuid, _>("id"),
                    "organization_id": ctx.organization.id.to_string(),
                    "provider_type": row.get::<String, _>("provider_type"),
                    "issuer_url": row.get::<String, _>("issuer_url"),
                    "client_id": row.get::<String, _>("client_id"),
                    "client_secret_set": secret_set,
                    "redirect_uri": row.get::<String, _>("redirect_uri"),
                    "allowed_domains": domains,
                    "role_mapping": role_mapping,
                    "enforce_sso": row.get::<bool, _>("enforce_sso"),
                    "status": row.get::<String, _>("status"),
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "sso_not_configured" })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "sso config read failed");
            db_unavailable("reading SSO config")
        }
    }
}

/// `DELETE /api/saas/sso/config` — remove SSO for the org.
pub async fn delete_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("removing SSO config");
    };
    let result = sqlx::query(
        "DELETE FROM tenant_sso_configs WHERE organization_id = $1",
    )
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match result {
        Ok(_) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.sso.config_removed",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (StatusCode::OK, Json(json!({ "removed": true }))).into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "sso config delete failed");
            db_unavailable("removing SSO config")
        }
    }
}

// ---------------------------------------------------------------------------
// The public flow: authorize → (IdP) → callback
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    /// The tenant slug whose IdP the browser should be sent to.
    pub organization: String,
}

/// `GET /api/saas/sso/authorize?organization=<slug>` — begin the flow.
/// Returns the IdP URL the front-end should redirect to (never redirects
/// server-side: the SPA owns navigation).
pub async fn authorize(
    State(state): State<ApiState>,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("SSO authorization");
    };
    let org = match state.saas.organization_by_slug(&query.organization).await {
        Ok(Some(o)) => o,
        _ => return sso_error_response(&SsoError::NotConfigured),
    };
    let row = sqlx::query(
        "SELECT id, issuer_url, client_id, redirect_uri, status \
         FROM tenant_sso_configs WHERE organization_id = $1",
    )
    .bind(org.id.as_uuid())
    .fetch_optional(db.pool())
    .await;
    let row = match row {
        Ok(Some(r)) if r.get::<String, _>("status") == "active" => r,
        Ok(_) => return sso_error_response(&SsoError::NotConfigured),
        Err(error) => {
            tracing::error!(error = %error, "sso config lookup failed");
            return db_unavailable("SSO authorization");
        }
    };
    let issuer_url: String = row.get("issuer_url");
    let client_id: String = row.get("client_id");
    let redirect_uri: String = row.get("redirect_uri");

    // Discovery BEFORE minting state: a broken IdP fails fast with a
    // typed error instead of leaving dead pending entries.
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return sso_error_response(&e),
    };
    let discovery = match fetch_discovery(&client, &issuer_url).await {
        Ok(d) => d,
        Err(e) => return sso_error_response(&e),
    };

    let state_value = state_token();
    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let pending = PendingAuth {
        organization_id: org.id.as_uuid(),
        config_id: row.get("id"),
        code_verifier: verifier,
        redirect_uri: redirect_uri.clone(),
        created_at: Utc::now(),
    };
    if let Err(error) = sso_state::insert(db.pool(), &state_value, &pending).await {
        tracing::error!(error = %error, "sso pending authorization could not be stored");
        return sso_error_response(&SsoError::Storage);
    }
    // Best-effort housekeeping; an expired row is harmless because take()
    // refuses it, so a failure here must not fail the request.
    if let Err(error) = sso_state::purge_expired(db.pool(), Utc::now()).await {
        tracing::warn!(error = %error, "sso expired-state purge failed");
    }
    let url = build_authorize_url(
        &discovery.authorization_endpoint,
        &client_id,
        &redirect_uri,
        &state_value,
        &challenge,
    );
    state
        .audit
        .success("saas", "saas.sso.authorize_started", Some(&org.id.to_string()))
        .await;
    (
        StatusCode::OK,
        Json(json!({ "authorize_url": url, "state": state_value })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackBody {
    pub state: String,
    pub code: String,
}

/// `POST /api/saas/sso/callback` — finish the flow: verify everything,
/// JIT-provision, issue a session token (returned exactly once).
pub async fn callback(
    State(state): State<ApiState>,
    Json(body): Json<CallbackBody>,
) -> Response {
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("SSO callback");
    };
    let pending = match sso_state::take(db.pool(), &body.state, Utc::now()).await {
        Ok(Some(p)) => p,
        Ok(None) => return sso_error_response(&SsoError::InvalidState),
        Err(error) => {
            tracing::error!(error = %error, "sso pending authorization lookup failed");
            return sso_error_response(&SsoError::Storage);
        }
    };
    // Reload the config under the pending entry's org+config ids.
    let row = sqlx::query(
        "SELECT issuer_url, client_id, client_secret_encrypted, allowed_domains, role_mapping \
         FROM tenant_sso_configs WHERE id = $1 AND organization_id = $2 AND status = 'active'",
    )
    .bind(pending.config_id)
    .bind(pending.organization_id)
    .fetch_optional(db.pool())
    .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return sso_error_response(&SsoError::NotConfigured),
        Err(error) => {
            tracing::error!(error = %error, "sso callback config lookup failed");
            return db_unavailable("SSO callback");
        }
    };
    let issuer_url: String = row.get("issuer_url");
    let client_id: String = row.get("client_id");
    let secret_enc: Option<String> = row.get("client_secret_encrypted");
    let allowed_domains: Vec<String> = row.get("allowed_domains");
    let role_mapping: serde_json::Value = row.get("role_mapping");

    let client_secret = match secret_enc {
        Some(stored) => match decrypt_client_secret(&stored) {
            Ok(s) => Some(s),
            Err(reason) => {
                tracing::error!(error = %reason, "sso client secret could not be decrypted");
                return sso_error_response(&SsoError::Storage);
            }
        },
        None => None,
    };

    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return sso_error_response(&e),
    };
    // Discovery + token exchange.
    let discovery = match fetch_discovery(&client, &issuer_url).await {
        Ok(d) => d,
        Err(e) => return sso_error_response(&e),
    };
    let tokens = match exchange_code(
        &client,
        &discovery.token_endpoint,
        &body.code,
        &pending.redirect_uri,
        &client_id,
        client_secret.as_deref(),
        &pending.code_verifier,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return sso_error_response(&e),
    };
    let id_token = tokens.id_token.expect("checked by exchange_code");

    // Signature verification against the issuer JWKS (always).
    let (header_b64, payload_b64, sig_b64, kid) = match split_jwt(&id_token) {
        Ok(parts) => parts,
        Err(e) => return sso_error_response(&e),
    };
    let jwk = match fetch_jwk_for_kid(&client, &discovery.jwks_uri, &kid).await {
        Ok(j) => j,
        Err(e) => return sso_error_response(&e),
    };
    let der = match jwk_to_rsa_der(&jwk.0, &jwk.1) {
        Ok(d) => d,
        Err(e) => return sso_error_response(&e),
    };
    let signing_input = format!("{header_b64}.{payload_b64}");
    if let Err(e) = verify_rs256(&signing_input, &sig_b64, &der) {
        return sso_error_response(&e);
    }

    // Claims validation.
    let claims = match validate_claims(&payload_b64, &issuer_url, &client_id, Utc::now()) {
        Ok(c) => c,
        Err(e) => return sso_error_response(&e),
    };
    if claims.email.is_empty() {
        return sso_error_response(&SsoError::NoEmail);
    }
    if !email_domain_allowed(&claims.email, &allowed_domains) {
        state
            .audit
            .denied(
                "saas",
                "saas.sso.domain_denied",
                Some(&pending.organization_id.to_string()),
                "email_domain_not_allowed",
            )
            .await;
        return sso_error_response(&SsoError::DomainNotAllowed);
    }

    // JIT provisioning: user first, membership second.
    let email = User::normalize_email(&claims.email);
    let user = match state.saas.user_by_email(&email).await {
        Ok(Some(existing)) => {
            if !existing.can_authenticate() {
                return sso_error_response(&SsoError::UserInactive);
            }
            existing
        }
        Ok(None) => {
            let now = Utc::now();
            let new_user = User {
                id: UserId::new(),
                email: email.clone(),
                email_verified: claims.email_verified,
                display_name: email.split('@').next().unwrap_or("SSO User").to_string(),
                // Unusable marker: SSO accounts authenticate ONLY via the
                // IdP unless an owner triggers an explicit password reset.
                password_hash: "sso-only:$disabled".to_string(),
                status: UserStatus::Active,
                platform_admin: false,
                created_at: now,
                updated_at: now,
                last_login_at: None,
            };
            if let Err(e) = state.saas.create_user(&new_user).await {
                tracing::error!(error = %e, "sso JIT user creation failed");
                return sso_error_response(&SsoError::Storage);
            }
            state
                .audit
                .success("saas", "saas.sso.user_provisioned", Some(&email))
                .await;
            new_user
        }
        Err(e) => {
            tracing::error!(error = %e, "sso user lookup failed");
            return sso_error_response(&SsoError::Storage);
        }
    };

    let org_id = OrganizationId::from(pending.organization_id);
    let role = map_role(claims.role.as_deref(), &role_mapping);
    match state.saas.membership(org_id, user.id).await {
        Ok(Some(_)) => {} // existing membership wins; SSO never upgrades
        Ok(None) => {
            let membership = Membership::new(org_id, user.id, role, None, Utc::now());
            if let Err(e) = state.saas.create_membership(&membership).await {
                tracing::error!(error = %e, "sso JIT membership creation failed");
                return sso_error_response(&SsoError::Storage);
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "sso membership lookup failed");
            return sso_error_response(&SsoError::Storage);
        }
    }

    // Session (mirrors users::login issuance exactly).
    let now = Utc::now();
    let token = generate_token("ses");
    let session = SessionRecord::new(
        user.id,
        Some(org_id),
        token.hash.clone(),
        token.prefix.clone(),
        Duration::hours(DEFAULT_SESSION_TTL_HOURS),
        now,
    );
    if let Err(e) = state.saas.create_session(&session).await {
        tracing::error!(error = %e, "sso session creation failed");
        return sso_error_response(&SsoError::Storage);
    }
    state
        .audit
        .success("saas", "saas.sso.session_created", Some(&session.token_prefix))
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "user": user.profile(),
            "organization_id": org_id.to_string(),
            "session": {
                "id": session.id,
                "prefix": session.token_prefix,
                "expires_at": session.expires_at,
            },
            // The ONLY time the session token is ever returned.
            "token": token.plaintext,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_appendix_b() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifiers_are_wellformed_and_unique() {
        let a = pkce_verifier();
        let b = pkce_verifier();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43, "32 bytes -> 43 base64url chars");
        assert!(a.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')
        }));
        assert_eq!(state_token().len(), 22);
    }

    #[test]
    fn authorize_url_carries_every_binding_parameter() {
        let url = build_authorize_url(
            "https://idp.example.com/authorize",
            "client one",
            "https://app.example.com/sso/callback",
            "st@te",
            "ch@llenge",
        );
        assert!(url.starts_with("https://idp.example.com/authorize?response_type=code"));
        assert!(url.contains("client_id=client%20one"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fapp.example.com%2Fsso%2Fcallback"));
        assert!(url.contains("state=st%40te"));
        assert!(url.contains("code_challenge=ch%40llenge"));
        assert!(url.contains("code_challenge_method=S256"));
        // An endpoint that already has a query uses &.
        let url2 = build_authorize_url("https://idp.example.com/auth?x=1", "c", "r", "s", "ch");
        assert!(url2.contains("?x=1&response_type=code"));
    }

    #[test]
    fn domain_gate_rules() {
        let domains = vec!["Example.com".to_string(), ".corp.net".to_string()];
        assert!(email_domain_allowed("alice@example.com", &domains));
        assert!(email_domain_allowed("bob@deep.corp.net", &domains) == false,
            "exact-domain match only, no suffix wildcard");
        assert!(email_domain_allowed("bob@corp.net", &domains));
        assert!(!email_domain_allowed("mallory@evil.org", &domains));
        assert!(!email_domain_allowed("no-at-sign", &domains));
        assert!(email_domain_allowed("any@one.com", &[]), "empty list = unrestricted");
    }

    #[test]
    fn role_mapping_falls_back_and_never_grants_platform_admin() {
        let mapping = json!({
            "it-admin": "org_admin",
            "desk": "trader",
            "evil": "platform_admin"
        });
        assert_eq!(map_role(Some("it-admin"), &mapping), MembershipRole::OrgAdmin);
        assert_eq!(map_role(Some("desk"), &mapping), MembershipRole::Trader);
        // Direct identity mapping (claim value equals a role name).
        assert_eq!(map_role(Some("auditor"), &json!({})), MembershipRole::Auditor);
        // Unknown claim -> viewer.
        assert_eq!(map_role(Some("nobody"), &json!({})), MembershipRole::Viewer);
        // Missing claim -> viewer.
        assert_eq!(map_role(None, &mapping), MembershipRole::Viewer);
        // platform_admin is unreachable even when the mapping says so.
        assert_eq!(map_role(Some("evil"), &mapping), MembershipRole::Viewer);
        assert_eq!(
            map_role(Some("platform_admin"), &json!({})),
            MembershipRole::Viewer
        );
    }

    #[test]
    fn claims_validation_enforces_iss_aud_exp() {
        let now = DateTime::from_timestamp(1_000_000, 0).unwrap();
        let claims = json!({
            "iss": "https://idp.example.com/",
            "aud": "client-1",
            "sub": "user-9",
            "exp": 2_000_000,
            "email": "Alice@Example.com",
            "email_verified": true,
            "role": "trader",
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let parsed = validate_claims(&payload, "https://idp.example.com", "client-1", now).unwrap();
        assert_eq!(parsed.email, "Alice@Example.com");
        assert!(parsed.email_verified);
        assert_eq!(parsed.role.as_deref(), Some("trader"));

        // Wrong audience.
        assert_eq!(
            validate_claims(&payload, "https://idp.example.com", "other-client", now)
                .unwrap_err(),
            SsoError::BadClaims
        );
        // Wrong issuer.
        assert_eq!(
            validate_claims(&payload, "https://evil.example.com", "client-1", now).unwrap_err(),
            SsoError::BadClaims
        );
        // Expired.
        let later = DateTime::from_timestamp(3_000_000, 0).unwrap();
        assert_eq!(
            validate_claims(&payload, "https://idp.example.com", "client-1", later).unwrap_err(),
            SsoError::BadClaims
        );
        // aud as array also validates.
        let arr_claims = json!({
            "iss": "https://idp.example.com",
            "aud": ["other", "client-1"],
            "sub": "u",
            "exp": 2_000_000,
            "email": "x@y.z",
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&arr_claims).unwrap());
        assert!(validate_claims(&payload, "https://idp.example.com", "client-1", now).is_ok());
    }

    #[test]
    fn split_jwt_rejects_alg_confusion() {
        let header_none = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let header_hs = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","kid":"k"}"#);
        let payload = URL_SAFE_NO_PAD.encode(b"{}");
        let token_none = format!("{header_none}.{payload}.sig");
        assert_eq!(split_jwt(&token_none).unwrap_err(), SsoError::MalformedIdToken);
        let token_hs = format!("{header_hs}.{payload}.sig");
        assert_eq!(split_jwt(&token_hs).unwrap_err(), SsoError::MalformedIdToken);
        // RS256 without kid is also malformed (we need the JWKS lookup key).
        let header_rs = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#);
        let token_rs = format!("{header_rs}.{payload}.sig");
        assert_eq!(split_jwt(&token_rs).unwrap_err(), SsoError::MalformedIdToken);
        // Two segments only.
        assert_eq!(split_jwt("a.b").unwrap_err(), SsoError::MalformedIdToken);
    }

    #[test]
    fn der_integer_encoding_is_canonical() {
        // High bit set -> leading zero pad.
        assert_eq!(der_integer(&[0x80]), vec![0x02, 0x02, 0x00, 0x80]);
        // Leading zeros stripped.
        assert_eq!(der_integer(&[0x00, 0x01]), vec![0x02, 0x01, 0x01]);
        // Small positive unchanged.
        assert_eq!(der_integer(&[0x03]), vec![0x02, 0x01, 0x03]);
        // Long-form length (>127 bytes).
        let big = vec![0x01; 200];
        let enc = der_integer(&big);
        assert_eq!(enc[0], 0x02);
        assert_eq!(enc[1], 0x81); // long form, one length byte
        assert_eq!(enc[2], 200);
    }

    #[test]
    fn jwk_to_der_rejects_small_moduli() {
        // 128-byte modulus (1024-bit) must be rejected.
        let small_n = URL_SAFE_NO_PAD.encode(vec![0x11; 128]);
        let e = URL_SAFE_NO_PAD.encode(vec![0x01, 0x00, 0x01]);
        assert!(jwk_to_rsa_der(&small_n, &e).is_err());
        // 256-byte modulus accepted, DER is a SEQUENCE of two INTEGERs.
        let n = URL_SAFE_NO_PAD.encode(vec![0x11; 256]);
        let der = jwk_to_rsa_der(&n, &e).unwrap();
        assert_eq!(der[0], 0x30);
        assert_eq!(der[1], 0x82); // long form, two length bytes
    }

    #[test]
    fn verify_rs256_rejects_garbage_signatures() {
        let n = URL_SAFE_NO_PAD.encode(vec![0x11; 256]);
        let e = URL_SAFE_NO_PAD.encode(vec![0x01, 0x00, 0x01]);
        let der = jwk_to_rsa_der(&n, &e).unwrap();
        let sig = URL_SAFE_NO_PAD.encode(vec![0xAB; 256]);
        assert_eq!(
            verify_rs256("aGVhZGVy.cGF5bG9hZA", &sig, &der).unwrap_err(),
            SsoError::BadSignature
        );
        // Non-base64 signature -> BadSignature (fail closed).
        assert_eq!(
            verify_rs256("a.b", "!!!not-base64!!!", &der).unwrap_err(),
            SsoError::BadSignature
        );
    }

    #[test]
    fn client_secret_encryption_round_trips_when_keyed() {
        std::env::set_var(SSO_KEY_ENV, STANDARD_NO_PAD.encode([9u8; 32]));
        let enc = encrypt_client_secret("super-secret-value").unwrap();
        assert!(enc.starts_with(ENC_PREFIX));
        assert_eq!(decrypt_client_secret(&enc).unwrap(), "super-secret-value");
        // Tampering breaks the GCM tag.
        let mut tampered = enc.clone();
        let len = tampered.len();
        tampered.replace_range(len - 4.., "AAAA");
        assert!(decrypt_client_secret(&tampered).is_err());
        std::env::remove_var(SSO_KEY_ENV);
        // Without the key both directions fail closed.
        assert!(encrypt_client_secret("x").is_err());
        assert!(decrypt_client_secret(&enc).is_err());
    }
}
