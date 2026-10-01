//! Tenant position / trade / balance handlers (PROMPT 3/10 #63).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;

use super::executions::window;
use super::orders::{page_request, plane_error};
use crate::api::ApiState;

/// `GET /api/tenant/positions?limit=&cursor=` — the caller's position
/// book (keyset-paginated, newest update first).
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
    let page = match page_request(org, &params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let scope = plane.read_scope(org);
    match plane.positions_read().list_page(&scope, &page).await {
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

/// `GET /api/tenant/positions/:id` — one of the caller's positions.
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
    match plane.positions_read().get(&scope, &id).await {
        Ok(position) => (StatusCode::OK, Json(json!({ "position": position }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/trades?limit=&cursor=` — the caller's fill
/// history (keyset-paginated, newest first).
pub async fn trades(
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
    let page = match page_request(org, &params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let scope = plane.read_scope(org);
    match plane.trades().list_page(&scope, &page).await {
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

/// `GET /api/tenant/trades/:position_id` — the caller's fills for one
/// of their positions.
pub async fn trades_for_position(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(position_id): Path<String>,
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
    match plane.trades().list_for_position(&scope, &position_id).await {
        Ok(rows) => (StatusCode::OK, Json(json!({ "items": rows }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/balances` — the caller's latest snapshot per
/// (address, asset) plus the USD aggregate.
pub async fn balances(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::WalletRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let org = auth.organization_id();
    let scope = plane.read_scope(org);
    let latest = match plane.balances().latest_per_asset(&scope).await {
        Ok(rows) => rows,
        Err(e) => return plane_error(e),
    };
    if let Some(address) = params.get("address") {
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
        return match plane
            .balances()
            .history_for_address(&scope, address, since, until)
            .await
        {
            Ok(rows) => (StatusCode::OK, Json(json!({ "items": rows }))).into_response(),
            Err(e) => plane_error(e),
        };
    }
    let total_usd = match plane.balances().total_usd_latest(&scope).await {
        Ok(v) => v,
        Err(e) => return plane_error(e),
    };
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "latest": latest,
            "total_usd": total_usd,
        })),
    )
        .into_response()
}
