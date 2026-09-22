//! Entitlements — the single answer to "may this tenant use feature X?"
//! (TASK 7A file 14).
//!
//! Without a central entitlement layer, plan checks scatter into handlers
//! (`if plan == "pro" { … }`), drift apart and become impossible to audit.
//! Here there is exactly one function, [`EntitlementSet::check`], and every
//! caller — middleware, handler, module — uses it.
//!
//! An entitlement is a grant of one feature to one tenant, from a source:
//!
//! | source | meaning | precedence |
//! |---|---|---|
//! | [`EntitlementSource::Override`] | an operator granted or revoked it explicitly | highest — an override always wins |
//! | [`EntitlementSource::Trial`] | temporary grant during an evaluation | middle |
//! | [`EntitlementSource::Plan`] | derived from the subscription's plan | base |
//!
//! Precedence is deterministic and total, so the same tenant state always
//! produces the same answer.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::plan::{FeatureLimit, Plan};
use super::subscription::Subscription;
use crate::tenant::OrganizationId;

/// An entitlement row identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntitlementId(pub Uuid);

impl EntitlementId {
    /// A fresh identifier.
    pub fn new() -> Self {
        EntitlementId(Uuid::new_v4())
    }

    /// The inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for EntitlementId {
    fn default() -> Self {
        EntitlementId::new()
    }
}

impl fmt::Display for EntitlementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a grant came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementSource {
    /// Derived from the subscription's plan.
    Plan,
    /// A temporary evaluation grant.
    Trial,
    /// An explicit operator decision; wins over everything.
    Override,
}

impl EntitlementSource {
    /// Every source, weakest first — this order IS the precedence.
    pub const ALL: [EntitlementSource; 3] = [
        EntitlementSource::Plan,
        EntitlementSource::Trial,
        EntitlementSource::Override,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntitlementSource::Plan => "plan",
            EntitlementSource::Trial => "trial",
            EntitlementSource::Override => "override",
        }
    }

    /// Inverse of [`EntitlementSource::as_str`].
    pub fn parse(s: &str) -> Option<EntitlementSource> {
        EntitlementSource::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Higher wins when two sources grant the same feature.
    pub fn precedence(&self) -> u8 {
        match self {
            EntitlementSource::Plan => 0,
            EntitlementSource::Trial => 1,
            EntitlementSource::Override => 2,
        }
    }
}

impl fmt::Display for EntitlementSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One grant of one feature to one tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entitlement {
    /// Identity.
    pub id: EntitlementId,
    /// The tenant.
    pub organization_id: OrganizationId,
    /// The feature key (see [`super::plan::features`]).
    pub feature: String,
    /// The ceiling; `None` with `enabled = true` means unlimited.
    pub limit_value: Option<f64>,
    /// Where it came from.
    pub source: EntitlementSource,
    /// `false` explicitly REVOKES the feature (an override may switch a
    /// plan grant off).
    pub enabled: bool,
    /// When the grant starts.
    pub starts_at: DateTime<Utc>,
    /// When it ends (`None` = open-ended).
    pub ends_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
}

impl Entitlement {
    /// An open-ended grant.
    pub fn new(
        organization_id: OrganizationId,
        feature: impl Into<String>,
        limit_value: Option<f64>,
        source: EntitlementSource,
        now: DateTime<Utc>,
    ) -> Self {
        Entitlement {
            id: EntitlementId::new(),
            organization_id,
            feature: feature.into(),
            limit_value,
            source,
            enabled: true,
            starts_at: now,
            ends_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Is this row in force at `now`?
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        if self.starts_at > now {
            return false;
        }
        match self.ends_at {
            Some(end) => end > now,
            None => true,
        }
    }

    /// The limit this row expresses.
    pub fn as_limit(&self) -> FeatureLimit {
        if !self.enabled {
            FeatureLimit::Disabled
        } else {
            match self.limit_value {
                Some(v) => FeatureLimit::Limited(v),
                None => FeatureLimit::Unlimited,
            }
        }
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "entitlement={} organization={} feature={} source={} enabled={} limit={}",
            self.id,
            self.organization_id,
            self.feature,
            self.source,
            self.enabled,
            self.limit_value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unlimited".into())
        )
    }
}

