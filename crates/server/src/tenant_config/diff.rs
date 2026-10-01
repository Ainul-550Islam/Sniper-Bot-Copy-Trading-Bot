//! Configuration change detection (STEP 3 file 35).
//!
//! [`ConfigDiff`] explains, in machine-readable form, what changed
//! between two documents. It powers two things:
//!
//! * the audit log (every write records its diff — the tenant sees
//!   "module X disabled, position cap lowered", not a jsonb blob);
//! * change reactions (e.g. a module being DISABLED must stop that
//!   module's engines for this tenant — the background supervisor
//!   subscribes to diffs, not to raw documents).

use serde::{Deserialize, Serialize};

use bot_core::models::ExecutionMode;
use bot_core::tenant::{ModuleDisableReason, ModuleEnablement, ModuleKind};

use super::model::{TenantConfigModel, TenantRiskLimits};

/// One discrete, human-observable change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// A module went from disabled to enabled.
    ModuleEnabled { module: ModuleKind },
    /// A module went from enabled to disabled (with the new reason).
    ModuleDisabled {
        module: ModuleKind,
        reason: ModuleDisableReason,
    },
    /// The disable reason changed while the module stayed off.
    ModuleReasonChanged {
        module: ModuleKind,
        from: ModuleDisableReason,
        to: ModuleDisableReason,
    },
    /// A risk override was added or narrowed.
    RiskTightened {
        field: String,
        from: Option<f64>,
        to: Option<f64>,
    },
    /// A risk override was removed or widened (still within platform
    /// bounds — the store validated the document).
    RiskLoosened {
        field: String,
        from: Option<f64>,
        to: Option<f64>,
    },
    /// A trading mode was added to the tenant's allowed set.
    ModeAllowed { mode: ExecutionMode },
    /// A trading mode was removed from the tenant's allowed set.
    ModeRevoked { mode: ExecutionMode },
    /// The preferred wallet label changed.
    PreferredWalletChanged {
        from: Option<String>,
        to: Option<String>,
    },
    /// The preferred signer changed.
    PreferredSignerChanged {
        from: Option<super::model::TenantSignerPreference>,
        to: Option<super::model::TenantSignerPreference>,
    },
}

impl Change {
    /// Stable, machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Change::ModuleEnabled { .. } => "module_enabled",
            Change::ModuleDisabled { .. } => "module_disabled",
            Change::ModuleReasonChanged { .. } => "module_reason_changed",
            Change::RiskTightened { .. } => "risk_tightened",
            Change::RiskLoosened { .. } => "risk_loosened",
            Change::ModeAllowed { .. } => "mode_allowed",
            Change::ModeRevoked { .. } => "mode_revoked",
            Change::PreferredWalletChanged { .. } => "preferred_wallet_changed",
            Change::PreferredSignerChanged { .. } => "preferred_signer_changed",
        }
    }

    /// Human phrasing for the audit trail.
    pub fn describe(&self) -> String {
        match self {
            Change::ModuleEnabled { module } => format!("module {module} enabled"),
            Change::ModuleDisabled { module, reason } => {
                format!("module {module} disabled ({reason})")
            }
            Change::ModuleReasonChanged { module, from, to } => {
                format!("module {module} disable reason changed: {from} -> {to}")
            }
            Change::RiskTightened { field, from, to } => {
                format!("{field} tightened: {} -> {}", fmt_opt(*from), fmt_opt(*to))
            }
            Change::RiskLoosened { field, from, to } => {
                format!("{field} loosened: {} -> {}", fmt_opt(*from), fmt_opt(*to))
            }
            Change::ModeAllowed { mode } => format!("trading mode {} allowed", mode.as_str()),
            Change::ModeRevoked { mode } => format!("trading mode {} revoked", mode.as_str()),
            Change::PreferredWalletChanged { from, to } => {
                format!(
                    "preferred wallet: {} -> {}",
                    fmt_opt_str(from.as_deref()),
                    fmt_opt_str(to.as_deref())
                )
            }
            Change::PreferredSignerChanged { from, to } => format!(
                "preferred signer: {} -> {}",
                fmt_opt_str(from.as_ref().map(|p| p.key_ref.clone()).as_deref()),
                fmt_opt_str(to.as_ref().map(|p| p.key_ref.clone()).as_deref())
            ),
        }
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v}"),
        None => "unset".into(),
    }
}

fn fmt_opt_str(v: Option<&str>) -> String {
    v.unwrap_or("unset").to_string()
}

/// The full diff between two configuration documents.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigDiff {
    /// Every discrete change, in a stable order (modules, risk, modes,
    /// preferences).
    pub changes: Vec<Change>,
}

