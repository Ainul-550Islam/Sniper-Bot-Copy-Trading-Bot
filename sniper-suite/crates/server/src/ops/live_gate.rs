//! Central live-action safety gate (Batch 7).
//! Validate explicit operator enablement, environment, owner/admin authorization,
//! required provider readiness, custody readiness, audit readiness, risk configuration.
//! Fail closed. Tests for every denied combination.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateDecision {
    Allow,
    Deny { reason: String },
}

impl GateDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn deny_reason(&self) -> Option<&str> {
        match self {
            Self::Deny { reason } => Some(reason),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveGateRequest {
    pub enable_flag: bool,
    pub environment: String,
    pub is_owner_or_admin: bool,
    pub provider_ready: bool,
    pub custody_ready: bool,
    pub audit_ready: bool,
    pub risk_configured: bool,
    pub emergency_stop_available: bool,
}

impl LiveGateRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        enable_flag: bool,
        environment: impl Into<String>,
        is_owner_or_admin: bool,
        provider_ready: bool,
        custody_ready: bool,
        audit_ready: bool,
        risk_configured: bool,
        emergency_stop_available: bool,
    ) -> Self {
        Self {
            enable_flag,
            environment: environment.into(),
            is_owner_or_admin,
            provider_ready,
            custody_ready,
            audit_ready,
            risk_configured,
            emergency_stop_available,
        }
    }
}

pub struct LiveGate;

impl LiveGate {
    pub fn check(req: &LiveGateRequest) -> GateDecision {
        if !req.enable_flag {
            return GateDecision::Deny {
                reason: "explicit operator enablement required (flag not set)".into(),
            };
        }
        if req.environment.trim().is_empty() {
            return GateDecision::Deny {
                reason: "environment must be specified".into(),
            };
        }
        let env_lower = req.environment.to_lowercase();
        if env_lower == "production" && !req.is_owner_or_admin {
            return GateDecision::Deny {
                reason: "production requires owner/admin authorization".into(),
            };
        }
        if !req.is_owner_or_admin {
            return GateDecision::Deny {
                reason: "owner/admin authorization required".into(),
            };
        }
        if !req.provider_ready {
            return GateDecision::Deny {
                reason: "provider not ready".into(),
            };
        }
        if !req.custody_ready {
            return GateDecision::Deny {
                reason: "custody not ready".into(),
            };
        }
        if !req.audit_ready {
            return GateDecision::Deny {
                reason: "audit trail not ready".into(),
            };
        }
        if !req.risk_configured {
            return GateDecision::Deny {
                reason: "risk configuration missing".into(),
            };
        }
        if !req.emergency_stop_available {
            return GateDecision::Deny {
                reason: "emergency stop not available".into(),
            };
        }
        // All gates passed
        GateDecision::Allow
    }

    pub fn check_strict_production(req: &LiveGateRequest) -> GateDecision {
        if req.environment.to_lowercase() != "production" {
            return GateDecision::Deny {
                reason: "strict production gate requires environment=production".into(),
            };
        }
        Self::check(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow_req() -> LiveGateRequest {
        LiveGateRequest::new(true, "production", true, true, true, true, true, true)
    }

    #[test]
    fn allow_when_all_gates_pass() {
        assert!(LiveGate::check(&allow_req()).is_allow());
    }

    #[test]
    fn deny_when_not_enabled() {
        let mut r = allow_req();
        r.enable_flag = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("enablement"))
        );
    }

    #[test]
    fn deny_when_not_owner() {
        let mut r = allow_req();
        r.is_owner_or_admin = false;
        assert!(matches!(LiveGate::check(&r), GateDecision::Deny { .. }));
    }

    #[test]
    fn deny_when_provider_not_ready() {
        let mut r = allow_req();
        r.provider_ready = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("provider"))
        );
    }

    #[test]
    fn deny_when_custody_not_ready() {
        let mut r = allow_req();
        r.custody_ready = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("custody"))
        );
    }

    #[test]
    fn deny_when_audit_not_ready() {
        let mut r = allow_req();
        r.audit_ready = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("audit"))
        );
    }

    #[test]
    fn deny_when_risk_not_configured() {
        let mut r = allow_req();
        r.risk_configured = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("risk"))
        );
    }

    #[test]
    fn deny_when_emergency_stop_missing() {
        let mut r = allow_req();
        r.emergency_stop_available = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("emergency"))
        );
    }

    #[test]
    fn deny_when_environment_empty() {
        let mut r = allow_req();
        r.environment = "".into();
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("environment"))
        );
    }

    #[test]
    fn production_requires_owner_even_if_flag_set() {
        let mut r = allow_req();
        r.environment = "production".into();
        r.is_owner_or_admin = false;
        assert!(
            matches!(LiveGate::check(&r), GateDecision::Deny { reason } if reason.contains("owner"))
        );
    }

    #[test]
    fn strict_production_gate_requires_production_env() {
        let mut r = allow_req();
        r.environment = "staging".into();
        assert!(
            matches!(LiveGate::check_strict_production(&r), GateDecision::Deny { reason } if reason.contains("production"))
        );
        r.environment = "production".into();
        assert!(LiveGate::check_strict_production(&r).is_allow());
    }

    #[test]
    fn every_denied_combination_fails_closed() {
        // Ensure gate is fail-closed for all single failures
        let base = allow_req();
        let variants = vec![
            LiveGateRequest::new(false, "production", true, true, true, true, true, true),
            LiveGateRequest::new(true, "", true, true, true, true, true, true),
            LiveGateRequest::new(true, "production", false, true, true, true, true, true),
            LiveGateRequest::new(true, "production", true, false, true, true, true, true),
            LiveGateRequest::new(true, "production", true, true, false, true, true, true),
            LiveGateRequest::new(true, "production", true, true, true, false, true, true),
            LiveGateRequest::new(true, "production", true, true, true, true, false, true),
            LiveGateRequest::new(true, "production", true, true, true, true, true, false),
        ];
        for v in variants {
            assert!(!LiveGate::check(&v).is_allow(), "should deny: {:?}", v);
        }
        // Also check base allows
        assert!(LiveGate::check(&base).is_allow());
    }
}