/// Why a feature check failed. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementDenyReason {
    /// The plan does not include the feature (or an override turned it off).
    NotEntitled,
    /// Entitled, but the requested amount exceeds the limit.
    LimitExceeded,
    /// The subscription is not granting entitlements (paused / cancelled /
    /// expired), or there is no subscription at all.
    NoActiveSubscription,
}

impl EntitlementDenyReason {
    /// Every reason, stable order.
    pub const ALL: [EntitlementDenyReason; 3] = [
        EntitlementDenyReason::NotEntitled,
        EntitlementDenyReason::LimitExceeded,
        EntitlementDenyReason::NoActiveSubscription,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntitlementDenyReason::NotEntitled => "not_entitled",
            EntitlementDenyReason::LimitExceeded => "limit_exceeded",
            EntitlementDenyReason::NoActiveSubscription => "no_active_subscription",
        }
    }

    /// Inverse of [`EntitlementDenyReason::as_str`].
    pub fn parse(s: &str) -> Option<EntitlementDenyReason> {
        EntitlementDenyReason::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }
}

impl fmt::Display for EntitlementDenyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The verdict of a feature check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntitlementVerdict {
    /// Allowed, with the limit that applied.
    Allow(FeatureLimit),
    /// Refused.
    Deny(EntitlementDenyReason),
}

impl EntitlementVerdict {
    /// True on allow.
    pub fn is_allowed(&self) -> bool {
        matches!(self, EntitlementVerdict::Allow(_))
    }

    /// The reason on deny.
    pub fn reason(&self) -> Option<EntitlementDenyReason> {
        match self {
            EntitlementVerdict::Allow(_) => None,
            EntitlementVerdict::Deny(r) => Some(*r),
        }
    }
}

/// One tenant's effective entitlements: the winning row per feature.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntitlementSet {
    effective: BTreeMap<String, Entitlement>,
    /// False when no subscription is granting entitlements right now.
    subscription_effective: bool,
}

impl EntitlementSet {
    /// Resolve a tenant's entitlements at `now`.
    ///
    /// The plan's limits become base `Plan` rows; stored rows are layered
    /// on top by [`EntitlementSource::precedence`]. When the subscription
    /// is not effective, plan-derived rows are dropped — but explicit
    /// overrides survive, so an operator can keep a customer working
    /// through a billing problem.
    pub fn resolve(
        plan: Option<&Plan>,
        subscription: Option<&Subscription>,
        stored: &[Entitlement],
        now: DateTime<Utc>,
    ) -> Self {
        let subscription_effective = subscription.map(|s| s.is_effective(now)).unwrap_or(false);
        let mut effective: BTreeMap<String, Entitlement> = BTreeMap::new();

        if subscription_effective {
            if let (Some(plan), Some(sub)) = (plan, subscription) {
                for (feature, limit) in &plan.limits {
                    let mut e = Entitlement::new(
                        sub.organization_id,
                        feature.clone(),
                        limit.limit(),
                        EntitlementSource::Plan,
                        now,
                    );
                    e.enabled = limit.is_enabled();
                    effective.insert(feature.clone(), e);
                }
            }
        }

        for row in stored {
            if !row.is_active(now) {
                continue;
            }
            if row.source == EntitlementSource::Plan && !subscription_effective {
                continue;
            }
            match effective.get(&row.feature) {
                Some(existing) if existing.source.precedence() > row.source.precedence() => {}
                _ => {
                    effective.insert(row.feature.clone(), row.clone());
                }
            }
        }

        EntitlementSet {
            effective,
            subscription_effective,
        }
    }

