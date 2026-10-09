//! Customer-safe platform and dependency health aggregation.
//!
//! Only components present in the process health registry are returned. This
//! endpoint does not invent provider rows, latency samples, incident counts,
//! or timestamps for health checks that were not recorded.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};

use crate::api::ApiState;

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/status", axum::routing::get(get_service_status))
}

fn category_for(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("rpc") || lower.contains("geyser") || lower.contains("solana") {
        "Market Data / Chain"
    } else if lower.contains("database") || lower.contains("postgres") || lower.contains("redis") {
        "Storage"
    } else if lower.contains("sign") || lower.contains("custody") || lower.contains("kms") {
        "Security / Signing"
    } else if lower.contains("billing") || lower.contains("stripe") || lower.contains("paddle") {
        "Billing"
    } else {
        "Deployment Component"
    }
}

pub async fn get_service_status(State(state): State<ApiState>) -> Response {
    let now = Utc::now();
    let snapshot = state.health.snapshot();
    let components: Vec<Value> = snapshot
        .components
        .into_iter()
        .map(|component| {
            let status = if component.status.healthy && component.status.ready {
                "operational"
            } else if component.status.ready || component.status.healthy {
                "degraded"
            } else {
                "unavailable"
            };
            json!({
                "name": component.name,
                "category": category_for(&component.name),
                "status": status,
                "latency_ms": Value::Null,
                "last_checked": Value::Null,
                "details": component.status.detail.unwrap_or_else(|| "No health detail was recorded".to_string()),
            })
        })
        .collect();
    let active_incidents_count = if components.is_empty() {
        Value::Null
    } else {
        json!(components
            .iter()
            .filter(
                |component| component.get("status").and_then(Value::as_str) != Some("operational")
            )
            .count())
    };
    let overall_status = if components.is_empty() {
        "unavailable"
    } else if snapshot.healthy && snapshot.ready {
        "operational"
    } else {
        "degraded"
    };

    (
        StatusCode::OK,
        Json(json!({
            "overall_status": overall_status,
            "as_of": now.to_rfc3339(),
            "components": components,
            "active_incidents_count": active_incidents_count,
            "source": "process health registry",
        })),
    )
        .into_response()
}
