//! OpenAPI v3 component schemas and paths for Portfolio, Risk, Alerts, Status & Pricing (THIRD.md §145).

use serde_json::{json, Value};

/// Generates OpenAPI v3 specification components for product APIs.
pub fn product_schemas() -> Value {
    json!({
        "PortfolioSummary": {
            "type": "object",
            "required": ["organization_id", "total_equity_usd_cents", "available_cash_usd_cents", "allocated_margin_usd_cents", "unrealized_pnl_usd_cents", "realized_pnl_30d_usd_cents", "max_drawdown_bps", "exposures", "as_of", "is_stale"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "total_equity_usd_cents": { "type": "integer" },
                "available_cash_usd_cents": { "type": "integer" },
                "allocated_margin_usd_cents": { "type": "integer" },
                "unrealized_pnl_usd_cents": { "type": "integer" },
                "realized_pnl_30d_usd_cents": { "type": "integer" },
                "max_drawdown_bps": { "type": "integer" },
                "exposures": {
                    "type": "array",
                    "items": { "type": "object" }
                },
                "as_of": { "type": "string", "format": "date-time" },
                "is_stale": { "type": "boolean" }
            }
        },
        "RiskDashboardState": {
            "type": "object",
            "required": ["organization_id", "kill_switch_active", "modules", "durable", "reference_asset", "max_drawdown_limit_ref", "current_drawdown_ref", "daily_loss_limit_ref", "current_daily_loss_ref", "rules", "as_of"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "kill_switch_active": { "type": "boolean" },
                "modules": { "type": "array", "items": { "type": "object" } },
                "durable": { "type": "boolean" },
                "reference_asset": { "type": "string" },
                "max_drawdown_limit_ref": { "description": "Reference-unit limit or null; see rules." },
                "current_drawdown_ref": { "description": "Reference-unit limit or null; see rules." },
                "daily_loss_limit_ref": { "description": "Reference-unit limit or null; see rules." },
                "current_daily_loss_ref": { "description": "Reference-unit limit or null; see rules." },
                "rules": { "type": "array", "items": { "type": "object" } },
                "as_of": { "type": "string", "format": "date-time" }
            }
        },
        "AlertItem": {
            "type": "object",
            "required": ["id", "organization_id", "severity", "category", "title", "message", "is_acknowledged", "created_at"],
            "properties": {
                "id": { "type": "string" },
                "organization_id": { "type": "string", "format": "uuid" },
                "severity": { "type": "string", "enum": ["info", "warning", "critical", "emergency"] },
                "category": { "type": "string" },
                "title": { "type": "string" },
                "message": { "type": "string" },
                "is_acknowledged": { "type": "boolean" },
                "created_at": { "type": "string", "format": "date-time" }
            }
        }
    })
}

/// Generates OpenAPI v3 paths documentation for product endpoints.
pub fn product_paths() -> Value {
    json!({
    })
}
