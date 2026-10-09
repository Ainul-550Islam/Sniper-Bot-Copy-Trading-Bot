//! Effective tenant entitlement — domain view (STEP 3 file 07).
//!
//! Billing truth lives in [`crate::billing`]: the plan catalogue, the
//! subscription, the entitlement rows and their precedence. This module
//! does NOT duplicate any of it — it is the narrow execution-side view
//! the guard chain needs: "is THIS module in THIS tenant's effective
//! entitlement set, and may the tenant trade live?".
//!
//! The conversion is total and one-way: from the billing
//! [`EntitlementSet`] to a guard verdict. Nothing here can grant,
//! revoke or persist entitlements.

use crate::billing::entitlement::EntitlementSet;
use crate::billing::features;
use crate::billing::plan::FeatureLimit;

use super::module_kind::{self, ModuleKind};

/// Why an entitlement check refused. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntitlementDenyReason {
    /// The module's plan feature is not granted (or is explicitly
    /// disabled) for this tenant.
    ModuleNotInPlan,
    /// The module is not plan-gated at all, but the caller demanded an
    /// entitlement check for it — a programming error, fail closed.
    NotPlanGated,
}

impl EntitlementDenyReason {
    /// Stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            EntitlementDenyReason::ModuleNotInPlan => "module_not_in_plan",
            EntitlementDenyReason::NotPlanGated => "not_plan_gated",
        }
    }

    /// Human explanation (safe to surface).
    pub fn detail(self) -> &'static str {
        match self {
            EntitlementDenyReason::ModuleNotInPlan => {
                "the module is not included in the organization's plan"
            }
            EntitlementDenyReason::NotPlanGated => {
                "the module is not plan-gated; an entitlement check is not applicable"
            }
        }
    }
}

/// The verdict of an entitlement gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementVerdict {
    /// The tenant's effective entitlements include the module.
    Allow,
    /// Refused, with the reason.
    Deny(EntitlementDenyReason),
}

impl EntitlementVerdict {
    /// True on allow.
    pub fn is_allowed(self) -> bool {
        matches!(self, EntitlementVerdict::Allow)
    }
}

/// The execution-side view of one tenant's effective entitlements.
#[derive(Debug, Clone)]
pub struct TenantEntitlementView {
    set: EntitlementSet,
}

impl TenantEntitlementView {
    /// Wrap the billing layer's effective set (as produced by
    /// `SaasStore::entitlements_of`).
    pub fn new(set: EntitlementSet) -> Self {
        TenantEntitlementView { set }
    }

    /// The effective limit for a plan feature.
    pub fn limit_for(&self, feature: &str) -> FeatureLimit {
        self.set.limit_for(feature)
    }

    /// Is a plan feature granted at all?
    pub fn has_feature(&self, feature: &str) -> bool {
        !matches!(self.set.limit_for(feature), FeatureLimit::Disabled)
    }

    /// Module gate: is this trading module in the tenant's plan?
    ///
    /// * Plan-gated modules (sniper/copy/polymarket): granted → Allow,
    ///   disabled/absent → Deny(ModuleNotInPlan).
    /// * Non-plan-gated modules (telegram, contract): the caller must use
    ///   [`TenantEntitlementView::check_module_configured`] instead — an
    ///   entitlement verdict does not apply, and this method fails closed
    ///   with [`EntitlementDenyReason::NotPlanGated`].
    pub fn check_module(&self, module: ModuleKind) -> EntitlementVerdict {
        match module_kind::feature_key(module) {
            Some(feature) => match self.set.limit_for(feature) {
                FeatureLimit::Disabled => {
                    EntitlementVerdict::Deny(EntitlementDenyReason::ModuleNotInPlan)
                }
                FeatureLimit::Limited(_) | FeatureLimit::Unlimited => EntitlementVerdict::Allow,
            },
            None => EntitlementVerdict::Deny(EntitlementDenyReason::NotPlanGated),
        }
    }

    /// Non-plan-gated modules are governed by configuration, not billing:
    /// this helper reports that explicitly so callers route them through
    /// the module-state guard instead of inventing an entitlement answer.
    pub fn check_module_configured(&self, module: ModuleKind) -> EntitlementVerdict {
        match module_kind::feature_key(module) {
            Some(_) => self.check_module(module),
            None => EntitlementVerdict::Allow,
        }
    }

    /// Live-trading gate: the `feature.live_trading` entitlement.
    pub fn live_trading_allowed(&self) -> bool {
        self.has_feature(features::LIVE_TRADING)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billing::plan::{Plan, PlanCode};
    use crate::billing::subscription::Subscription;
    use crate::tenant::OrganizationId;
    use chrono::Utc;

    fn set_with(feature: &str, limit: FeatureLimit) -> EntitlementSet {
        let now = Utc::now();
        let plan = Plan::new(PlanCode::Pro, "Pro", now).with_limit(feature, limit);
        let sub = Subscription::manual(OrganizationId::new(), plan.id, now);
        EntitlementSet::resolve(Some(&plan), Some(&sub), &[], now)
    }

    fn empty_set() -> EntitlementSet {
        EntitlementSet::default()
    }

    #[test]
    fn plan_gated_module_granted() {
        let view =
            TenantEntitlementView::new(set_with(features::MODULE_SNIPER, FeatureLimit::Unlimited));
        assert!(view.check_module(ModuleKind::Sniper).is_allowed());
        assert!(view.has_feature(features::MODULE_SNIPER));
    }

    #[test]
    fn plan_gated_module_denied_when_disabled_or_absent() {
        let disabled =
            TenantEntitlementView::new(set_with(features::MODULE_SNIPER, FeatureLimit::Disabled));
        assert_eq!(
            disabled.check_module(ModuleKind::Sniper),
            EntitlementVerdict::Deny(EntitlementDenyReason::ModuleNotInPlan)
        );
        let absent = TenantEntitlementView::new(empty_set());
        assert_eq!(
            absent.check_module(ModuleKind::Copy),
            EntitlementVerdict::Deny(EntitlementDenyReason::ModuleNotInPlan)
        );
    }

    #[test]
    fn limited_grant_still_allows_the_gate() {
        // A monthly-order limit is a usage cap, not an on/off switch: the
        // module gate passes; usage accounting enforces the number.
        let view = TenantEntitlementView::new(set_with(
            features::MODULE_POLYMARKET,
            FeatureLimit::Limited(100.0),
        ));
        assert!(view.check_module(ModuleKind::Polymarket).is_allowed());
    }

    #[test]
    fn non_plan_gated_module_fails_closed_in_entitlement_gate() {
        let view = TenantEntitlementView::new(empty_set());
        assert_eq!(
            view.check_module(ModuleKind::Telegram),
            EntitlementVerdict::Deny(EntitlementDenyReason::NotPlanGated)
        );
        // …and the configured-path helper routes it to Allow so callers
        // do not treat the control plane as unentitled.
        assert!(view
            .check_module_configured(ModuleKind::Telegram)
            .is_allowed());
    }

    #[test]
    fn live_trading_follows_the_feature_flag() {
        let on =
            TenantEntitlementView::new(set_with(features::LIVE_TRADING, FeatureLimit::Unlimited));
        assert!(on.live_trading_allowed());
        let off = TenantEntitlementView::new(empty_set());
        assert!(!off.live_trading_allowed());
    }
}
