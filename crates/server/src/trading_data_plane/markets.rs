//! Tenant-safe market discovery and screener API (SECOND.md §84).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use bot_core::membership::Permission;

use super::authorization_chain::{guard, TradingModuleFamily};
use super::market_service::MarketService;
use crate::api::ApiState;

#[derive(Debug, Deserialize, Default)]
pub struct MarketQuery {
    pub venue: Option<String>,
    pub q: Option<String>,
}

/// `GET /api/tenant/markets`
pub async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<MarketQuery>,
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

    let org = auth.organization_id();
    let all_markets = MarketService::get_available_markets();

    let filtered: Vec<_> = all_markets
        .into_iter()
        .filter(|m| {
            if let Some(ref v) = query.venue {
                if !m.venue.eq_ignore_ascii_case(v) {
                    return false;
                }
            }
            if let Some(ref q) = query.q {
                let q_lower = q.to_lowercase();
                if !m.symbol.to_lowercase().contains(&q_lower)
                    && !m.name.to_lowercase().contains(&q_lower)
                {
                    return false;
                }
            }
            true
        })
        .map(|m| {
            json!({
                "id": m.id,
                "symbol": m.symbol,
                "name": m.name,
                "venue": m.venue,
                "base_asset": m.base_asset,
                "quote_asset": m.quote_asset,
                "price_usd": m.price_usd_cents as f64 / 100.0,
                "change_24h_pct": m.change_24h_bps as f64 / 100.0,
                "volume_24h_usd": m.volume_24h_usd_cents as f64 / 100.0,
                "liquidity_usd": m.liquidity_usd_cents as f64 / 100.0,
                "is_active": m.is_active,
                "compatible_modules": m.compatible_modules,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": filtered,
            "count": filtered.len(),
        })),
    )
        .into_response()
}

/// `GET /api/tenant/markets/:id`
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

    let org = auth.organization_id();
    let all_markets = MarketService::get_available_markets();

    if let Some(m) = all_markets.into_iter().find(|m| m.id == id || m.symbol.eq_ignore_ascii_case(&id)) {
        return (
            StatusCode::OK,
            Json(json!({
                "id": m.id,
                "organization_id": org.to_string(),
                "symbol": m.symbol,
                "name": m.name,
                "venue": m.venue,
                "base_asset": m.base_asset,
                "quote_asset": m.quote_asset,
                "price_usd": m.price_usd_cents as f64 / 100.0,
                "change_24h_pct": m.change_24h_bps as f64 / 100.0,
                "volume_24h_usd": m.volume_24h_usd_cents as f64 / 100.0,
                "liquidity_usd": m.liquidity_usd_cents as f64 / 100.0,
                "is_active": m.is_active,
                "compatible_modules": m.compatible_modules,
            })),
        )
            .into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "market_not_found" })),
    )
        .into_response()
}
