//! Gateway decisions and the closed deny vocabulary (STEP 3 file 15).
//!
//! Every guard returns a [`GuardOutcome`]; the gateway folds them into a
//! [`TenantDecision`]. Deny reasons are enum values (machine-readable,
//! greppable, impossible to typo into a security decision) with stable
//! `as_str()` labels used by metrics and the audit trail.

/// One guard's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardOutcome {
    /// This guard allows. Carries a short note for the trace.
    Allow(&'static str),
    /// This guard denies, with the closed reason.
    Deny(DenyReason),
}

impl GuardOutcome {
    /// Did the guard allow?
    pub fn is_allow(&self) -> bool {
        matches!(self, GuardOutcome::Allow(_))
    }

    /// The deny reason, when denying.
    pub fn deny_reason(&self) -> Option<&DenyReason> {
        match self {
            GuardOutcome::Allow(_) => None,
            GuardOutcome::Deny(reason) => Some(reason),
        }
    }
}

/// The closed vocabulary of gateway denials. Order of variants mirrors
/// the chain order; the labels are stable API (metrics group by them).
#[derive(Debug, Clone, PartialEq)]
pub enum DenyReason {
    /// The organization is not active (suspended/closed/pending).
    TenantState { state: &'static str },
    /// The tenant's plan does not include this module (or it is
    /// explicitly disabled by the plan).
    EntitlementModule {
        module: &'static str,
        reason: &'static str,
    },
    /// The tenant's plan does not allow live trading at all.
    EntitlementLiveTrading,
    /// The module is disabled in the tenant's configuration.
    ModuleDisabled {
        module: &'static str,
        reason: &'static str,
    },
    /// The trading mode is not allowed (platform, tenant config or
    /// runtime degradation).
    ModeNotAllowed { mode: &'static str },
    /// The wallet label does not resolve to an active wallet the tenant
    /// owns.
    WalletNotBound { label: String },
    /// The signer reference does not resolve to an active signer the
    /// tenant owns.
    SignerNotBound { key_ref: String },
    /// The wallet is not active for executions.
    WalletInactive { label: String },
    /// The signer is not active for executions.
    SignerInactive { key_ref: String },
    /// The requested size exceeds the effective position cap.
    PositionSizeExceeds { requested: f64, cap: f64 },
    /// The requested slippage exceeds the effective cap.
    SlippageExceeds { requested_bps: u32, cap_bps: u32 },
    /// The size/slippage values are not usable (negative, NaN).
    RiskValuesInvalid,
    /// The runtime fence failed: the claiming runtime is not the
    /// tenant's current live runtime (superseded, stale generation, no
    /// live runtime or not lease-live).
    FenceFailed { verdict: &'static str },
    /// The context could not be issued after all guards passed (scope
    /// or authority construction error — never retried blindly).
    ContextIssue { reason: &'static str },
    /// A guard dependency failed (storage, config read). Fail closed.
    DependencyError { source: &'static str },
}

impl DenyReason {
    /// Stable machine-readable label (metrics + audit grouping).
    pub fn as_str(&self) -> &'static str {
        match self {
            DenyReason::TenantState { .. } => "tenant_state",
            DenyReason::EntitlementModule { .. } => "entitlement_module",
            DenyReason::EntitlementLiveTrading => "entitlement_live_trading",
            DenyReason::ModuleDisabled { .. } => "module_disabled",
            DenyReason::ModeNotAllowed { .. } => "mode_not_allowed",
            DenyReason::WalletNotBound { .. } => "wallet_not_bound",
            DenyReason::SignerNotBound { .. } => "signer_not_bound",
            DenyReason::WalletInactive { .. } => "wallet_inactive",
            DenyReason::SignerInactive { .. } => "signer_inactive",
            DenyReason::PositionSizeExceeds { .. } => "position_size_exceeds",
            DenyReason::SlippageExceeds { .. } => "slippage_exceeds",
            DenyReason::RiskValuesInvalid => "risk_values_invalid",
            DenyReason::FenceFailed { .. } => "fence_failed",
            DenyReason::ContextIssue { .. } => "context_issue",
            DenyReason::DependencyError { .. } => "dependency_error",
        }
    }

    /// Human phrasing for logs and operator dashboards. Never includes
    /// secret material (there is none to include).
    pub fn detail(&self) -> String {
        match self {
            DenyReason::TenantState { state } => {
                format!("tenant is {state}; only active tenants may execute")
            }
            DenyReason::EntitlementModule { module, reason } => {
                format!("plan does not allow the {module} module ({reason})")
            }
            DenyReason::EntitlementLiveTrading => "plan does not allow live trading".into(),
            DenyReason::ModuleDisabled { module, reason } => {
                format!("module {module} is disabled in tenant configuration ({reason})")
            }
            DenyReason::ModeNotAllowed { mode } => {
                format!("trading mode {mode} is not allowed for this execution")
            }
            DenyReason::WalletNotBound { label } => {
                format!("wallet {label:?} is not an active wallet of this tenant")
            }
            DenyReason::SignerNotBound { key_ref } => {
                format!("signer {key_ref:?} is not an active signer of this tenant")
            }
            DenyReason::WalletInactive { label } => {
                format!("wallet {label:?} is not active for executions")
            }
            DenyReason::SignerInactive { key_ref } => {
                format!("signer {key_ref:?} is not active for executions")
            }
            DenyReason::PositionSizeExceeds { requested, cap } => {
                format!("requested size {requested} USD exceeds the effective cap {cap} USD")
            }
            DenyReason::SlippageExceeds {
                requested_bps,
                cap_bps,
            } => {
                format!("requested slippage {requested_bps} bps exceeds the effective cap {cap_bps} bps")
            }
            DenyReason::RiskValuesInvalid => {
                "risk values are not usable (negative or non-finite)".into()
            }
            DenyReason::FenceFailed { verdict } => {
                format!("runtime fence failed ({verdict})")
            }
            DenyReason::ContextIssue { reason } => {
                format!("execution context could not be issued: {reason}")
            }
            DenyReason::DependencyError { source } => {
                format!("guard dependency {source} failed; failing closed")
            }
        }
    }

    /// Is this deny retryable without operator action? Config/bindings
    /// denials clear when the tenant changes them; fence denials clear
    /// when the runtime recovers. Dependency errors may clear alone.
    /// State and context denials need attention.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            DenyReason::FenceFailed { .. } | DenyReason::DependencyError { .. }
        )
    }
}

/// The gateway's final answer.
#[derive(Debug, Clone, PartialEq)]
pub enum TenantDecision {
    /// All guards passed; the core context was issued. Carries the
    /// guard notes in chain order (for the execution trace).
    Allow(Vec<&'static str>),
    /// The first denying guard won; later guards did not run.
    Deny(DenyReason),
}

impl TenantDecision {
    /// Allowed?
    pub fn is_allow(&self) -> bool {
        matches!(self, TenantDecision::Allow(_))
    }

    /// The deny reason, when denying.
    pub fn deny_reason(&self) -> Option<&DenyReason> {
        match self {
            TenantDecision::Allow(_) => None,
            TenantDecision::Deny(reason) => Some(reason),
        }
    }

    /// Machine label for metrics/audit.
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantDecision::Allow(_) => "allow",
            TenantDecision::Deny(reason) => reason.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_deny_reason_has_a_stable_label_and_detail() {
        // One representative per shape; as_str is the metrics API.
        let reasons = vec![
            DenyReason::TenantState { state: "suspended" },
            DenyReason::EntitlementModule {
                module: "copy",
                reason: "module_not_in_plan",
            },
            DenyReason::EntitlementLiveTrading,
            DenyReason::ModuleDisabled {
                module: "sniper",
                reason: "operator",
            },
            DenyReason::ModeNotAllowed { mode: "live" },
            DenyReason::WalletNotBound {
                label: "main".into(),
            },
            DenyReason::SignerNotBound {
                key_ref: "k".into(),
            },
            DenyReason::WalletInactive {
                label: "main".into(),
            },
            DenyReason::SignerInactive {
                key_ref: "k".into(),
            },
            DenyReason::PositionSizeExceeds {
                requested: 10.0,
                cap: 1.0,
            },
            DenyReason::SlippageExceeds {
                requested_bps: 50,
                cap_bps: 10,
            },
            DenyReason::RiskValuesInvalid,
            DenyReason::FenceFailed {
                verdict: "superseded",
            },
            DenyReason::ContextIssue {
                reason: "authority",
            },
            DenyReason::DependencyError {
                source: "config_store",
            },
        ];
        let labels: Vec<&str> = reasons.iter().map(|r| r.as_str()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels must be unique");
        for reason in &reasons {
            assert!(!reason.detail().is_empty());
        }
    }

    #[test]
    fn retryability_is_conservative() {
        assert!(DenyReason::FenceFailed {
            verdict: "stale_generation"
        }
        .retryable());
        assert!(DenyReason::DependencyError { source: "db" }.retryable());
        assert!(!DenyReason::TenantState { state: "closed" }.retryable());
        assert!(!DenyReason::PositionSizeExceeds {
            requested: 1.0,
            cap: 1.0
        }
        .retryable());
    }

    #[test]
    fn decisions_expose_their_label() {
        assert_eq!(TenantDecision::Allow(vec![]).as_str(), "allow");
        assert_eq!(
            TenantDecision::Deny(DenyReason::EntitlementLiveTrading).as_str(),
            "entitlement_live_trading"
        );
    }
}
