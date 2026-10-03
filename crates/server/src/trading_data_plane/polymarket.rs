//! Tenant polymarket handlers (PROMPT 3/10 #65).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;
use bot_core::models::BotModule;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::executions::window;
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use super::orders::{page_request, plane_error};
use crate::api::ApiState;

/// `GET /api/tenant/polymarket/orders?limit=&cursor=` — the caller's
/// mirror book (keyset-paginated, newest submission first).
pub async fn orders(
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

/// `GET /api/tenant/polymarket/orders/:venue_order_id` — the caller's
/// mirror row for that venue order, with its fills.
pub async fn order(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(venue_order_id): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
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

/// `GET /api/tenant/polymarket/fills?since=&until=` — the caller's
/// booked fills in a window.
pub async fn fills(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
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

/// `GET /api/tenant/polymarket/reconciliation` — the caller's mirror
/// drift (operator triage, tenant-scoped).
pub async fn reconciliation(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(_params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
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

/// `GET /api/tenant/polymarket/status` — the caller's polymarket module
/// state: entitlement (verified by the chain), tenant override, runtime
/// phase.
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

/// `POST /api/tenant/polymarket/controls` — tenant-level enable/disable
/// for the caller's OWN organization (runtime lifecycle stays
/// runtime-owned).
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
