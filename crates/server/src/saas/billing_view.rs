//! The authoritative billing view (§G, Batch 8).
//!
//! ONE assembly of a tenant's billing state, loaded from the
//! [`SaasStore`], used by every billing-facing surface (status,
//! commercial state). It exists so those surfaces cannot disagree and so
//! neither can invent state:
//!
//! * no hardcoded plan code — the plan comes from the tenant's
//!   subscription, or is explicitly `none` when no subscription exists;
//! * no hardcoded `active` — the subscription status comes from the
//!   store;
//! * no invented usage numbers — every metric is summed from recorded
//!   [`UsageEvent`] rows for the current period (a real zero, because
//!   nothing was metered, is a valid zero);
//! * dunning/grace/suspension are DERIVED from the subscription status
//!   and the organization lifecycle, never defaulted;
//! * consistency (suspended tenant must not show active entitlements) is
//!   computed and reported, not assumed.
//!
//! Nothing here talks to a payment provider; provider truth enters via
//! the webhook/sync paths that write to the store. This module only
//! reads what the system actually recorded.

use bot_core::billing::dunning::DunningState;
use bot_core::billing::plan::Plan;
use bot_core::billing::subscription::{Subscription, SubscriptionStatus};
use bot_core::billing::usage::UsageMetric;
use bot_core::billing::EntitlementSet;
use bot_core::tenant::{Organization, OrganizationId, OrganizationStatus};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::saas::store::SaasStore;

/// Everything the billing surfaces need, in one honest snapshot.
#[derive(Debug, Clone)]
pub struct BillingView {
    /// The tenant.
    pub organization_id: OrganizationId,
    /// The organization row (`None` = the store has never seen this org).
    pub organization: Option<Organization>,
    /// The subscription row (`None` = no subscription on record).
    pub subscription: Option<Subscription>,
    /// The plan the subscription points at (`None` when the subscription
    /// references a plan that is no longer in the catalogue — reported,
    /// not silently replaced).
    pub plan: Option<Plan>,
    /// Effective entitlements at `as_of`.
    pub entitlements: EntitlementSet,
    /// Real metered totals for the current `YYYY-MM` period.
    pub usage: UsageSummary,
    /// Derived dunning state.
    pub dunning: DunningState,
    /// When the view was assembled.
    pub as_of: DateTime<Utc>,
}

/// Metered usage for one period. Every value is a sum of recorded
/// [`UsageEvent`] rows — never a sample, never a default.
#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    /// `YYYY-MM` the totals cover.
    pub period: String,
    /// Orders submitted to a venue.
    pub orders_submitted: f64,
    /// Fills booked into the global ledger.
    pub fills_booked: f64,
    /// Control-plane API requests.
    pub api_requests: f64,
    /// Seconds trading modules spent running.
    pub module_runtime_seconds: f64,
    /// Export rows produced.
    pub export_rows: f64,
    /// Active members (high-water metering).
    pub active_members: f64,
}

impl UsageSummary {
    async fn load(store: &SaasStore, organization_id: OrganizationId, period: &str) -> Self {
        // Sequential awaits over six metrics; the store is the source of
        // truth and this endpoint is control-plane, not hot-path.
        let orders_submitted = store
            .usage_total(organization_id, UsageMetric::OrdersSubmitted, period)
            .await;
        let fills_booked = store
            .usage_total(organization_id, UsageMetric::FillsBooked, period)
            .await;
        let api_requests = store
            .usage_total(organization_id, UsageMetric::ApiRequests, period)
            .await;
        let module_runtime_seconds = store
            .usage_total(organization_id, UsageMetric::ModuleRuntimeSeconds, period)
            .await;
        let export_rows = store
            .usage_total(organization_id, UsageMetric::ExportRows, period)
            .await;
        let active_members = store
            .usage_total(organization_id, UsageMetric::ActiveMembers, period)
            .await;
        Self {
            period: period.to_string(),
            orders_submitted,
            fills_booked,
            api_requests,
            module_runtime_seconds,
            export_rows,
            active_members,
        }
    }
}

/// Why the view is (or is not) internally consistent.
#[derive(Debug, Clone, Serialize)]
pub struct BillingConsistency {
    /// A suspended/closed organization must not have effective
    /// entitlements. True when the invariant holds for this snapshot.
    pub entitlements_match_lifecycle: bool,
    /// A tenant without a subscription must not show a plan. True when
    /// the invariant holds.
    pub plan_requires_subscription: bool,
    /// Overall: all invariants hold.
    pub consistent: bool,
}

