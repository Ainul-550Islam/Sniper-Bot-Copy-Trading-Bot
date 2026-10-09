//! Guard 2/7: plan entitlements (STEP 3 file 18).
//!
//! What the tenant's PLAN pays for: which modules, and whether live
//! trading is included at all. The verdict comes from the PURE core
//! view (`bot_core::tenant::TenantEntitlementView`) so the billing
//! pipeline and the execution gateway can never disagree.

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::TenantEntitlementView;

use super::decision::{DenyReason, GuardOutcome};

/// Check the module entitlement for this request.
pub fn check_module(view: &TenantEntitlementView, module: BotModule) -> GuardOutcome {
    match view.check_module(module) {
        bot_core::tenant::EntitlementVerdict::Allow => GuardOutcome::Allow("module_entitled"),
        bot_core::tenant::EntitlementVerdict::Deny(reason) => {
            GuardOutcome::Deny(DenyReason::EntitlementModule {
                module: module.as_str(),
                reason: reason.as_str(),
            })
        }
    }
}

/// Check the mode entitlement: live trading requires the plan to allow
/// it; paper and simulate are always plan-allowed (the mode guard still
/// applies platform/config constraints).
pub fn check_mode(view: &TenantEntitlementView, mode: ExecutionMode) -> GuardOutcome {
    if mode == ExecutionMode::Live && !view.live_trading_allowed() {
        GuardOutcome::Deny(DenyReason::EntitlementLiveTrading)
    } else {
        GuardOutcome::Allow("mode_entitled")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::billing::entitlement::{Entitlement, EntitlementSet, EntitlementSource};
    use bot_core::tenant::OrganizationId;
    use chrono::Utc;

    /// An entitlement view backed by explicit override rows (they stay
    /// in force even without a subscription — the same path operators
    /// use to keep a customer working through billing problems).
    fn view(features: &[&str]) -> TenantEntitlementView {
        let org = OrganizationId::new();
        let now = Utc::now();
        let stored: Vec<Entitlement> = features
            .iter()
            .map(|f| Entitlement::new(org, *f, None, EntitlementSource::Override, now))
            .collect();
        TenantEntitlementView::new(EntitlementSet::resolve(None, None, &stored, now))
    }

    #[test]
    fn an_entitled_module_passes() {
        let v = view(&["module.copy"]);
        assert!(check_module(&v, BotModule::Copy).is_allow());
    }

    #[test]
    fn an_unentitled_module_denies() {
        let v = view(&["module.copy"]);
        let outcome = check_module(&v, BotModule::Sniper);
        let reason = outcome.deny_reason().unwrap();
        assert_eq!(reason.as_str(), "entitlement_module");
        assert!(matches!(
            reason,
            DenyReason::EntitlementModule {
                module: "sniper",
                reason: "module_not_in_plan"
            }
        ));
    }

    #[test]
    fn live_trading_requires_the_plan_feature() {
        let without = view(&["module.copy"]);
        assert_eq!(
            check_mode(&without, ExecutionMode::Live)
                .deny_reason()
                .unwrap()
                .as_str(),
            "entitlement_live_trading"
        );

        let with = view(&["module.copy", "feature.live_trading"]);
        assert!(check_mode(&with, ExecutionMode::Live).is_allow());
        // Paper is always plan-allowed.
        assert!(check_mode(&without, ExecutionMode::Paper).is_allow());
    }
}
