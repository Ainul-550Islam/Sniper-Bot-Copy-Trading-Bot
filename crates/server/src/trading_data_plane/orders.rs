//! Tenant order handlers (PROMPT 3/10 #61).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::HashMap;

use bot_core::membership::Permission;
use bot_core::trading_repository::not_found::ResourceKind;
use bot_core::trading_repository::pagination::TenantPageRequest;

use crate::api::ApiState;

/// `503` when the data plane is not attached (no database).
pub(super) fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "trading_data_plane_unavailable" })),
    )
        .into_response()
}

/// Shared page-request parsing: `?limit=&cursor=`.
///
/// The `Err` carries the ready-made HTTP 400 (axum's `Response` is a
/// large composite) — the deliberate early-return-error pattern of
/// these handlers.
#[allow(clippy::result_large_err)]
pub(super) fn page_request(
    organization_id: bot_core::tenant::OrganizationId,
    params: &HashMap<String, String>,
) -> Result<TenantPageRequest, Response> {
    let limit = params.get("limit").and_then(|v| v.parse::<u32>().ok());
    let cursor = params.get("cursor").map(|s| s.as_str());
    TenantPageRequest::new(organization_id, limit, cursor).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_pagination" })),
        )
            .into_response()
    })
}

/// `GET /api/tenant/orders?limit=&cursor=` — the caller's orders.
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
    let result = plane.orders_read().list_page(&scope, &page).await;
    match result {
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

/// `GET /api/tenant/orders/:id` — one of the caller's orders.
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
    match plane.orders_read().get(&scope, &id).await {
        Ok(order) => (StatusCode::OK, Json(json!({ "order": order }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `POST /api/tenant/orders/:id/cancel` — cancel one of the caller's
/// non-terminal orders (OrderManage; the tenant guard is inside the
/// repository's SELECT … FOR UPDATE).
pub async fn cancel(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain at manage level
    // (authenticate → org from credential → plane → lifecycle →
    // entitlement → module family).
    let auth = match super::authorization_chain::guard_manage(
        &state,
        &headers,
        Permission::OrderManage,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
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
    let write = match plane.write_scope(auth.organization_id(), &auth.ctx.actor_label()) {
        Ok(w) => w,
        Err(_) => return unavailable(),
    };
    match plane
        .orders_write()
        .cancel(&write, &id, Some("tenant_api"), chrono::Utc::now())
        .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "cancelled": id, "kind": ResourceKind::Order.as_str() })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// Map a [`RepositoryError`] onto the HTTP contract. A tenant miss is
/// a 404 WITHOUT revealing whether the id exists for another tenant.
pub(super) fn plane_error(
    e: bot_core::trading_repository::repository_error::RepositoryError,
) -> Response {
    use bot_core::trading_repository::repository_error::RepositoryError as E;
    match e {
        E::NotFound(kind) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "kind": kind })),
        )
            .into_response(),
        E::TenantMismatch => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "tenant_mismatch" })),
        )
            .into_response(),
        E::StaleWrite(what) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "stale_write", "what": what })),
        )
            .into_response(),
        E::Validation(what) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "validation", "what": what })),
        )
            .into_response(),
        E::Conflict(what) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "conflict", "what": what })),
        )
            .into_response(),
        E::Storage(detail) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "storage", "detail": detail })),
        )
            .into_response(),
    }
}
