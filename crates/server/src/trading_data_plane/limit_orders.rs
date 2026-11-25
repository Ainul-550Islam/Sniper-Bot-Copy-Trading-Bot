//! Tenant limit-order API (remediation tree, Part 2).
//!
//! Routes (every one: authorize → org-scope → DB → audit → test):
//! * `POST /api/tenant/limit-orders`            — create an ACTIVE order (OrderManage);
//! * `GET  /api/tenant/limit-orders`            — list this org's orders (OrderRead),
//!   filterable by `status`, keyset-paged by `before` (created_at, exclusive);
//! * `POST /api/tenant/limit-orders/:id/cancel` — cancel an ACTIVE order (OrderManage).
//!
//! Rules:
//! * records INTENT only — no funds move in these handlers; execution happens in
//!   `limit_order_worker`;
//! * every query carries `organization_id`, taken from the authenticated context,
//!   never from the request body;
//! * validation reuses the module's own `LimitOrder::validate`, so the API and the
//!   trigger math cannot disagree about what is legal.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use module_sniper::limit_orders::{LimitOrder, OrderSide, OrderStatus, TriggerKind};

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

/// Default and maximum page size for the list endpoint.
pub const DEFAULT_LIST_LIMIT: i64 = 50;
pub const MAX_LIST_LIMIT: i64 = 200;
/// Longest allowed expiry (one year). Longer horizons are good-til-cancelled.
pub const MAX_EXPIRY_SECS: i64 = 365 * 24 * 60 * 60;
/// Upper bound on prices and sizes; keeps every value inside NUMERIC(24,9).
pub const MAX_NUMERIC_VALUE: f64 = 1.0e12;

/// Request body for creating an order.
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub mint: String,
    /// `"buy"` or `"sell"`.
    pub side: String,
    /// `"at_or_below"` or `"at_or_above"`.
    pub trigger: String,
    pub price_sol: f64,
    pub amount: f64,
    /// Optional lifetime in seconds; absent means good-til-cancelled.
    #[serde(default)]
    pub expires_in_secs: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    /// Exclusive upper bound on `created_at` (RFC 3339) for keyset paging.
    #[serde(default)]
    pub before: Option<DateTime<Utc>>,
    /// Tie-breaker for [`ListQuery::before`]: the `id` of the last order on
    /// the previous page. Together they form the full `(created_at, id)`
    /// cursor, so rows sharing a timestamp are never skipped. Required
    /// alongside `before` for a lossless page boundary.
    #[serde(default)]
    pub before_id: Option<String>,
}

/// Public view of one order. Never exposes lease internals.
#[derive(Debug, Serialize)]
pub struct OrderView {
    pub id: String,
    pub mint: String,
    pub side: String,
    pub trigger: String,
    pub price_sol: f64,
    pub amount: f64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub last_error: Option<String>,
}

/// Parse the API side string. Unknown values are rejected, never defaulted.
pub fn parse_side(raw: &str) -> Result<OrderSide, &'static str> {
    match raw {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        _ => Err("side must be \"buy\" or \"sell\""),
    }
}

/// Parse the API trigger string. Unknown values are rejected, never defaulted.
pub fn parse_trigger(raw: &str) -> Result<TriggerKind, &'static str> {
    match raw {
        "at_or_below" => Ok(TriggerKind::AtOrBelow),
        "at_or_above" => Ok(TriggerKind::AtOrAbove),
        _ => Err("trigger must be \"at_or_below\" or \"at_or_above\""),
    }
}

/// Storage form of a trigger (the schema's CHECK values).
pub fn trigger_str(trigger: TriggerKind) -> &'static str {
    match trigger {
        TriggerKind::AtOrBelow => "at_or_below",
        TriggerKind::AtOrAbove => "at_or_above",
    }
}

/// Status strings accepted by the list filter.
pub fn parse_status(raw: &str) -> Result<&'static str, &'static str> {
    match raw {
        "active" => Ok("active"),
        "triggered" => Ok("triggered"),
        "cancelled" => Ok("cancelled"),
        "expired" => Ok("expired"),
        _ => Err("status must be one of active, triggered, cancelled, expired"),
    }
}

/// Base58 mint check without a decoder dependency: 32–44 characters from the
/// base58 alphabet (no `0`, `O`, `I`, `l`). Structural only; the chain is the
/// authority on whether the mint exists.
pub fn is_plausible_mint(mint: &str) -> bool {
    let len = mint.len();
    (32..=44).contains(&len)
        && mint
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l'))
}

