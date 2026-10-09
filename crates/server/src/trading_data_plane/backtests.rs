//! Durable tenant-scoped backtest job and history API.
//!
//! A request is persisted as `queued` in PostgreSQL. The API never fabricates
//! historical prices, fills, PnL, Sharpe, or drawdown values. Result metrics
//! are returned only when a trusted backtest worker has written `result_json`
//! for the same tenant-owned run.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use bot_core::membership::Permission;
use bot_core::strategy::StrategyId;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use crate::api::ApiState;

const MAX_INITIAL_BALANCE_CENTS: f64 = 10_000_000_000_000.0;
const MAX_PERIOD: Duration = Duration::days(3650);
const MAX_VENUE_LENGTH: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBacktestBody {
    pub strategy_id: String,
    pub period_start: String,
    pub period_end: String,
    pub venue: String,
    pub initial_balance_usd: f64,
    pub fee_rate_bps: u32,
    pub slippage_bps: u32,
}

fn error_response(status: StatusCode, error: &'static str, reason: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": error,
            "reason": reason.into(),
        })),
    )
        .into_response()
}

fn database(state: &ApiState) -> Result<&bot_core::db::Database, Response> {
    state.db.as_deref().ok_or_else(|| {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "backtest storage requires an attached PostgreSQL database",
        )
    })
}

fn parse_period(value: &str, field: &'static str) -> Result<DateTime<Utc>, Response> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_backtest_period",
                format!("{field} must be RFC3339"),
            )
        })
}

fn initial_balance_cents(value: f64) -> Result<i64, Response> {
    if !value.is_finite() || value <= 0.0 || value > MAX_INITIAL_BALANCE_CENTS {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_initial_balance",
            "initial balance must be finite, positive, and within the supported range",
        ));
    }
    let cents = (value * 100.0).round();
    if cents < 1.0 || cents > i64::MAX as f64 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_initial_balance",
            "initial balance is outside the supported range",
        ));
    }
    Ok(cents as i64)
}

fn validate_inputs(
    body: &CreateBacktestBody,
) -> Result<(Uuid, DateTime<Utc>, DateTime<Utc>, i64), Response> {
    let strategy_id = StrategyId::parse(&body.strategy_id)
        .map(|value| *value.as_uuid())
        .ok_or_else(|| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_strategy_id",
                "strategy id must be a UUID",
            )
        })?;
    let period_start = parse_period(&body.period_start, "period_start")?;
    let period_end = parse_period(&body.period_end, "period_end")?;
    if period_start >= period_end {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_backtest_period",
            "period_start must be before period_end",
        ));
    }
    if period_end - period_start > MAX_PERIOD {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_backtest_period",
            "backtest period is too long",
        ));
    }
    if body.venue.trim().is_empty() || body.venue.trim().len() > MAX_VENUE_LENGTH {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_backtest_venue",
            "venue is required and must be short",
        ));
    }
    if body.fee_rate_bps > 10_000 || body.slippage_bps > 10_000 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_backtest_costs",
            "fee and slippage must not exceed 10000 bps",
        ));
    }
    Ok((
        strategy_id,
        period_start,
        period_end,
        initial_balance_cents(body.initial_balance_usd)?,
    ))
}

