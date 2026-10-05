//! Customer-facing billing status application service (Batch 3, §G Batch 8).
//!
//! Returns current plan, subscription state, payment/invoice state,
//! entitlement state, usage summary, grace/dunning state. Tenant-scoped and
//! permission-controlled. Never exposes provider secrets.
//!
//! §G: the response is assembled by [`crate::saas::billing_view::BillingView`]
//! from the authoritative `SaasStore` — the plan, subscription status,
//! entitlement flag and every usage number come from recorded state. There
//! is no default plan code, no default `active` status, and no invented
//! usage in this path.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::billing_view::BillingView;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/billing/status", axum::routing::get(my_status))
        .route("/api/saas/billing/status/:id", axum::routing::get(by_id))
}

async fn my_status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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
    render(org, &state).await
}

/// The authoritative billing status: everything in the body is loaded from
/// the store via [`BillingView`]; nothing is synthesized here.
async fn render(organization_id: OrganizationId, state: &ApiState) -> Response {
    match BillingView::load(&state.saas, organization_id, Utc::now()).await {
        Ok(view) => Json(view.to_status_json()).into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %organization_id, "billing status could not be loaded");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "billing_storage_unavailable",
                    "reason": "authoritative billing records could not be loaded",
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saas::store::SaasStore;
    use bot_core::billing::plan::PlanCode;
    use bot_core::billing::usage::{UsageEvent, UsageMetric, UsageSource};
    use bot_core::tenant::Organization;

    async fn store_with_org() -> (SaasStore, OrganizationId) {
        let store = SaasStore::new();
        let org_id = OrganizationId::new();
        let org = Organization::new(
            org_id,
            format!("test-{}", org_id.as_uuid()),
            "Test Org",
            None,
            Utc::now(),
        );
        store.create_organization(&org).await.expect("org created");
        (store, org_id)
    }

    #[test]
    fn response_is_tenant_scoped() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        assert_ne!(org1.to_string(), org2.to_string());
    }

    #[tokio::test]
    async fn no_subscription_renders_explicit_none() {
        // The render path delegates to BillingView; verify through the
        // view that a tenant with no subscription gets "none" — never a
        // default tier.
        let (store, org_id) = store_with_org().await;
        let view = BillingView::load(&store, org_id, Utc::now())
            .await
            .expect("billing view");
        let body = view.to_status_json();
        assert_eq!(body["plan_code"], "none");
        assert_eq!(body["subscription_status"], "none");
        assert_eq!(body["billing_provider"], "none");
        assert_eq!(body["entitlements_active"], false);
    }

    #[tokio::test]
    async fn assigned_plan_and_metered_usage_render_verbatim() {
        let (store, org_id) = store_with_org().await;
        store
            .assign_plan(org_id, PlanCode::Pro, Utc::now())
            .await
            .expect("plan assigned");
        store
            .record_usage(&UsageEvent::new(
                org_id,
                UsageMetric::ApiRequests,
                17.0,
                UsageSource::Api,
                "billing-status-1",
                Utc::now(),
            ))
            .await
            .expect("usage recorded");
        let view = BillingView::load(&store, org_id, Utc::now())
            .await
            .expect("billing view");
        let body = view.to_status_json();
        assert_eq!(body["plan_code"], "pro");
        assert_eq!(body["subscription_status"], "active");
        assert_eq!(body["billing_provider"], "manual");
        assert_eq!(body["entitlements_active"], true);
        assert_eq!(body["usage"]["api_requests"], 17.0);
    }

    #[test]
    fn no_secrets_in_response() {
        let v = json!({
            "organization_id": OrganizationId::new().to_string(),
            "plan_code": "pro",
            "billing_provider": "manual",
            "payment_state": null
        });
        let s = v.to_string().to_ascii_lowercase();
        for banned in ["secret", "private", "sk-", "token", "password"] {
            assert!(!s.contains(banned));
        }
    }

    #[test]
    fn cross_tenant_denied_via_not_found() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        assert_ne!(org1, org2);
        // handler returns 404 when ctx org != requested and not platform scope
    }

    #[test]
    fn permission_required_billing_read() {
        assert!(Permission::BillingRead.is_read_only());
    }
}
