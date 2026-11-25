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
            // A health-registry sample proves the deployment reached the
            // component a moment ago — it is NOT live-verification evidence of
            // a trade, a fill or a provider contract. Only `sampled` is
            // claimed; `live_verified` is left for the endpoints that actually
            // carry a venue/chain proof, and never inferred from health alone.
            let status = if component.status.healthy && component.status.ready {
                "connected"
            } else if component.status.ready {
                "degraded"
            } else {
                "error"
            };
            let evidence_level = if component.status.healthy && component.status.ready {
                "sampled"
            } else {
                "not_run"
            };
            let detail = if component.status.healthy && component.status.ready {
                Some(format!(
                    "{} (a health sample, not proof of a live trade or provider contract)",
                    component
                        .status
                        .detail
                        .unwrap_or_else(|| "healthy".to_string())
                ))
            } else {
                component.status.detail
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
                "evidence_level_meaning": "sampled = a health-registry sample within the registry interval; live_verified is never claimed by this endpoint",
                "detail": detail,
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

#[cfg(test)]
mod tests {
    /// A health sample is evidence of a sample — never of a live trade or a
    /// provider contract. The vocabulary is asserted at the source so a future
    /// edit cannot quietly restore `live_verified`.
    #[test]
    fn a_healthy_sample_is_never_labelled_live_verified() {
        let source = include_str!("integrations.rs");
        assert!(
            !source.contains("\"live_verified\""),
            "health sampling must never claim live verification"
        );
        assert!(source.contains("\"sampled\""), "the sample label must stay");
        assert!(
            source.contains("not proof of a live trade or provider contract"),
            "the detail must say what the label means"
        );
    }
}
