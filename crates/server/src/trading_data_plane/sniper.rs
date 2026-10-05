//! Tenant Sniper controls, configuration and status (§J, spec file 74).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::State;
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
use crate::api::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SniperConfigPayload {
    pub organization_id: String,
    pub min_liquidity_sol: f64,
    pub max_slippage_bps: u32,
    pub anti_mev_protection: bool,
    pub priority_fee_lamports: u64,
    pub entry_amount_sol: f64,
    pub take_profit_pct: f64,
    pub stop_loss_pct: f64,
    pub trailing_stop_pct: f64,
    pub auto_sell_timeout_seconds: u32,
    pub dry_run: bool,
    pub blacklisted_tokens: Vec<String>,
    pub dex_routing: String,
    pub updated_at: String,
}

static CONFIG_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, SniperConfigPayload>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

/// `GET /api/tenant/sniper/status`
pub async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
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
                BotModule::Sniper,
                feature_key_for(BotModule::Sniper),
            )
            .await,
        ),
    )
        .into_response()
}

/// `POST /api/tenant/sniper/controls`
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
    let auth = match guard_manage(&state, &headers, permission, TradingModuleFamily::Sniper).await {
        Ok(a) => a,
        Err(response) => return response,
    };
    apply_control(&state, &auth, BotModule::Sniper, action).await
}

/// `GET /api/tenant/sniper/config`
pub async fn get_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };

    let org = auth.organization_id();
    let mut lock = CONFIG_STORE.lock().unwrap();
    let cfg = lock.entry(org).or_insert_with(|| SniperConfigPayload {
        organization_id: org.to_string(),
        min_liquidity_sol: 5.0,
        max_slippage_bps: 150,
        anti_mev_protection: true,
        priority_fee_lamports: 500_000,
        entry_amount_sol: 0.5,
        take_profit_pct: 100.0,
        stop_loss_pct: 20.0,
        trailing_stop_pct: 10.0,
        auto_sell_timeout_seconds: 300,
        dry_run: true,
        blacklisted_tokens: vec![],
        dex_routing: "auto".into(),
        updated_at: Utc::now().to_rfc3339(),
    });

    (StatusCode::OK, Json(cfg.clone())).into_response()
}

/// `PUT /api/tenant/sniper/config`
pub async fn update_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };

    let org = auth.organization_id();
    let mut lock = CONFIG_STORE.lock().unwrap();
    let current = lock.entry(org).or_insert_with(|| SniperConfigPayload {
        organization_id: org.to_string(),
        min_liquidity_sol: 5.0,
        max_slippage_bps: 150,
        anti_mev_protection: true,
        priority_fee_lamports: 500_000,
        entry_amount_sol: 0.5,
        take_profit_pct: 100.0,
        stop_loss_pct: 20.0,
        trailing_stop_pct: 10.0,
        auto_sell_timeout_seconds: 300,
        dry_run: true,
        blacklisted_tokens: vec![],
        dex_routing: "auto".into(),
        updated_at: Utc::now().to_rfc3339(),
    });

    if let Some(v) = body.get("min_liquidity_sol").and_then(|x| x.as_f64()) {
        current.min_liquidity_sol = v;
    }
    if let Some(v) = body.get("max_slippage_bps").and_then(|x| x.as_u64()) {
        current.max_slippage_bps = v as u32;
    }
    if let Some(v) = body.get("anti_mev_protection").and_then(|x| x.as_bool()) {
        current.anti_mev_protection = v;
    }
    if let Some(v) = body.get("priority_fee_lamports").and_then(|x| x.as_u64()) {
        current.priority_fee_lamports = v;
    }
    if let Some(v) = body.get("entry_amount_sol").and_then(|x| x.as_f64()) {
        current.entry_amount_sol = v;
    }
    if let Some(v) = body.get("take_profit_pct").and_then(|x| x.as_f64()) {
        current.take_profit_pct = v;
    }
    if let Some(v) = body.get("stop_loss_pct").and_then(|x| x.as_f64()) {
        current.stop_loss_pct = v;
    }
    if let Some(v) = body.get("trailing_stop_pct").and_then(|x| x.as_f64()) {
        current.trailing_stop_pct = v;
    }
    if let Some(v) = body.get("auto_sell_timeout_seconds").and_then(|x| x.as_u64()) {
        current.auto_sell_timeout_seconds = v as u32;
    }
    if let Some(v) = body.get("dry_run").and_then(|x| x.as_bool()) {
        current.dry_run = v;
    }
    if let Some(v) = body.get("blacklisted_tokens").and_then(|x| x.as_array()) {
        current.blacklisted_tokens = v
            .iter()
            .filter_map(|s| s.as_str().map(|str_val| str_val.to_string()))
            .collect();
    }
    if let Some(v) = body.get("dex_routing").and_then(|x| x.as_str()) {
        current.dex_routing = v.to_string();
    }
    current.updated_at = Utc::now().to_rfc3339();

    (StatusCode::OK, Json(current.clone())).into_response()
}
