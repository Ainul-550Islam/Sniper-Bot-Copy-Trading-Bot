//! Effective configuration resolution (STEP 3 file 30).
//!
//! The precedence ladder, top wins, no exceptions:
//!
//! ```text
//! global immutable safety bounds        (platform, cannot be overridden)
//!        ↓ constrains
//! tenant configuration                  (this tenant's document)
//!        ↓ constrains
//! runtime effective configuration       (degradation, e.g. custody outage)
//!        ↓ constrains
//! operation-specific constraints        (per order/job)
//! ```
//!
//! Every numeric limit takes the TIGHTEST value along the ladder; every
//! boolean permission is the AND of the ladder. A tenant can only ever
//! NARROW what the platform allows — never widen it.

use bot_core::models::ExecutionMode;
use bot_core::tenant::{
    ModuleDisableReason, ModuleEnablement, ModuleKind, TenantModuleSet, TenantModuleState,
};

use super::model::{TenantConfigModel, TenantRiskLimits};

/// The platform's immutable safety bounds. Derived from the global
/// deployment configuration; supplied by the caller (the service wiring)
/// so this module stays pure and testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalSafetyBounds {
    /// The largest single position ANY tenant may open (USD).
    pub max_position_usd: f64,
    /// The largest daily loss cap ANY tenant may set (USD).
    pub daily_loss_usd_cap: f64,
    /// The largest slippage ANY tenant may accept (bps).
    pub max_slippage_bps: u32,
    /// The trading modes the PLATFORM allows (e.g. live disabled
    /// deployment-wide). A tenant may only remove modes, never add them.
    pub allowed_modes: &'static [ExecutionMode],
}

impl GlobalSafetyBounds {
    /// The documented hard platform defaults (used when the global
    /// config does not define explicit bounds).
    pub fn platform_defaults() -> Self {
        GlobalSafetyBounds {
            max_position_usd: 100_000.0,
            daily_loss_usd_cap: 50_000.0,
            max_slippage_bps: 2_000,
            allowed_modes: &[ExecutionMode::Paper, ExecutionMode::Simulate],
        }
    }
}

/// Runtime-level degradation a resolver caller may inject (e.g. custody
/// provider unreachable → tighten risk, drop live mode).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RuntimeOverrides {
    /// Further cap the position size.
    pub max_position_usd: Option<f64>,
    /// Further cap the daily loss.
    pub daily_loss_usd_cap: Option<f64>,
    /// Modules the runtime degraded (recorded with the Degraded reason).
    pub degraded_modules: &'static [ModuleKind],
    /// Modes the runtime currently refuses (always a subset of what the
    /// ladder above allows).
    pub refused_modes: &'static [ExecutionMode],
}

/// Per-operation constraints (the tightest layer).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OperationConstraints {
    /// This operation's own position cap.
    pub max_position_usd: Option<f64>,
    /// This operation's own slippage cap.
    pub max_slippage_bps: Option<u32>,
}

/// The resolved, effective configuration for one execution context.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveTenantConfig {
    /// Effective module table (tenant's set, minus runtime-degraded).
    pub modules: TenantModuleSet,
    /// Effective risk limits (tightest along the ladder).
    pub risk: TenantRiskLimits,
    /// Effective trading modes (platform ∩ tenant ∩ runtime).
    pub allowed_modes: Vec<ExecutionMode>,
}

impl EffectiveTenantConfig {
    /// Module gate under the RESOLVED configuration.
    pub fn module_enabled(&self, module: ModuleKind) -> ModuleEnablement {
        match self.modules.check(module) {
            bot_core::tenant::ModuleVerdict::Allow => ModuleEnablement::Enabled,
            bot_core::tenant::ModuleVerdict::Deny(reason) => ModuleEnablement::Disabled(reason),
        }
    }

    /// Mode gate: may this execution mode be used?
    pub fn mode_allowed(&self, mode: ExecutionMode) -> bool {
        self.allowed_modes.contains(&mode)
    }
}

/// The tightest of an optional chain of upper bounds (`None` when no
/// layer set a bound).
fn tightest_f64(bounds: &[Option<f64>]) -> Option<f64> {
    let mut tightest: Option<f64> = None;
    for bound in bounds.iter().flatten() {
        tightest = Some(match tightest {
            Some(current) if current < *bound => current,
            _ => *bound,
        });
    }
    tightest
}

