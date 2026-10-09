//! Tenant DCA schedule API (remediation tree, Part 3).
//!
//! Routes (every one: authorize → org-scope → DB → audit → test):
//! * `POST /api/tenant/dca-schedules`                 — create (OrderManage);
//! * `GET  /api/tenant/dca-schedules`                 — list (OrderRead), `status` filter;
//! * `GET  /api/tenant/dca-schedules/:id/runs`        — run ledger for one schedule (OrderRead);
//! * `POST /api/tenant/dca-schedules/:id/pause`       — active → paused (OrderManage);
//! * `POST /api/tenant/dca-schedules/:id/resume`      — paused → active (OrderManage);
//! * `POST /api/tenant/dca-schedules/:id/cancel`      — active|paused → cancelled (OrderManage).
//!
//! Semantics fixed by the schema and the module model:
//! * the budget is a HARD ceiling; unlimited schedules are refused;
//! * pause and resume never create a catch-up burst: resume re-anchors
//!   `next_run_at` to `now + interval`, so missed slots are not replayed;
//! * cancel is refused while a worker holds the lease (it may be recording a run).

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

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};
use crate::trading_data_plane::limit_orders::{
    is_plausible_mint, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT, MAX_NUMERIC_VALUE,
};

/// Longest supported interval (one year). Longer schedules are not DCA.
pub const MAX_INTERVAL_SECS: i64 = 365 * 24 * 60 * 60;

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub mint: String,
    pub amount_per_run_sol: f64,
    pub interval_secs: i64,
    pub budget_sol: f64,
    /// Optional delay before the first run; absent means "first slot is now".
    #[serde(default)]
    pub start_in_secs: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ScheduleView {
    pub id: String,
    pub mint: String,
    pub amount_per_run_sol: f64,
    pub interval_secs: i64,
    pub budget_sol: f64,
    pub spent_sol: f64,
    pub remaining_sol: f64,
    pub status: String,
    pub next_run_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Validate a DCA request before it is stored. Fails closed.
pub fn validate_schedule(
    mint: &str,
    amount_per_run_sol: f64,
    interval_secs: i64,
    budget_sol: f64,
) -> Result<(), &'static str> {
    if !is_plausible_mint(mint) {
        return Err("mint must be a base58 Solana address");
    }
    if !amount_per_run_sol.is_finite()
        || amount_per_run_sol <= 0.0
        || amount_per_run_sol > MAX_NUMERIC_VALUE
    {
        return Err("amount_per_run_sol must be a positive finite number within range");
    }
    if !(1..=MAX_INTERVAL_SECS).contains(&interval_secs) {
        return Err("interval_secs must be between 1 and one year");
    }
    if !budget_sol.is_finite() || budget_sol <= 0.0 || budget_sol > MAX_NUMERIC_VALUE {
        return Err("budget_sol must be positive; unlimited schedules are not supported");
    }
    if budget_sol < amount_per_run_sol {
        return Err("budget_sol cannot be smaller than one run");
    }
    Ok(())
}

pub fn parse_status(raw: &str) -> Result<&'static str, &'static str> {
    match raw {
        "active" => Ok("active"),
        "completed" => Ok("completed"),
        "paused" => Ok("paused"),
        "cancelled" => Ok("cancelled"),
        _ => Err("status must be one of active, completed, paused, cancelled"),
    }
}

fn validation_error(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": "dca_schedule_invalid", "reason": reason })),
    )
        .into_response()
}

fn db_unavailable(what: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "dca_storage_unavailable",
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
            "error": "dca_storage_unavailable",
            "reason": format!("{what} could not be completed"),
        })),
    )
        .into_response()
}

/// `POST /api/tenant/dca-schedules`
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
        return db_unavailable("creating a DCA schedule");
    };
    if let Err(reason) = validate_schedule(
        &body.mint,
        body.amount_per_run_sol,
        body.interval_secs,
        body.budget_sol,
    ) {
        return validation_error(reason);
    }
    let now = Utc::now();
    let start_in = body.start_in_secs.unwrap_or(0);
    if !(0..=MAX_INTERVAL_SECS).contains(&start_in) {
        return validation_error("start_in_secs must be between 0 and one year");
    }
    let first_run = now + Duration::seconds(start_in);
    let id = format!("dca_{}", uuid::Uuid::new_v4().simple());

    let inserted = sqlx::query(
        "INSERT INTO dca_schedules \
         (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, spent_sol, \
          status, created_at, next_run_at) \
         VALUES ($1, $2, $3, $4::float8, $5, $6::float8, 0, 'active', $7, $8)",
    )
    .bind(&id)
    .bind(ctx.organization.id.as_uuid())
    .bind(&body.mint)
    .bind(body.amount_per_run_sol)
    .bind(body.interval_secs)
    .bind(body.budget_sol)
    .bind(now)
    .bind(first_run)
    .execute(db.pool())
    .await;
    if let Err(error) = inserted {
        return storage_error("creating a DCA schedule", &error);
    }

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.dca_schedule.created",
            Some(&ctx.organization.id.to_string()),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({ "id": id, "status": "active", "next_run_at": first_run })),
    )
        .into_response()
}

