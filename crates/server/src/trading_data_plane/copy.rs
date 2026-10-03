//! Tenant copy-trading handlers (PROMPT 3/10 #64).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;
use bot_core::models::BotModule;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use super::orders::{plane_error, unavailable};
use crate::api::ApiState;

/// `GET /api/tenant/copy/leaders` — the caller's followed leaders
/// (the SAME external address followed by another tenant never
/// appears here).
pub async fn leaders(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(_params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::BotRead,
        super::authorization_chain::TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = match state.trading.as_ref() {
        Some(p) => p,
        None => return unavailable(),
    };
    let org = auth.organization_id();
    let scope = plane.read_scope(org);
    match plane.copy_read().leaders(&scope).await {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "items": rows })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/copy/leaders/:address` — the caller's OWN row for
/// that leader (label, status, counters) plus its lifecycle events.
pub async fn leader(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(address): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::BotRead,
        super::authorization_chain::TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = match state.trading.as_ref() {
        Some(p) => p,
        None => return unavailable(),
    };
    let org = auth.organization_id();
    let scope = plane.read_scope(org);
    let leader = match plane.copy_read().leader(&scope, &address).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "kind": "copy_leader" })),
            )
                .into_response()
        }
        Err(e) => return plane_error(e),
    };
    let events = match plane.copy_read().leader_events(&scope, &address, 100).await {
        Ok(rows) => rows,
        Err(e) => return plane_error(e),
    };
    (
        StatusCode::OK,
        Json(json!({ "leader": leader, "events": events })),
    )
        .into_response()
}

/// `GET /api/tenant/copy/links` — the caller's OPEN follower↔leader
/// links (reconciliation input).
pub async fn open_links(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::BotRead,
        super::authorization_chain::TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = match state.trading.as_ref() {
        Some(p) => p,
        None => return unavailable(),
    };
    let org = auth.organization_id();
    let scope = plane.read_scope(org);
    let links = if let (Some(leader), Some(mint)) = (params.get("leader"), params.get("mint")) {
        plane.copy_read().open_links_for(&scope, leader, mint).await
    } else {
        plane.copy_read().open_links(&scope).await
    };
    match links {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "items": rows })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/copy/status` — the caller's copy module state:
/// entitlement (verified by the chain), tenant override, runtime phase.
pub async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    (
        StatusCode::OK,
        Json(
            status_payload(
                &state,
                &auth,
                BotModule::Copy,
                feature_key_for(BotModule::Copy),
            )
            .await,
        ),
    )
        .into_response()
}

/// `POST /api/tenant/copy/controls` — tenant-level enable/disable for
/// the caller's OWN organization (runtime lifecycle stays runtime-owned).
pub async fn controls(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let action = match ControlAction::parse(&body) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let permission = match action {
        ControlAction::Enable => Permission::BotStart,
        ControlAction::Disable { .. } => Permission::BotStop,
    };
    let auth = match guard_manage(&state, &headers, permission, TradingModuleFamily::Copy).await {
        Ok(a) => a,
        Err(response) => return response,
    };
    apply_control(&state, &auth, BotModule::Copy, action).await
}