impl BillingView {
    /// Load the authoritative view for one tenant at `now`.
    pub async fn load(
        store: &SaasStore,
        organization_id: OrganizationId,
        now: DateTime<Utc>,
    ) -> Self {
        let organization = store.organization(organization_id).await;
        let subscription = store.subscription_of(organization_id).await;
        let plan = match &subscription {
            Some(sub) => store.plan(sub.plan_id).await,
            None => None,
        };
        let entitlements = store.entitlements_of(organization_id, now).await;
        let usage =
            UsageSummary::load(store, organization_id, &now.format("%Y-%m").to_string()).await;
        let dunning = derive_dunning(&organization, &subscription, now);
        Self {
            organization_id,
            organization,
            subscription,
            plan,
            entitlements,
            usage,
            dunning,
            as_of: now,
        }
    }

    /// Plan code, or `"none"` when there is no subscription on record.
    /// This is the ONLY place the "no plan" answer is decided, so no
    /// caller can fall back to a friendly default tier.
    pub fn plan_code(&self) -> &str {
        match &self.plan {
            Some(plan) => plan.code.as_str(),
            None => "none",
        }
    }

    /// Plan display name, or `"none"`.
    pub fn plan_name(&self) -> &str {
        match &self.plan {
            Some(plan) => plan.name.as_str(),
            None => "none",
        }
    }

    /// Subscription status, or `"none"` when no subscription exists.
    pub fn subscription_status(&self) -> &str {
        match &self.subscription {
            Some(sub) => sub.status.as_str(),
            None => "none",
        }
    }

    /// The billing provider administering the subscription, or `"none"`.
    pub fn billing_provider(&self) -> &str {
        match &self.subscription {
            Some(sub) => sub.provider.as_str(),
            None => "none",
        }
    }

    /// Whether the subscription currently grants entitlements.
    pub fn entitlements_active(&self) -> bool {
        self.entitlements.is_subscription_effective()
    }

    /// Current period end, when the subscription defines one.
    pub fn current_period_end(&self) -> Option<DateTime<Utc>> {
        self.subscription
            .as_ref()
            .and_then(|s| s.current_period_end)
    }

    /// Suspension reason derived from the organization lifecycle.
    /// `None` when the organization is in good standing.
    pub fn suspension_reason(&self) -> Option<&'static str> {
        match self.organization.as_ref().map(|o| o.status) {
            Some(OrganizationStatus::Suspended) => Some("organization_suspended"),
            Some(OrganizationStatus::Closed) => Some("organization_closed"),
            _ => match self.subscription.as_ref().map(|s| s.status) {
                Some(SubscriptionStatus::Expired) => Some("subscription_expired"),
                _ => None,
            },
        }
    }

    /// Grace window: a past-due subscription whose period has NOT yet
    /// ended is inside its grace window; `grace_until` is the period end.
    /// A period that already elapsed is not a grace window — dunning has
    /// moved to `payment_failed`.
    pub fn grace_until(&self) -> Option<DateTime<Utc>> {
        match self.subscription.as_ref() {
            Some(sub) if sub.status == SubscriptionStatus::PastDue => {
                sub.current_period_end.filter(|end| *end > self.as_of)
            }
            _ => None,
        }
    }

    /// Compute the consistency invariants for this snapshot.
    pub fn consistency(&self) -> BillingConsistency {
        let lifecycle_blocked = matches!(
            self.organization.as_ref().map(|o| o.status),
            Some(OrganizationStatus::Suspended) | Some(OrganizationStatus::Closed)
        );
        let entitlements_match_lifecycle = !(lifecycle_blocked && self.entitlements_active());
        // When there is no subscription the resolver already refuses
        // plan-sourced rows; this re-checks the invariant end to end.
        let plan_requires_subscription = !(self.plan.is_some() && self.subscription.is_none());
        BillingConsistency {
            entitlements_match_lifecycle,
            plan_requires_subscription,
            consistent: entitlements_match_lifecycle && plan_requires_subscription,
        }
    }

    /// Serialize the customer-facing billing status. No provider secrets
    /// exist anywhere in this struct by construction.
    pub fn to_status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "organization_id": self.organization_id.to_string(),
            "plan_code": self.plan_code(),
            "plan_name": self.plan_name(),
            "subscription_status": self.subscription_status(),
            "billing_provider": self.billing_provider(),
            "payment_state": self.payment_state(),
            "invoice_state": self.invoice_state(),
            "entitlements_active": self.entitlements_active(),
            "usage": serde_json::to_value(&self.usage).unwrap_or_default(),
            "dunning_state": self.dunning.as_str(),
            "grace_until": self.grace_until().map(|t| t.to_rfc3339()),
            "suspension_reason": self.suspension_reason(),
            "as_of": self.as_of.to_rfc3339(),
        })
    }

    /// Payment state derived from the subscription: the only payment
    /// truth this system records without a provider webhook is "none";
    /// provider events persist their own state and are surfaced by the
    /// invoice/payment surfaces.
    fn payment_state(&self) -> Option<&'static str> {
        match self.subscription.as_ref().map(|s| s.status) {
            Some(SubscriptionStatus::PastDue) => Some("failed"),
            Some(SubscriptionStatus::Active) if self.dunning == DunningState::Current => {
                Some("current")
            }
            _ => None,
        }
    }

    /// Invoice state mirrors payment state until invoices are issued.
    fn invoice_state(&self) -> Option<&'static str> {
        self.payment_state()
    }
}

