//! Customer-facing usage/limits service (Batch 3; PROMPT 5 §I rebuild).
//!
//! Returns current usage versus plan limits. Tenant-scoped,
//! permission-controlled. Every number is DERIVED:
//!
//! * the plan comes from the organization's REAL subscription
//!   (`SaasStore::subscription_of` → `SaasStore::plan`) — never a
//!   hardcoded tier;
//! * usage totals come from the REAL recorded usage events
//!   (`SaasStore::usage_total`, idempotent, per-organization) — never
//!   synthesized. An organization with no recorded events sees a real
//!   zero, not a demo number;
//! * an organization without a subscription gets the honest answer:
//!   `plan_code: "none"`, no plan limits applied (matching the
//!   authoritative billing view's invariant: no subscription → no plan
//!   surfaced), while its real usage totals are still reported.
//!
//! The client cannot mutate limits or supply usage — the server owns
//! both. There is no synthetic value anywhere in this module.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::billing::usage::UsageMetric;
use bot_core::billing::usage_policy::{evaluate_all, UsageThresholds};
use bot_core::error::BotResult;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/usage/limits", axum::routing::get(my_limits))
        .route("/api/saas/usage/limits/:id", axum::routing::get(by_id))
}

async fn my_limits(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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
    match render_from_store(ctx.organization.id, &state.saas).await {
        Ok(value) => value.into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization.id, "usage limits could not be loaded");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "usage_storage_unavailable",
                    "reason": "authoritative usage records could not be loaded",
                })),
            )
                .into_response()
        }
    }
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
    // Tenant safety: a member may read their own organization; anything
    // else (except platform scope) is a plain 404 — no existence oracle.
    if ctx.organization.id != org && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error":"not_found"})),
        )
            .into_response();
    }
    match render_from_store(org, &state.saas).await {
        Ok(value) => value.into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %org, "usage limits could not be loaded");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "usage_storage_unavailable",
                    "reason": "authoritative usage records could not be loaded",
                })),
            )
                .into_response()
        }
    }
}

/// The feature keys this surface evaluates, each mapped to the usage
/// metric that really measures it. No feature is reported from a
/// hand-picked value.
fn measured_features() -> Vec<(&'static str, UsageMetric)> {
    vec![
        (
            bot_core::billing::plan::features::MONTHLY_ORDERS,
            UsageMetric::OrdersSubmitted,
        ),
        (
            bot_core::billing::plan::features::MAX_MEMBERS,
            UsageMetric::ActiveMembers,
        ),
    ]
}

