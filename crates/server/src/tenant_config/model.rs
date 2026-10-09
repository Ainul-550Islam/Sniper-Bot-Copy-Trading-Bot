//! Typed tenant configuration model (STEP 3 file 29).
//!
//! A tenant's configuration is a NARROW, typed document — the knobs a
//! tenant may legitimately turn (module enablement, its OWN risk limits
//! within the platform's hard bounds, its own wallet/signer labels).
//! It is derived from the concerns of the existing global config
//! (`bot_core::config`), NOT a copy of it: global deployment settings
//! (RPC endpoints, secrets, HA, custody providers) are not per-tenant
//! and never enter this model.
//!
//! What can NEVER be configured here (enforced by the resolver and
//! validator, and by the absence of fields): another tenant's wallet or
//! signer, disabling audit or authorization, bypassing the kill switch,
//! overriding global safety bounds, or any secret value.

use bot_core::models::ExecutionMode;
use bot_core::tenant::{
    ModuleDisableReason, ModuleEnablement, ModuleKind, TenantModuleSet, TenantModuleState,
};
use serde::{Deserialize, Serialize};

/// Per-tenant risk limits. `None` = "no tenant override; the platform
/// value applies". A set value can only ever NARROW the effective limit
/// (see `resolver.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct TenantRiskLimits {
    /// Largest single position this tenant may open, in USD.
    pub max_position_usd: Option<f64>,
    /// Largest daily realized loss before new entries stop, in USD.
    pub daily_loss_usd_cap: Option<f64>,
    /// Maximum acceptable slippage, in basis points.
    pub max_slippage_bps: Option<u32>,
}

impl TenantRiskLimits {
    /// No overrides at all (the deployment default).
    pub fn none() -> Self {
        TenantRiskLimits::default()
    }

    /// Are all overrides absent?
    pub fn is_empty(&self) -> bool {
        self.max_position_usd.is_none()
            && self.daily_loss_usd_cap.is_none()
            && self.max_slippage_bps.is_none()
    }
}

/// A tenant's preferred signer binding, BY LABEL ONLY. The actual
/// binding (and its ownership) is resolved through the wallet/signer
/// registries at guard time — a tenant cannot name another tenant's
/// signer and have it honored (the guard verifies ownership).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantSignerPreference {
    /// Which provider kind the tenant prefers.
    pub provider: bot_core::tenant::SignerProvider,
    /// The provider-side key reference (public identifier, never a secret).
    pub key_ref: String,
}

/// The typed tenant configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantConfigModel {
    /// Module enablement for this tenant.
    pub modules: TenantModuleSet,
    /// The tenant's own risk limits (may only narrow platform bounds).
    pub risk: TenantRiskLimits,
    /// The trading environments this tenant is allowed to use. An empty
    /// set means "paper only" — the fail-closed default.
    #[serde(default)]
    pub allowed_modes: Vec<ExecutionMode>,
    /// The wallet label the tenant prefers for new executions (binding
    /// still verified by the wallet guard).
    #[serde(default)]
    pub preferred_wallet_label: Option<String>,
    /// The signer the tenant prefers (binding still verified by the
    /// signer guard).
    #[serde(default)]
    pub preferred_signer: Option<TenantSignerPreference>,
}

impl Default for TenantConfigModel {
    fn default() -> Self {
        // The deployment default: every module enabled, no risk
        // overrides, paper-only until the platform allows more. A new
        // tenant starts conservative; entitlements and operator action
        // open things up.
        TenantConfigModel {
            modules: TenantModuleSet::deployment_default(),
            risk: TenantRiskLimits::none(),
            allowed_modes: vec![ExecutionMode::Paper],
            preferred_wallet_label: None,
            preferred_signer: None,
        }
    }
}

impl TenantConfigModel {
    /// The deployment-legacy configuration: exactly what a single-operator
    /// install had — every module on, no tenant overrides, and the modes
    /// the GLOBAL config already allows (the resolver clamps to them).
    pub fn deployment_legacy(global_modes: &[ExecutionMode]) -> Self {
        TenantConfigModel {
            allowed_modes: if global_modes.is_empty() {
                vec![ExecutionMode::Paper]
            } else {
                global_modes.to_vec()
            },
            ..TenantConfigModel::default()
        }
    }

    /// Is a module enabled for this tenant?
    pub fn module_enabled(&self, module: ModuleKind) -> ModuleEnablement {
        match self.modules.check(module) {
            bot_core::tenant::ModuleVerdict::Allow => ModuleEnablement::Enabled,
            bot_core::tenant::ModuleVerdict::Deny(reason) => ModuleEnablement::Disabled(reason),
        }
    }

    /// Enable a module (operator/tenant action).
    pub fn enable_module(&mut self, module: ModuleKind) {
        self.modules.set(TenantModuleState::enabled(module));
    }

    /// Disable a module with an explicit reason.
    pub fn disable_module(&mut self, module: ModuleKind, reason: ModuleDisableReason) {
        self.modules
            .set(TenantModuleState::disabled(module, reason));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::BotModule;

    #[test]
    fn default_is_all_modules_paper_only_no_overrides() {
        let c = TenantConfigModel::default();
        for m in bot_core::tenant::ALL_MODULES {
            assert!(c.module_enabled(m).is_enabled(), "{m}");
        }
        assert!(c.risk.is_empty());
        assert_eq!(c.allowed_modes, vec![ExecutionMode::Paper]);
        assert!(c.preferred_wallet_label.is_none());
        assert!(c.preferred_signer.is_none());
    }

    #[test]
    fn module_toggles_round_trip() {
        let mut c = TenantConfigModel::default();
        c.disable_module(BotModule::Copy, ModuleDisableReason::Operator);
        assert!(!c.module_enabled(BotModule::Copy).is_enabled());
        c.enable_module(BotModule::Copy);
        assert!(c.module_enabled(BotModule::Copy).is_enabled());
    }

    #[test]
    fn deployment_legacy_inherits_the_global_modes() {
        let c = TenantConfigModel::deployment_legacy(&[ExecutionMode::Paper, ExecutionMode::Live]);
        assert!(c.allowed_modes.contains(&ExecutionMode::Live));
        let empty = TenantConfigModel::deployment_legacy(&[]);
        assert_eq!(empty.allowed_modes, vec![ExecutionMode::Paper]);
    }

    #[test]
    fn model_round_trips_through_json() {
        let mut c = TenantConfigModel::default();
        c.disable_module(BotModule::Polymarket, ModuleDisableReason::Plan);
        c.risk.max_position_usd = Some(5_000.0);
        let json = serde_json::to_string(&c).unwrap();
        let back: TenantConfigModel = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
        // The stored document carries no secret-shaped fields at all:
        // the serialized form mentions exactly the known keys.
        for forbidden in ["secret", "api_key", "private", "mnemonic", "seed"] {
            assert!(
                !json.to_ascii_lowercase().contains(forbidden),
                "{forbidden} leaked"
            );
        }
    }
}
