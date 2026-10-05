//! Tenant-scoped backtesting orchestration and history service (SECOND.md §83).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use bot_core::backtest::{BacktestConfig, BacktestRecord, BacktestRunId, BacktestStatus};
use bot_core::membership::Permission;
use bot_core::strategy::StrategyId;
use bot_core::tenant::OrganizationId;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::backtest_service::BacktestService;
use crate::api::ApiState;

static STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, Vec<BacktestRecord>>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

#[derive(Debug, Deserialize)]
pub struct CreateBacktestBody {
    pub strategy_id: String,
    pub period_start: String,
    pub period_end: String,
    pub venue: String,
    pub initial_balance_usd: f64,
    pub fee_rate_bps: u32,
    pub slippage_bps: u32,
}

/// `GET /api/tenant/backtests`
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
    let mut lock = STORE.lock().unwrap();
    let records = lock.entry(org).or_insert_with(|| {
        let now = Utc::now();
        let sample_config = BacktestConfig {
            strategy_id: StrategyId::new(),
            period_start: now - chrono::Duration::days(30),
            period_end: now,
            venue: "raydium_v4".into(),
            initial_balance_usd_cents: 1_000_000,
            fee_rate_bps: 25,
            slippage_bps: 50,
        };
        vec![BacktestService::simulate(org, sample_config)]
    });

    let items: Vec<_> = records
        .iter()
        .map(|r| {
            let res = r.result.as_ref();
            json!({
                "id": r.id.to_string(),
                "organization_id": r.organization_id.to_string(),
                "strategy_id": r.config.strategy_id.to_string(),
                "strategy_name": "Dynamic Quantitative Backtest",
                "venue": r.config.venue,
                "period_start": r.config.period_start.to_rfc3339(),
                "period_end": r.config.period_end.to_rfc3339(),
                "initial_balance_usd": r.config.initial_balance_usd_cents as f64 / 100.0,
                "final_balance_usd": res.map(|x| x.final_balance_usd_cents as f64 / 100.0).unwrap_or(0.0),
                "net_pnl_usd": res.map(|x| x.net_pnl_usd_cents as f64 / 100.0).unwrap_or(0.0),
                "net_roi_pct": res.map(|x| x.net_roi_bps as f64 / 100.0).unwrap_or(0.0),
                "max_drawdown_pct": res.map(|x| x.max_drawdown_bps as f64 / 100.0).unwrap_or(0.0),
                "total_trades": res.map(|x| x.total_trades).unwrap_or(0),
                "win_rate_pct": res.map(|x| x.win_rate_bps as f64 / 100.0).unwrap_or(0.0),
                "sharpe_ratio": res.map(|x| x.sharpe_ratio_scaled as f64 / 100.0).unwrap_or(0.0),
                "fee_rate_bps": r.config.fee_rate_bps,
                "slippage_bps": r.config.slippage_bps,
                "status": r.status.as_str(),
                "created_at": r.created_at.to_rfc3339(),
                "completed_at": r.completed_at.map(|t| t.to_rfc3339()),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len(),
        })),
    )
        .into_response()
}

