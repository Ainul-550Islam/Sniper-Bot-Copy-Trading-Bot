//! Tenant external integrations and RPC provider status catalog.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;

use super::authorization_chain::{guard, TradingModuleFamily};
use crate::api::ApiState;

/// `GET /api/tenant/integrations`
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

    let items = vec![
        json!({
            "id": "int-helius-rpc",
            "name": "Helius Solana RPC & Geyser",
            "type": "solana_rpc",
            "status": "connected",
            "latency_ms": 38,
            "endpoint": "https://mainnet.helius-rpc.com",
            "rate_limit_rps": 100,
            "is_primary": true
        }),
        json!({
            "id": "int-quicknode-backup",
            "name": "QuickNode Dedicated Node",
            "type": "solana_rpc_backup",
            "status": "standby",
            "latency_ms": 45,
            "endpoint": "https://solana-mainnet.quiknode.pro",
            "rate_limit_rps": 50,
            "is_primary": false
        }),
        json!({
            "id": "int-jup-swap-api",
            "name": "Jupiter DEX Aggregator V6",
            "type": "dex_router",
            "status": "connected",
            "latency_ms": 22,
            "endpoint": "https://quote-api.jup.ag/v6",
            "rate_limit_rps": 60,
            "is_primary": true
        }),
        json!({
            "id": "int-polymarket-clob",
            "name": "Polymarket CLOB V2 Gateway",
            "type": "prediction_clob",
            "status": "connected",
            "latency_ms": 64,
            "endpoint": "https://clob.polymarket.com",
            "rate_limit_rps": 30,
            "is_primary": true
        }),
        json!({
            "id": "int-telegram-bot",
            "name": "Telegram Alert & Command Bot",
            "type": "messaging_channel",
            "status": "connected",
            "latency_ms": 82,
            "endpoint": "https://api.telegram.org",
            "rate_limit_rps": 20,
            "is_primary": true
        })
    ];

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len()
        })),
    )
        .into_response()
}