fn validation_error(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": "limit_order_invalid", "reason": reason })),
    )
        .into_response()
}

fn db_unavailable(what: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "limit_order_storage_unavailable",
            "reason": format!("{what} requires an attached PostgreSQL database"),
        })),
    )
        .into_response()
}

fn storage_error(what: &str, error: &sqlx::Error) -> Response {
    tracing::error!(error = %error, "{what} failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "limit_order_storage_unavailable",
            "reason": format!("{what} could not be completed"),
        })),
    )
        .into_response()
}

/// `POST /api/tenant/limit-orders`
pub async fn create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::OrderManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("creating a limit order");
    };

    let side = match parse_side(&body.side) {
        Ok(s) => s,
        Err(reason) => return validation_error(reason),
    };
    let trigger = match parse_trigger(&body.trigger) {
        Ok(t) => t,
        Err(reason) => return validation_error(reason),
    };
    if !is_plausible_mint(&body.mint) {
        return validation_error("mint must be a base58 Solana address");
    }
    if !body.price_sol.is_finite() || body.price_sol <= 0.0 || body.price_sol > MAX_NUMERIC_VALUE {
        return validation_error("price_sol must be a positive finite number within range");
    }
    if !body.amount.is_finite() || body.amount <= 0.0 || body.amount > MAX_NUMERIC_VALUE {
        return validation_error("amount must be a positive finite number within range");
    }

    let now = Utc::now();
    let expires_at = match body.expires_in_secs {
        None => None,
        Some(secs) if (1..=MAX_EXPIRY_SECS).contains(&secs) => Some(now + Duration::seconds(secs)),
        Some(_) => return validation_error("expires_in_secs must be between 1 and one year"),
    };

    let id = format!("lo_{}", uuid::Uuid::new_v4().simple());
    let candidate = LimitOrder {
        id: id.clone(),
        organization_id: ctx.organization.id.to_string(),
        mint: body.mint.clone(),
        side,
        trigger,
        price_sol: body.price_sol,
        amount: body.amount,
        status: OrderStatus::Active,
        created_at: now,
        expires_at,
    };
    if let Err(error) = candidate.validate() {
        return validation_error(&error.to_string());
    }

    let inserted = sqlx::query(
        "INSERT INTO limit_orders \
         (id, organization_id, mint, side, trigger_kind, price_sol, amount, status, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6::float8, $7::float8, 'active', $8, $9)",
    )
    .bind(&id)
    .bind(ctx.organization.id.as_uuid())
    .bind(&body.mint)
    .bind(side.as_str())
    .bind(trigger_str(trigger))
    .bind(body.price_sol)
    .bind(body.amount)
    .bind(now)
    .bind(expires_at)
    .execute(db.pool())
    .await;
    if let Err(error) = inserted {
        return storage_error("creating a limit order", &error);
    }

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.limit_order.created",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "organization_id": ctx.organization.id.to_string(),
            "status": "active",
        })),
    )
        .into_response()
}

/// ONE page of this organization's limit orders.
///
/// The keyset predicate lives here rather than inline in the handler so the
/// real-Postgres suite exercises the production SQL instead of a copy of it:
/// page order is `(created_at DESC, id DESC)`, so the cursor must be the FULL
/// pair. `before` alone compares `created_at` only and loses every row that
/// shares the boundary timestamp.
pub async fn fetch_page(
    db: &bot_core::db::Database,
    organization_id: uuid::Uuid,
    status: Option<&str>,
    cursor: Option<(DateTime<Utc>, Option<String>)>,
    limit: i64,
) -> Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> {
    let before_ts = cursor.as_ref().map(|(ts, _)| *ts);
    let before_id = cursor.as_ref().and_then(|(_, id)| id.clone());
    sqlx::query(
        "SELECT id, mint, side, trigger_kind, price_sol::float8 AS price_sol, \
                amount::float8 AS amount, status, created_at, expires_at, resolved_at, \
                attempts, last_error \
         FROM limit_orders \
         WHERE organization_id = $1 \
           AND ($2::text IS NULL OR status = $2) \
           AND ($3::timestamptz IS NULL OR created_at < $3 \
                OR (created_at = $3 AND id < $5)) \
         ORDER BY created_at DESC, id DESC \
         LIMIT $4",
    )
    .bind(organization_id)
    .bind(status)
    .bind(before_ts)
    .bind(limit)
    .bind(before_id)
    .fetch_all(db.pool())
    .await
}

