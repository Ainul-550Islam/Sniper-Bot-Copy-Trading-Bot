//! Per-identity throttling for password, MFA, and enrollment operations.
//!
//! The API router also applies its coarser per-IP limiter. This second
//! limiter prevents one account or TOTP device from receiving the full IP
//! budget. Keys are hashed before entering the in-memory bucket map.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::auth::{sha256_hex, RateVerdict};

use crate::api::ApiState;

/// Return an HTTP 429 response when the sensitive-identity bucket is empty.
/// The production bucket is deliberately lower-volume than the general API
/// bucket; it is a per-process control and should be paired with edge limits
/// when the service is deployed across multiple replicas.
pub async fn reject_sensitive_attempt(
    state: &ApiState,
    operation: &str,
    subject: &str,
) -> Option<Response> {
    if state.sensitive_limiter.rpm() == 0 {
        return None;
    }
    let material = format!("saas-sensitive-v1:{operation}:{subject}");
    let key = sha256_hex(&material);
    match state.sensitive_limiter.check(&key).await {
        RateVerdict::Allowed => None,
        RateVerdict::Limited { retry_after_secs } => Some(
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                Json(json!({
                    "error": "rate_limited",
                    "reason": format!("too many {operation} attempts; retry after {retry_after_secs} seconds"),
                })),
            )
                .into_response(),
        ),
    }
}