/// Resolve the effective configuration along the precedence ladder.
pub fn resolve(
    bounds: &GlobalSafetyBounds,
    tenant: Option<&TenantConfigModel>,
    runtime: Option<&RuntimeOverrides>,
    operation: Option<&OperationConstraints>,
) -> EffectiveTenantConfig {
    let tenant = tenant.cloned().unwrap_or_default();

    // Modules: the tenant's set, with runtime-degraded entries forced to
    // the Degraded reason (an enabled-but-degraded module is denied for
    // as long as the degradation lasts).
    let mut modules = tenant.modules.clone();
    if let Some(runtime) = runtime {
        for module in runtime.degraded_modules {
            modules.set(TenantModuleState::disabled(
                *module,
                ModuleDisableReason::Degraded,
            ));
        }
    }

    // Risk: every layer may only tighten.
    let risk = TenantRiskLimits {
        max_position_usd: tightest_f64(&[
            Some(bounds.max_position_usd),
            tenant.risk.max_position_usd,
            runtime.and_then(|r| r.max_position_usd),
            operation.and_then(|o| o.max_position_usd),
        ]),
        daily_loss_usd_cap: tightest_f64(&[
            Some(bounds.daily_loss_usd_cap),
            tenant.risk.daily_loss_usd_cap,
            runtime.and_then(|r| r.daily_loss_usd_cap),
        ]),
        max_slippage_bps: {
            let candidates = [
                Some(bounds.max_slippage_bps),
                tenant.risk.max_slippage_bps,
                operation.and_then(|o| o.max_slippage_bps),
            ];
            candidates.iter().flatten().copied().reduce(u32::min)
        },
    };

    // Modes: platform ∩ tenant ∩ runtime.
    let mut allowed: Vec<ExecutionMode> = bounds
        .allowed_modes
        .iter()
        .copied()
        .filter(|m| tenant.allowed_modes.contains(m))
        .collect();
    if let Some(runtime) = runtime {
        allowed.retain(|m| !runtime.refused_modes.contains(m));
    }

    EffectiveTenantConfig {
        modules,
        risk,
        allowed_modes: allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::BotModule;

    fn bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &[ExecutionMode::Paper, ExecutionMode::Simulate],
        }
    }

    #[test]
    fn no_tenant_config_means_platform_defaults() {
        let e = resolve(&bounds(), None, None, None);
        assert_eq!(e.risk.max_position_usd, Some(10_000.0));
        assert_eq!(e.risk.max_slippage_bps, Some(500));
        assert!(e.mode_allowed(ExecutionMode::Paper));
        assert!(!e.mode_allowed(ExecutionMode::Live));
        assert!(e.module_enabled(BotModule::Sniper).is_enabled());
    }

    #[test]
    fn a_tenant_may_only_narrow_never_widen() {
        let mut tenant = TenantConfigModel::default();
        tenant.risk.max_position_usd = Some(2_000.0); // narrower — honored
        let e = resolve(&bounds(), Some(&tenant), None, None);
        assert_eq!(e.risk.max_position_usd, Some(2_000.0));

        let mut greedy = TenantConfigModel::default();
        greedy.risk.max_position_usd = Some(1_000_000.0); // wider — clamped
        let e = resolve(&bounds(), Some(&greedy), None, None);
        assert_eq!(e.risk.max_position_usd, Some(10_000.0));
    }

    #[test]
    fn every_layer_tightens_further() {
        let mut tenant = TenantConfigModel::default();
        tenant.risk.max_position_usd = Some(5_000.0);
        let runtime = RuntimeOverrides {
            max_position_usd: Some(3_000.0),
            ..RuntimeOverrides::default()
        };
        let op = OperationConstraints {
            max_position_usd: Some(1_500.0),
            ..OperationConstraints::default()
        };
        let e = resolve(&bounds(), Some(&tenant), Some(&runtime), Some(&op));
        assert_eq!(e.risk.max_position_usd, Some(1_500.0));
    }

    #[test]
    fn runtime_degradation_denies_the_module() {
        let runtime = RuntimeOverrides {
            degraded_modules: &[BotModule::Polymarket],
            ..RuntimeOverrides::default()
        };
        let e = resolve(&bounds(), None, Some(&runtime), None);
        assert!(!e.module_enabled(BotModule::Polymarket).is_enabled());
        assert!(e.module_enabled(BotModule::Sniper).is_enabled());
    }

    #[test]
    fn modes_intersect_all_layers() {
        let tenant = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Paper],
            ..TenantConfigModel::default()
        };
        let runtime = RuntimeOverrides {
            refused_modes: &[ExecutionMode::Simulate],
            ..RuntimeOverrides::default()
        };
        let e = resolve(&bounds(), Some(&tenant), Some(&runtime), None);
        assert_eq!(e.allowed_modes, vec![ExecutionMode::Paper]);
        assert!(!e.mode_allowed(ExecutionMode::Live));
    }

    #[test]
    fn live_mode_requires_every_layer_to_allow_it() {
        let tenant = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Paper, ExecutionMode::Live],
            ..TenantConfigModel::default()
        };
        // Platform does not allow live: the tenant cannot grant itself.
        let e = resolve(&bounds(), Some(&tenant), None, None);
        assert!(!e.mode_allowed(ExecutionMode::Live));
    }
}
