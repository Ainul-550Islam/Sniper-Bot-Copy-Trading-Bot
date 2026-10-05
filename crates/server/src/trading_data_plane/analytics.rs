//! Tenant trading analytics backed by authoritative PostgreSQL read models.
//!
//! No metric in this handler is a demo constant. Empty periods return zero
//! aggregates, while metrics for which the current schema has no authoritative
//! source return JSON null instead of an invented value.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use bot_core::membership::Permission;

use super::authorization_chain::{guard, TradingModuleFamily};
use crate::api::ApiState;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AnalyticsQuery {
    pub timeframe: Option<String>,
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

fn timeframe_window(
    value: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(&'static str, DateTime<Utc>), Response> {
    match value.unwrap_or("7d") {
        "24h" => Ok(("24h", now - Duration::hours(24))),
        "7d" => Ok(("7d", now - Duration::days(7))),
        "30d" => Ok(("30d", now - Duration::days(30))),
        "all" => Ok(("all", DateTime::<Utc>::from(std::time::UNIX_EPOCH))),
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_timeframe",
            "timeframe must be 24h, 7d, 30d, or all",
        )),
    }
}

/// `GET /api/tenant/analytics`.
pub async fn summary(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<AnalyticsQuery>,
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
    let db = match state.db.as_deref() {
        Some(value) => value,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "trading_data_plane_unavailable",
                "analytics requires an attached PostgreSQL database",
            )
        }
    };
    let now = Utc::now();
    let (timeframe, since) = match timeframe_window(query.timeframe.as_deref(), now) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let since_date = since.date_naive();

    let accounting = sqlx::query(
        "SELECT
             COALESCE(SUM(realized_pnl_usd_exact::double precision), 0.0) AS realized_pnl,
             COALESCE(SUM(unrealized_pnl_usd_exact::double precision), 0.0) AS unrealized_pnl,
             COALESCE(SUM(total_volume_usd_exact::double precision), 0.0) AS volume,
             COALESCE(SUM(total_fees_usd_exact::double precision), 0.0) AS fees,
             COALESCE(SUM(trades_count), 0)::bigint AS trades
           FROM tenant_daily_accounting
          WHERE organization_id = $1 AND trade_date >= $2",
    )
    .bind(org.as_uuid())
    .bind(since_date)
    .fetch_one(db.pool())
    .await;
    let accounting = match accounting {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant analytics accounting");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "analytics_storage_error",
                "accounting analytics could not be loaded",
            );
        }
    };

    let unrealized_from_positions = sqlx::query(
        "SELECT COALESCE(SUM((last_mark * qty) - cost_basis), 0.0) AS unrealized_pnl
           FROM positions
          WHERE organization_id = $1 AND status IN ('open', 'closing')",
    )
    .bind(org.as_uuid())
    .fetch_one(db.pool())
    .await;
    let unrealized_from_positions = match unrealized_from_positions {
        Ok(value) => value.get::<f64, _>("unrealized_pnl"),
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant unrealized PnL");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "analytics_storage_error",
                "position analytics could not be loaded",
            );
        }
    };

    let drawdown = sqlx::query(
        "SELECT MIN(drawdown_bps)::double precision AS drawdown_bps
           FROM portfolio_snapshots_hourly
          WHERE organization_id = $1 AND snapshot_at >= $2",
    )
    .bind(org.as_uuid())
    .bind(since)
    .fetch_one(db.pool())
    .await;
    let max_drawdown_pct = match drawdown {
        Ok(value) => value
            .try_get::<Option<f64>, _>("drawdown_bps")
            .ok()
            .flatten()
            .map(|bps| bps / 100.0),
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant drawdown analytics");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "analytics_storage_error",
                "drawdown analytics could not be loaded",
            );
        }
    };

    let execution = sqlx::query(
        "SELECT
             AVG(latency_ms)::double precision AS avg_fill_time_ms,
             AVG(CASE WHEN ok THEN 0.0 ELSE 1.0 END) * 100.0 AS failed_attempts_pct,
             COALESCE(SUM(CASE WHEN NOT ok THEN 1 ELSE 0 END), 0)::bigint AS failed_attempts
           FROM executions
          WHERE organization_id = $1 AND ts >= $2 AND kind IN ('send', 'confirm', 'simulate')",
    )
    .bind(org.as_uuid())
    .bind(since)
    .fetch_one(db.pool())
    .await;
    let execution = match execution {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant execution analytics");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "analytics_storage_error",
                "execution analytics could not be loaded",
            );
        }
    };

    let pnl_rows = sqlx::query(
        "SELECT trade_date,
                COALESCE(realized_pnl_usd_exact::double precision, 0.0) AS pnl_usd,
                SUM(COALESCE(realized_pnl_usd_exact::double precision, 0.0))
                  OVER (ORDER BY trade_date ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS cumulative_usd
           FROM tenant_daily_accounting
          WHERE organization_id = $1 AND trade_date >= $2
          ORDER BY trade_date ASC",
    )
    .bind(org.as_uuid())
    .bind(since_date)
    .fetch_all(db.pool())
    .await;
    let pnl_rows = match pnl_rows {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant PnL series");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "analytics_storage_error",
                "PnL series could not be loaded",
            );
        }
    };
    let pnl_series: Vec<_> = pnl_rows
        .into_iter()
        .map(|row| {
            let date: NaiveDate = row.get("trade_date");
            json!({
                "date": date.to_string(),
                "pnl_usd": row.get::<f64, _>("pnl_usd"),
                "cumulative_usd": row.get::<f64, _>("cumulative_usd"),
            })
        })
        .collect();

    let module_rows = sqlx::query(
        "SELECT source AS module,
                COUNT(*)::bigint AS trades_count,
                COALESCE(SUM(ABS(amount_in) + ABS(amount_out)), 0.0) AS volume_usd,
                COALESCE(SUM(fee), 0.0) AS fees_usd
           FROM trades
          WHERE organization_id = $1 AND ts >= $2
          GROUP BY source
          ORDER BY source ASC",
    )
    .bind(org.as_uuid())
    .bind(since)
    .fetch_all(db.pool())
    .await;
    let module_rows = match module_rows {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant module analytics");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "analytics_storage_error",
                "module analytics could not be loaded",
            );
        }
    };
    let module_breakdown: Vec<_> = module_rows
        .into_iter()
        .map(|row| {
            json!({
                "module": row.get::<String, _>("module"),
                "trades_count": row.get::<i64, _>("trades_count"),
                "volume_usd": row.get::<f64, _>("volume_usd"),
                "pnl_usd": Value::Null,
                "win_rate_pct": Value::Null,
                "fees_usd": row.get::<f64, _>("fees_usd"),
            })
        })
        .collect();

    let total_trades = accounting.get::<i64, _>("trades");
    let realized_pnl = accounting.get::<f64, _>("realized_pnl");
    let total_volume = accounting.get::<f64, _>("volume");
    let total_fees = accounting.get::<f64, _>("fees");
    let avg_fill_time_ms = execution
        .try_get::<Option<f64>, _>("avg_fill_time_ms")
        .ok()
        .flatten();
    let failed_attempts_pct = execution
        .try_get::<Option<f64>, _>("failed_attempts_pct")
        .ok()
        .flatten();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "timeframe": timeframe,
            "total_trades": total_trades,
            "win_rate_pct": Value::Null,
            "realized_pnl_usd": realized_pnl,
            "unrealized_pnl_usd": unrealized_from_positions,
            "total_volume_usd": total_volume,
            "total_fees_usd": total_fees,
            "sharpe_ratio": Value::Null,
            "max_drawdown_pct": max_drawdown_pct,
            "pnl_series": pnl_series,
            "module_breakdown": module_breakdown,
            "execution_quality": {
                "avg_fill_time_ms": avg_fill_time_ms,
                "avg_slippage_bps": Value::Null,
                "failed_attempts_pct": failed_attempts_pct,
                "reverted_txs": execution.get::<i64, _>("failed_attempts"),
            }
        })),
    )
        .into_response()
}
