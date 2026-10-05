//! Tenant copy-trading handlers (PROMPT 3/10 #64).

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
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use super::orders::{plane_error, unavailable};
use crate::api::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyConfigPayload {
    pub organization_id: String,
    pub max_exposure_usd: f64,
    pub allocation_per_trade_sol: f64,
    pub max_slippage_bps: u32,
    pub mirror_buys: bool,
    pub mirror_sells: bool,
    pub stale_event_timeout_seconds: u32,
    pub allowed_tokens: Vec<String>,
    pub blocked_tokens: Vec<String>,
    pub dry_run: bool,
    pub copy_ratio_pct: f64,
    pub updated_at: String,
}

static COPY_CONFIG_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, CopyConfigPayload>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

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
        Ok(a) => a,
        Err(response) => return response,
    };

    let org = auth.organization_id();
    let mut lock = COPY_CONFIG_STORE.lock().unwrap();
    let cfg = lock.entry(org).or_insert_with(|| CopyConfigPayload {
        organization_id: org.to_string(),
        max_exposure_usd: 1000.0,
        allocation_per_trade_sol: 0.25,
        max_slippage_bps: 100,
        mirror_buys: true,
        mirror_sells: true,
        stale_event_timeout_seconds: 15,
        allowed_tokens: vec![],
        blocked_tokens: vec![],
        dry_run: true,
        copy_ratio_pct: 100.0,
        updated_at: Utc::now().to_rfc3339(),
    });

    (StatusCode::OK, Json(cfg.clone())).into_response()
}

/// `PUT /api/tenant/copy/config`
pub async fn update_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Copy,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };

    let org = auth.organization_id();
    let mut lock = COPY_CONFIG_STORE.lock().unwrap();
    let current = lock.entry(org).or_insert_with(|| CopyConfigPayload {
        organization_id: org.to_string(),
        max_exposure_usd: 1000.0,
        allocation_per_trade_sol: 0.25,
        max_slippage_bps: 100,
        mirror_buys: true,
        mirror_sells: true,
        stale_event_timeout_seconds: 15,
        allowed_tokens: vec![],
        blocked_tokens: vec![],
        dry_run: true,
        copy_ratio_pct: 100.0,
        updated_at: Utc::now().to_rfc3339(),
    });

    if let Some(v) = body.get("max_exposure_usd").and_then(|x| x.as_f64()) {
        current.max_exposure_usd = v;
    }
    if let Some(v) = body.get("allocation_per_trade_sol").and_then(|x| x.as_f64()) {
        current.allocation_per_trade_sol = v;
    }
    if let Some(v) = body.get("max_slippage_bps").and_then(|x| x.as_u64()) {
        current.max_slippage_bps = v as u32;
    }
    if let Some(v) = body.get("mirror_buys").and_then(|x| x.as_bool()) {
        current.mirror_buys = v;
    }
    if let Some(v) = body.get("mirror_sells").and_then(|x| x.as_bool()) {
        current.mirror_sells = v;
    }
    if let Some(v) = body.get("stale_event_timeout_seconds").and_then(|x| x.as_u64()) {
        current.stale_event_timeout_seconds = v as u32;
    }
    if let Some(v) = body.get("dry_run").and_then(|x| x.as_bool()) {
        current.dry_run = v;
    }
    if let Some(v) = body.get("copy_ratio_pct").and_then(|x| x.as_f64()) {
        current.copy_ratio_pct = v;
    }
    if let Some(v) = body.get("allowed_tokens").and_then(|x| x.as_array()) {
        current.allowed_tokens = v
            .iter()
            .filter_map(|s| s.as_str().map(|str_val| str_val.to_string()))
            .collect();
    }
    if let Some(v) = body.get("blocked_tokens").and_then(|x| x.as_array()) {
        current.blocked_tokens = v
            .iter()
            .filter_map(|s| s.as_str().map(|str_val| str_val.to_string()))
            .collect();
    }
    current.updated_at = Utc::now().to_rfc3339();

    (StatusCode::OK, Json(current.clone())).into_response()
}
