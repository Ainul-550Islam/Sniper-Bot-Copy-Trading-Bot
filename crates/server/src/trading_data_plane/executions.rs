//! Tenant execution / transaction handlers (PROMPT 3/10 #62).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;
use chrono::Utc;

use super::orders::plane_error;
use crate::api::ApiState;

/// `GET /api/tenant/executions?limit=&cursor=&order_id=` — the
/// caller's execution attempts.
pub async fn list(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: the full customer-API authorization chain — authenticate →
    // organization (credential-side only) → plane → lifecycle →
    // entitlement → module family. Denials carry typed codes.
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let plane = auth.plane.clone();
    let scope = plane.read_scope(org);
    if let Some(order_id) = params.get("order_id") {
        return match plane
            .executions_read()
            .list_for_order(&scope, order_id, 500)
            .await
        {
            Ok(rows) => (
                StatusCode::OK,
                Json(json!({ "organization_id": org.to_string(), "items": rows })),
            )
                .into_response(),
            Err(e) => plane_error(e),
        };
    }
    let (since, until) = match window(&params) {
        Some(w) => w,
        None => return bad_window(),
    };
    match plane
        .executions_read()
        .list_between(&scope, since, until, 1000)
        .await
    {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "items": rows })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/executions/:id` — one of the caller's executions.
pub async fn get_one(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let scope = plane.read_scope(auth.organization_id());
    let id = match id.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_execution_id" })),
            )
                .into_response()
        }
    };
    match plane.executions_read().get(&scope, id).await {
        Ok(execution) => (StatusCode::OK, Json(json!({ "execution": execution }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/transactions/:signature` — the caller's on-chain
/// transaction record (the signature is a global identity; the
/// repository still answers only for the acting tenant's row).
pub async fn transaction(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(signature): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let scope = plane.read_scope(auth.organization_id());
    match plane
        .executions_read()
        .transaction(&scope, &signature)
        .await
    {
        Ok(tx) => match tx {
            Some(tx) => (StatusCode::OK, Json(json!({ "transaction": tx }))).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "kind": "transaction" })),
            )
                .into_response(),
        },
        Err(e) => plane_error(e),
    }
}

/// `?since=<rfc3339>&until=<rfc3339>` (defaults: last 24h).
pub(super) fn window(
    params: &HashMap<String, String>,
) -> Option<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)> {
    let until = params
        .get("until")
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let since = params
        .get("since")
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| until - chrono::Duration::hours(24));
    if since >= until {
        return None;
    }
    Some((since, until))
}

fn bad_window() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_window" })),
    )
        .into_response()
}
