//! Guard 4/7: trading mode (STEP 3 file 20).
//!
//! Paper is the platform default; simulate and live must be allowed at
//! EVERY layer of the resolver's ladder (platform bounds, tenant
//! configuration, runtime degradation). The effective config already
//! computed that intersection — this guard reads it and denies with the
//! mode's stable name. Entitlement's live check ran earlier in the
//! chain; this guard is the config-side half.

use bot_core::models::ExecutionMode;

use crate::tenant_config::EffectiveTenantConfig;

use super::decision::{DenyReason, GuardOutcome};

/// Check the trading mode against the effective configuration.
pub fn check(effective: &EffectiveTenantConfig, mode: ExecutionMode) -> GuardOutcome {
    if effective.mode_allowed(mode) {
        GuardOutcome::Allow("mode_allowed")
    } else {
        GuardOutcome::Deny(DenyReason::ModeNotAllowed {
            mode: mode.as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::{resolve, GlobalSafetyBounds, TenantConfigModel};

    fn bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &[ExecutionMode::Paper, ExecutionMode::Simulate],
        }
    }

    #[test]
    fn allowed_modes_pass() {
        // A tenant that allows both platform modes.
        let tenant = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Paper, ExecutionMode::Simulate],
            ..TenantConfigModel::default()
        };
        let effective = resolve(&bounds(), Some(&tenant), None, None);
        assert!(check(&effective, ExecutionMode::Paper).is_allow());
        assert!(check(&effective, ExecutionMode::Simulate).is_allow());
    }

    #[test]
    fn an_unconfigured_tenant_is_paper_only_by_design() {
        // The deployment default fails closed: paper only.
        let effective = resolve(&bounds(), None, None, None);
        assert!(check(&effective, ExecutionMode::Paper).is_allow());
        assert!(!check(&effective, ExecutionMode::Simulate).is_allow());
    }

    #[test]
    fn a_mode_no_layer_allows_denies() {
        let effective = resolve(&bounds(), None, None, None);
        let outcome = check(&effective, ExecutionMode::Live);
        let reason = outcome.deny_reason().unwrap();
        assert_eq!(reason.as_str(), "mode_not_allowed");
        assert!(matches!(
            reason,
            DenyReason::ModeNotAllowed { mode: "LIVE" }
        ));
    }

    #[test]
    fn a_tenant_can_only_remove_modes_never_add_them() {
        let tenant = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Paper],
            ..TenantConfigModel::default()
        };
        let effective = resolve(&bounds(), Some(&tenant), None, None);
        assert!(check(&effective, ExecutionMode::Paper).is_allow());
        assert_eq!(
            check(&effective, ExecutionMode::Simulate)
                .deny_reason()
                .unwrap()
                .as_str(),
            "mode_not_allowed"
        );
    }
}
