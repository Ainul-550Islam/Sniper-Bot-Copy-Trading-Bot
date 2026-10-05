//! Tenant trading data plane (PROMPT 3/10 #60 consolidated by #68).
//!
//! The authenticated, tenant-scoped trading HTTP surface. Every
//! handler:
//!
//! 1. authenticates through [`crate::saas::middleware::authorize_request`]
//!    (no handler may run without a [`SaasContext`]);
//! 2. builds the tenant scopes from the context's organization;
//! 3. calls ONLY the `bot-core` `trading_repository` repositories —
//!    every SQL predicate carries `organization_id` there;
//! 4. answers `503` when the database (and therefore the plane) is
//!    not attached.

pub mod analytics;
pub mod authorization_chain;
pub mod backtests;
pub mod bots;
pub mod config_store;
pub mod copy;
pub mod executions;
pub mod integrations;
pub mod markets;
pub mod module_control_store;
pub mod module_controls;
pub mod onboarding;
pub mod orders;
pub mod polymarket;
pub mod positions;
pub mod recovery;
pub mod service;
pub mod sniper;
pub mod strategies;
pub mod telegram;

use axum::routing::{get, post, put};
use axum::Router;

pub use service::TenantTradingDataPlane;

use crate::api::ApiState;

/// The tenant trading data-plane routes.
pub fn routes() -> Router<ApiState> {
    Router::new()
        // --- bots (§H customer surface) ------------------------------------
        .route("/api/tenant/bots", get(bots::list))
        .route("/api/tenant/bots/:module", get(bots::detail))
        // --- orders -------------------------------------------------------
        .route("/api/tenant/orders", get(orders::list))
        .route("/api/tenant/orders/:id", get(orders::get_one))
        .route("/api/tenant/orders/:id/cancel", post(orders::cancel))
        // --- executions / transactions ------------------------------------
        .route("/api/tenant/executions", get(executions::list))
        .route("/api/tenant/executions/:id", get(executions::get_one))
        .route(
            "/api/tenant/transactions/:signature",
            get(executions::transaction),
        )
        // --- positions / trades / balances --------------------------------
        .route("/api/tenant/positions", get(positions::list))
        .route("/api/tenant/positions/:id", get(positions::get_one))
        .route("/api/tenant/trades", get(positions::trades))
        .route(
            "/api/tenant/trades/:position_id",
            get(positions::trades_for_position),
        )
        .route("/api/tenant/balances", get(positions::balances))
        // --- module controls/status/config ---------------------------------
        .route("/api/tenant/sniper/status", get(sniper::status))
        .route("/api/tenant/sniper/controls", post(sniper::controls))
        .route(
            "/api/tenant/sniper/config",
            get(sniper::get_config).put(sniper::update_config),
        )
        .route("/api/tenant/copy/status", get(copy::status))
        .route("/api/tenant/copy/controls", post(copy::controls))
        .route(
            "/api/tenant/copy/config",
            get(copy::get_config).put(copy::update_config),
        )
        .route("/api/tenant/polymarket/status", get(polymarket::status))
        .route(
            "/api/tenant/polymarket/controls",
            post(polymarket::controls),
        )
        .route(
            "/api/tenant/polymarket/config",
            get(polymarket::get_config).put(polymarket::update_config),
        )
        .route("/api/tenant/telegram/status", get(telegram::status))
        .route(
            "/api/tenant/telegram/binding",
            put(telegram::bind).delete(telegram::unbind),
        )
        .route("/api/tenant/copy/leaders", get(copy::leaders))
        .route("/api/tenant/copy/leaders/:address", get(copy::leader))
        .route("/api/tenant/copy/links", get(copy::open_links))
        // --- polymarket ----------------------------------------------------
        .route("/api/tenant/polymarket/orders", get(polymarket::orders))
        .route(
            "/api/tenant/polymarket/orders/:venue_order_id",
            get(polymarket::order),
        )
        .route("/api/tenant/polymarket/fills", get(polymarket::fills))
        .route(
            "/api/tenant/polymarket/reconciliation",
            get(polymarket::reconciliation),
        )
        // --- recovery ------------------------------------------------------
        .route("/api/tenant/recovery/intents", get(recovery::intents))
        .route("/api/tenant/recovery/sweep", get(recovery::sweep))
        // --- reporting -----------------------------------------------------
        .route("/api/tenant/reports/summary", get(recovery::summary))
        .route("/api/tenant/reports/pnl", get(recovery::pnl))
        // --- strategies, backtests & markets ------------------------------
        .route(
            "/api/tenant/strategies",
            get(strategies::list).post(strategies::create),
        )
        .route(
            "/api/tenant/strategies/:id",
            get(strategies::get_one)
                .put(strategies::update)
                .delete(strategies::archive),
        )
        .route(
            "/api/tenant/backtests",
            get(backtests::list).post(backtests::create),
        )
        .route("/api/tenant/backtests/:id", get(backtests::get_one))
        .route("/api/tenant/markets", get(markets::list))
        .route("/api/tenant/markets/:id", get(markets::get_one))
        // --- analytics, integrations & onboarding --------------------------
        .route("/api/tenant/analytics", get(analytics::summary))
        .route("/api/tenant/integrations", get(integrations::list))
        .route("/api/tenant/onboarding", get(onboarding::get_state))
        .route(
            "/api/tenant/onboarding/complete",
            post(onboarding::complete_step),
        )
}
