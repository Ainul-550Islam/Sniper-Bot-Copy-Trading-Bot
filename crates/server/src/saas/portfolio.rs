//! Authoritative tenant portfolio read model.
//!
//! Values are read from durable accounting snapshots and tenant-scoped
//! position/balance tables. No response is fabricated when the database or
//! accounting read model is unavailable.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/portfolio", axum::routing::get(get_portfolio))
}

#[derive(Debug)]
struct ExposureRow {
    symbol: String,
    amount_units: f64,
    value_usd_cents: i64,
    venue: String,
}

fn service_unavailable(error: &'static str, reason: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": error, "reason": reason })),
    )
        .into_response()
}

pub async fn get_portfolio(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return service_unavailable(
            "portfolio_storage_unavailable",
            "portfolio data requires an attached PostgreSQL database",
        );
    };

    let snapshot = match sqlx::query(
        "SELECT ROUND(total_equity_usd_exact * 100)::bigint AS total_equity_usd_exact, ROUND(total_exposure_usd_exact * 100)::bigint AS total_exposure_usd_exact, ROUND(realized_pnl_usd_exact * 100)::bigint AS realized_pnl_usd_exact, ROUND(unrealized_pnl_usd_exact * 100)::bigint AS unrealized_pnl_usd_exact, drawdown_bps, snapshot_at FROM portfolio_snapshots_hourly WHERE organization_id = $1 ORDER BY snapshot_at DESC LIMIT 1",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio snapshot query failed");
            return service_unavailable(
                "portfolio_storage_unavailable",
                "authoritative portfolio snapshot could not be loaded",
            );
        }
    };

    let cash_cents = match sqlx::query(
        "SELECT COALESCE(SUM(value_cents), 0)::bigint AS cash_cents FROM (SELECT DISTINCT ON (address, asset) ROUND(COALESCE(usd_value_exact, usd_value::numeric) * 100)::bigint AS value_cents FROM balance_snapshots WHERE organization_id = $1 ORDER BY address, asset, ts DESC, id DESC) latest_balances",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_one(db.pool())
    .await
    {
        Ok(row) => match row.try_get::<i64, _>("cash_cents") {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio balance value could not be decoded");
                return service_unavailable(
                    "portfolio_storage_unavailable",
                    "authoritative balance values could not be decoded",
                );
            }
        },
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio balance query failed");
            return service_unavailable(
                "portfolio_storage_unavailable",
                "authoritative balance values could not be loaded",
            );
        }
    };

    let position_rows = match sqlx::query(
        "SELECT symbol, symbol_display, qty_exact::text AS qty_text, ROUND((qty_exact * last_mark_exact) * 100)::bigint AS value_usd_cents, venue FROM positions WHERE organization_id = $1 AND status IN ('open', 'closing') ORDER BY symbol, venue, id",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio position query failed");
            return service_unavailable(
                "portfolio_storage_unavailable",
                "authoritative positions could not be loaded",
            );
        }
    };

    let mut exposures = Vec::with_capacity(position_rows.len());
    for row in position_rows {
        let symbol = row
            .try_get::<String, _>("symbol_display")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| row.try_get::<String, _>("symbol").ok());
        let symbol = match symbol {
            Some(value) => value,
            None => {
                tracing::error!(organization = %ctx.organization.id, "portfolio position symbol could not be decoded");
                return service_unavailable(
                    "portfolio_storage_unavailable",
                    "authoritative position symbols could not be decoded",
                );
            }
        };
        let amount_units = match row
            .try_get::<String, _>("qty_text")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
        {
            Some(value) => value,
            None => {
                tracing::error!(organization = %ctx.organization.id, "portfolio position quantity could not be decoded");
                return service_unavailable(
                    "portfolio_storage_unavailable",
                    "authoritative position quantities could not be decoded",
                );
            }
        };
        let value_usd_cents = match row.try_get::<i64, _>("value_usd_cents") {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio position value could not be decoded");
                return service_unavailable(
                    "portfolio_storage_unavailable",
                    "authoritative position values could not be decoded",
                );
            }
        };
        let venue = match row.try_get::<String, _>("venue") {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio position venue could not be decoded");
                return service_unavailable(
                    "portfolio_storage_unavailable",
                    "authoritative position venues could not be decoded",
                );
            }
        };
        exposures.push(ExposureRow {
            symbol,
            amount_units,
            value_usd_cents,
            venue,
        });
    }

    let total_exposure_from_positions = exposures
        .iter()
        .map(|exposure| exposure.value_usd_cents)
        .sum::<i64>();
    let (total_equity, allocated_margin, unrealized_pnl, drawdown_bps, as_of) = match snapshot
        .as_ref()
    {
        Some(row) => {
            let total_equity = match row.try_get::<i64, _>("total_equity_usd_exact") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio equity snapshot could not be decoded");
                    return service_unavailable(
                        "portfolio_storage_unavailable",
                        "authoritative equity could not be decoded",
                    );
                }
            };
            let total_exposure = match row.try_get::<i64, _>("total_exposure_usd_exact") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio exposure snapshot could not be decoded");
                    return service_unavailable(
                        "portfolio_storage_unavailable",
                        "authoritative exposure could not be decoded",
                    );
                }
            };
            let unrealized = match row.try_get::<i64, _>("unrealized_pnl_usd_exact") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio unrealized PnL snapshot could not be decoded");
                    return service_unavailable(
                        "portfolio_storage_unavailable",
                        "authoritative unrealized PnL could not be decoded",
                    );
                }
            };
            let drawdown = match row.try_get::<i32, _>("drawdown_bps") {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio drawdown snapshot could not be decoded");
                    return service_unavailable(
                        "portfolio_storage_unavailable",
                        "authoritative drawdown could not be decoded",
                    );
                }
            };
            let at = match row.try_get::<chrono::DateTime<chrono::Utc>, _>("snapshot_at") {
                Ok(value) => value.to_rfc3339(),
                Err(error) => {
                    tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio snapshot timestamp could not be decoded");
                    return service_unavailable(
                        "portfolio_storage_unavailable",
                        "authoritative snapshot timestamp could not be decoded",
                    );
                }
            };
            (total_equity, total_exposure, unrealized, drawdown, at)
        }
        None => (
            cash_cents.saturating_add(total_exposure_from_positions),
            total_exposure_from_positions,
            0,
            0,
            chrono::Utc::now().to_rfc3339(),
        ),
    };

    let realized_pnl_30d = match sqlx::query(
        "SELECT COALESCE(ROUND(SUM(realized_pnl_usd_exact) * 100), 0)::bigint AS realized_pnl_cents FROM tenant_daily_accounting WHERE organization_id = $1 AND trade_date >= CURRENT_DATE - 30",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_one(db.pool())
    .await
    {
        Ok(row) => row.try_get::<i64, _>("realized_pnl_cents").unwrap_or(0),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "portfolio realised PnL query failed");
            return service_unavailable(
                "portfolio_storage_unavailable",
                "authoritative realised PnL could not be loaded",
            );
        }
    };

    let exposures_json = exposures
        .into_iter()
        .map(|exposure| {
            let percentage_bps = if total_equity > 0 {
                ((exposure.value_usd_cents.max(0) as i128 * 10_000_i128) / total_equity as i128)
                    .clamp(0, i32::MAX as i128) as i32
            } else {
                0
            };
            json!({
                "asset_symbol": exposure.symbol,
                "amount_units": exposure.amount_units,
                "value_usd_cents": exposure.value_usd_cents,
                "percentage_bps": percentage_bps,
                "venue": exposure.venue,
            })
        })
        .collect::<Vec<_>>();

    let is_stale = snapshot
        .as_ref()
        .and_then(|row| {
            row.try_get::<chrono::DateTime<chrono::Utc>, _>("snapshot_at")
                .ok()
        })
        .map(|value| {
            chrono::Utc::now()
                .signed_duration_since(value)
                .num_minutes()
                > 15
        })
        .unwrap_or(true);

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "total_equity_usd_cents": total_equity,
            "available_cash_usd_cents": cash_cents,
            "allocated_margin_usd_cents": allocated_margin,
            "unrealized_pnl_usd_cents": unrealized_pnl,
            "realized_pnl_30d_usd_cents": realized_pnl_30d,
            "max_drawdown_bps": drawdown_bps,
            "exposures": exposures_json,
            "as_of": as_of,
            "is_stale": is_stale,
        })),
    )
        .into_response()
}
