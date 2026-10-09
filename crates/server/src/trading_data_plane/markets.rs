//! Tenant-safe market discovery surface.
//!
//! Served by the LIVE [`super::market_service::MarketService`] (GAP-MAP
//! P1): real provider feeds (Jupiter SOL/USD, Polymarket Gamma discovery)
//! behind a TTL cache. The fail-closed posture is kept: when no feed has
//! anything real to say, the endpoint answers `503
//! market_data_unavailable` — it never serves a static or fabricated
//! catalog.

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

fn unavailable(feeds: &[super::market_service::FeedStatus]) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "market_data_unavailable",
            "detail": "no live market-data feed returned data for this deployment",
            "feeds": feeds,
        })),
    )
        .into_response()
}

fn plane_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "trading_data_plane_unavailable",
            "detail": "market discovery requires an attached PostgreSQL database",
        })),
    )
        .into_response()
}

fn matches_query(
    ticker: &bot_core::market_data::MarketTicker,
    query: &MarketQuery,
) -> bool {
    if let Some(venue) = query.venue.as_deref() {
        let venue = venue.trim().to_ascii_lowercase();
        if !venue.is_empty() {
            let ticker_venue = format!("{:?}", ticker.venue).to_ascii_lowercase();
            if ticker_venue != venue {
                return false;
            }
        }
    }
    if let Some(q) = query.q.as_deref() {
        let q = q.trim().to_ascii_lowercase();
        if !q.is_empty()
            && !ticker.symbol.to_ascii_lowercase().contains(&q)
            && !ticker.name.to_ascii_lowercase().contains(&q)
            && !ticker.base_asset.to_ascii_lowercase().contains(&q)
        {
            return false;
        }
    }
    true
}

/// `GET /api/tenant/markets`
pub async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<MarketQuery>,
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
    let Some(trading) = state.trading.as_ref() else {
        return plane_unavailable();
    };

    let snapshot = trading.markets().snapshot().await;
    if snapshot.tickers.is_empty() {
        return unavailable(&snapshot.feeds);
    }
    let items: Vec<serde_json::Value> = snapshot
        .tickers
        .iter()
        .filter(|t| matches_query(t, &query))
        .map(|t| serde_json::to_value(t).unwrap_or_default())
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "items": items,
            "count": items.len(),
            "fetched_at": snapshot.fetched_at,
            "from_cache": snapshot.from_cache,
            // Feed health is part of the honest answer: a 200 with a
            // failing feed says so.
            "feeds": snapshot.feeds,
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
    let Some(trading) = state.trading.as_ref() else {
        return plane_unavailable();
    };

    let snapshot = trading.markets().snapshot().await;
    if snapshot.tickers.is_empty() {
        return unavailable(&snapshot.feeds);
    }
    match snapshot.tickers.iter().find(|t| t.id == id) {
        Some(ticker) => match serde_json::to_value(ticker) {
            Ok(value) => (StatusCode::OK, Json(value)).into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "market_serialization_error",
                    "detail": error.to_string(),
                })),
            )
                .into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "market_not_found",
                "detail": "no live feed currently reports this market id",
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::market_data::MarketTicker;
    use bot_core::models::{BotModule, Venue};
    use chrono::Utc;

    fn ticker(venue: Venue, symbol: &str, name: &str) -> MarketTicker {
        MarketTicker::new(
            format!("id-{symbol}"),
            symbol.into(),
            name.into(),
            venue,
            "BASE".into(),
            "QUOTE".into(),
            100,
            0,
            0,
            0,
            vec![BotModule::Sniper],
            Utc::now(),
        )
    }

    #[test]
    fn venue_and_text_filters_apply() {
        let sol = ticker(Venue::Jupiter, "SOL/USDC", "Solana");
        let poly = ticker(Venue::PolymarketClob, "BTC-100K", "Bitcoin above 100k");

        let q = MarketQuery {
            venue: Some("jupiter".into()),
            q: None,
        };
        assert!(matches_query(&sol, &q));
        assert!(!matches_query(&poly, &q));

        let q = MarketQuery {
            venue: None,
            q: Some("bitcoin".into()),
        };
        assert!(!matches_query(&sol, &q));
        assert!(matches_query(&poly, &q));

        let q = MarketQuery::default();
        assert!(matches_query(&sol, &q) && matches_query(&poly, &q));
    }

    #[test]
    fn filters_are_case_insensitive_and_trimmed() {
        let sol = ticker(Venue::Jupiter, "SOL/USDC", "Solana");
        let q = MarketQuery {
            venue: Some("  JUPITER ".into()),
            q: Some(" sol ".into()),
        };
        assert!(matches_query(&sol, &q));
    }
}
