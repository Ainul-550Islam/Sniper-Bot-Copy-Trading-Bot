//! Authoritative product plan catalogue and commercial tiers (THIRD.md §138).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;

use crate::api::ApiState;

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/pricing", axum::routing::get(get_pricing_catalog))
}

pub async fn get_pricing_catalog(State(_state): State<ApiState>) -> Response {
    let plans = vec![
        json!({
            "code": "starter",
            "name": "Starter Quant",
            "price_monthly_usd_cents": 19900, // $199.00 / month
            "price_yearly_usd_cents": 199000, // $1,990.00 / year
            "description": "Essential automated execution for solo algorithmic traders.",
            "features": [
                "1 Active Solana Sniper Bot",
                "Raydium & Pump.fun execution",
                "Shared Jito MEV Gateway",
                "Up to $10,000 monthly volume",
                "Standard Email Support"
            ],
            "limits": {
                "max_bots": 1,
                "max_strategies": 5,
                "monthly_volume_usd": 10000,
                "team_members": 1
            }
        }),
        json!({
            "code": "pro",
            "name": "Professional Fund",
            "price_monthly_usd_cents": 59900, // $599.00 / month
            "price_yearly_usd_cents": 599000, // $5,990.00 / year
            "description": "High-throughput execution with multi-wallet copy trading and Polymarket CLOB.",
            "is_popular": true,
            "features": [
                "5 Active Trading Bots (Sniper + Copy Trading)",
                "Polymarket Binary Market Making",
                "Dedicated Yellowstone Geyser Stream",
                "Up to $100,000 monthly volume",
                "FIPS 140-3 AWS KMS Custody Profile",
                "5 Team Member Seats",
                "Priority 1-hour SLA Support"
            ],
            "limits": {
                "max_bots": 5,
                "max_strategies": 25,
                "monthly_volume_usd": 100000,
                "team_members": 5
            }
        }),
        json!({
            "code": "enterprise",
            "name": "Institutional Asset",
            "price_monthly_usd_cents": 199900, // $1,999.00 / month
            "price_yearly_usd_cents": 1999000, // $19,990.00 / year
            "description": "Unlimited execution scale, custom Jito tip configurations, and SOC2 compliance ledgers.",
            "features": [
                "Unlimited Active Bots & Strategies",
                "Custom Colocated Jito RPC Relay",
                "Direct Memory Yellowstone gRPC",
                "Unlimited Volume Allowance",
                "Dedicated VPC & Custom KMS HSM Signers",
                "Unlimited Team Seats & RBAC",
                "Dedicated Account Executive & 24/7 SLA"
            ],
            "limits": {
                "max_bots": 9999,
                "max_strategies": 9999,
                "monthly_volume_usd": -1,
                "team_members": 9999
            }
        }),
    ];

    (
        StatusCode::OK,
        Json(json!({
            "plans": plans,
            "currency": "USD"
        })),
    )
        .into_response()
}
