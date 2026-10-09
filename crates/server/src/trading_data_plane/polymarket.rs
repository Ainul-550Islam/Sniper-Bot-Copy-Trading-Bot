//! Tenant polymarket handlers (PROMPT 3/10 #65).

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
use super::executions::window;
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use super::orders::{page_request, plane_error};
use crate::api::ApiState;

/// `GET /api/tenant/polymarket/orders?limit=&cursor=`
pub async fn orders(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let plane = auth.plane.clone();
    if params
        .get("open")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        let scope = plane.read_scope(org);
        return match plane.poly_read().open_orders(&scope).await {
            Ok(rows) => (
                StatusCode::OK,
                Json(json!({ "organization_id": org.to_string(), "items": rows })),
            )
                .into_response(),
            Err(e) => plane_error(e),
        };
    }
    let page = match page_request(org, &params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let scope = plane.read_scope(org);
    match plane.poly_read().orders_page(&scope, &page).await {
        Ok(page) => (
            StatusCode::OK,
            Json(json!({
                "organization_id": org.to_string(),
                "items": page.items,
                "next_cursor": page.next.as_ref().map(|c| c.encode()),
            })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/polymarket/orders/:venue_order_id`
pub async fn order(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(venue_order_id): Path<String>,
) -> Response {
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let scope = plane.read_scope(auth.organization_id());
    let order = match plane.poly_read().order(&scope, &venue_order_id).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "kind": "poly_order" })),
            )
                .into_response()
        }
        Err(e) => return plane_error(e),
    };
    let fills = match plane
        .poly_read()
        .fills_for_order(&scope, &venue_order_id)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return plane_error(e),
    };
    (
        StatusCode::OK,
        Json(json!({ "order": order, "fills": fills })),
    )
        .into_response()
}

/// `GET /api/tenant/polymarket/fills?since=&until=`
pub async fn fills(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let (since, until) = match window(&params) {
        Some(w) => w,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_window" })),
            )
                .into_response()
        }
    };
    let scope = plane.read_scope(auth.organization_id());
    match plane.poly_read().fills_between(&scope, since, until).await {
        Ok(rows) => (StatusCode::OK, Json(json!({ "items": rows }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/polymarket/reconciliation`
pub async fn reconciliation(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(_params): Query<HashMap<String, String>>,
) -> Response {
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::ReconciliationRead,
        super::authorization_chain::TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let org = auth.organization_id();
    let scope = plane.read_scope(org);
    match plane
        .poly_recon()
        .detect_drift(&scope, chrono::Duration::minutes(30), chrono::Utc::now())
        .await
    {
        Ok(drift) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "drift": drift })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/polymarket/status`
pub async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Polymarket,
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
                BotModule::Polymarket,
                feature_key_for(BotModule::Polymarket),
            )
            .await,
        ),
    )
        .into_response()
}

/// `POST /api/tenant/polymarket/controls`
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
    let auth = match guard_manage(
        &state,
        &headers,
        permission,
        TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    apply_control(&state, &auth, BotModule::Polymarket, action).await
}

fn validate_config(body: Value) -> Result<Map<String, Value>, Response> {
    let Some(object) = body.as_object() else {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config", "detail": "configuration body must be a JSON object" }))).into_response());
    };
    const ALLOWED: &[&str] = &[
        "active_condition_ids",
        "max_position_size_usdc",
        "max_market_exposure_usdc",
        "spread_threshold_bps",
        "reprice_interval_seconds",
        "cancel_stale_orders",
        "dry_run",
        "order_type",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "unknown_config_field", "detail": format!("unsupported Polymarket configuration field: {unknown}") }))).into_response());
    }
    for key in ["max_position_size_usdc", "max_market_exposure_usdc"] {
        if let Some(value) = object.get(key) {
            let Some(number) = value.as_f64() else {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} must be a number") }))).into_response());
            };
            if !number.is_finite() || number < 0.0 || number > 1_000_000_000.0 {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} is outside its allowed range") }))).into_response());
            }
        }
    }
    for (key, maximum) in [
        ("spread_threshold_bps", 10_000),
        ("reprice_interval_seconds", 31_536_000),
    ] {
        if let Some(value) = object.get(key) {
            if value.as_u64().is_none_or(|number| number > maximum) {
                return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} is outside its allowed range") }))).into_response());
            }
        }
    }
    for key in ["cancel_stale_orders", "dry_run"] {
        if object.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": format!("{key} must be boolean") }))).into_response());
        }
    }
    if let Some(value) = object.get("active_condition_ids") {
        let Some(items) = value.as_array() else {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": "active_condition_ids must be an array of strings" }))).into_response());
        };
        if items.len() > 10_000
            || items.iter().any(|item| {
                item.as_str()
                    .is_none_or(|text| text.is_empty() || text.len() > 256)
            })
        {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": "active_condition_ids contains an invalid item" }))).into_response());
        }
    }
    if let Some(value) = object.get("order_type") {
        if !matches!(value.as_str(), Some("limit" | "fok" | "gtc")) {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_config_field", "detail": "order_type must be limit, fok, or gtc" }))).into_response());
        }
    }
    Ok(object.clone())
}

/// `GET /api/tenant/polymarket/config`
pub async fn get_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "trading_data_plane_unavailable", "detail": "Polymarket configuration requires PostgreSQL" }))).into_response();
    };
    match read_module(db, auth.organization_id(), "polymarket").await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to read tenant Polymarket configuration");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "config_storage_error", "detail": "Polymarket configuration could not be loaded" }))).into_response()
        }
    }
}

/// `PUT /api/tenant/polymarket/config`
pub async fn update_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Polymarket,
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
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "trading_data_plane_unavailable", "detail": "Polymarket configuration requires PostgreSQL" }))).into_response();
    };
    match write_module(
        db,
        auth.organization_id(),
        "polymarket",
        &patch,
        &auth.ctx.actor_label(),
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to write tenant Polymarket configuration");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "config_storage_error", "detail": "Polymarket configuration could not be saved" }))).into_response()
        }
    }
}