/// `GET /api/tenant/dca-schedules`
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
        return db_unavailable("listing DCA schedules");
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

    let rows = sqlx::query(
        "SELECT id, mint, amount_per_run_sol::float8 AS amount_per_run_sol, interval_secs, \
                budget_sol::float8 AS budget_sol, spent_sol::float8 AS spent_sol, status, \
                next_run_at, last_run_at, attempts, last_error, created_at \
         FROM dca_schedules \
         WHERE organization_id = $1 AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(ctx.organization.id.as_uuid())
    .bind(status)
    .bind(limit)
    .fetch_all(db.pool())
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(error) => return storage_error("listing DCA schedules", &error),
    };
    let schedules: Vec<ScheduleView> = rows
        .iter()
        .map(|row| {
            let budget: f64 = row.get("budget_sol");
            let spent: f64 = row.get("spent_sol");
            ScheduleView {
                id: row.get("id"),
                mint: row.get("mint"),
                amount_per_run_sol: row.get("amount_per_run_sol"),
                interval_secs: row.get("interval_secs"),
                budget_sol: budget,
                spent_sol: spent,
                remaining_sol: (budget - spent).max(0.0),
                status: row.get("status"),
                next_run_at: row.get("next_run_at"),
                last_run_at: row.get("last_run_at"),
                attempts: row.get("attempts"),
                last_error: row.get("last_error"),
                created_at: row.get("created_at"),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "schedules": schedules,
        })),
    )
        .into_response()
}

/// `GET /api/tenant/dca-schedules/:id/runs` — the run ledger, newest first.
pub async fn runs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
        return db_unavailable("reading DCA runs");
    };
    // Scope the schedule first so a foreign id is indistinguishable from a missing one.
    let owned = sqlx::query("SELECT 1 FROM dca_schedules WHERE id = $1 AND organization_id = $2")
        .bind(&id)
        .bind(ctx.organization.id.as_uuid())
        .fetch_optional(db.pool())
        .await;
    match owned {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "dca_schedule_not_found" })),
            )
                .into_response()
        }
        Err(error) => return storage_error("reading DCA runs", &error),
        Ok(Some(_)) => {}
    }
    let rows = sqlx::query(
        "SELECT id, slot_at, amount_sol::float8 AS amount_sol, status, created_at \
         FROM dca_runs WHERE schedule_id = $1 AND organization_id = $2 \
         ORDER BY slot_at DESC LIMIT $3",
    )
    .bind(&id)
    .bind(ctx.organization.id.as_uuid())
    .bind(MAX_LIST_LIMIT)
    .fetch_all(db.pool())
    .await;
    match rows {
        Ok(rows) => {
            let runs: Vec<_> = rows
                .iter()
                .map(|row| {
                    json!({
                        "run_id": row.get::<i64, _>("id"),
                        "slot_at": row.get::<DateTime<Utc>, _>("slot_at"),
                        "amount_sol": row.get::<f64, _>("amount_sol"),
                        "status": row.get::<String, _>("status"),
                        "created_at": row.get::<DateTime<Utc>, _>("created_at"),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "schedule_id": id, "runs": runs })),
            )
                .into_response()
        }
        Err(error) => storage_error("reading DCA runs", &error),
    }
}

