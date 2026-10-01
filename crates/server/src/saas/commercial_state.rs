//! Aggregate customer-facing commercial state (Batch 3, §G Batch 8).
//!
//! Coordinates billing state, usage policy, entitlements, and lifecycle state.
//! Provides a single stable service consumed by REST/OpenAPI/frontend/SDK.
//!
//! §G: the billing half of this response is assembled by
//! [`crate::saas::billing_view::BillingView`] from the authoritative
//! `SaasStore`. The lifecycle half comes from the organization row. There
//! is no hardcoded plan, no hardcoded `active`/`past_due`, and no invented
//! usage total in this path; `commercial_consistent` is COMPUTED by the
//! view's consistency invariants, not asserted.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::billing_view::BillingView;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Serialize)]
pub struct CommercialStateResponse {
    pub organization_id: String,
    pub plan_code: String,
    pub plan_name: String,
    pub subscription_status: String,
    pub billing_provider: String,
    pub entitlements_active: bool,
    pub dunning_state: String,
    pub usage: serde_json::Value,
    pub lifecycle_status: String,
    pub suspension_reason: Option<String>,
    pub grace_until: Option<String>,
    pub commercial_consistent: bool,
    pub as_of: String,
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/commercial/state", axum::routing::get(my_state))
        .route("/api/saas/commercial/state/:id", axum::routing::get(by_id))
}

async fn my_state(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::BillingRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org = ctx.organization.id;
    render(org, &state).await.into_response()
}

async fn by_id(
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
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org = match OrganizationId::parse(&id) {
        Some(o) => o,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_organization_id"})),
            )
                .into_response()
        }
    };
    if ctx.organization.id != org && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error":"not_found"})),
        )
            .into_response();
    }
    render(org, &state).await.into_response()
}

/// The authoritative commercial state: billing facts via [`BillingView`],
/// lifecycle facts via the organization row, consistency COMPUTED.
async fn render(org: OrganizationId, state: &ApiState) -> Json<serde_json::Value> {
    let view = BillingView::load(&state.saas, org, Utc::now()).await;
    let lifecycle_status = view
        .organization
        .as_ref()
        .map(|o| o.status.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let consistency = view.consistency();
    let response = CommercialStateResponse {
        organization_id: org.to_string(),
        plan_code: view.plan_code().to_string(),
        plan_name: view.plan_name().to_string(),
        subscription_status: view.subscription_status().to_string(),
        billing_provider: view.billing_provider().to_string(),
        entitlements_active: view.entitlements_active(),
        dunning_state: view.dunning.as_str().to_string(),
        usage: serde_json::to_value(&view.usage).unwrap_or_default(),
        lifecycle_status,
        suspension_reason: view.suspension_reason().map(|s| s.to_string()),
        grace_until: view.grace_until().map(|t| t.to_rfc3339()),
        commercial_consistent: consistency.consistent,
        as_of: view.as_of.to_rfc3339(),
    };
    Json(json!(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saas::store::SaasStore;
    use bot_core::billing::plan::PlanCode;
    use bot_core::billing::usage::{UsageEvent, UsageMetric, UsageSource};
    use bot_core::tenant::{Organization, OrganizationStatus};

    async fn store_with_org(status: OrganizationStatus) -> (SaasStore, OrganizationId) {
        let store = SaasStore::new();
        let org_id = OrganizationId::new();
        let mut org = Organization::new(
            org_id,
            format!("test-{}", org_id.as_uuid()),
            "Test Org",
            None,
            Utc::now(),
        );
        org.status = status;
        store.create_organization(&org).await.expect("org created");
        (store, org_id)
    }

    #[tokio::test]
    async fn active_tenant_with_plan_is_consistent_and_verbatim() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        store
            .assign_plan(org_id, PlanCode::Starter, Utc::now())
            .await
            .expect("plan assigned");
        store
            .record_usage(&UsageEvent::new(
                org_id,
                UsageMetric::OrdersSubmitted,
                4.0,
                UsageSource::Sniper,
                "commercial-1",
                Utc::now(),
            ))
            .await
            .expect("usage recorded");
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        let consistency = view.consistency();
        assert!(consistency.consistent);
        assert!(consistency.entitlements_match_lifecycle);
        assert!(consistency.plan_requires_subscription);
        assert_eq!(view.plan_code(), "starter");
        assert_eq!(view.usage.orders_submitted, 4.0);
    }

    #[tokio::test]
    async fn tenant_without_subscription_has_no_plan_and_still_consistent() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.plan_code(), "none");
        assert!(view.consistency().consistent);
    }

    #[tokio::test]
    async fn suspended_tenant_reflects_lifecycle_not_invented_past_due() {
        let (store, org_id) = store_with_org(OrganizationStatus::Suspended).await;
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        // Dunning is DERIVED from the lifecycle row, never defaulted.
        assert_eq!(
            view.dunning,
            bot_core::billing::dunning::DunningState::BillingSuspended
        );
        assert_eq!(view.suspension_reason(), Some("organization_suspended"));
        // The old code invented "past_due" + "payment_failed" for any
        // suspended org; the honest answer is the lifecycle reason.
        assert_ne!(view.subscription_status(), "past_due");
    }

    #[test]
    fn response_shape_carries_plan_name_and_grace() {
        // Serialization contract: the new fields survive round-tripping.
        let resp = CommercialStateResponse {
            organization_id: OrganizationId::new().to_string(),
            plan_code: "pro".to_string(),
            plan_name: "Pro".to_string(),
            subscription_status: "active".to_string(),
            billing_provider: "manual".to_string(),
            entitlements_active: true,
            dunning_state: "current".to_string(),
            usage: serde_json::json!({"period": "2026-09", "api_requests": 0.0}),
            lifecycle_status: "active".to_string(),
            suspension_reason: None,
            grace_until: None,
            commercial_consistent: true,
            as_of: Utc::now().to_rfc3339(),
        };
        let v = serde_json::to_value(&resp).expect("serializes");
        assert_eq!(v["plan_name"], "Pro");
        assert_eq!(v["commercial_consistent"], true);
        assert!(v["usage"]["period"].as_str().is_some());
    }
}
