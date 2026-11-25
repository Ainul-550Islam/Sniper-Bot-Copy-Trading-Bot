//! Tenant recovery + reporting handlers (PROMPT 3/10 #66/#67).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;

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
    // Paged, so the response states what it is NOT returning: `has_more` and
    // `total_remaining` (gap #74). A caller that stops early now knows it
    // stopped early, and `next_before`/`next_before_id` continue the walk.
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(bot_core::trading_repository::intent::RECOVERY_PAGE_MAX);
    let cursor = match (params.get("before"), params.get("before_id")) {
        (Some(at), Some(id)) => match chrono::DateTime::parse_from_rfc3339(at) {
            Ok(at) => Some((at.with_timezone(&chrono::Utc), id.clone())),
            Err(_) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({ "error": "before must be an RFC3339 timestamp" })),
                )
                    .into_response()
            }
        },
        (None, None) => None,
        // Half a keyset can silently re-serve or skip rows — refuse it.
        _ => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "before and before_id must be supplied together" })),
            )
                .into_response()
        }
    };
    match plane
        .recovery()
        .sweep_page(&scope, cutoff, cursor, limit)
        .await
    {
        Ok(page) => {
            let (next_before, next_before_id) = match page.next_cursor {
                Some((at, id)) => (json!(at), json!(id)),
                None => (json!(null), json!(null)),
            };
            (
                StatusCode::OK,
                Json(json!({
                    "organization_id": org.to_string(),
                    "items": page
                        .items
                        .into_iter()
                        .map(|i| json!({
                            "intent": i.intent,
                            "open_order_ids": i.open_order_ids,
                        }))
                        .collect::<Vec<_>>(),
                    // Open orders with no intent attribution: reported, never
                    // guessed onto an intent and never dropped.
                    "unattributed_open_order_ids": page.unattributed_open_order_ids,
                    "has_more": page.has_more,
                    "total_remaining": page.total_remaining,
                    "next_before": next_before,
                    "next_before_id": next_before_id,
                })),
            )
                .into_response()
        }
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
        let (since, until) = match super::executions::window_or_response(&params) {
            Ok(w) => w,
            Err(response) => return response,
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
    // No window supplied ⇒ the documented "today (UTC)" report. A window that
    // cannot be parsed is a 400, NOT a silent fall back to today.
    let realized = match optional_window(&params) {
        Ok(Some((since, until))) => report.realized_between(&scope, since, until).await,
        Ok(None) => report.realized_today(&scope).await,
        Err(response) => return response,
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

/// `Ok(Some(...))` when the caller asked for a window, `Ok(None)` when it asked
/// for none, `Err(400)` when the request is malformed or impossible. A missing
/// window keeps the documented "today (UTC)" default; a broken one is refused
/// instead of being silently replaced by that default.
fn optional_window(
    params: &std::collections::HashMap<String, String>,
) -> Result<Option<super::executions::Window>, axum::response::Response> {
    if !params.contains_key("since") && !params.contains_key("until") {
        return Ok(None);
    }
    super::executions::window_or_response(params).map(Some)
}
