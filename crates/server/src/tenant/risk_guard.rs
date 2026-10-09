//! Guard 6/7: risk limits (STEP 3 file 22).
//!
//! The last economic gate before the fence: the request's size and
//! slippage against the EFFECTIVE limits (the tightest of platform
//! bounds, tenant overrides, runtime degradation and any operation
//! constraints — already computed by the config resolver). Requests
//! that do not carry a size or slippage pass this guard (nothing to
//! check); the module engines enforce per-order checks downstream with
//! the same effective numbers.

use crate::tenant_config::EffectiveTenantConfig;

use super::decision::{DenyReason, GuardOutcome};

/// Check the request's declared economics against the effective limits.
pub fn check(
    effective: &EffectiveTenantConfig,
    size_usd: Option<f64>,
    slippage_bps: Option<u32>,
) -> GuardOutcome {
    if let Some(size) = size_usd {
        if !size.is_finite() || size <= 0.0 {
            return GuardOutcome::Deny(DenyReason::RiskValuesInvalid);
        }
        if let Some(cap) = effective.risk.max_position_usd {
            if size > cap {
                return GuardOutcome::Deny(DenyReason::PositionSizeExceeds {
                    requested: size,
                    cap,
                });
            }
        }
    }
    if let Some(bps) = slippage_bps {
        if bps == 0 {
            return GuardOutcome::Deny(DenyReason::RiskValuesInvalid);
        }
        if let Some(cap_bps) = effective.risk.max_slippage_bps {
            if bps > cap_bps {
                return GuardOutcome::Deny(DenyReason::SlippageExceeds {
                    requested_bps: bps,
                    cap_bps,
                });
            }
        }
    }
    GuardOutcome::Allow("risk_within_limits")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::{resolve, GlobalSafetyBounds, TenantConfigModel};
    use bot_core::models::ExecutionMode;

    fn effective(position_cap: Option<f64>) -> EffectiveTenantConfig {
        let mut tenant = TenantConfigModel::default();
        tenant.risk.max_position_usd = position_cap;
        resolve(&bounds(), Some(&tenant), None, None)
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
    fn unspecified_economics_pass() {
        assert!(check(&effective(None), None, None).is_allow());
    }

    #[test]
    fn sizes_within_the_cap_pass() {
        assert!(check(&effective(Some(1_000.0)), Some(1_000.0), None).is_allow());
        assert!(check(&effective(Some(1_000.0)), Some(10.0), None).is_allow());
    }

    #[test]
    fn oversized_requests_deny_with_the_exact_numbers() {
        let outcome = check(&effective(Some(1_000.0)), Some(1_500.0), None);
        let reason = outcome.deny_reason().unwrap();
        assert!(matches!(
            reason,
            DenyReason::PositionSizeExceeds {
                requested: 1500.0,
                cap: 1000.0
            }
        ));
        assert_eq!(reason.as_str(), "position_size_exceeds");
    }

    #[test]
    fn the_tenant_cap_can_only_be_tighter_than_the_platform() {
        // The resolver already clamps a wider tenant value to the
        // platform bound; the guard sees only the effective number.
        let mut tenant = TenantConfigModel::default();
        tenant.risk.max_position_usd = Some(1_000_000.0);
        let effective = resolve(&bounds(), Some(&tenant), None, None);
        assert_eq!(effective.risk.max_position_usd, Some(10_000.0));
        assert!(check(&effective, Some(50_000.0), None)
            .deny_reason()
            .is_some());
    }

    #[test]
    fn garbage_values_fail_closed() {
        assert_eq!(
            check(&effective(None), Some(f64::NAN), None)
                .deny_reason()
                .unwrap()
                .as_str(),
            "risk_values_invalid"
        );
        assert_eq!(
            check(&effective(None), Some(0.0), None)
                .deny_reason()
                .unwrap()
                .as_str(),
            "risk_values_invalid"
        );
        assert_eq!(
            check(&effective(None), Some(-5.0), None)
                .deny_reason()
                .unwrap()
                .as_str(),
            "risk_values_invalid"
        );
    }

    #[test]
    fn slippage_follows_the_same_rules() {
        assert!(check(&effective(None), None, Some(50)).is_allow());
        let outcome = check(&effective(None), None, Some(600));
        assert!(matches!(
            outcome.deny_reason().unwrap(),
            DenyReason::SlippageExceeds {
                requested_bps: 600,
                cap_bps: 500
            }
        ));
        assert_eq!(
            check(&effective(None), None, Some(0))
                .deny_reason()
                .unwrap()
                .as_str(),
            "risk_values_invalid"
        );
    }
}