    /// The winning limit for `feature` (`Disabled` when nothing grants it).
    pub fn limit_for(&self, feature: &str) -> FeatureLimit {
        self.effective
            .get(feature)
            .map(|e| e.as_limit())
            .unwrap_or(FeatureLimit::Disabled)
    }

    /// The single feature check every caller uses.
    ///
    /// `current` / `requested` are for counted limits (members, API keys,
    /// active modules); pass `0.0` / `0.0` for a pure on/off feature.
    pub fn check(&self, feature: &str, current: f64, requested: f64) -> EntitlementVerdict {
        let limit = self.limit_for(feature);
        if !limit.is_enabled() {
            // Distinguish "your plan lacks it" from "you have no plan".
            return if self.effective.contains_key(feature) || self.subscription_effective {
                EntitlementVerdict::Deny(EntitlementDenyReason::NotEntitled)
            } else {
                EntitlementVerdict::Deny(EntitlementDenyReason::NoActiveSubscription)
            };
        }
        if limit.allows(current, requested) {
            EntitlementVerdict::Allow(limit)
        } else {
            EntitlementVerdict::Deny(EntitlementDenyReason::LimitExceeded)
        }
    }

    /// Convenience for on/off features.
    pub fn allows(&self, feature: &str) -> bool {
        self.check(feature, 0.0, 0.0).is_allowed()
    }

    /// Is a subscription currently granting plan entitlements?
    pub fn has_active_subscription(&self) -> bool {
        self.subscription_effective
    }

    /// Every effective row, sorted by feature (stable API output).
    pub fn rows(&self) -> impl Iterator<Item = &Entitlement> {
        self.effective.values()
    }

    /// How many features are resolved.
    pub fn len(&self) -> usize {
        self.effective.len()
    }

    /// True when nothing is granted.
    pub fn is_empty(&self) -> bool {
        self.effective.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billing::plan::{features, PlanCode};
    use crate::billing::subscription::{Subscription, SubscriptionStatus};
    use chrono::Duration;

    fn setup(now: DateTime<Utc>) -> (Plan, Subscription) {
        let plan = Plan::new(PlanCode::Pro, "Pro", now)
            .with_limit(features::MODULE_SNIPER, FeatureLimit::Unlimited)
            .with_limit(features::MODULE_POLYMARKET, FeatureLimit::Disabled)
            .with_limit(features::MAX_MEMBERS, FeatureLimit::Limited(5.0));
        let sub = Subscription::manual(OrganizationId::new(), plan.id, now);
        (plan, sub)
    }

    #[test]
    fn plan_limits_become_entitlements() {
        let now = Utc::now();
        let (plan, sub) = setup(now);
        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &[], now);
        assert!(set.allows(features::MODULE_SNIPER));
        assert!(!set.allows(features::MODULE_POLYMARKET));
        assert!(set.has_active_subscription());
        assert_eq!(set.len(), 3);
        // Counted limit.
        assert!(set.check(features::MAX_MEMBERS, 4.0, 1.0).is_allowed());
        assert_eq!(
            set.check(features::MAX_MEMBERS, 5.0, 1.0).reason(),
            Some(EntitlementDenyReason::LimitExceeded)
        );
        // Unknown feature is never granted by omission.
        assert_eq!(
            set.check("feature.invented", 0.0, 0.0).reason(),
            Some(EntitlementDenyReason::NotEntitled)
        );
    }