/// `GET /api/tenant/limit-orders`
pub async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::OrderRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("listing limit orders");
    };

    let status = match query.status.as_deref() {
        None => None,
        Some(raw) => match parse_status(raw) {
            Ok(s) => Some(s),
            Err(reason) => return validation_error(reason),
        },
    };
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if !(1..=MAX_LIST_LIMIT).contains(&limit) {
        return validation_error("limit must be between 1 and 200");
    }

    // Keyset pagination over the FULL sort key (created_at, id). A cursor
    // that carried only `created_at` skipped every row sharing the boundary
    // timestamp, because the page order breaks ties by id while the cursor
    // did not. `before` alone stays supported (timestamp-only, documented as
    // lossy at a tie) so existing clients keep working.
    let cursor: Option<(DateTime<Utc>, Option<String>)> = match (&query.before, &query.before_id) {
        (Some(ts), Some(id)) => Some((*ts, Some(id.clone()))),
        (Some(ts), None) => Some((*ts, None)),
        (None, Some(_)) => {
            return validation_error("before_id requires before (RFC 3339 created_at)")
        }
        (None, None) => None,
    };
    let composite = matches!(&cursor, Some((_, Some(id))) if !id.is_empty());

    let rows = fetch_page(db, ctx.organization.id.as_uuid(), status, cursor, limit).await;

    let rows = match rows {
        Ok(r) => r,
        Err(error) => return storage_error("listing limit orders", &error),
    };
    let orders: Vec<OrderView> = rows
        .iter()
        .map(|row| OrderView {
            id: row.get("id"),
            mint: row.get("mint"),
            side: row.get("side"),
            trigger: row.get("trigger_kind"),
            price_sol: row.get("price_sol"),
            amount: row.get("amount"),
            status: row.get("status"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
            resolved_at: row.get("resolved_at"),
            attempts: row.get("attempts"),
            last_error: row.get("last_error"),
        })
        .collect();

    // A full page returns BOTH parts of the cursor so the next request can
    // resume exactly at the boundary; `next_before` stays for older clients
    // but `next_before_id` is what makes the page boundary lossless.
    let (next_before, next_before_id) = match (
        orders.len() as i64 == limit,
        orders.last().map(|o| (o.created_at, o.id.clone())),
    ) {
        (true, Some((ts, id))) => (Some(ts), Some(id)),
        _ => (None, None),
    };

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "orders": orders,
            "next_before": next_before,
            "next_before_id": next_before_id,
            "cursor_is_lossless": composite || next_before_id.is_none(),
        })),
    )
        .into_response()
}

/// `POST /api/tenant/limit-orders/:id/cancel`
///
/// Only an ACTIVE order can be cancelled. The UPDATE is conditional on
/// `status = 'active'`, so a cancel racing a trigger has exactly one winner.
pub async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::OrderManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("cancelling a limit order");
    };

    let updated = sqlx::query(
        "UPDATE limit_orders \
         SET status = 'cancelled', resolved_at = now(), lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = $1 AND organization_id = $2 AND status = 'active' \
           AND (lease_expires_at IS NULL OR lease_expires_at < now()) \
         RETURNING id",
    )
    .bind(&id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await;

    match updated {
        Ok(Some(_)) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.limit_order.cancelled",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "id": id, "status": "cancelled" })),
            )
                .into_response()
        }
        Ok(None) => {
            // Distinguish "not yours / missing" from "not active anymore".
            let existing = sqlx::query(
                "SELECT status, \
                        (lease_expires_at IS NOT NULL AND lease_expires_at > now()) AS leased \
                 FROM limit_orders WHERE id = $1 AND organization_id = $2",
            )
            .bind(&id)
            .bind(ctx.organization.id.as_uuid())
            .fetch_optional(db.pool())
            .await;
            match existing {
                Ok(Some(row)) => {
                    let current: String = row.get("status");
                    let leased: bool = row.get("leased");
                    if current == "active" && leased {
                        // A worker holds the lease and may be handing off right now.
                        return (
                            StatusCode::CONFLICT,
                            Json(json!({
                                "error": "limit_order_being_triggered",
                                "reason": "order is being executed; it can no longer be cancelled",
                            })),
                        )
                            .into_response();
                    }
                    (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": "limit_order_not_active",
                            "reason": format!("order is already {current}"),
                        })),
                    )
                        .into_response()
                }
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "limit_order_not_found" })),
                )
                    .into_response(),
                Err(error) => storage_error("cancelling a limit order", &error),
            }
        }
        Err(error) => storage_error("cancelling a limit order", &error),
    }
}