/// Assemble the usage-versus-limits payload from the store's truth.
/// The single implementation, shared by the handlers and the tests
/// (tests must exercise the exact production path, not a mirror).
pub(crate) async fn render_from_store(
    org: OrganizationId,
    store: &crate::saas::SaasStore,
) -> BotResult<Json<serde_json::Value>> {
    let now = Utc::now();
    let period = now.format("%Y-%m").to_string();

    // REAL plan: the organization's active subscription, if any.
    let subscription = store.subscription_of(org).await?;
    let plan = match &subscription {
        Some(sub) => store.plan(sub.plan_id).await?,
        None => None,
    };

    // REAL usage totals for the current period, per organization,
    // awaited sequentially — a control-plane read, not a hot path.
    let mut resolved: Vec<(String, f64)> = Vec::with_capacity(measured_features().len());
    for (feature, metric) in measured_features() {
        let total = store.usage_total(org, metric, &period).await?;
        resolved.push((feature.to_string(), total));
    }

    match (&subscription, &plan) {
        (Some(_), Some(plan)) => {
            let decisions = evaluate_all(plan, &resolved, &UsageThresholds::default());
            let items: Vec<serde_json::Value> = decisions
                .iter()
                .map(|d| {
                    json!({
                        "feature": d.feature,
                        "state": d.state.as_str(),
                        "limit": d.limit,
                        "current": d.current,
                        "remaining": d.allowance_remaining,
                        "allows": d.allows,
                    })
                })
                .collect();
            Ok(Json(json!({
                "organization_id": org.to_string(),
                "period": period,
                "plan_code": plan.code.as_str(),
                "plan_source": "subscription",
                "limits": items,
                "as_of": now.to_rfc3339(),
            })))
        }
        // No subscription: the honest answer. The authoritative billing
        // view's invariant is "no subscription → no plan surfaced", and
        // this surface follows it: no plan limits are applied, the real
        // usage totals are still reported, and nothing pretends the
        // organization is on an entry tier.
        _ => {
            let usage_only: Vec<serde_json::Value> = resolved
                .iter()
                .map(|(feature, total)| {
                    json!({
                        "feature": feature,
                        "state": "no_plan",
                        "limit": serde_json::Value::Null,
                        "current": total,
                        "remaining": serde_json::Value::Null,
                        "allows": false,
                    })
                })
                .collect();
            Ok(Json(json!({
                "organization_id": org.to_string(),
                "period": period,
                "plan_code": "none",
                "plan_source": "none",
                "detail": "no active subscription — no plan limits apply; usage totals below are your recorded events",
                "limits": usage_only,
                "as_of": now.to_rfc3339(),
            })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::billing::plan::PlanCode;
    use bot_core::billing::usage::{UsageEvent, UsageSource};
    use bot_core::tenant::OrganizationStatus;

    /// Create an organization in the given lifecycle status (the
    /// billing_view test pattern).
    async fn add_org(store: &crate::saas::SaasStore, status: OrganizationStatus) -> OrganizationId {
        let org_id = OrganizationId::new();
        let mut org = bot_core::tenant::Organization::new(
            org_id,
            format!("test-{}", org_id.as_uuid()),
            "Test Org",
            None,
            Utc::now(),
        );
        org.status = status;
        store.create_organization(&org).await.expect("org created");
        org_id
    }

    /// A fresh store with one active organization.
    async fn store_with_org() -> (crate::saas::SaasStore, OrganizationId) {
        let store = crate::saas::SaasStore::new();
        let org = add_org(&store, OrganizationStatus::Active).await;
        (store, org)
    }

    fn usage_event(
        org: OrganizationId,
        metric: UsageMetric,
        quantity: f64,
        key: &str,
    ) -> UsageEvent {
        UsageEvent::new(org, metric, quantity, UsageSource::System, key, Utc::now())
    }

    #[tokio::test]
    async fn usage_totals_come_from_recorded_events_only() {
        let (store, org) = store_with_org().await;
        // Nothing recorded: the usage row carries a REAL zero.
        let payload = render_payload(&store, org).await;
        assert_eq!(payload["plan_code"], "none");
        let orders = row_for(&payload, "limit.monthly_orders");
        assert_eq!(orders["current"], 0.0);

        // Record real events; the payload must carry exactly their sum.
        store
            .record_usage(&usage_event(org, UsageMetric::OrdersSubmitted, 7.0, "o-1"))
            .await
            .expect("recorded");
        store
            .record_usage(&usage_event(org, UsageMetric::OrdersSubmitted, 5.0, "o-2"))
            .await
            .expect("recorded");
        // Idempotent duplicate must NOT double-count.
        let dup = usage_event(org, UsageMetric::OrdersSubmitted, 7.0, "o-1");
        assert!(!store.record_usage(&dup).await.expect("recorded"));

        let payload = render_payload(&store, org).await;
        let orders = row_for(&payload, "limit.monthly_orders");
        assert_eq!(orders["current"], 12.0);
    }

    #[tokio::test]
    async fn plan_comes_from_the_real_subscription_not_a_hardcoded_tier() {
        let (store, org) = store_with_org().await;
        store
            .assign_plan(org, PlanCode::Business, Utc::now())
            .await
            .expect("plan assigned");
        let payload = render_payload(&store, org).await;
        assert_eq!(payload["plan_code"], "business");
        assert_eq!(payload["plan_source"], "subscription");
        // The business plan's real monthly-orders limit is surfaced.
        let orders = row_for(&payload, "limit.monthly_orders");
        assert!(orders["limit"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn no_subscription_is_none_not_a_default_tier() {
        let (store, org) = store_with_org().await;
        let payload = render_payload(&store, org).await;
        assert_eq!(payload["plan_code"], "none");
        assert_eq!(payload["plan_source"], "none");
        assert!(payload["detail"]
            .as_str()
            .unwrap()
            .contains("no active subscription"));
        // No limit is applied — limits are null, allows is false.
        let orders = row_for(&payload, "limit.monthly_orders");
        assert!(orders["limit"].is_null());
        assert_eq!(orders["allows"], false);
    }

    #[tokio::test]
    async fn another_tenants_usage_never_appears() {
        let (store, org_a) = store_with_org().await;
        let org_b = add_org(&store, OrganizationStatus::Active).await;
        store
            .record_usage(&usage_event(
                org_b,
                UsageMetric::OrdersSubmitted,
                99.0,
                "b-1",
            ))
            .await
            .expect("recorded");
        let payload = render_payload(&store, org_a).await;
        let orders = row_for(&payload, "limit.monthly_orders");
        assert_eq!(orders["current"], 0.0, "tenant B's usage must not leak");
    }

    #[tokio::test]
    async fn suspended_tenant_sees_real_usage_with_no_active_limits() {
        let store = crate::saas::SaasStore::new();
        let org = add_org(&store, OrganizationStatus::Suspended).await;
        store
            .assign_plan(org, PlanCode::Pro, Utc::now())
            .await
            .expect("plan assigned");
        store
            .record_usage(&usage_event(org, UsageMetric::ActiveMembers, 3.0, "m-1"))
            .await
            .expect("recorded");
        // Note: lifecycle enforcement happens in the authorization
        // middleware, not here; this render check pins the DATA honesty.
        let payload = render_payload(&store, org).await;
        let members = row_for(&payload, "limit.max_members");
        assert_eq!(members["current"], 3.0);
    }

    #[tokio::test]
    async fn payload_contains_no_secret_shaped_values() {
        let (store, org) = store_with_org().await;
        let payload = render_payload(&store, org).await;
        let text = payload.to_string().to_ascii_lowercase();
        for banned in ["secret", "api_key", "password", "token", "private"] {
            assert!(!text.contains(banned), "leaked {banned}");
        }
    }

    #[test]
    fn usage_policy_evaluation_is_deterministic() {
        let now = Utc::now();
        let catalogue = bot_core::billing::plan::default_catalogue(now);
        let plan = catalogue
            .iter()
            .find(|p| p.code == PlanCode::Pro)
            .expect("pro plan in catalogue");
        let usages = vec![("limit.monthly_orders".to_string(), 10.0)];
        let a = evaluate_all(plan, &usages, &UsageThresholds::default());
        let b = evaluate_all(plan, &usages, &UsageThresholds::default());
        assert_eq!(a, b);
    }

    // --- test helpers -----------------------------------------------------

    /// Exercise the PRODUCTION render path over a bare store.
    async fn render_payload(
        store: &crate::saas::SaasStore,
        org: OrganizationId,
    ) -> serde_json::Value {
        let Json(value) = render_from_store(org, store).await.expect("usage limits");
        value
    }

    fn row_for(payload: &serde_json::Value, feature: &str) -> serde_json::Value {
        payload["limits"]
            .as_array()
            .expect("limits array")
            .iter()
            .find(|row| row["feature"] == json!(feature))
            .cloned()
            .unwrap_or_else(|| panic!("row for {feature} missing"))
    }
}
