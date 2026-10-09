//! Wallet pools REST (GAP-MAP v2, P2; durable schema in migration 0050).
//!
//! Named pools of a tenant's OWN custody signers, used by
//! `module-sniper`'s executor to spread buys across wallets. Routes:
//! * `POST   /api/saas/wallet-pools` — create a pool;
//! * `GET    /api/saas/wallet-pools` — list this org's pools;
//! * `GET    /api/saas/wallet-pools/:id` — one pool with its members;
//! * `PATCH  /api/saas/wallet-pools/:id` — rename / re-allocate / disable;
//! * `POST   /api/saas/wallet-pools/:id/members` — add a custody signer;
//! * `DELETE /api/saas/wallet-pools/:id/members/:member_id` — remove a member.
//!
//! Tenant boundaries enforced here AND in Postgres (defence in depth):
//! * every query filters on `organization_id = <caller org>`;
//! * membership inserts are validated against the SAME-org custody signer
//!   in Rust before the `check_wallet_pool_member_org()` trigger runs;
//! * a pool never exposes another org's signer: the member query joins
//!   `custody_signers` scoped to the caller org.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

/// Pool allocation strategies — mirrors `wallet_pools.allocation` and
/// `module_sniper::tenant_executor::PoolAllocation`.
const ROUND_ROBIN: &str = "round_robin";
const WEIGHTED_SPLIT: &str = "weighted_split";

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/wallet-pools", axum::routing::post(create_pool))
        .route("/api/saas/wallet-pools", axum::routing::get(list_pools))
        .route("/api/saas/wallet-pools/:id", axum::routing::get(get_pool))
        .route("/api/saas/wallet-pools/:id", axum::routing::patch(update_pool))
        .route(
            "/api/saas/wallet-pools/:id/members",
            axum::routing::post(add_member),
        )
        .route(
            "/api/saas/wallet-pools/:id/members/:member_id",
            axum::routing::delete(remove_member),
        )
}

fn db_unavailable(what: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "wallet_pool_storage_unavailable",
            "reason": format!("{what} requires an attached PostgreSQL database"),
        })),
    )
        .into_response()
}

fn bad_request(reason: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_wallet_pool_request", "reason": reason })),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "wallet_pool_not_found",
            "reason": "no wallet pool with that id belongs to this organization",
        })),
    )
        .into_response()
}

/// Parse a uuid path segment, failing closed to 404.
fn parse_uuid(raw: &str) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(raw).ok()
}

fn valid_allocation(value: &str) -> bool {
    value == ROUND_ROBIN || value == WEIGHTED_SPLIT
}

#[derive(Debug, Deserialize)]
pub struct CreatePoolBody {
    pub name: String,
    /// `round_robin` (default) or `weighted_split`.
    #[serde(default = "default_allocation")]
    pub allocation: String,
}

fn default_allocation() -> String {
    ROUND_ROBIN.to_string()
}

/// `POST /api/saas/wallet-pools` — create a pool owned by the caller's org.
pub async fn create_pool(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreatePoolBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return bad_request("pool name must be 1..=64 characters");
    }
    if !valid_allocation(&body.allocation) {
        return bad_request("allocation must be round_robin or weighted_split");
    }
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("creating a wallet pool");
    };

    let result = sqlx::query(
        "INSERT INTO wallet_pools (organization_id, name, allocation, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING id, name, allocation, status, rotation_cursor, created_at",
    )
    .bind(ctx.organization.id.as_uuid())
    .bind(name)
    .bind(&body.allocation)
    .bind(ctx.authorization.user_id.map(|u| u.as_uuid()))
    .fetch_one(db.pool())
    .await;
    match result {
        Ok(row) => {
            let id: uuid::Uuid = row.get("id");
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.wallet_pool.created",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": id,
                    "organization_id": ctx.organization.id.to_string(),
                    "name": row.get::<String, _>("name"),
                    "allocation": row.get::<String, _>("allocation"),
                    "status": row.get::<String, _>("status"),
                    "rotation_cursor": row.get::<i32, _>("rotation_cursor"),
                    "created_at": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                })),
            )
                .into_response()
        }
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "wallet_pool_name_taken",
                "reason": "this organization already has a pool with that name",
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "wallet pool create failed");
            db_unavailable("creating a wallet pool")
        }
    }
}

