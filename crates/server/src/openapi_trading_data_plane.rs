//! OpenAPI v3 component schemas and paths for Trading Data Plane (SECOND.md §90).
//!
//! Exposes typed specifications for Strategies, Backtests, and Markets discovery endpoints.

use serde_json::{json, Value};

/// Generates OpenAPI v3 specification components for trading data plane.
pub fn trading_data_plane_schemas() -> Value {
    json!({
        "StrategyRecord": {
            "type": "object",
            "required": ["id", "organization_id", "module_family", "name", "status", "version", "parameters", "created_at", "updated_at"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "organization_id": { "type": "string", "format": "uuid" },
                "module_family": { "type": "string", "enum": ["sniper", "copy", "polymarket"] },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "status": { "type": "string", "enum": ["draft", "active", "paused", "archived"] },
                "version": { "type": "integer" },
                "parameters": { "type": "object" },
                "created_at": { "type": "string", "format": "date-time" },
                "updated_at": { "type": "string", "format": "date-time" }
            }
        },
        "BacktestRecord": {
            "type": "object",
            "required": ["id", "organization_id", "strategy_id", "strategy_name", "venue", "period_start", "period_end", "initial_balance_usd", "status", "created_at"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "organization_id": { "type": "string", "format": "uuid" },
                "strategy_id": { "type": "string", "format": "uuid" },
                "strategy_name": { "type": "string" },
                "venue": { "type": "string" },
                "period_start": { "type": "string", "format": "date-time" },
                "period_end": { "type": "string", "format": "date-time" },
                "initial_balance_usd": { "type": "number" },
                "final_balance_usd": { "type": "number" },
                "net_pnl_usd": { "type": "number" },
                "net_roi_pct": { "type": "number" },
                "max_drawdown_pct": { "type": "number" },
                "total_trades": { "type": "integer" },
                "win_rate_pct": { "type": "number" },
                "sharpe_ratio": { "type": "number" },
                "status": { "type": "string", "enum": ["pending", "running", "completed", "failed", "cancelled"] },
                "created_at": { "type": "string", "format": "date-time" },
                "completed_at": { "type": ["string", "null"], "format": "date-time" }
            }
        },
        "MarketTicker": {
            "type": "object",
            "required": ["id", "symbol", "name", "venue", "base_asset", "quote_asset", "price_usd", "change_24h_pct", "volume_24h_usd", "liquidity_usd", "is_active", "compatible_modules"],
            "properties": {
                "id": { "type": "string" },
                "symbol": { "type": "string" },
                "name": { "type": "string" },
                "venue": { "type": "string" },
                "base_asset": { "type": "string" },
                "quote_asset": { "type": "string" },
                "price_usd": { "type": "number" },
                "change_24h_pct": { "type": "number" },
                "volume_24h_usd": { "type": "number" },
                "liquidity_usd": { "type": "number" },
                "is_active": { "type": "boolean" },
                "compatible_modules": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        }
    })
}

/// Generates OpenAPI v3 paths documentation for trading data plane endpoints.
pub fn trading_data_plane_paths() -> Value {
    json!({
        "/api/tenant/strategies": {
            "get": {
                "operationId": "tenant.listStrategies",
                "summary": "List tenant strategies",
                "tags": ["Strategies"],
                "parameters": [
                    { "name": "module", "in": "query", "schema": { "type": "string" } }
                ],
                "responses": {
                    "200": { "description": "Strategy list returned", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/StrategyRecord" } } } } },
                    "401": { "description": "Unauthorized" },
                    "403": { "description": "Forbidden" }
                }
            },
            "post": {
                "operationId": "tenant.createStrategy",
                "summary": "Create strategy",
                "tags": ["Strategies"],
                "responses": {
                    "201": { "description": "Strategy created", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/StrategyRecord" } } } },
                    "400": { "description": "Validation error" },
                    "401": { "description": "Unauthorized" },
                    "403": { "description": "Forbidden" }
                }
            }
        },
        "/api/tenant/backtests": {
            "get": {
                "operationId": "tenant.listBacktests",
                "summary": "List backtests",
                "tags": ["Backtesting"],
                "responses": {
                    "200": { "description": "Backtest list returned", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/BacktestRecord" } } } } }
                }
            },
            "post": {
                "operationId": "tenant.createBacktest",
                "summary": "Queue backtest execution",
                "tags": ["Backtesting"],
                "responses": {
                    "201": { "description": "Backtest queued", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/BacktestRecord" } } } }
                }
            }
        },
        "/api/tenant/markets": {
            "get": {
                "operationId": "tenant.listMarkets",
                "summary": "List discovered markets",
                "tags": ["Market Data"],
                "responses": {
                    "200": { "description": "Markets list returned", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/MarketTicker" } } } } }
                }
            }
        }
    })
}
