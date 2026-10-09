//! Tenant configuration validation (STEP 3 file 31).
//!
//! Pure validation of a [`TenantConfigModel`] against the platform's
//! [`GlobalSafetyBounds`]: compatibility of network/mode, module and
//! risk settings. The store REFUSES to persist an invalid document, and
//! the risk guard refuses to execute under one — an invalid tenant
//! configuration is a DENY, never a "best effort".

use bot_core::models::ExecutionMode;

use super::model::{TenantConfigModel, TenantSignerPreference};
use super::resolver::GlobalSafetyBounds;

/// One validation finding. Closed vocabulary — machine-readable.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigIssue {
    /// A risk override tries to EXCEED the platform bound (it will be
    /// clamped at resolve time; refusing the write keeps the document
    /// honest about what it says).
    RiskBoundExceeded {
        /// Which limit.
        field: &'static str,
        /// The attempted value.
        requested: f64,
        /// The platform bound.
        bound: f64,
    },
    /// A risk value is not finite or not positive.
    RiskValueInvalid { field: &'static str },
    /// The tenant allows a trading mode the platform does not.
    ModeNotAllowed { mode: ExecutionMode },
    /// The tenant allows NO trading mode at all (fail closed forever).
    NoModesAllowed,
    /// A module-unknown entry was found (the module vocabulary is
    /// closed).
    UnknownModule { module: String },
    /// The preferred signer reference is malformed.
    SignerPreferenceInvalid { reason: &'static str },
}

impl ConfigIssue {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigIssue::RiskBoundExceeded { .. } => "risk_bound_exceeded",
            ConfigIssue::RiskValueInvalid { .. } => "risk_value_invalid",
            ConfigIssue::ModeNotAllowed { .. } => "mode_not_allowed",
            ConfigIssue::NoModesAllowed => "no_modes_allowed",
            ConfigIssue::UnknownModule { .. } => "unknown_module",
            ConfigIssue::SignerPreferenceInvalid { .. } => "signer_preference_invalid",
        }
    }

    /// Human explanation (safe to surface to the tenant).
    pub fn detail(&self) -> String {
        match self {
            ConfigIssue::RiskBoundExceeded {
                field,
                requested,
                bound,
            } => format!("{field} {requested} exceeds the platform bound {bound}"),
            ConfigIssue::RiskValueInvalid { field } => {
                format!("{field} must be a positive, finite number")
            }
            ConfigIssue::ModeNotAllowed { mode } => {
                format!(
                    "trading mode {} is not available on this platform",
                    mode.as_str()
                )
            }
            ConfigIssue::NoModesAllowed => "the configuration allows no trading mode at all".into(),
            ConfigIssue::UnknownModule { module } => {
                format!("unknown module {module:?}")
            }
            ConfigIssue::SignerPreferenceInvalid { reason } => {
                format!("preferred signer is invalid: {reason}")
            }
        }
    }
}

fn check_positive(value: f64, field: &'static str, issues: &mut Vec<ConfigIssue>) {
    if !value.is_finite() || value <= 0.0 {
        issues.push(ConfigIssue::RiskValueInvalid { field });
    }
}

fn check_bound(value: f64, bound: f64, field: &'static str, issues: &mut Vec<ConfigIssue>) {
    check_positive(value, field, issues);
    if value.is_finite() && value > bound {
        issues.push(ConfigIssue::RiskBoundExceeded {
            field,
            requested: value,
            bound,
        });
    }
}

fn check_signer_preference(pref: &TenantSignerPreference, issues: &mut Vec<ConfigIssue>) {
    let probe = bot_core::tenant::TenantSignerRef::new(
        bot_core::tenant::OrganizationId::new(),
        pref.provider,
        pref.key_ref.clone(),
    );
    if probe.is_err() {
        issues.push(ConfigIssue::SignerPreferenceInvalid {
            reason: "key reference must be 1..=128 chars of provider-safe characters",
        });
    }
}

