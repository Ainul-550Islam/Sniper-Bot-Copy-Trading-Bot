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
            "required": ["organization_id", "kill_switch_active", "max_drawdown_limit_bps", "current_drawdown_bps", "daily_loss_limit_usd_cents", "current_daily_loss_cents", "rules", "as_of"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "kill_switch_active": { "type": "boolean" },
                "max_drawdown_limit_bps": { "type": "integer" },
                "current_drawdown_bps": { "type": "integer" },
                "daily_loss_limit_usd_cents": { "type": "integer" },
                "current_daily_loss_cents": { "type": "integer" },
                "rules": {
                    "type": "array",
                    "items": { "type": "object" }
                },
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
        "/api/saas/portfolio": {
            "get": {
                "summary": "Get authoritative tenant portfolio summary and exposures",
                "tags": ["Portfolio"],
                "responses": {
                    "200": { "description": "Portfolio summary returned" }
                }
            }
        },
        "/api/saas/risk-dashboard": {
            "get": {
                "summary": "Get risk limits and utilization dashboard",
                "tags": ["Risk Management"],
                "responses": {
                    "200": { "description": "Risk posture returned" }
                }
            }
        },
        "/api/saas/risk-dashboard/kill-switch": {
            "post": {
                "summary": "Toggle tenant emergency kill switch",
                "tags": ["Risk Management"],
                "responses": {
                    "200": { "description": "Kill switch state toggled" }
                }
            }
        },
        "/api/saas/alerts": {
            "get": {
                "summary": "List tenant alerts and notifications",
                "tags": ["Alerts"],
                "responses": {
                    "200": { "description": "Alerts list returned" }
                }
            }
        },
        "/api/saas/status": {
            "get": {
                "summary": "Get customer-safe infrastructure status",
                "tags": ["Status"],
                "responses": {
                    "200": { "description": "Platform status returned" }
                }
            }
        },
        "/api/saas/pricing": {
            "get": {
                "summary": "Get product plan catalog",
                "tags": ["Commercial"],
                "responses": {
                    "200": { "description": "Pricing catalog returned" }
                }
            }
        },
        "/api/saas/activity": {
            "get": {
                "summary": "Get unified tenant activity timeline",
                "tags": ["Activity"],
                "responses": {
                    "200": { "description": "Activity timeline returned" }
                }
            }
        },
        "/api/saas/support/tickets": {
            "get": {
                "summary": "List tenant support tickets",
                "tags": ["Support"],
                "responses": {
                    "200": { "description": "Support tickets returned" }
                }
            },
            "post": {
                "summary": "Create a tenant support ticket",
                "tags": ["Support"],
                "responses": {
                    "201": { "description": "Support ticket created" }
                }
            }
        },
        "/api/saas/notifications/preferences": {
            "get": {
                "summary": "Read tenant notification preferences",
                "tags": ["Notifications"],
                "responses": {
                    "200": { "description": "Notification preferences returned" }
                }
            },
            "put": {
                "summary": "Update tenant notification preferences",
                "tags": ["Notifications"],
                "responses": {
                    "200": { "description": "Notification preferences updated" }
                }
            }
        }
    })
}