/// `GET /api/saas/wallet-pools` — list this org's pools with member counts.
pub async fn list_pools(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("listing wallet pools");
    };
    let rows = sqlx::query(
        "SELECT p.id, p.name, p.allocation, p.status, p.rotation_cursor, p.created_at, \
                COUNT(m.id) FILTER (WHERE m.status = 'active') AS active_members \
         FROM wallet_pools p \
         LEFT JOIN wallet_pool_members m ON m.pool_id = p.id \
         WHERE p.organization_id = $1 \
         GROUP BY p.id \
         ORDER BY p.created_at DESC",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(error) => {
            tracing::error!(error = %error, "wallet pool list failed");
            return db_unavailable("listing wallet pools");
        }
    };
    let pools: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<uuid::Uuid, _>("id"),
                "name": row.get::<String, _>("name"),
                "allocation": row.get::<String, _>("allocation"),
                "status": row.get::<String, _>("status"),
                "rotation_cursor": row.get::<i32, _>("rotation_cursor"),
                "active_members": row.get::<i64, _>("active_members"),
                "created_at": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "pools": pools,
        })),
    )
        .into_response()
}

/// `GET /api/saas/wallet-pools/:id` — one pool with its ACTIVE members,
/// each resolved to the same-org custody signer.
pub async fn get_pool(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(pool_id) = parse_uuid(&id) else {
        return not_found();
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("reading a wallet pool");
    };

    let pool = sqlx::query(
        "SELECT id, name, allocation, status, rotation_cursor, created_at \
         FROM wallet_pools WHERE id = $1 AND organization_id = $2",
    )
    .bind(pool_id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await;
    let pool = match pool {
        Ok(Some(p)) => p,
        Ok(None) => return not_found(),
        Err(error) => {
            tracing::error!(error = %error, "wallet pool read failed");
            return db_unavailable("reading a wallet pool");
        }
    };

    // Members joined to the SAME-org custody signer. The org filter on the
    // join is the Rust-side mirror of the cross-tenant trigger.
    let members = sqlx::query(
        "SELECT m.id, m.weight, m.status, m.added_at, s.logical_identity, s.public_address \
         FROM wallet_pool_members m \
         JOIN custody_signers s ON s.id = m.custody_signer_id \
         WHERE m.pool_id = $1 AND s.organization_id = $2 AND m.status = 'active' \
         ORDER BY m.added_at ASC",
    )
    .bind(pool_id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await;
    let members = match members {
        Ok(m) => m,
        Err(error) => {
            tracing::error!(error = %error, "wallet pool member read failed");
            return db_unavailable("reading a wallet pool");
        }
    };
    let members: Vec<serde_json::Value> = members
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<uuid::Uuid, _>("id"),
                "weight": row.get::<i32, _>("weight"),
                "status": row.get::<String, _>("status"),
                "logical_identity": row.get::<String, _>("logical_identity"),
                "public_address": row.get::<String, _>("public_address"),
                "added_at": row.get::<chrono::DateTime<chrono::Utc>, _>("added_at"),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "id": pool.get::<uuid::Uuid, _>("id"),
            "organization_id": ctx.organization.id.to_string(),
            "name": pool.get::<String, _>("name"),
            "allocation": pool.get::<String, _>("allocation"),
            "status": pool.get::<String, _>("status"),
            "rotation_cursor": pool.get::<i32, _>("rotation_cursor"),
            "created_at": pool.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
            "members": members,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct UpdatePoolBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub allocation: Option<String>,
    /// `active` or `disabled`.
    #[serde(default)]
    pub status: Option<String>,
}

/// `PATCH /api/saas/wallet-pools/:id` — partial update of name/allocation/status.
pub async fn update_pool(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdatePoolBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(pool_id) = parse_uuid(&id) else {
        return not_found();
    };
    if let Some(name) = &body.name {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 64 {
            return bad_request("pool name must be 1..=64 characters");
        }
    }
    if let Some(allocation) = &body.allocation {
        if !valid_allocation(allocation) {
            return bad_request("allocation must be round_robin or weighted_split");
        }
    }
    if let Some(status) = &body.status {
        if status != "active" && status != "disabled" {
            return bad_request("status must be active or disabled");
        }
    }
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("updating a wallet pool");
    };

    let result = sqlx::query(
        "UPDATE wallet_pools SET \
            name = COALESCE($3, name), \
            allocation = COALESCE($4, allocation), \
            status = COALESCE($5, status), \
            updated_at = now() \
         WHERE id = $1 AND organization_id = $2 \
         RETURNING id, name, allocation, status, rotation_cursor",
    )
    .bind(pool_id)
    .bind(ctx.organization.id.as_uuid())
    .bind(body.name.as_deref().map(str::trim))
    .bind(body.allocation.as_deref())
    .bind(body.status.as_deref())
    .fetch_optional(db.pool())
    .await;
    match result {
        Ok(Some(row)) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.wallet_pool.updated",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "id": row.get::<uuid::Uuid, _>("id"),
                    "name": row.get::<String, _>("name"),
                    "allocation": row.get::<String, _>("allocation"),
                    "status": row.get::<String, _>("status"),
                    "rotation_cursor": row.get::<i32, _>("rotation_cursor"),
                })),
            )
                .into_response()
        }
        Ok(None) => not_found(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "wallet_pool_name_taken",
                "reason": "this organization already has a pool with that name",
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "wallet pool update failed");
            db_unavailable("updating a wallet pool")
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AddMemberBody {
    /// The tenant-bound custody signer to add.
    pub custody_signer_id: String,
    /// Selection weight for weighted_split (1..=1000; ignored by round_robin).
    #[serde(default = "default_weight")]
    pub weight: i32,
}