fn metric(result: Option<&Value>, key: &str) -> Value {
    result
        .and_then(|value| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

fn run_json(row: &sqlx::postgres::PgRow) -> Value {
    let result = row
        .try_get::<Option<Value>, _>("result_json")
        .ok()
        .flatten();
    let initial_cents: i64 = row.get("initial_balance_usd_cents");
    json!({
        "id": row.get::<Uuid, _>("id"),
        "organization_id": row.get::<Uuid, _>("organization_id"),
        "strategy_id": row.get::<Uuid, _>("strategy_id"),
        "strategy_name": row.get::<String, _>("strategy_name"),
        "module": row.get::<String, _>("module"),
        "venue": row.get::<String, _>("venue"),
        "period_start": row.get::<DateTime<Utc>, _>("period_start"),
        "period_end": row.get::<DateTime<Utc>, _>("period_end"),
        "initial_balance_usd": initial_cents as f64 / 100.0,
        "final_balance_usd": metric(result.as_ref(), "final_balance_usd"),
        "net_pnl_usd": metric(result.as_ref(), "net_pnl_usd"),
        "net_roi_pct": metric(result.as_ref(), "net_roi_pct"),
        "max_drawdown_pct": metric(result.as_ref(), "max_drawdown_pct"),
        "total_trades": metric(result.as_ref(), "total_trades"),
        "win_rate_pct": metric(result.as_ref(), "win_rate_pct"),
        "sharpe_ratio": metric(result.as_ref(), "sharpe_ratio"),
        "fee_rate_bps": row.get::<i32, _>("fee_rate_bps"),
        "slippage_bps": row.get::<i32, _>("slippage_bps"),
        "status": row.get::<String, _>("status"),
        "error": row.try_get::<Option<String>, _>("error").ok().flatten(),
        "created_at": row.get::<DateTime<Utc>, _>("created_at"),
        "completed_at": row.try_get::<Option<DateTime<Utc>>, _>("completed_at").ok().flatten(),
    })
}

const SELECT_RUN: &str = "SELECT b.id, b.organization_id, b.strategy_id, s.name AS strategy_name,
                                  s.module, b.venue, b.period_start, b.period_end,
                                  b.initial_balance_usd_cents, b.fee_rate_bps,
                                  b.slippage_bps, b.status, b.result_json, b.error,
                                  b.created_at, b.completed_at
                             FROM backtest_runs b
                             JOIN tenant_strategies s ON s.id = b.strategy_id
                            WHERE b.organization_id = $1";

/// `GET /api/tenant/backtests`.
pub async fn list(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = format!("{SELECT_RUN} ORDER BY b.created_at DESC, b.id DESC");
    let rows = match sqlx::query(&query)
        .bind(auth.organization_id().as_uuid())
        .fetch_all(db.pool())
        .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to list tenant backtests");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "backtest_storage_error",
                "backtests could not be loaded",
            );
        }
    };
    let items: Vec<Value> = rows.iter().map(run_json).collect();
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": auth.organization_id().to_string(),
            "items": items,
            "count": items.len(),
        })),
    )
        .into_response()
}

/// `GET /api/tenant/backtests/:id`.
pub async fn get_one(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let run_id = match Uuid::parse_str(&id) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_backtest_id",
                "backtest id must be a UUID",
            )
        }
    };
    let query = format!("{SELECT_RUN} AND b.id = $2");
    let row = match sqlx::query(&query)
        .bind(auth.organization_id().as_uuid())
        .bind(run_id)
        .fetch_optional(db.pool())
        .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant backtest");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "backtest_storage_error",
                "backtest could not be loaded",
            );
        }
    };
    match row {
        Some(value) => (StatusCode::OK, Json(run_json(&value))).into_response(),
        None => error_response(
            StatusCode::NOT_FOUND,
            "backtest_not_found",
            "backtest was not found",
        ),
    }
}

/// `POST /api/tenant/backtests`.
pub async fn create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateBacktestBody>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotStart,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (strategy_id, period_start, period_end, initial_cents) = match validate_inputs(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let strategy_exists = sqlx::query(
        "SELECT 1 FROM tenant_strategies WHERE id = $1 AND organization_id = $2 AND status <> 'archived'",
    )
    .bind(strategy_id)
    .bind(auth.organization_id().as_uuid())
    .fetch_optional(db.pool())
    .await;
    match strategy_exists {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "strategy_not_found",
                "strategy does not belong to this organization",
            )
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to verify strategy ownership for backtest");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "backtest_storage_error",
                "strategy ownership could not be verified",
            );
        }
    }

    let run_id = Uuid::new_v4();
    let now = Utc::now();
    let insert = sqlx::query(
        "INSERT INTO backtest_runs
             (id, organization_id, strategy_id, venue, period_start, period_end,
              initial_balance_usd_cents, fee_rate_bps, slippage_bps, status, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'queued', $10)",
    )
    .bind(run_id)
    .bind(auth.organization_id().as_uuid())
    .bind(strategy_id)
    .bind(body.venue.trim())
    .bind(period_start)
    .bind(period_end)
    .bind(initial_cents)
    .bind(body.fee_rate_bps as i32)
    .bind(body.slippage_bps as i32)
    .bind(now)
    .execute(db.pool())
    .await;
    if let Err(error) = insert {
        tracing::error!(error = %error, "failed to queue tenant backtest");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "backtest_storage_error",
            "backtest could not be queued",
        );
    }

    let query = format!("{SELECT_RUN} AND b.id = $2");
    let row = match sqlx::query(&query)
        .bind(auth.organization_id().as_uuid())
        .bind(run_id)
        .fetch_one(db.pool())
        .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to reload queued tenant backtest");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "backtest_storage_error",
                "queued backtest could not be loaded",
            );
        }
    };

    (StatusCode::CREATED, Json(run_json(&row))).into_response()
}