    #[test]
    fn overrides_beat_the_plan_in_both_directions() {
        let now = Utc::now();
        let (plan, sub) = setup(now);
        // Grant something the plan disables.
        let grant = Entitlement::new(
            sub.organization_id,
            features::MODULE_POLYMARKET,
            None,
            EntitlementSource::Override,
            now,
        );
        // Revoke something the plan allows.
        let mut revoke = Entitlement::new(
            sub.organization_id,
            features::MODULE_SNIPER,
            None,
            EntitlementSource::Override,
            now,
        );
        revoke.enabled = false;

        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &[grant, revoke], now);
        assert!(set.allows(features::MODULE_POLYMARKET), "override grants");
        assert!(!set.allows(features::MODULE_SNIPER), "override revokes");
    }

    #[test]
    fn trial_beats_plan_but_loses_to_override() {
        let now = Utc::now();
        let (plan, sub) = setup(now);
        let trial = Entitlement::new(
            sub.organization_id,
            features::MAX_MEMBERS,
            Some(50.0),
            EntitlementSource::Trial,
            now,
        );
        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &[trial.clone()], now);
        assert_eq!(set.limit_for(features::MAX_MEMBERS), FeatureLimit::Limited(50.0));

        let over = Entitlement::new(
            sub.organization_id,
            features::MAX_MEMBERS,
            Some(3.0),
            EntitlementSource::Override,
            now,
        );
        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &[trial, over], now);
        assert_eq!(
            set.limit_for(features::MAX_MEMBERS),
            FeatureLimit::Limited(3.0),
            "override wins regardless of order"
        );
    }

    #[test]
    fn expired_rows_and_future_rows_do_not_apply() {
        let now = Utc::now();
        let (plan, sub) = setup(now);
        let mut expired = Entitlement::new(
            sub.organization_id,
            features::MODULE_POLYMARKET,
            None,
            EntitlementSource::Trial,
            now - Duration::days(10),
        );
        expired.ends_at = Some(now - Duration::days(1));
        let mut future = Entitlement::new(
            sub.organization_id,
            features::EXPORTS,
            None,
            EntitlementSource::Override,
            now + Duration::days(1),
        );
        future.starts_at = now + Duration::days(1);

        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &[expired, future], now);
        assert!(!set.allows(features::MODULE_POLYMARKET));
        assert!(!set.allows(features::EXPORTS));
    }

    #[test]
    fn without_an_effective_subscription_only_overrides_survive() {
        let now = Utc::now();
        let (plan, mut sub) = setup(now);
        sub.status = SubscriptionStatus::Expired;
        let keep_working = Entitlement::new(
            sub.organization_id,
            features::MODULE_SNIPER,
            None,
            EntitlementSource::Override,
            now,
        );
        let set = EntitlementSet::resolve(Some(&plan), Some(&sub), &[keep_working], now);
        assert!(!set.has_active_subscription());
        assert!(
            set.allows(features::MODULE_SNIPER),
            "an operator override keeps a customer working through a billing problem"
        );
        assert!(!set.allows(features::MAX_MEMBERS), "plan rows are gone");
        assert_eq!(
            set.check(features::MAX_MEMBERS, 0.0, 1.0).reason(),
            Some(EntitlementDenyReason::NoActiveSubscription)
        );
    }

    #[test]
    fn no_subscription_at_all_denies_with_the_right_reason() {
        let now = Utc::now();
        let set = EntitlementSet::resolve(None, None, &[], now);
        assert!(set.is_empty());
        assert!(!set.has_active_subscription());
        assert_eq!(
            set.check(features::MODULE_SNIPER, 0.0, 0.0).reason(),
            Some(EntitlementDenyReason::NoActiveSubscription)
        );
    }

    #[test]
    fn vocabularies_round_trip() {
        for s in EntitlementSource::ALL {
            assert_eq!(EntitlementSource::parse(s.as_str()), Some(s));
        }
        for r in EntitlementDenyReason::ALL {
            assert_eq!(EntitlementDenyReason::parse(r.as_str()), Some(r));
        }
        assert!(
            EntitlementSource::Override.precedence()
                > EntitlementSource::Trial.precedence()
        );
        assert!(
            EntitlementSource::Trial.precedence() > EntitlementSource::Plan.precedence()
        );
    }
}
