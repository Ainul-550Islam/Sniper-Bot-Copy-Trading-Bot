//! Tenant-safe market discovery surface.
//!
//! The current schema does not contain an authoritative market/catalog read
//! model and no provider client is attached to this handler. The endpoint
//! therefore fails closed instead of returning stale or fabricated prices.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use bot_core::membership::Permission;

use super::authorization_chain::{guard, TradingModuleFamily};
use crate::api::ApiState;

#[derive(Debug, Deserialize, Default)]
pub struct MarketQuery {
    pub venue: Option<String>,
    pub q: Option<String>,
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "market_data_unavailable",
            "detail": "no authoritative market catalog or live market-data provider is attached to this deployment",
        })),
    )
        .into_response()
}

/// `GET /api/tenant/markets`
pub async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(_query): Query<MarketQuery>,
) -> Response {
    if let Err(response) = guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        return response;
    }
    unavailable()
}

/// `GET /api/tenant/markets/:id`
pub async fn get_one(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(_id): Path<String>,
) -> Response {
    if let Err(response) = guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        return response;
    }
    unavailable()
}