/// Validate a tenant configuration against the platform bounds.
///
/// Returns every issue found (an empty vec = valid). Unknown modules
/// cannot occur in the typed model — the check exists for documents
/// deserialized from storage, where a future vocabulary change could
/// have left a stale name (the module set stores enum values, so this
/// arms the day that changes).
pub fn validate(config: &TenantConfigModel, bounds: &GlobalSafetyBounds) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();

    if let Some(v) = config.risk.max_position_usd {
        check_bound(v, bounds.max_position_usd, "max_position_usd", &mut issues);
    }
    if let Some(v) = config.risk.daily_loss_usd_cap {
        check_bound(
            v,
            bounds.daily_loss_usd_cap,
            "daily_loss_usd_cap",
            &mut issues,
        );
    }
    if let Some(v) = config.risk.max_slippage_bps {
        if v == 0 || v > bounds.max_slippage_bps {
            issues.push(ConfigIssue::RiskBoundExceeded {
                field: "max_slippage_bps",
                requested: v as f64,
                bound: bounds.max_slippage_bps as f64,
            });
        }
    }

    if config.allowed_modes.is_empty() {
        issues.push(ConfigIssue::NoModesAllowed);
    }
    for mode in &config.allowed_modes {
        if !bounds.allowed_modes.contains(mode) {
            issues.push(ConfigIssue::ModeNotAllowed { mode: *mode });
        }
    }

    if let Some(pref) = &config.preferred_signer {
        check_signer_preference(pref, &mut issues);
    }

    // The module vocabulary is closed by construction — `TenantModuleSet`
    // is keyed by the typed `ModuleKind` enum, so an unknown module
    // cannot be expressed in a stored document (deserialization of an
    // unknown name fails at the serde layer before validation runs).
    // `ConfigIssue::UnknownModule` exists for forward-compatible
    // reporting when older servers read newer documents.

    issues
}

/// Validate and return a Result form (the store uses this).
pub fn validate_or_issues(
    config: &TenantConfigModel,
    bounds: &GlobalSafetyBounds,
) -> Result<(), Vec<ConfigIssue>> {
    let issues = validate(config, bounds);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::model::TenantRiskLimits;
    use bot_core::tenant::SignerProvider;

    fn bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 10_000.0,
            daily_loss_usd_cap: 1_000.0,
            max_slippage_bps: 500,
            allowed_modes: &[ExecutionMode::Paper, ExecutionMode::Simulate],
        }
    }

    #[test]
    fn the_default_document_is_valid() {
        assert!(validate_or_issues(&TenantConfigModel::default(), &bounds()).is_ok());
    }

    #[test]
    fn exceeding_bounds_is_refused_before_persist() {
        let c = TenantConfigModel {
            risk: TenantRiskLimits {
                max_position_usd: Some(50_000.0),
                ..TenantRiskLimits::default()
            },
            ..TenantConfigModel::default()
        };
        let issues = validate(&c, &bounds());
        assert!(issues.iter().any(|i| i.as_str() == "risk_bound_exceeded"));
    }

    #[test]
    fn narrower_values_pass() {
        let mut c = TenantConfigModel::default();
        c.risk.max_position_usd = Some(500.0);
        c.risk.daily_loss_usd_cap = Some(100.0);
        c.risk.max_slippage_bps = Some(50);
        assert!(validate_or_issues(&c, &bounds()).is_ok());
    }

    #[test]
    fn non_finite_and_non_positive_values_are_invalid() {
        let mut c = TenantConfigModel::default();
        c.risk.max_position_usd = Some(f64::NAN);
        let issues = validate(&c, &bounds());
        assert!(issues.iter().any(|i| i.as_str() == "risk_value_invalid"));
        c.risk.max_position_usd = Some(0.0);
        assert!(validate(&c, &bounds())
            .iter()
            .any(|i| i.as_str() == "risk_value_invalid"));
    }

    #[test]
    fn disallowed_modes_and_no_modes_are_refused() {
        let c = TenantConfigModel {
            allowed_modes: vec![ExecutionMode::Live],
            ..TenantConfigModel::default()
        };
        let issues = validate(&c, &bounds());
        assert!(issues.iter().any(|i| i.as_str() == "mode_not_allowed"));

        let empty = TenantConfigModel {
            allowed_modes: vec![],
            ..TenantConfigModel::default()
        };
        let issues = validate(&empty, &bounds());
        assert!(issues.iter().any(|i| i.as_str() == "no_modes_allowed"));
    }

    #[test]
    fn malformed_signer_preference_is_refused() {
        let c = TenantConfigModel {
            preferred_signer: Some(TenantSignerPreference {
                provider: SignerProvider::Custody,
                key_ref: "bad key with spaces".into(),
            }),
            ..TenantConfigModel::default()
        };
        let issues = validate(&c, &bounds());
        assert!(issues
            .iter()
            .any(|i| i.as_str() == "signer_preference_invalid"));
    }

    #[test]
    fn issues_carry_human_details() {
        let c = TenantConfigModel {
            risk: TenantRiskLimits {
                max_position_usd: Some(50_000.0),
                ..TenantRiskLimits::default()
            },
            ..TenantConfigModel::default()
        };
        let issues = validate(&c, &bounds());
        let detail = issues[0].detail();
        assert!(detail.contains("max_position_usd"));
        assert!(detail.contains("10000"));
    }
}
