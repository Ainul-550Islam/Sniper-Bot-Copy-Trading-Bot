//! The `/api/ops/**` operator surface (P1).
//!
//! # Why this file exists
//!
//! The published contract described `/api/ops/...` endpoints that **no
//! router ever served**. The description lived in
//! `crate::api::openapi_ops`, which (until the fragments were merged)
//! was itself never called, so nothing contradicted it. Merging the
//! fragments turned a hidden inconsistency into a visible one: a
//! contract promising endpoints that answer 404.
//!
//! There were two honest ways out — implement them or stop publishing
//! them. Both were taken, deliberately:
//!
//! * `/api/ops/migration-health` is implemented here, because the
//!   schema-versus-binary question has no other answer from outside a
//!   running process and production deliberately runs with
//!   `DATABASE_AUTO_MIGRATE=false`.
//! * `preflight`, `runtime-config` and `release-artifact` are **removed
//!   from the published contract** until something serves them. Their
//!   builder code still exists in `crate::ops`; what was removed is the
//!   claim that a deployment answers on those paths.
//!
//! `crates/server/tests/openapi_router_conformance.rs` now makes that
//! choice permanent: a documented path that is not routed fails CI.
//!
//! # Authorization
//!
//! This surface describes the DEPLOYMENT, not a tenant: migration state
//! is the same fact for every customer, and it leaks the shape of the
//! schema. It therefore requires a **platform administrator**, not
//! merely an authenticated tenant with a read permission. A tenant
//! operator having `AuditRead` inside their own organization is not a
//! reason to show them the deployment's schema drift.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;

use bot_core::authorization::{AccessRequest, Decision};
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::ops::migration_health;
use crate::saas::middleware::{authorize_request, deny_response, SaasContext};

/// Operator routes. Merged into the main router next to the SaaS and
/// trading planes so they share the rate limiter, the audit trail and
/// the request-id middleware.
pub fn routes() -> Router<ApiState> {
    Router::new().route(
        "/api/ops/migration-health",
        axum::routing::get(migration_health_handler),
    )
}

/// Authenticate, then require platform administration.
///
/// The permission check comes first so an unauthenticated caller gets
/// 401 and a tenant caller gets the standard tenant refusal; only a
/// legitimately authenticated non-admin reaches the admin check. The
/// refusal is `Decision::role`, so it is audited through the same path
/// as every other authorization denial rather than inventing a second
/// one.
/// `Box`ed refusal: an `axum::Response` is a large value, and clippy's
/// `result_large_err` is right that an error variant that big is a cost
/// paid on every success too. Boxing keeps the happy path cheap.
async fn require_platform_admin(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<SaasContext, Box<Response>> {
    let ctx =
        match authorize_request(state, headers, AccessRequest::read(Permission::AuditRead)).await {
            Ok(ctx) => ctx,
            Err(decision) => return Err(Box::new(deny_response(state, &decision).await)),
        };
    if !ctx.authorization.platform_admin {
        let decision = Decision::role(
            "the operator surface describes the deployment, not a tenant; platform administration is required",
        );
        return Err(Box::new(deny_response(state, &decision).await));
    }
    Ok(ctx)
}

/// `GET /api/ops/migration-health` — does the database match this build?
///
/// 200 when in sync or merely ahead (a rollback is a planned state, not
/// a failure), 503 when migrations this build needs are missing, a
/// migration did not complete, or a checksum disagrees. 503 is chosen so
/// a load balancer can act on it without parsing the body.
async fn migration_health_handler(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(refusal) = require_platform_admin(&state, &headers).await {
        return *refusal;
    }

    // No database attached is not "healthy with zero migrations" — it is
    // a different question entirely, and answering 200 here would let a
    // memory-only deployment pass a schema gate it never took.
    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "database_not_attached",
                "reason": "no database is attached to this process, so schema state cannot be determined",
            })),
        )
            .into_response();
    };

    match migration_health::evaluate(db).await {
        Ok(report) => {
            let status = StatusCode::from_u16(report.state.http_status())
                .unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
            (status, Json(report.to_json())).into_response()
        }
        // A failed inspection is reported as a failed inspection. The
        // one answer this endpoint must never give is a confident
        // "in_sync" derived from a query that did not run.
        Err(reason) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "migration_state_unavailable",
                "reason": reason,
            })),
        )
            .into_response(),
    }
}