/// `GET /api/tenant/backtests/:id`
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

    let run_id = match BacktestRunId::parse(&id) {
        Some(bid) => bid,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_backtest_id" })),
            )
                .into_response()
        }
    };

    let org = auth.organization_id();
    let mut lock = STORE.lock().unwrap();
    let records = lock.entry(org).or_default();

    if let Some(r) = records.iter().find(|x| x.id == run_id) {
        let res = r.result.as_ref();
        return (
            StatusCode::OK,
            Json(json!({
                "id": r.id.to_string(),
                "organization_id": r.organization_id.to_string(),
                "strategy_id": r.config.strategy_id.to_string(),
                "strategy_name": "Quantitative Backtest Execution",
                "venue": r.config.venue,
                "period_start": r.config.period_start.to_rfc3339(),
                "period_end": r.config.period_end.to_rfc3339(),
                "initial_balance_usd": r.config.initial_balance_usd_cents as f64 / 100.0,
                "final_balance_usd": res.map(|x| x.final_balance_usd_cents as f64 / 100.0).unwrap_or(0.0),
                "net_pnl_usd": res.map(|x| x.net_pnl_usd_cents as f64 / 100.0).unwrap_or(0.0),
                "net_roi_pct": res.map(|x| x.net_roi_bps as f64 / 100.0).unwrap_or(0.0),
                "max_drawdown_pct": res.map(|x| x.max_drawdown_bps as f64 / 100.0).unwrap_or(0.0),
                "total_trades": res.map(|x| x.total_trades).unwrap_or(0),
                "win_rate_pct": res.map(|x| x.win_rate_bps as f64 / 100.0).unwrap_or(0.0),
                "sharpe_ratio": res.map(|x| x.sharpe_ratio_scaled as f64 / 100.0).unwrap_or(0.0),
                "fee_rate_bps": r.config.fee_rate_bps,
                "slippage_bps": r.config.slippage_bps,
                "status": r.status.as_str(),
                "created_at": r.created_at.to_rfc3339(),
                "completed_at": r.completed_at.map(|t| t.to_rfc3339()),
            })),
        )
            .into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "backtest_not_found" })),
    )
        .into_response()
}

/// `POST /api/tenant/backtests`
pub async fn create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateBacktestBody>,
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

    let strategy_id = match StrategyId::parse(&body.strategy_id) {
        Some(sid) => sid,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_strategy_id" })),
            )
                .into_response()
        }
    };

    let period_start = match DateTime::parse_from_rfc3339(&body.period_start) {
        Ok(t) => t.with_timezone(&Utc),
        Err(_) => Utc::now() - chrono::Duration::days(30),
    };

    let period_end = match DateTime::parse_from_rfc3339(&body.period_end) {
        Ok(t) => t.with_timezone(&Utc),
        Err(_) => Utc::now(),
    };

    let org = auth.organization_id();
    let initial_cents = (body.initial_balance_usd * 100.0) as u64;

    let config = BacktestConfig {
        strategy_id,
        period_start,
        period_end,
        venue: body.venue,
        initial_balance_usd_cents: initial_cents,
        fee_rate_bps: body.fee_rate_bps,
        slippage_bps: body.slippage_bps,
    };

    let record = BacktestService::simulate(org, config);
    let res = record.result.as_ref().unwrap();

    let mut lock = STORE.lock().unwrap();
    lock.entry(org).or_default().push(record.clone());

    (
        StatusCode::CREATED,
        Json(json!({
            "id": record.id.to_string(),
            "organization_id": record.organization_id.to_string(),
            "strategy_id": record.config.strategy_id.to_string(),
            "strategy_name": "Quantitative Backtest Execution",
            "venue": record.config.venue,
            "period_start": record.config.period_start.to_rfc3339(),
            "period_end": record.config.period_end.to_rfc3339(),
            "initial_balance_usd": body.initial_balance_usd,
            "final_balance_usd": res.final_balance_usd_cents as f64 / 100.0,
            "net_pnl_usd": res.net_pnl_usd_cents as f64 / 100.0,
            "net_roi_pct": res.net_roi_bps as f64 / 100.0,
            "max_drawdown_pct": res.max_drawdown_bps as f64 / 100.0,
            "total_trades": res.total_trades,
            "win_rate_pct": res.win_rate_bps as f64 / 100.0,
            "sharpe_ratio": res.sharpe_ratio_scaled as f64 / 100.0,
            "fee_rate_bps": record.config.fee_rate_bps,
            "slippage_bps": record.config.slippage_bps,
            "status": record.status.as_str(),
            "created_at": record.created_at.to_rfc3339(),
            "completed_at": record.completed_at.map(|t| t.to_rfc3339()),
        })),
    )
        .into_response()
}
