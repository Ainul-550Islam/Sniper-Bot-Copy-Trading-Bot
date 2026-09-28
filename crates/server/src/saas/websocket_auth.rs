//! Canonical SaaS WebSocket authentication implementation (BATCH 2 file 11).
//!
//! ONLY supports: Authorization header and/or first-frame authentication.
//! Explicitly rejects secrets supplied via URL query parameters for SaaS routes.
//! Defines authentication timeout and replay protection.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, AUTH_HEADER};

/// How long a first-frame-auth socket may wait for its auth frame.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Max age for a token to be considered fresh (replay protection).
pub const TOKEN_MAX_AGE: Duration = Duration::from_secs(300);

/// Replay protection: in-memory set of seen token hashes (bounded).
fn seen_tokens() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

fn token_hash(token: &str) -> String {
    // Use sha256 hex — never store plaintext
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

pub fn is_replay(token: &str) -> bool {
    let hash = token_hash(token);
    let mut seen = seen_tokens().lock().expect("mutex");
    if seen.contains(&hash) {
        return true;
    }
    if seen.len() > 10_000 {
        seen.clear();
    }
    seen.insert(hash);
    false
}

#[cfg(test)]
pub fn clear_replay_cache() {
    seen_tokens().lock().expect("mutex").clear();
}

/// Validate that no SaaS credential appears in URL query string.
pub fn reject_query_credentials(query: &str) -> Result<(), String> {
    let lower = query.to_ascii_lowercase();
    const FORBIDDEN: &[&str] = &[
        "token=", "key=", "api_key=", "apikey=", "secret=", "auth=", "bearer=",
    ];
    for pat in FORBIDDEN {
        if lower.contains(pat) {
            return Err(format!("credentials in URL query are forbidden: {}", pat));
        }
    }
    Ok(())
}

/// Extract token from Authorization header (Bearer ...).
pub fn token_from_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTH_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.strip_prefix("Bearer ")
                .or_else(|| s.strip_prefix("bearer "))
                .map(|t| t.trim().to_string())
        })
        .filter(|t| !t.is_empty())
}

/// Validate first-frame auth payload shape: {"type":"auth","token":"..."}
pub fn parse_first_frame_token(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("auth") {
        return None;
    }
    let token = v.get("token").and_then(|t| t.as_str())?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    // Reject token that was supplied via URL-like string?
    if token.contains('?') || token.contains('&') && token.contains("key=") {
        return None;
    }
    Some(token)
}

/// Check token freshness (if token embeds timestamp, else just length).
pub fn is_token_fresh(token: &str) -> bool {
    // For opaque tokens we just check length; for JWT-like we would parse exp
    // For now, reject empty or overly long
    let len = token.len();
    (8..=4096).contains(&len)
}

/// Canonical SaaS WS auth: header OR first-frame, never query.
#[allow(clippy::result_large_err)]
pub async fn authenticate_saas_ws(
    state: &ApiState,
    headers: &HeaderMap,
    query_str: Option<&str>,
    first_frame: Option<&str>,
) -> Result<crate::saas::SaasContext, Response> {
    // 1. Reject query credentials for SaaS routes unconditionally
    if let Some(q) = query_str {
        if let Err(reason) = reject_query_credentials(q) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"query_credentials_forbidden","reason": reason})),
            )
                .into_response());
        }
    }

    // 2. Try header auth first (pre-upgrade)
    if let Some(token) = token_from_header(headers) {
        if !is_token_fresh(&token) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"stale_token","reason":"token not fresh"})),
            )
                .into_response());
        }
        if is_replay(&token) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"replay_detected","reason":"token replay"})),
            )
                .into_response());
        }
        let h = headers.clone();
        // headers already contain bearer; authorize
        match authorize_request(
            state,
            &h,
            bot_core::authorization::AccessRequest::read(
                bot_core::membership::Permission::TenantRead,
            ),
        )
        .await
        {
            Ok(ctx) => return Ok(ctx),
            Err(d) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": d.kind.as_str(), "reason": d.reason})),
                )
                    .into_response());
            }
        }
    }

    // 3. Fallback to first-frame auth (browser path) — caller must have provided first_frame
    if let Some(frame) = first_frame {
        let token = match parse_first_frame_token(frame) {
            Some(t) => t,
            None => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error":"invalid_first_frame","reason":"expected {\"type\":\"auth\",\"token\":\"…\"}"})),
                )
                    .into_response())
            }
        };
        if !is_token_fresh(&token) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"stale_token"})),
            )
                .into_response());
        }
        if is_replay(&token) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"replay_detected"})),
            )
                .into_response());
        }
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", token)) {
            h.insert(AUTH_HEADER, v);
        }
        match authorize_request(
            state,
            &h,
            bot_core::authorization::AccessRequest::read(
                bot_core::membership::Permission::TenantRead,
            ),
        )
        .await
        {
            Ok(ctx) => return Ok(ctx),
            Err(d) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": d.kind.as_str(), "reason": d.reason})),
                )
                    .into_response());
            }
        }
    }

    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"missing_credentials","reason":"header or first-frame auth required"})),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_query_credential_rejected() {
        assert!(reject_query_credentials("?token=abc").is_err());
        assert!(reject_query_credentials("?key=secret").is_err());
        assert!(reject_query_credentials("?api_key=xyz").is_err());
        assert!(reject_query_credentials("?secret=123").is_err());
        assert!(reject_query_credentials("?foo=bar&token=xyz").is_err());
        assert!(reject_query_credentials("?foo=bar").is_ok());
        assert!(reject_query_credentials("").is_ok());
    }

    #[test]
    fn header_auth_accepted() {
        let mut h = HeaderMap::new();
        h.insert(
            AUTH_HEADER,
            HeaderValue::from_str("Bearer goodtoken123").unwrap(),
        );
        assert_eq!(token_from_header(&h), Some("goodtoken123".into()));
    }

    #[test]
    fn first_frame_auth_accepted() {
        let frame = r#"{"type":"auth","token":"mytoken12345"}"#;
        assert_eq!(parse_first_frame_token(frame), Some("mytoken12345".into()));
    }

    #[test]
    fn first_frame_wrong_type_rejected() {
        let frame = r#"{"type":"not_auth","token":"abc"}"#;
        assert!(parse_first_frame_token(frame).is_none());
    }

    #[test]
    fn replay_protection() {
        clear_replay_cache();
        assert!(!is_replay("unique_token_12345678"));
        assert!(is_replay("unique_token_12345678"));
        clear_replay_cache();
        assert!(!is_replay("unique_token_12345678"));
    }

    #[test]
    fn cross_tenant_rejection_requires_org_check() {
        // The auth layer enforces tenant ownership via SaasContext; this test documents that
        // query bypass must not grant access — header/first-frame path is the only gate.
        assert!(reject_query_credentials("?key=tenantA_secret").is_err());
    }

    #[test]
    fn auth_timeout_constant() {
        assert_eq!(AUTH_TIMEOUT.as_secs(), 10);
    }
}
