//! Subscriptions (TASK 7A file 13).
//!
//! What a tenant currently has. Provider-neutral by construction: the
//! record names a [`BillingProvider`] and an opaque `provider_ref`, so a
//! Stripe or Paddle adapter can arrive later and populate those two fields
//! without changing the domain, the database schema or any caller.
//!
//! Today the only implemented provider is [`BillingProvider::Manual`] —
//! an operator assigns a plan. That is deliberate: TASK 7A builds the
//! foundation, not the payment integration.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::plan::PlanId;
use crate::tenant::OrganizationId;

/// A subscription identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubscriptionId(pub Uuid);

impl SubscriptionId {
    /// A fresh identifier.
    pub fn new() -> Self {
        SubscriptionId(Uuid::new_v4())
    }

    /// The inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Parse the canonical string form.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(SubscriptionId)
    }
}

impl Default for SubscriptionId {
    fn default() -> Self {
        SubscriptionId::new()
    }
}

impl fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where the subscription is administered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingProvider {
    /// An operator assigned the plan directly (the only one implemented in
    /// TASK 7A).
    Manual,
    /// Reserved for the future Stripe adapter.
    Stripe,
    /// Reserved for the future Paddle adapter.
    Paddle,
}

impl BillingProvider {
    /// Every provider, stable order.
    pub const ALL: [BillingProvider; 3] = [
        BillingProvider::Manual,
        BillingProvider::Stripe,
        BillingProvider::Paddle,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            BillingProvider::Manual => "manual",
            BillingProvider::Stripe => "stripe",
            BillingProvider::Paddle => "paddle",
        }
    }

    /// Inverse of [`BillingProvider::as_str`].
    pub fn parse(s: &str) -> Option<BillingProvider> {
        BillingProvider::ALL
            .iter()
            .copied()
            .find(|p| p.as_str() == s.trim())
    }

    /// Is an adapter implemented for this provider in this build?
    pub fn is_implemented(&self) -> bool {
        matches!(self, BillingProvider::Manual)
    }
}

impl fmt::Display for BillingProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle of a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    /// Inside a trial window; entitlements apply.
    Trialing,
    /// Paid and current.
    Active,
    /// Payment failed; the tenant keeps read access (see
    /// [`crate::tenant::policy`]).
    PastDue,
    /// Temporarily paused by the customer.
    Paused,
    /// Cancelled; entitlements stop at the period end.
    Canceled,
    /// The period ended without renewal; terminal.
    Expired,
}

impl SubscriptionStatus {
    /// Every status, stable order.
    pub const ALL: [SubscriptionStatus; 6] = [
        SubscriptionStatus::Trialing,
        SubscriptionStatus::Active,
        SubscriptionStatus::PastDue,
        SubscriptionStatus::Paused,
        SubscriptionStatus::Canceled,
        SubscriptionStatus::Expired,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SubscriptionStatus::Trialing => "trialing",
            SubscriptionStatus::Active => "active",
            SubscriptionStatus::PastDue => "past_due",
            SubscriptionStatus::Paused => "paused",
            SubscriptionStatus::Canceled => "canceled",
            SubscriptionStatus::Expired => "expired",
        }
    }

    /// Inverse of [`SubscriptionStatus::as_str`].
    pub fn parse(s: &str) -> Option<SubscriptionStatus> {
        SubscriptionStatus::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Do the plan's entitlements apply while in this state?
    ///
    /// `PastDue` deliberately still grants them: the tenant-status gate
    /// already blocks new trading and management, and revoking feature
    /// access as well would stop the customer from reading their own
    /// positions or paying the invoice.
    pub fn grants_entitlements(&self) -> bool {
        matches!(
            self,
            SubscriptionStatus::Trialing
                | SubscriptionStatus::Active
                | SubscriptionStatus::PastDue
        )
    }

    /// Terminal states never come back.
    pub fn is_terminal(&self) -> bool {
        matches!(self, SubscriptionStatus::Expired)
    }
}

impl fmt::Display for SubscriptionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a tenant currently has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    /// Identity.
    pub id: SubscriptionId,
    /// The tenant.
    pub organization_id: OrganizationId,
    /// The plan.
    pub plan_id: PlanId,
    /// Where it is administered.
    pub provider: BillingProvider,
    /// The provider's own id, when there is one.
    pub provider_ref: Option<String>,
    /// Lifecycle.
    pub status: SubscriptionStatus,
    /// Start of the current period.
    pub current_period_start: DateTime<Utc>,
    /// End of the current period (`None` = open-ended / manual).
    pub current_period_end: Option<DateTime<Utc>>,
    /// Cancel when the period ends instead of immediately.
    pub cancel_at_period_end: bool,
    /// When cancellation was requested.
    pub canceled_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
}

