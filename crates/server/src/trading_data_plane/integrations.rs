//! Tenant infrastructure status from the server health registry.
//!
//! This endpoint reports only components registered and sampled by the
//! deployment. It never invents providers, connectivity, latency, or live
//! verification evidence.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use bot_core::membership::Permission;

use super::authorization_chain::{guard, TradingModuleFamily};
use crate::api::ApiState;

fn category_for(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("rpc") || lower.contains("geyser") || lower.contains("solana") {
        "solana_rpc"
    } else if lower.contains("billing") || lower.contains("stripe") || lower.contains("paddle") {
        "billing"
    } else if lower.contains("custody")
        || lower.contains("kms")
        || lower.contains("vault")
        || lower.contains("sign")
    {
        "custody_signer"
    } else if lower.contains("telegram") || lower.contains("alert") {
        "alerts"
    } else {
        "other"
    }
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// `GET /api/tenant/integrations`.
pub async fn list(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    let snapshot = state.health.snapshot();
    let items: Vec<_> = snapshot
        .components
        .into_iter()
        .map(|component| {
            let status = if component.status.healthy && component.status.ready {
                "connected"
            } else if component.status.ready {
                "degraded"
            } else {
                "error"
            };
            let evidence_level = if component.status.healthy && component.status.ready {
                "live_verified"
            } else {
                "not_run"
            };
            json!({
                "id": slug(&component.name),
                "organization_id": auth.organization_id().to_string(),
                "provider_name": component.name,
                "category": category_for(&component.name),
                "status": status,
                "endpoint_url_masked": Value::Null,
                "evidence_level": evidence_level,
                "last_health_check_at": Value::Null,
                "latency_ms": Value::Null,
                "detail": component.status.detail,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": auth.organization_id().to_string(),
            "items": items,
            "count": items.len(),
            "health_registry": {
                "status": snapshot.status,
                "healthy": snapshot.healthy,
                "ready": snapshot.ready,
            }
        })),
    )
        .into_response()
}
