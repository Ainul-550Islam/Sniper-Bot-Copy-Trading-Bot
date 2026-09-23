//! Billing foundation — provider-neutral (TASK 7A file 11).
//!
//! A SaaS product needs to answer three questions that trading code must
//! never answer for itself:
//!
//! 1. **What did we sell?** → [`plan`] (catalogue, feature limits).
//! 2. **What does this tenant have?** → [`subscription`] (lifecycle,
//!    provider-neutral).
//! 3. **May this tenant use feature X, up to what?** → [`entitlement`],
//!    the ONE function every caller asks ([`EntitlementSet::check`]).
//!
//! Plus [`usage`]: append-only, idempotent metering of what was consumed.
//!
//! | file | concern |
//! |---|---|
//! | `plan.rs` | [`Plan`], [`PlanCode`], [`FeatureLimit`], the shipped feature keys, the default catalogue |
//! | `subscription.rs` | [`Subscription`], [`BillingProvider`], [`SubscriptionStatus`] |
//! | `entitlement.rs` | [`Entitlement`], source precedence, [`EntitlementSet`] and the single feature check |
//! | `usage.rs` | [`UsageMetric`], [`UsageEvent`], [`UsageLedger`] with `(tenant, idempotency_key)` identity |
//!
//! # Provider neutrality
//!
//! Nothing here imports or models Stripe, Paddle or any other processor.
//! A subscription carries a [`BillingProvider`] tag and an opaque
//! `provider_ref`; an adapter added later fills those in and changes no
//! other type. Prices, currencies, taxes and invoices are deliberately
//! absent: they belong to the processor, and putting them here would drag
//! commercial assumptions into code that decides trades.
//!
//! # Not a source of truth for money that MOVED
//!
//! The TASK 5 global ledger remains the record of realised PnL, fees and
//! positions. Billing records what the CUSTOMER owes the platform; the
//! ledger records what the market did to the customer. The two never merge.

pub mod entitlement;
pub mod plan;
pub mod subscription;
pub mod usage;

pub use entitlement::{
    Entitlement, EntitlementDenyReason, EntitlementId, EntitlementSet, EntitlementSource,
    EntitlementVerdict,
};
pub use plan::{default_catalogue, features, FeatureLimit, Plan, PlanCode, PlanId, PlanStatus};
pub use subscription::{BillingProvider, Subscription, SubscriptionId, SubscriptionStatus};
pub use usage::{UsageEvent, UsageLedger, UsageMetric, UsageOutcome, UsageSource};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::BotResult;
use crate::tenant::OrganizationId;

/// Durable billing storage.
///
/// Every tenant-owned lookup takes the [`OrganizationId`] explicitly, so a
/// caller cannot read another tenant's plan, entitlements or usage by
/// forgetting a filter.
#[async_trait]
pub trait BillingStore: Send + Sync {
    /// Upsert a catalogue plan (seeding, plan edits).
    async fn upsert_plan(&self, plan: &Plan) -> BotResult<()>;

    /// One plan by its stable code.
    async fn plan_by_code(&self, code: PlanCode) -> BotResult<Option<Plan>>;

    /// One plan by id.
    async fn plan(&self, id: PlanId) -> BotResult<Option<Plan>>;

    /// The whole catalogue.
    async fn plans(&self) -> BotResult<Vec<Plan>>;

    /// Create a subscription for a tenant.
    async fn create_subscription(&self, sub: &Subscription) -> BotResult<()>;