/// Shared shape for pause / resume / cancel. `from` lists the states the
/// transition may start from; the UPDATE is conditional on them.
async fn transition(
    state: &ApiState,
    headers: &HeaderMap,
    id: String,
    from: &[&str],
    to: &str,
    action: &str,
) -> Response {
    let ctx = match authorize_request(
        state,
        headers,
        AccessRequest::manage(Permission::OrderManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("changing a DCA schedule");
    };

    // Resume re-anchors the clock so missed slots are not replayed.
    let sql = if to == "active" {
        "UPDATE dca_schedules SET status = 'active', next_run_at = now() + make_interval(secs => interval_secs::float8) \
         WHERE id = $1 AND organization_id = $2 AND status = ANY($3) \
           AND (lease_expires_at IS NULL OR lease_expires_at < now()) RETURNING id"
    } else {
        "UPDATE dca_schedules SET status = $4 \
         WHERE id = $1 AND organization_id = $2 AND status = ANY($3) \
           AND (lease_expires_at IS NULL OR lease_expires_at < now()) RETURNING id"
    };
    let from_vec: Vec<String> = from.iter().map(|s| (*s).to_string()).collect();
    let mut query = sqlx::query(sql)
        .bind(&id)
        .bind(ctx.organization.id.as_uuid())
        .bind(from_vec);
    if to != "active" {
        query = query.bind(to);
    }
    let updated = query.fetch_optional(db.pool()).await;

    match updated {
        Ok(Some(_)) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    action,
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (StatusCode::OK, Json(json!({ "id": id, "status": to }))).into_response()
        }
        Ok(None) => {
            let existing = sqlx::query(
                "SELECT status, (lease_expires_at IS NOT NULL AND lease_expires_at > now()) AS leased \
                 FROM dca_schedules WHERE id = $1 AND organization_id = $2",
            )
            .bind(&id)
            .bind(ctx.organization.id.as_uuid())
            .fetch_optional(db.pool())
            .await;
            match existing {
                Ok(Some(row)) => {
                    let leased: bool = row.get("leased");
                    let current: String = row.get("status");
                    if leased {
                        (
                            StatusCode::CONFLICT,
                            Json(json!({
                                "error": "dca_schedule_busy",
                                "reason": "a worker is recording a run; retry shortly",
                            })),
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::CONFLICT,
                            Json(json!({
                                "error": "dca_schedule_invalid_transition",
                                "reason": format!("schedule is {current}; cannot {action}"),
                            })),
                        )
                            .into_response()
                    }
                }
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "dca_schedule_not_found" })),
                )
                    .into_response(),
                Err(error) => storage_error("changing a DCA schedule", &error),
            }
        }
        Err(error) => storage_error("changing a DCA schedule", &error),
    }
}

/// `POST /api/tenant/dca-schedules/:id/pause`
pub async fn pause(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    transition(
        &state,
        &headers,
        id,
        &["active"],
        "paused",
        "saas.dca_schedule.paused",
    )
    .await
}

/// `POST /api/tenant/dca-schedules/:id/resume`
pub async fn resume(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    transition(
        &state,
        &headers,
        id,
        &["paused"],
        "active",
        "saas.dca_schedule.resumed",
    )
    .await
}

/// `POST /api/tenant/dca-schedules/:id/cancel`
pub async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    transition(
        &state,
        &headers,
        id,
        &["active", "paused"],
        "cancelled",
        "saas.dca_schedule.cancelled",
    )
    .await
}

/// Routes for the tenant DCA API.
pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/tenant/dca-schedules", post(create).get(list))
        .route(
            "/api/tenant/dca-schedules/:id/runs",
            axum::routing::get(runs),
        )
        .route("/api/tenant/dca-schedules/:id/pause", post(pause))
        .route("/api/tenant/dca-schedules/:id/resume", post(resume))
        .route("/api/tenant/dca-schedules/:id/cancel", post(cancel))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINT: &str = "So11111111111111111111111111111111111111112";

    #[test]
    fn accepts_a_well_formed_schedule() {
        assert!(validate_schedule(MINT, 0.5, 3600, 5.0).is_ok());
    }

    #[test]
    fn refuses_unlimited_and_inverted_budgets() {
        assert!(validate_schedule(MINT, 0.5, 3600, 0.0).is_err());
        assert!(validate_schedule(MINT, 0.5, 3600, f64::INFINITY).is_err());
        assert!(
            validate_schedule(MINT, 5.0, 3600, 1.0).is_err(),
            "budget below one run"
        );
    }

    #[test]
    fn refuses_bad_intervals_and_amounts() {
        assert!(validate_schedule(MINT, 0.5, 0, 5.0).is_err());
        assert!(validate_schedule(MINT, 0.5, MAX_INTERVAL_SECS + 1, 5.0).is_err());
        assert!(validate_schedule(MINT, f64::NAN, 3600, 5.0).is_err());
        assert!(validate_schedule(MINT, -1.0, 3600, 5.0).is_err());
        assert!(validate_schedule("not-a-mint", 0.5, 3600, 5.0).is_err());
    }

    #[test]
    fn status_filter_is_closed() {
        assert_eq!(parse_status("paused").unwrap(), "paused");
        assert!(parse_status("running").is_err());
    }
}
