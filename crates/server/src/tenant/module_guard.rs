//! Guard 3/7: module enablement under the tenant's configuration
//! (STEP 3 file 19).
//!
//! Distinct from the entitlement guard: entitlements are what the PLAN
//! provides; this guard is what the tenant's OWN configuration (or an
//! operator, or a runtime degradation) currently allows. Both must say
//! yes — the gateway runs them in order, entitlement first.

use bot_core::models::BotModule;

use crate::tenant_config::EffectiveTenantConfig;

use super::decision::{DenyReason, GuardOutcome};

/// Check module enablement under the resolved (effective) config.
pub fn check(effective: &EffectiveTenantConfig, module: BotModule) -> GuardOutcome {
    match effective.module_enabled(module) {
        bot_core::tenant::ModuleEnablement::Enabled => GuardOutcome::Allow("module_enabled"),
        bot_core::tenant::ModuleEnablement::Disabled(reason) => {
            GuardOutcome::Deny(DenyReason::ModuleDisabled {
                module: module.as_str(),
                reason: reason.as_str(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::{resolve, GlobalSafetyBounds, TenantConfigModel};
    use bot_core::models::ExecutionMode;
    use bot_core::tenant::ModuleDisableReason;

    fn effective_with(disabled: Option<(BotModule, ModuleDisableReason)>) -> EffectiveTenantConfig {
        let mut config = TenantConfigModel::default();
        if let Some((module, reason)) = disabled {
            config.disable_module(module, reason);
        }
        resolve(&bounds(), Some(&config), None, None)
    }

    fn bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &[ExecutionMode::Paper],
        }
    }

    #[test]
    fn an_enabled_module_passes() {
        assert!(check(&effective_with(None), BotModule::Sniper).is_allow());
    }

    #[test]
    fn a_disabled_module_denies_with_the_reason() {
        let outcome = check(
            &effective_with(Some((BotModule::Polymarket, ModuleDisableReason::Operator))),
            BotModule::Polymarket,
        );
        let reason = outcome.deny_reason().unwrap();
        assert_eq!(reason.as_str(), "module_disabled");
        assert!(matches!(
            reason,
            DenyReason::ModuleDisabled { module, reason: "operator" } if *module == "polymarket"
        ));
    }

    #[test]
    fn runtime_degradation_reaches_this_guard_through_the_config() {
        use crate::tenant_config::RuntimeOverrides;
        use bot_core::tenant::ModuleKind;

        const DEGRADED: [ModuleKind; 1] = [ModuleKind::Copy];
        let runtime = RuntimeOverrides {
            degraded_modules: &DEGRADED,
            ..RuntimeOverrides::default()
        };
        let effective = resolve(&bounds(), None, Some(&runtime), None);
        assert_eq!(
            check(&effective, BotModule::Copy)
                .deny_reason()
                .unwrap()
                .as_str(),
            "module_disabled"
        );
    }
}