    /// The tenant's current subscription, if any.
    async fn subscription_of(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<Subscription>>;

    /// Persist a changed subscription (renewal, cancellation, status).
    async fn update_subscription(&self, sub: &Subscription) -> BotResult<()>;

    /// Upsert one entitlement row (plan sync, operator override, trial).
    async fn upsert_entitlement(&self, ent: &Entitlement) -> BotResult<()>;

    /// Every stored entitlement row of one tenant.
    async fn entitlements_of(&self, organization_id: OrganizationId)
        -> BotResult<Vec<Entitlement>>;

    /// Record one usage event. `Ok(true)` = counted, `Ok(false)` = the
    /// `(tenant, idempotency_key)` pair was already recorded.
    async fn record_usage(&self, event: &UsageEvent) -> BotResult<bool>;

    /// Total of one metric for one tenant in one `YYYY-MM` period.
    async fn usage_total(
        &self,
        organization_id: OrganizationId,
        metric: UsageMetric,
        period: &str,
    ) -> BotResult<f64>;
}

/// Resolve a tenant's effective entitlements from storage.
///
/// One helper so middleware, handlers and modules all resolve the same way
/// (plan → stored rows → precedence) instead of each re-implementing it.
pub async fn resolve_entitlements(
    store: &dyn BillingStore,
    organization_id: OrganizationId,
    now: DateTime<Utc>,
) -> BotResult<EntitlementSet> {
    let subscription = store.subscription_of(organization_id).await?;
    let plan = match &subscription {
        Some(s) => store.plan(s.plan_id).await?,
        None => None,
    };
    let stored = store.entitlements_of(organization_id).await?;
    Ok(EntitlementSet::resolve(
        plan.as_ref(),
        subscription.as_ref(),
        &stored,
        now,
    ))
}

/// Derive the entitlement rows a plan implies for a tenant. Used when a
/// subscription is created or the plan changes, so the durable rows match
/// the catalogue without a handler hand-writing them.
pub fn entitlements_from_plan(
    organization_id: OrganizationId,
    plan: &Plan,
    now: DateTime<Utc>,
) -> Vec<Entitlement> {
    plan.limits
        .iter()
        .map(|(feature, limit)| {
            let mut e = Entitlement::new(
                organization_id,
                feature.clone(),
                limit.limit(),
                EntitlementSource::Plan,
                now,
            );
            e.enabled = limit.is_enabled();
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rows_are_derived_faithfully() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let plan = Plan::new(PlanCode::Pro, "Pro", now)
            .with_limit(features::MODULE_SNIPER, FeatureLimit::Unlimited)
            .with_limit(features::MODULE_POLYMARKET, FeatureLimit::Disabled)
            .with_limit(features::MAX_MEMBERS, FeatureLimit::Limited(5.0));

        let rows = entitlements_from_plan(org, &plan, now);
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(r.organization_id, org);
            assert_eq!(r.source, EntitlementSource::Plan);
            assert!(r.is_active(now));
        }
        let sniper = rows
            .iter()
            .find(|r| r.feature == features::MODULE_SNIPER)
            .unwrap();
        assert!(sniper.enabled);
        assert_eq!(sniper.limit_value, None, "unlimited has no ceiling");
        let poly = rows
            .iter()
            .find(|r| r.feature == features::MODULE_POLYMARKET)
            .unwrap();
        assert!(!poly.enabled, "a disabled plan feature stays disabled");
        let members = rows
            .iter()
            .find(|r| r.feature == features::MAX_MEMBERS)
            .unwrap();
        assert_eq!(members.limit_value, Some(5.0));

        // The derived rows resolve back to the same limits.
        let sub = Subscription::manual(org, plan.id, now);
        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &rows, now);
        assert!(set.allows(features::MODULE_SNIPER));
        assert!(!set.allows(features::MODULE_POLYMARKET));
        assert_eq!(
            set.limit_for(features::MAX_MEMBERS),
            FeatureLimit::Limited(5.0)
        );
    }

    #[test]
    fn billing_never_claims_to_be_the_trading_ledger() {
        // A compile-time-ish guard: the billing module must not re-export
        // anything from the accounting domain.
        let now = Utc::now();
        let json = serde_json::to_string(&default_catalogue(now)).unwrap();
        for word in ["realized", "unrealized", "position", "fill"] {
            assert!(
                !json.to_ascii_lowercase().contains(word),
                "billing must not model trading truth: found {word}"
            );
        }
    }
}