fn default_weight() -> i32 {
    1
}

/// `POST /api/saas/wallet-pools/:id/members` — add a SAME-org signer.
pub async fn add_member(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AddMemberBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(pool_id) = parse_uuid(&id) else {
        return not_found();
    };
    let Some(signer_id) = parse_uuid(&body.custody_signer_id) else {
        return bad_request("custody_signer_id must be a uuid");
    };
    if !(1..=1000).contains(&body.weight) {
        return bad_request("weight must be between 1 and 1000");
    }
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("adding a wallet pool member");
    };

    // Confirm the pool belongs to the caller (org-scope before mutation).
    let pool = sqlx::query("SELECT status FROM wallet_pools WHERE id = $1 AND organization_id = $2")
        .bind(pool_id)
        .bind(ctx.organization.id.as_uuid())
        .fetch_optional(db.pool())
        .await;
    let pool = match pool {
        Ok(Some(p)) => p,
        Ok(None) => return not_found(),
        Err(error) => {
            tracing::error!(error = %error, "wallet pool ownership check failed");
            return db_unavailable("adding a wallet pool member");
        }
    };
    if pool.get::<String, _>("status") == "disabled" {
        return bad_request("cannot add members to a disabled pool");
    }

    // Confirm the signer is an ACTIVE custody signer of the SAME org. The
    // Postgres trigger repeats this; doing it in Rust returns a clean 400
    // instead of a raw constraint error.
    let signer = sqlx::query(
        "SELECT status FROM custody_signers WHERE id = $1 AND organization_id = $2",
    )
    .bind(signer_id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await;
    let signer = match signer {
        Ok(s) => s,
        Err(error) => {
            tracing::error!(error = %error, "custody signer check failed");
            return db_unavailable("adding a wallet pool member");
        }
    };
    match signer.as_ref().map(|r| r.get::<String, _>("status")) {
        None => return bad_request("custody signer not found in this organization"),
        Some(status) if status != "active" => {
            return bad_request("only active custody signers can join a pool")
        }
        Some(_) => {}
    }

    let result = sqlx::query(
        "INSERT INTO wallet_pool_members (pool_id, custody_signer_id, weight, added_by) \
         VALUES ($1, $2, $3, $4) RETURNING id, weight, status, added_at",
    )
    .bind(pool_id)
    .bind(signer_id)
    .bind(body.weight)
    .bind(ctx.authorization.user_id.map(|u| u.as_uuid()))
    .fetch_one(db.pool())
    .await;
    match result {
        Ok(row) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.wallet_pool.member_added",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": row.get::<uuid::Uuid, _>("id"),
                    "pool_id": pool_id,
                    "custody_signer_id": signer_id,
                    "weight": row.get::<i32, _>("weight"),
                    "status": row.get::<String, _>("status"),
                    "added_at": row.get::<chrono::DateTime<chrono::Utc>, _>("added_at"),
                })),
            )
                .into_response()
        }
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "member_already_in_pool",
                "reason": "that custody signer is already a member of this pool",
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "wallet pool member insert failed");
            db_unavailable("adding a wallet pool member")
        }
    }
}

/// `DELETE /api/saas/wallet-pools/:id/members/:member_id` — mark a member
/// removed. Membership changes are immediate for NEW orders (the executor
/// stops picking the wallet); open positions stay with their wallet.
pub async fn remove_member(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((id, member_id)): Path<(String, String)>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(pool_id) = parse_uuid(&id) else {
        return not_found();
    };
    let Some(member_id) = parse_uuid(&member_id) else {
        return not_found();
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("removing a wallet pool member");
    };

    // Org-scoped: pool must belong to the caller, member to that pool.
    let result = sqlx::query(
        "UPDATE wallet_pool_members m SET status = 'removed', removed_at = now() \
         FROM wallet_pools p \
         WHERE m.id = $1 AND m.pool_id = $2 AND m.pool_id = p.id \
           AND p.organization_id = $3 AND m.status = 'active'",
    )
    .bind(member_id)
    .bind(pool_id)
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match result {
        Ok(out) if out.rows_affected() == 1 => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.wallet_pool.member_removed",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "id": member_id, "status": "removed" })),
            )
                .into_response()
        }
        Ok(_) => not_found(),
        Err(error) => {
            tracing::error!(error = %error, "wallet pool member removal failed");
            db_unavailable("removing a wallet pool member")
        }
    }
}
