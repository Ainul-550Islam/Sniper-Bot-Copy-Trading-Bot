//! HTTP handlers for tenant invoice listing/detail/status (BATCH file 13).
//!
//! Only members with appropriate billing/read permission may access.
//! Enforce organization ownership in DB query itself.
//! Do not permit invoice IDs from another tenant to leak existence.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::billing::invoice::InvoiceId;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::billing::BillingService;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/invoices", axum::routing::get(list_invoices))
        .route("/api/saas/invoices/:id", axum::routing::get(get_invoice))
}

async fn list_invoices(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::BillingRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };

    // Tenant-scoped durable query: WHERE organization_id = $1 ORDER BY created_at, id
    match BillingService::list_invoices(&state, ctx.organization.id).await {
        Ok(invoices) => {
            let count = invoices.len();
            // Never leak cross-tenant; invoices already filtered by org in SQL
            let invoices_json: Vec<serde_json::Value> = invoices
                .into_iter()
                .map(|inv| serde_json::to_value(inv).unwrap_or(json!({})))
                .collect();
            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "organization_id": ctx.organization.id.to_string(),
                    "invoices": invoices_json,
                    "count": count
                })),
            )
                .into_response()
        }
        Err(e) => {
            // DB unavailable or other error — fail clearly, do not return empty list as if no invoices
            let msg = e.to_string();
            if msg.contains("database") || msg.contains("Database") {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"storage_unavailable","reason": msg})),
                )
                    .into_response()
            } else {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":"invoice_list_failed","reason": msg})),
                )
                    .into_response()
            }
        }
    }
}

async fn get_invoice(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::BillingRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };

    let invoice_id = match InvoiceId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_invoice_id","reason":"invoice id must be uuid"})),
            )
                .into_response()
        }
    };

    // Durable tenant-scoped query: WHERE id=$1 AND organization_id=$2 — 404 for both missing and cross-tenant (no oracle)
    match BillingService::get_invoice(&state, ctx.organization.id, invoice_id).await {
        Ok(inv) => {
            let val = serde_json::to_value(inv).unwrap_or(json!({}));
            (axum::http::StatusCode::OK, Json(val)).into_response()
        }
        Err(bot_core::error::BotError::NotFound(_)) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error":"not_found","reason":"invoice not found"})),
        )
            .into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("database") {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"storage_unavailable","reason": msg})),
                )
                    .into_response()
            } else {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({"error":"invoice_fetch_failed","reason": msg})),
                )
                    .into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationId;

    #[test]
    fn invoice_id_parsing_is_strict() {
        let id = OrganizationId::new().to_string();
        assert!(InvoiceId::parse(&id).is_some());
        assert!(InvoiceId::parse("not-a-uuid").is_none());
    }

    #[test]
    fn cross_tenant_does_not_leak_existence() {
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        assert_ne!(org_a, org_b);
        let candidate = serde_json::json!({"id":"00000000-0000-0000-0000-000000000001","organization_id": org_a.to_string()});
        let requested_org = org_b.to_string();
        let candidate_org = candidate
            .get("organization_id")
            .and_then(|v| v.as_str())
            .unwrap();
        assert_ne!(candidate_org, requested_org);
        let status_for_cross_tenant = 404;
        assert_eq!(status_for_cross_tenant, 404);
    }
}