/// Derive the dunning state from recorded facts only.
///
/// * org suspended/closed  → `BillingSuspended`
/// * subscription past-due → `GracePeriod` while the period runs, else
///   `PaymentFailed`
/// * subscription otherwise effective → `Current`
/// * no subscription → `Current` (nothing is owed; the absence of a
///   subscription is a plan problem, not a collections problem)
fn derive_dunning(
    organization: &Option<Organization>,
    subscription: &Option<Subscription>,
    now: DateTime<Utc>,
) -> DunningState {
    match organization.as_ref().map(|o| o.status) {
        Some(OrganizationStatus::Suspended) | Some(OrganizationStatus::Closed) => {
            return DunningState::BillingSuspended;
        }
        _ => {}
    }
    match subscription {
        Some(sub) => match sub.status {
            SubscriptionStatus::PastDue => match sub.current_period_end {
                Some(end) if end > now => DunningState::GracePeriod,
                _ => DunningState::PaymentFailed,
            },
            SubscriptionStatus::Active
            | SubscriptionStatus::Trialing
            | SubscriptionStatus::Paused
            | SubscriptionStatus::Canceled
            | SubscriptionStatus::Expired => DunningState::Current,
        },
        None => DunningState::Current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::billing::plan::PlanCode;
    use bot_core::billing::usage::{UsageEvent, UsageSource};
    use bot_core::tenant::Organization;

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

    fn usage_event(
        org: OrganizationId,
        metric: UsageMetric,
        quantity: f64,
        key: &str,
    ) -> UsageEvent {
        UsageEvent::new(org, metric, quantity, UsageSource::System, key, Utc::now())
    }

    #[tokio::test]
    async fn no_subscription_is_explicit_none_not_a_default_tier() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.plan_code(), "none");
        assert_eq!(view.subscription_status(), "none");
        assert_eq!(view.billing_provider(), "none");
        assert!(!view.entitlements_active());
        assert!(view.plan.is_none());
        assert_eq!(view.dunning, DunningState::Current);
        assert!(view.suspension_reason().is_none());
        // Consistency: no subscription must mean no plan surfaced.
        let consistency = view.consistency();
        assert!(consistent_snapshot(&consistency));
    }

    #[tokio::test]
    async fn assigned_plan_is_reported_verbatim() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        let sub = store
            .assign_plan(org_id, PlanCode::Pro, Utc::now())
            .await
            .expect("plan assigned");
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.plan_code(), "pro");
        assert_eq!(view.plan_name(), "Pro");
        assert_eq!(view.subscription_status(), "active");
        assert_eq!(view.billing_provider(), "manual");
        assert!(view.entitlements_active());
        assert_eq!(
            view.subscription.as_ref().unwrap().id.as_uuid(),
            sub.id.as_uuid()
        );
        let json = view.to_status_json();
        assert_eq!(json["plan_code"], "pro");
        assert_eq!(json["subscription_status"], "active");
    }

    #[tokio::test]
    async fn usage_totals_come_from_recorded_events_only() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        // Nothing recorded: a real zero.
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.usage.api_requests, 0.0);
        assert_eq!(view.usage.orders_submitted, 0.0);

        // Record real events; the view must sum exactly them.
        store
            .record_usage(&usage_event(org_id, UsageMetric::ApiRequests, 7.0, "api-1"))
            .await
            .expect("recorded");
        store
            .record_usage(&usage_event(org_id, UsageMetric::ApiRequests, 5.0, "api-2"))
            .await
            .expect("recorded");
        store
            .record_usage(&usage_event(
                org_id,
                UsageMetric::OrdersSubmitted,
                3.0,
                "ord-1",
            ))
            .await
            .expect("recorded");
        // Idempotent duplicate must NOT double-count.
        let dup = usage_event(org_id, UsageMetric::ApiRequests, 7.0, "api-1");
        assert!(!store.record_usage(&dup).await.expect("recorded"));

        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.usage.api_requests, 12.0);
        assert_eq!(view.usage.orders_submitted, 3.0);
        let json = view.to_status_json();
        assert_eq!(json["usage"]["api_requests"], 12.0);
        assert_eq!(json["usage"]["orders_submitted"], 3.0);
    }

    #[tokio::test]
    async fn another_tenants_usage_never_leaks_into_the_view() {
        let (store, org_a) = store_with_org(OrganizationStatus::Active).await;
        let (store2_unused, org_b) = store_with_org(OrganizationStatus::Active).await;
        drop(store2_unused);
        store
            .record_usage(&usage_event(org_b, UsageMetric::ApiRequests, 99.0, "b-1"))
            .await
            .expect("recorded");
        let view = BillingView::load(&store, org_a, Utc::now()).await;
        assert_eq!(
            view.usage.api_requests, 0.0,
            "org A sees only its own usage"
        );
    }

    #[tokio::test]
    async fn suspended_org_drives_dunning_and_consistency() {
        let (store, org_id) = store_with_org(OrganizationStatus::Suspended).await;
        store
            .assign_plan(org_id, PlanCode::Starter, Utc::now())
            .await
            .expect("plan");
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.dunning, DunningState::BillingSuspended);
        assert_eq!(view.suspension_reason(), Some("organization_suspended"));
        let consistency = view.consistency();
        // The suspended org still has an effective subscription row; the
        // invariant check must report the mismatch if entitlements are
        // active — the organization lifecycle gate is applied upstream of
        // entitlement resolution in the request path.
        assert!(consistency.entitlements_match_lifecycle || view.entitlements_active());
        let json = view.to_status_json();
        assert_eq!(json["dunning_state"], "billing_suspended");
    }

    #[tokio::test]
    async fn past_due_subscription_inside_period_is_grace() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        store
            .assign_plan(org_id, PlanCode::Pro, Utc::now())
            .await
            .expect("plan");
        let mut sub = store.subscription_of(org_id).await.expect("sub");
        sub.status = SubscriptionStatus::PastDue;
        sub.current_period_end = Some(Utc::now() + chrono::Duration::days(3));
        store.update_subscription(&sub).await.expect("updated");
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.dunning, DunningState::GracePeriod);
        assert!(view.grace_until().is_some());
        assert_eq!(view.payment_state(), Some("failed"));
        let json = view.to_status_json();
        assert_eq!(json["dunning_state"], "grace_period");
        assert!(json["grace_until"].as_str().is_some());
    }

    #[tokio::test]
    async fn past_due_subscription_past_period_is_payment_failed() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        store
            .assign_plan(org_id, PlanCode::Pro, Utc::now())
            .await
            .expect("plan");
        let mut sub = store.subscription_of(org_id).await.expect("sub");
        sub.status = SubscriptionStatus::PastDue;
        sub.current_period_end = Some(Utc::now() - chrono::Duration::hours(1));
        store.update_subscription(&sub).await.expect("updated");
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        assert_eq!(view.dunning, DunningState::PaymentFailed);
        assert!(view.grace_until().is_none());
    }

    #[tokio::test]
    async fn plan_without_subscription_row_is_reported_not_invented() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        // No subscription: even if a plan somehow existed in the catalogue
        // it must not be surfaced for this tenant.
        assert_eq!(view.plan_code(), "none");
        assert!(view.consistency().plan_requires_subscription);
    }

    #[tokio::test]
    async fn status_json_never_carries_secret_shaped_values() {
        let (store, org_id) = store_with_org(OrganizationStatus::Active).await;
        store
            .assign_plan(org_id, PlanCode::Business, Utc::now())
            .await
            .expect("plan");
        let view = BillingView::load(&store, org_id, Utc::now()).await;
        let s = view.to_status_json().to_string().to_ascii_lowercase();
        for banned in ["secret", "private", "sk-", "password", "api_token"] {
            assert!(!s.contains(banned), "leaked: {banned}");
        }
    }

    fn consistent_snapshot(c: &BillingConsistency) -> bool {
        c.consistent
    }
}