/// Routes for the tenant limit-order API.
pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/tenant/limit-orders", post(create).get(list))
        .route("/api/tenant/limit-orders/:id/cancel", post(cancel))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page predicate must be a strict total order: with `(created_at, id)`
    /// no row can be both skipped and repeated at a boundary where several
    /// rows share the same `created_at` (the defect this fixes).
    #[test]
    fn composite_cursor_leaves_no_row_between_pages() {
        // Simulated page of equal timestamps, newest page first.
        let ts = Utc::now();
        let all: Vec<(DateTime<Utc>, String)> = vec![
            (ts, "ord-c".into()),
            (ts, "ord-b".into()),
            (ts, "ord-a".into()),
            (ts - Duration::seconds(1), "ord-z".into()),
        ];
        // Worst case for the OLD cursor: the whole page shares one timestamp.
        let page1: Vec<_> = all.iter().take(2).cloned().collect();
        let (last_ts, last_id) = page1.last().cloned().unwrap();
        // Full keyset: strictly after (last_ts, last_id) in (created_at, id) DESC.
        let page2: Vec<_> = all
            .iter()
            .filter(|(t, id)| *t < last_ts || (*t == last_ts && id < &last_id))
            .cloned()
            .collect();
        assert_eq!(
            page2,
            vec![
                (ts, "ord-a".into()),
                (ts - Duration::seconds(1), "ord-z".into())
            ],
            "the tie-boundary row is neither skipped nor repeated"
        );
        // The OLD timestamp-only cursor would have dropped `ord-a` entirely.
        let old_page2: Vec<_> = all.iter().filter(|(t, _)| *t < last_ts).cloned().collect();
        assert_eq!(old_page2.len(), 1, "timestamp-only paging skips the tie");
    }

    #[test]
    fn before_id_without_before_is_a_validation_error_worth_reporting() {
        // Guards the contract asserted by the handler: a tie-breaker alone
        // cannot form a cursor.
        let q: ListQuery = serde_json::from_str(r#"{"before_id":"ord-1"}"#).unwrap();
        assert!(q.before.is_none() && q.before_id.is_some());
    }

    #[test]
    fn side_and_trigger_parse_strictly() {
        assert_eq!(parse_side("buy").unwrap(), OrderSide::Buy);
        assert_eq!(parse_side("sell").unwrap(), OrderSide::Sell);
        assert!(
            parse_side("BUY").is_err(),
            "case-sensitive, never defaulted"
        );
        assert!(parse_side("").is_err());
        assert_eq!(
            parse_trigger("at_or_below").unwrap(),
            TriggerKind::AtOrBelow
        );
        assert_eq!(
            parse_trigger("at_or_above").unwrap(),
            TriggerKind::AtOrAbove
        );
        assert!(parse_trigger("above").is_err());
    }

    #[test]
    fn trigger_round_trips_through_storage_form() {
        for t in [TriggerKind::AtOrBelow, TriggerKind::AtOrAbove] {
            assert_eq!(parse_trigger(trigger_str(t)).unwrap(), t);
        }
    }

    #[test]
    fn status_filter_accepts_only_schema_values() {
        for s in ["active", "triggered", "cancelled", "expired"] {
            assert_eq!(parse_status(s).unwrap(), s);
        }
        assert!(parse_status("filled").is_err());
        assert!(parse_status("active; DROP TABLE x").is_err());
    }

    #[test]
    fn mint_check_is_structural_base58() {
        // Wrapped SOL mint — a real, well-formed address.
        assert!(is_plausible_mint(
            "So11111111111111111111111111111111111111112"
        ));
        assert!(!is_plausible_mint(""));
        assert!(!is_plausible_mint("short"));
        // Contains `0` (not base58) and is too long.
        assert!(!is_plausible_mint(
            "0o11111111111111111111111111111111111111112"
        ));
        assert!(!is_plausible_mint(&"1".repeat(45)));
        // Injection-shaped input is rejected by the alphabet check.
        assert!(!is_plausible_mint(
            "So1111111111111111111111111111'; DROP TABLE"
        ));
    }
}
