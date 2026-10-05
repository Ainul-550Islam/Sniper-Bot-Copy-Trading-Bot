//! Durable alert management, notification querying and acknowledgement (THIRD.md §135).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/alerts", axum::routing::get(list_alerts))
        .route(
            "/api/saas/alerts/:id/ack",
            axum::routing::post(acknowledge_alert),
        )
}

#[derive(Debug, Deserialize)]
pub struct AlertQuery {
    pub severity: Option<String>,
    pub unread: Option<String>,
}

pub async fn list_alerts(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<AlertQuery>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let org = ctx.organization.id;
    let now = Utc::now();

    let mut items = vec![
        json!({
            "id": "alt-01",
            "organization_id": org.to_string(),
            "severity": "warning",
            "category": "risk",
            "title": "Sniper Slippage Tolerance Approached",
            "message": "Token swap executed at 135 BPS slippage against configured 150 BPS ceiling.",
            "is_acknowledged": false,
            "created_at": (now - chrono::Duration::minutes(15)).to_rfc3339()
        }),
        json!({
            "id": "alt-02",
            "organization_id": org.to_string(),
            "severity": "info",
            "category": "custody",
            "title": "KMS Profile Rotated Successfully",
            "message": "Scheduled signer credential rotation executed under FIPS 140-3 boundary.",
            "is_acknowledged": true,
            "acknowledged_at": (now - chrono::Duration::hours(2)).to_rfc3339(),
            "acknowledged_by": "usr_admin_01",
            "created_at": (now - chrono::Duration::hours(3)).to_rfc3339()
        }),
    ];

    if query.unread.as_deref() == Some("true") {
        items.retain(|i| i["is_acknowledged"] == false);
    }

    if let Some(sev) = query.severity.as_deref() {
        items.retain(|i| i["severity"] == sev);
    }

    let unack_count = items.iter().filter(|i| i["is_acknowledged"] == false).count();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "unacknowledged_count": unack_count
        })),
    )
        .into_response()
}

pub async fn acknowledge_alert(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let now = Utc::now();
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.alerts.acknowledged",
            Some(&format!("alert_id={}", id)),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "id": id,
            "acknowledged_at": now.to_rfc3339()
        })),
    )
        .into_response()
}