impl ConfigDiff {
    /// Compute the diff from `old` to `new`.
    pub fn between(old: &TenantConfigModel, new: &TenantConfigModel) -> Self {
        let mut changes = Vec::new();

        for module in bot_core::tenant::ALL_MODULES {
            let was = old.module_enabled(module);
            let now = new.module_enabled(module);
            match (was, now) {
                (ModuleEnablement::Enabled, ModuleEnablement::Enabled) => {}
                (ModuleEnablement::Disabled(_), ModuleEnablement::Disabled(_)) => {
                    if let (ModuleEnablement::Disabled(from), ModuleEnablement::Disabled(to)) =
                        (was, now)
                    {
                        if from != to {
                            changes.push(Change::ModuleReasonChanged { module, from, to });
                        }
                    }
                }
                (ModuleEnablement::Enabled, ModuleEnablement::Disabled(reason)) => {
                    changes.push(Change::ModuleDisabled { module, reason })
                }
                (ModuleEnablement::Disabled(_), ModuleEnablement::Enabled) => {
                    changes.push(Change::ModuleEnabled { module })
                }
            }
        }

        diff_risk(&old.risk, &new.risk, &mut changes);

        for mode in [
            ExecutionMode::Paper,
            ExecutionMode::Simulate,
            ExecutionMode::Live,
        ] {
            let was = old.allowed_modes.contains(&mode);
            let now = new.allowed_modes.contains(&mode);
            if was && !now {
                changes.push(Change::ModeRevoked { mode });
            } else if !was && now {
                changes.push(Change::ModeAllowed { mode });
            }
        }

        if old.preferred_wallet_label != new.preferred_wallet_label {
            changes.push(Change::PreferredWalletChanged {
                from: old.preferred_wallet_label.clone(),
                to: new.preferred_wallet_label.clone(),
            });
        }
        if old.preferred_signer != new.preferred_signer {
            changes.push(Change::PreferredSignerChanged {
                from: old.preferred_signer.clone(),
                to: new.preferred_signer.clone(),
            });
        }

        ConfigDiff { changes }
    }

    /// No changes?
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Did this diff disable the given module?
    pub fn disables_module(&self, module: ModuleKind) -> bool {
        self.changes
            .iter()
            .any(|c| matches!(c, Change::ModuleDisabled { module: m, .. } if *m == module))
    }

    /// Did this diff revoke the given trading mode?
    pub fn revokes_mode(&self, mode: ExecutionMode) -> bool {
        self.changes
            .iter()
            .any(|c| matches!(c, Change::ModeRevoked { mode: m, .. } if *m == mode))
    }

    /// Every risk-tightening change (a supervisor may act immediately).
    pub fn tightenings(&self) -> Vec<&Change> {
        self.changes
            .iter()
            .filter(|c| matches!(c, Change::RiskTightened { .. }))
            .collect()
    }
}

fn diff_risk(old: &TenantRiskLimits, new: &TenantRiskLimits, changes: &mut Vec<Change>) {
    let fields: [(&str, Option<f64>, Option<f64>); 3] = [
        (
            "max_position_usd",
            old.max_position_usd,
            new.max_position_usd,
        ),
        (
            "daily_loss_usd_cap",
            old.daily_loss_usd_cap,
            new.daily_loss_usd_cap,
        ),
        (
            "max_slippage_bps",
            old.max_slippage_bps.map(|v| v as f64),
            new.max_slippage_bps.map(|v| v as f64),
        ),
    ];
    for (field, from, to) in fields {
        let field = field.to_string();
        if from == to {
            continue;
        }
        // Tightened = the bound got strictly smaller (or appeared). A
        // change from None to Some is tightening (it was unbounded).
        let tightened = match (from, to) {
            (None, Some(_)) => true,
            (Some(f), Some(t)) => t < f,
            _ => false,
        };
        if tightened {
            changes.push(Change::RiskTightened { field, from, to });
        } else {
            changes.push(Change::RiskLoosened { field, from, to });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::BotModule;

    #[test]
    fn identical_documents_diff_empty() {
        let a = TenantConfigModel::default();
        assert!(ConfigDiff::between(&a, &a).is_empty());
    }

    #[test]
    fn module_disabling_is_detected() {
        let old = TenantConfigModel::default();
        let mut new = old.clone();
        new.disable_module(BotModule::Copy, ModuleDisableReason::Operator);
        let diff = ConfigDiff::between(&old, &new);
        assert!(diff.disables_module(BotModule::Copy));
        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].as_str(), "module_disabled");
        assert!(diff.changes[0].describe().contains("copy"));
    }

    #[test]
    fn risk_appearing_is_tightening_and_removal_is_loosening() {
        let old = TenantConfigModel::default();
        let mut mid = old.clone();
        mid.risk.max_position_usd = Some(5_000.0);
        assert!(ConfigDiff::between(&old, &mid)
            .changes
            .iter()
            .any(|c| c.as_str() == "risk_tightened"));

        // Removing the override loosens back to platform-bound.
        let diff = ConfigDiff::between(&mid, &old);
        assert!(diff.changes.iter().any(|c| c.as_str() == "risk_loosened"));
    }

    #[test]
    fn narrowing_twice_keeps_tightening() {
        let mut a = TenantConfigModel::default();
        a.risk.max_position_usd = Some(5_000.0);
        let mut b = a.clone();
        b.risk.max_position_usd = Some(1_000.0);
        let diff = ConfigDiff::between(&a, &b);
        assert_eq!(diff.tightenings().len(), 1);
    }

    #[test]
    fn mode_revocation_is_detected() {
        let old = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Paper, ExecutionMode::Simulate],
            ..TenantConfigModel::default()
        };
        let new = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Paper],
            ..old.clone()
        };
        let diff = ConfigDiff::between(&old, &new);
        assert!(diff.revokes_mode(ExecutionMode::Simulate));
        assert!(!diff.revokes_mode(ExecutionMode::Paper));
    }

    #[test]
    fn preference_changes_are_reported() {
        let old = TenantConfigModel::default();
        let mut new = old.clone();
        new.preferred_wallet_label = Some("main".into());
        let diff = ConfigDiff::between(&old, &new);
        assert_eq!(diff.changes[0].as_str(), "preferred_wallet_changed");
        assert!(diff.changes[0].describe().contains("main"));
    }
}
