//! Tenant polymarket handlers (PROMPT 3/10 #65).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::membership::Permission;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::executions::window;
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use super::orders::{page_request, plane_error};
use crate::api::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymarketConfigPayload {
    pub organization_id: String,
    pub active_condition_ids: Vec<String>,
    pub max_position_size_usdc: f64,
    pub max_market_exposure_usdc: f64,
    pub spread_threshold_bps: u32,
    pub reprice_interval_seconds: u32,
    pub cancel_stale_orders: bool,
    pub dry_run: bool,
    pub order_type: String,
    pub updated_at: String,
}

static POLYMARKET_CONFIG_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, PolymarketConfigPayload>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

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
        Ok(a) => a,
        Err(response) => return response,
    };

    let org = auth.organization_id();
    let mut lock = POLYMARKET_CONFIG_STORE.lock().unwrap();
    let cfg = lock.entry(org).or_insert_with(|| PolymarketConfigPayload {
        organization_id: org.to_string(),
        active_condition_ids: vec![],
        max_position_size_usdc: 500.0,
        max_market_exposure_usdc: 2500.0,
        spread_threshold_bps: 50,
        reprice_interval_seconds: 5,
        cancel_stale_orders: true,
        dry_run: true,
        order_type: "limit".into(),
        updated_at: Utc::now().to_rfc3339(),
    });

    (StatusCode::OK, Json(cfg.clone())).into_response()
}

/// `PUT /api/tenant/polymarket/config`
pub async fn update_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Polymarket,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };

    let org = auth.organization_id();
    let mut lock = POLYMARKET_CONFIG_STORE.lock().unwrap();
    let current = lock.entry(org).or_insert_with(|| PolymarketConfigPayload {
        organization_id: org.to_string(),
        active_condition_ids: vec![],
        max_position_size_usdc: 500.0,
        max_market_exposure_usdc: 2500.0,
        spread_threshold_bps: 50,
        reprice_interval_seconds: 5,
        cancel_stale_orders: true,
        dry_run: true,
        order_type: "limit".into(),
        updated_at: Utc::now().to_rfc3339(),
    });

    if let Some(v) = body.get("max_position_size_usdc").and_then(|x| x.as_f64()) {
        current.max_position_size_usdc = v;
    }
    if let Some(v) = body.get("max_market_exposure_usdc").and_then(|x| x.as_f64()) {
        current.max_market_exposure_usdc = v;
    }
    if let Some(v) = body.get("spread_threshold_bps").and_then(|x| x.as_u64()) {
        current.spread_threshold_bps = v as u32;
    }
    if let Some(v) = body.get("reprice_interval_seconds").and_then(|x| x.as_u64()) {
        current.reprice_interval_seconds = v as u32;
    }
    if let Some(v) = body.get("cancel_stale_orders").and_then(|x| x.as_bool()) {
        current.cancel_stale_orders = v;
    }
    if let Some(v) = body.get("dry_run").and_then(|x| x.as_bool()) {
        current.dry_run = v;
    }
    if let Some(v) = body.get("order_type").and_then(|x| x.as_str()) {
        current.order_type = v.to_string();
    }
    if let Some(v) = body.get("active_condition_ids").and_then(|x| x.as_array()) {
        current.active_condition_ids = v
            .iter()
            .filter_map(|s| s.as_str().map(|str_val| str_val.to_string()))
            .collect();
    }
    current.updated_at = Utc::now().to_rfc3339();

    (StatusCode::OK, Json(current.clone())).into_response()
}
