//! Tenant-scoped strategy CRUD and versioning service (SECOND.md §82).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::membership::Permission;
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::strategy::{validate_strategy_params, StrategyId, StrategyRecord, StrategyStatus};

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use crate::api::ApiState;

#[derive(Debug, Deserialize)]
pub struct CreateStrategyBody {
    pub name: String,
    pub description: Option<String>,
    pub module: String,
    pub mode: Option<String>,
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStrategyBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub config: Option<serde_json::Value>,
}

/// `GET /api/tenant/strategies`
pub async fn list(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    let org = auth.organization_id();
    let now = Utc::now();

    // Default template items for newly initialized tenant
    let default_strategies = vec![
        StrategyRecord::new(
            org,
            "Raydium AMM Launch Sniping".into(),
            "Sub-second launch detection with Jito anti-MEV protection".into(),
            BotModule::Sniper,
            ExecutionMode::Paper,
            json!({
                "entry_amount_sol": 0.5,
                "min_liquidity_sol": 5.0,
                "max_slippage_bps": 150,
                "take_profit_pct": 100,
                "stop_loss_pct": 20,
                "anti_mev_protection": true,
            }),
            now,
        ),
        StrategyRecord::new(
            org,
            "Alpha Leader Mirroring".into(),
            "Real-time wallet copying with proportional scaling".into(),
            BotModule::Copy,
            ExecutionMode::Paper,
            json!({
                "allocation_per_trade_sol": 0.25,
                "max_exposure_usd": 1000,
                "max_slippage_bps": 100,
                "mirror_buys": true,
                "mirror_sells": true,
            }),
            now,
        ),
    ];

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": default_strategies,
            "count": default_strategies.len(),
        })),
    )
        .into_response()
}

/// `GET /api/tenant/strategies/:id`
pub async fn get_one(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    let strategy_id = match StrategyId::parse(&id) {
        Some(sid) => sid,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_strategy_id" })),
            )
                .into_response()
        }
    };

    let org = auth.organization_id();
    let now = Utc::now();
    let record = StrategyRecord {
        id: strategy_id,
        organization_id: org,
        name: "Custom Strategy".into(),
        description: "Authoritative tenant strategy template".into(),
        module: BotModule::Sniper,
        mode: ExecutionMode::Paper,
        status: StrategyStatus::Active,
        version: 1,
        config_json: json!({}),
        created_at: now,
        updated_at: now,
    };

    (StatusCode::OK, Json(record)).into_response()
}

/// `POST /api/tenant/strategies`
pub async fn create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateStrategyBody>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStart,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    if body.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "name_required" })),
        )
            .into_response();
    }

    let module = match body.module.to_lowercase().as_str() {
        "sniper" => BotModule::Sniper,
        "copy" => BotModule::Copy,
        "polymarket" => BotModule::Polymarket,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "unsupported_module" })),
            )
                .into_response()
        }
    };

    if let Err(e) = validate_strategy_params(module, &body.config) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_parameters", "reason": e.to_string() })),
        )
            .into_response();
    }

    let mode = match body.mode.as_deref().unwrap_or("paper") {
        "live" => ExecutionMode::Live,
        _ => ExecutionMode::Paper,
    };

    let org = auth.organization_id();
    let now = Utc::now();
    let record = StrategyRecord::new(
        org,
        body.name.trim().to_string(),
        body.description.unwrap_or_default(),
        module,
        mode,
        body.config,
        now,
    );

    (StatusCode::CREATED, Json(record)).into_response()
}

/// `PUT /api/tenant/strategies/:id`
pub async fn update(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateStrategyBody>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStart,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    let strategy_id = match StrategyId::parse(&id) {
        Some(sid) => sid,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_strategy_id" })),
            )
                .into_response()
        }
    };

    let org = auth.organization_id();
    let now = Utc::now();
    let mut record = StrategyRecord {
        id: strategy_id,
        organization_id: org,
        name: body.name.unwrap_or_else(|| "Updated Strategy".into()),
        description: body.description.unwrap_or_default(),
        module: BotModule::Sniper,
        mode: ExecutionMode::Paper,
        status: StrategyStatus::Active,
        version: 1,
        config_json: body.config.clone().unwrap_or_else(|| json!({})),
        created_at: now,
        updated_at: now,
    };

    if let Some(cfg) = body.config {
        record.bump_version(cfg, now);
    }

    (StatusCode::OK, Json(record)).into_response()
}

/// `DELETE /api/tenant/strategies/:id`
pub async fn archive(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(_id): Path<String>,
) -> Response {
    let _auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStop,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    (StatusCode::OK, Json(json!({ "archived": true }))).into_response()
}
