//! Module enabled/disabled/degraded state for a tenant (STEP 3 file 06).
//!
//! Pure domain model: for each module, is it enabled for THIS tenant, and
//! if not, why. The sources (tenant configuration, plan entitlement) are
//! evaluated elsewhere — `tenant_config` resolves the effective set, the
//! billing layer owns entitlements. This type is the closed-vocabulary
//! RESULT those layers agree on, and the module guard consumes.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::module_kind::{ModuleKind, ALL_MODULES};

/// Why a module is not enabled for a tenant. Closed vocabulary — stable
/// machine-readable labels for guards, metrics and audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleDisableReason {
    /// Disabled by the tenant/operator explicitly.
    Operator,
    /// Not included in the tenant's plan (see the entitlement guard).
    Plan,
    /// The runtime deliberately degraded it (dependency outage, custody
    /// unavailable, feed down). Not a permanent refusal.
    Degraded,
}

impl ModuleDisableReason {
    /// Every reason, stable order.
    pub const ALL: [ModuleDisableReason; 3] = [
        ModuleDisableReason::Operator,
        ModuleDisableReason::Plan,
        ModuleDisableReason::Degraded,
    ];

    /// Stable label.
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleDisableReason::Operator => "operator",
            ModuleDisableReason::Plan => "plan",
            ModuleDisableReason::Degraded => "degraded",
        }
    }

    /// Inverse of [`ModuleDisableReason::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        ModuleDisableReason::ALL
            .iter()
            .copied()
            .find(|r| r.as_str() == s.trim())
    }

    /// Human explanation (safe to surface).
    pub fn detail(self) -> &'static str {
        match self {
            ModuleDisableReason::Operator => "the module is disabled for this organization",
            ModuleDisableReason::Plan => "the module is not part of the current plan",
            ModuleDisableReason::Degraded => {
                "the module is temporarily degraded for this organization"
            }
        }
    }
}

impl fmt::Display for ModuleDisableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The enablement state of one module for one tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleEnablement {
    /// Enabled and healthy.
    Enabled,
    /// Disabled, with the machine-readable reason.
    Disabled(ModuleDisableReason),
}

impl ModuleEnablement {
    /// Is the module usable?
    pub fn is_enabled(self) -> bool {
        matches!(self, ModuleEnablement::Enabled)
    }
}

/// The per-tenant module state table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantModuleSet {
    states: Vec<TenantModuleState>,
}

/// One row of the per-tenant module table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantModuleState {
    /// Which module.
    pub module: ModuleKind,
    /// Its enablement for this tenant.
    pub enablement: ModuleEnablement,
}

impl TenantModuleState {
    /// An enabled entry.
    pub fn enabled(module: ModuleKind) -> Self {
        TenantModuleState {
            module,
            enablement: ModuleEnablement::Enabled,
        }
    }

    /// A disabled entry with the reason.
    pub fn disabled(module: ModuleKind, reason: ModuleDisableReason) -> Self {
        TenantModuleState {
            module,
            enablement: ModuleEnablement::Disabled(reason),
        }
    }
}

/// The verdict of the module enablement check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleVerdict {
    /// The module is enabled for the tenant.
    Allow,
    /// Refused, with the reason.
    Deny(ModuleDisableReason),
}

impl ModuleVerdict {
    /// True on allow.
    pub fn is_allowed(self) -> bool {
        matches!(self, ModuleVerdict::Allow)
    }
}

impl TenantModuleSet {
    /// Build from an explicit list (later entries win on duplicates).
    pub fn new(states: impl IntoIterator<Item = TenantModuleState>) -> Self {
        let mut set = TenantModuleSet::default();
        for s in states {
            set.set(s);
        }
        set
    }

    /// The deployment default: every trading module enabled, control
    /// plane enabled. An operator that never configured modules keeps
    /// exactly the single-tenant behavior.
    pub fn deployment_default() -> Self {
        TenantModuleSet::new(ALL_MODULES.map(TenantModuleState::enabled))
    }

    /// Insert or replace one module's state.
    pub fn set(&mut self, state: TenantModuleState) {
        match self.states.iter_mut().find(|s| s.module == state.module) {
            Some(slot) => *slot = state,
            None => self.states.push(state),
        }
    }

    /// Look up one module. `None` = not configured — fail closed at the
    /// call site via [`TenantModuleSet::check`].
    pub fn state_of(&self, module: ModuleKind) -> Option<ModuleEnablement> {
        self.states
            .iter()
            .find(|s| s.module == module)
            .map(|s| s.enablement)
    }

    /// The guard check: enabled → Allow; disabled → the recorded reason;
    /// not configured → Deny(Operator) (a module the tenant never
    /// configured is not enabled — fail closed, never guess).
    pub fn check(&self, module: ModuleKind) -> ModuleVerdict {
        match self.state_of(module) {
            Some(ModuleEnablement::Enabled) => ModuleVerdict::Allow,
            Some(ModuleEnablement::Disabled(reason)) => ModuleVerdict::Deny(reason),
            None => ModuleVerdict::Deny(ModuleDisableReason::Operator),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_default_enables_everything() {
        let set = TenantModuleSet::deployment_default();
        for m in ALL_MODULES {
            assert!(set.check(m).is_allowed(), "{m} should be enabled");
        }
    }

    #[test]
    fn disabled_reason_round_trips() {
        assert_eq!(
            ModuleDisableReason::parse("operator"),
            Some(ModuleDisableReason::Operator)
        );
        assert_eq!(
            ModuleDisableReason::parse("plan"),
            Some(ModuleDisableReason::Plan)
        );
        assert_eq!(
            ModuleDisableReason::parse("degraded"),
            Some(ModuleDisableReason::Degraded)
        );
        assert_eq!(ModuleDisableReason::parse("nope"), None);
    }

    #[test]
    fn unconfigured_module_is_denied_fail_closed() {
        let set = TenantModuleSet::default();
        assert_eq!(
            set.check(ModuleKind::Sniper),
            ModuleVerdict::Deny(ModuleDisableReason::Operator)
        );
    }

    #[test]
    fn check_reports_the_recorded_reason() {
        let set = TenantModuleSet::new([
            TenantModuleState::enabled(ModuleKind::Sniper),
            TenantModuleState::disabled(ModuleKind::Copy, ModuleDisableReason::Plan),
            TenantModuleState::disabled(ModuleKind::Polymarket, ModuleDisableReason::Degraded),
        ]);
        assert!(set.check(ModuleKind::Sniper).is_allowed());
        assert_eq!(
            set.check(ModuleKind::Copy),
            ModuleVerdict::Deny(ModuleDisableReason::Plan)
        );
        assert_eq!(
            set.check(ModuleKind::Polymarket),
            ModuleVerdict::Deny(ModuleDisableReason::Degraded)
        );
    }

    #[test]
    fn later_entries_win_on_duplicates() {
        let mut set = TenantModuleSet::new([TenantModuleState::enabled(ModuleKind::Sniper)]);
        set.set(TenantModuleState::disabled(
            ModuleKind::Sniper,
            ModuleDisableReason::Operator,
        ));
        assert!(!set.check(ModuleKind::Sniper).is_allowed());
    }
}
