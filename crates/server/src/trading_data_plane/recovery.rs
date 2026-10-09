//! Tenant recovery + reporting handlers (PROMPT 3/10 #66/#67).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;

use super::executions::window;
use super::orders::plane_error;
use crate::api::ApiState;

/// `GET /api/tenant/recovery/intents` — the caller's orphaned
/// pre-broadcast intents (crash-recovery triage input).
pub async fn intents(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::ReconciliationRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let org = auth.organization_id();
    let scope = plane.read_scope(org);
    let cutoff = params
        .get("older_than_minutes")
        .and_then(|v| v.parse::<i64>().ok())
        .map(|m| chrono::Utc::now() - chrono::Duration::minutes(m.max(1)))
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::minutes(10));
    match plane
        .intents_read()
        .list_orphaned(&scope, cutoff, 200)
        .await
    {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "items": rows })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/recovery/sweep` — run the caller's recovery sweep
/// under the tenant advisory lock (orphaned intents + non-terminal
/// orders, one consistent snapshot).
pub async fn sweep(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::ReconciliationRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let plane = auth.plane.clone();
    let scope = plane.read_scope(org);
    let cutoff = params
        .get("older_than_minutes")
        .and_then(|v| v.parse::<i64>().ok())
        .map(|m| chrono::Utc::now() - chrono::Duration::minutes(m.max(1)))
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::minutes(10));
    match plane.recovery().sweep(&scope, cutoff).await {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({
                "organization_id": org.to_string(),
                "items": items
                    .into_iter()
                    .map(|i| json!({
                        "intent": i.intent,
                        "open_order_ids": i.open_order_ids,
                    }))
                    .collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/reports/summary` — the caller's trading summary
/// (counts, position book, execution stats, realized PnL — every
/// aggregate tenant-scoped in SQL).
pub async fn summary(State(state): State<ApiState>, headers: axum::http::HeaderMap) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::LedgerRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let scope = plane.read_scope(auth.organization_id());
    match plane.reporting().summary(&scope).await {
        Ok(summary) => (StatusCode::OK, Json(json!({ "summary": summary }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/reports/pnl?since=&until=` — the caller's
/// realized PnL over a window (default: today, UTC).
pub async fn pnl(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::LedgerRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let plane = auth.plane.clone();
    let scope = plane.read_scope(org);
    let report = plane.reporting().pnl();
    if params.contains_key("by_symbol") {
        let (since, until) = match window(&params) {
            Some(w) => w,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "invalid_window" })),
                )
                    .into_response()
            }
        };
        return match report.by_symbol(&scope, since, until, 50).await {
            Ok(rows) => (
                StatusCode::OK,
                Json(json!({ "organization_id": org.to_string(), "symbols": rows })),
            )
                .into_response(),
            Err(e) => plane_error(e),
        };
    }
    let realized = match window(&params) {
        Some((since, until)) => report.realized_between(&scope, since, until).await,
        None => report.realized_today(&scope).await,
    };
    match realized {
        Ok(pnl) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "pnl": pnl })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}
