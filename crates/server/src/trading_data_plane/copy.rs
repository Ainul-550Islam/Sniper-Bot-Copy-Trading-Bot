//! Tenant copy-trading handlers (PROMPT 3/10 #64).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Map, Value};

use bot_core::membership::Permission;
use bot_core::models::BotModule;

use super::config_store::{read_module, write_module};

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use super::orders::{plane_error, unavailable};
use crate::api::ApiState;

/// `GET /api/tenant/copy/leaders`
pub async fn leaders(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(_params): Query<HashMap<String, String>>,
) -> Response {
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

/// `GET /api/tenant/copy/leaders/:address`
pub async fn leader(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(address): Path<String>,
) -> Response {
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

/// `GET /api/tenant/copy/links`
pub async fn open_links(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
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

/// `GET /api/tenant/copy/status`
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

/// `POST /api/tenant/copy/controls`
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

fn validate_config(body: Value) -> Result<Map<String, Value>, Response> {
    let Some(object) = body.as_object() else {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config", "detail": "configuration body must be a JSON object" }))).into_response());
    };
    const ALLOWED: &[&str] = &[
        "max_exposure_usd",
        "allocation_per_trade_sol",
        "max_slippage_bps",
        "mirror_buys",
        "mirror_sells",
        "stale_event_timeout_seconds",
        "allowed_tokens",
        "blocked_tokens",
        "dry_run",
        "copy_ratio_pct",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "unknown_config_field", "detail": format!("unsupported copy configuration field: {unknown}") }))).into_response());
    }
    for key in [
        "max_exposure_usd",
        "allocation_per_trade_sol",
        "copy_ratio_pct",
    ] {
        if let Some(value) = object.get(key) {
            let Some(number) = value.as_f64() else {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} must be a number") }))).into_response());
            };
            if !number.is_finite()
                || number < 0.0
                || number > 1_000_000_000.0
                || (key == "copy_ratio_pct" && number > 1_000.0)
            {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} is outside its allowed range") }))).into_response());
            }
        }
    }
    if let Some(value) = object.get("max_slippage_bps") {
        if value.as_u64().is_none_or(|number| number > 10_000) {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": "max_slippage_bps must be an integer between 0 and 10000" }))).into_response());
        }
    }
    if let Some(value) = object.get("stale_event_timeout_seconds") {
        if value.as_u64().is_none_or(|number| number > 31_536_000) {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": "stale_event_timeout_seconds is outside its allowed range" }))).into_response());
        }
    }
    for key in ["mirror_buys", "mirror_sells", "dry_run"] {
        if object.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} must be boolean") }))).into_response());
        }
    }
    for key in ["allowed_tokens", "blocked_tokens"] {
        if let Some(value) = object.get(key) {
            let Some(items) = value.as_array() else {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} must be an array of strings") }))).into_response());
            };
            if items.len() > 10_000
                || items.iter().any(|item| {
                    item.as_str()
                        .is_none_or(|text| text.is_empty() || text.len() > 128)
                })
            {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} contains an invalid item") }))).into_response());
            }
        }
    }
    Ok(object.clone())
}

/// `GET /api/tenant/copy/config`
pub async fn get_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "trading_data_plane_unavailable", "detail": "copy configuration requires PostgreSQL" }))).into_response();
    };
    match read_module(db, auth.organization_id(), "copy").await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to read tenant copy configuration");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "config_storage_error", "detail": "copy configuration could not be loaded" }))).into_response()
        }
    }
}

/// `PUT /api/tenant/copy/config`
pub async fn update_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let patch = match validate_config(body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "trading_data_plane_unavailable", "detail": "copy configuration requires PostgreSQL" }))).into_response();
    };
    match write_module(
        db,
        auth.organization_id(),
        "copy",
        &patch,
        &auth.ctx.actor_label(),
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to write tenant copy configuration");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "config_storage_error", "detail": "copy configuration could not be saved" }))).into_response()
        }
    }
}