impl Subscription {
    /// A manual, active subscription starting now.
    pub fn manual(
        organization_id: OrganizationId,
        plan_id: PlanId,
        now: DateTime<Utc>,
    ) -> Self {
        Subscription {
            id: SubscriptionId::new(),
            organization_id,
            plan_id,
            provider: BillingProvider::Manual,
            provider_ref: None,
            status: SubscriptionStatus::Active,
            current_period_start: now,
            current_period_end: None,
            cancel_at_period_end: false,
            canceled_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Is the subscription granting its plan's entitlements at `now`?
    ///
    /// Both conditions must hold: the status grants entitlements, and the
    /// current period has not ended.
    pub fn is_effective(&self, now: DateTime<Utc>) -> bool {
        if !self.status.grants_entitlements() {
            return false;
        }
        match self.current_period_end {
            Some(end) => end > now,
            None => true,
        }
    }

    /// Request cancellation. `at_period_end = false` ends it immediately.
    pub fn cancel(&mut self, at_period_end: bool, now: DateTime<Utc>) {
        self.cancel_at_period_end = at_period_end;
        self.canceled_at = Some(now);
        self.updated_at = now;
        if !at_period_end {
            self.status = SubscriptionStatus::Canceled;
            self.current_period_end = Some(now);
        }
    }

    /// Advance the period (renewal). Clears a past-due state.
    pub fn renew(&mut self, period_end: Option<DateTime<Utc>>, now: DateTime<Utc>) {
        self.current_period_start = now;
        self.current_period_end = period_end;
        if self.status == SubscriptionStatus::PastDue {
            self.status = SubscriptionStatus::Active;
        }
        self.updated_at = now;
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "subscription={} organization={} plan={} provider={} status={} cancel_at_period_end={}",
            self.id,
            self.organization_id,
            self.plan_id,
            self.provider,
            self.status,
            self.cancel_at_period_end
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn sub(now: DateTime<Utc>) -> Subscription {
        Subscription::manual(OrganizationId::new(), PlanId::new(), now)
    }

    #[test]
    fn vocabularies_round_trip() {
        for p in BillingProvider::ALL {
            assert_eq!(BillingProvider::parse(p.as_str()), Some(p));
        }
        for s in SubscriptionStatus::ALL {
            assert_eq!(SubscriptionStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(BillingProvider::parse("bitcoin"), None);
        assert!(BillingProvider::Manual.is_implemented());
        assert!(
            !BillingProvider::Stripe.is_implemented(),
            "TASK 7A ships the foundation, not the adapter"
        );
    }

    #[test]
    fn entitlement_granting_states_are_explicit() {
        assert!(SubscriptionStatus::Active.grants_entitlements());
        assert!(SubscriptionStatus::Trialing.grants_entitlements());
        assert!(
            SubscriptionStatus::PastDue.grants_entitlements(),
            "past due keeps feature access; the tenant gate stops new trading"
        );
        for s in [
            SubscriptionStatus::Paused,
            SubscriptionStatus::Canceled,
            SubscriptionStatus::Expired,
        ] {
            assert!(!s.grants_entitlements(), "{s}");
        }
        assert!(SubscriptionStatus::Expired.is_terminal());
    }

    #[test]
    fn effectiveness_considers_status_and_period() {
        let t0 = Utc::now();
        let mut s = sub(t0);
        assert!(s.is_effective(t0 + Duration::days(365)), "manual is open-ended");

        s.current_period_end = Some(t0 + Duration::days(30));
        assert!(s.is_effective(t0 + Duration::days(29)));
        assert!(!s.is_effective(t0 + Duration::days(31)), "period ended");

        s.current_period_end = None;
        s.status = SubscriptionStatus::Paused;
        assert!(!s.is_effective(t0), "paused grants nothing");
    }

    #[test]
    fn cancellation_modes_differ() {
        let t0 = Utc::now();

        let mut immediate = sub(t0);
        immediate.cancel(false, t0);
        assert_eq!(immediate.status, SubscriptionStatus::Canceled);
        assert!(!immediate.is_effective(t0));
        assert_eq!(immediate.canceled_at, Some(t0));

        let mut at_end = sub(t0);
        at_end.current_period_end = Some(t0 + Duration::days(10));
        at_end.cancel(true, t0);
        assert_eq!(at_end.status, SubscriptionStatus::Active, "still active until the end");
        assert!(at_end.cancel_at_period_end);
        assert!(at_end.is_effective(t0 + Duration::days(9)));
        assert!(!at_end.is_effective(t0 + Duration::days(11)));
    }

    #[test]
    fn renewal_clears_past_due() {
        let t0 = Utc::now();
        let mut s = sub(t0);
        s.status = SubscriptionStatus::PastDue;
        s.current_period_end = Some(t0);
        assert!(!s.is_effective(t0 + Duration::seconds(1)));
        s.renew(Some(t0 + Duration::days(30)), t0);
        assert_eq!(s.status, SubscriptionStatus::Active);
        assert!(s.is_effective(t0 + Duration::days(1)));
    }

    #[test]
    fn the_record_is_provider_neutral() {
        let s = sub(Utc::now());
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"provider\":\"manual\""));
        assert!(json.contains("\"provider_ref\":null"));
        // A future adapter only fills these two fields in.
        let mut stripe = s.clone();
        stripe.provider = BillingProvider::Stripe;
        stripe.provider_ref = Some("sub_123".into());
        assert_eq!(stripe.plan_id, s.plan_id, "the domain does not change");
    }
}
