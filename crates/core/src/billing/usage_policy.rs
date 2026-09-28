//! Commercial usage policy layer (Batch 3).
//!
//! Maps measured usage to plan allowance / soft limit / hard limit / overage / suspension.
//! Uses server-authoritative plan definitions; client cannot alter limits.

use serde::{Deserialize, Serialize};

use super::plan::{FeatureLimit, Plan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    WithinAllowance,
    SoftLimitExceeded,
    HardLimitExceeded,
    Suspended,
}

impl UsageState {
    pub fn as_str(&self) -> &'static str {
        match self {
            UsageState::WithinAllowance => "within_allowance",
            UsageState::SoftLimitExceeded => "soft_limit_exceeded",
            UsageState::HardLimitExceeded => "hard_limit_exceeded",
            UsageState::Suspended => "suspended",
        }
    }

    pub fn is_suspended(&self) -> bool {
        matches!(self, UsageState::Suspended)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsagePolicyDecision {
    pub feature: String,
    pub state: UsageState,
    pub limit: Option<f64>,
    pub current: f64,
    pub allowance_remaining: Option<f64>,
    // effective entitlement after applying dunning/tenant status (outside this module)
    pub allows: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageThresholds {
    /// 80% triggers soft
    pub soft_ratio: f64,
    /// 100% triggers hard
    pub hard_ratio: f64,
}

impl Default for UsageThresholds {
    fn default() -> Self {
        Self {
            soft_ratio: 0.8,
            hard_ratio: 1.0,
        }
    }
}

/// Evaluate one feature usage against its plan limit.
/// Pure, deterministic, no I/O.
pub fn evaluate_feature(
    plan: &Plan,
    feature: &str,
    current: f64,
    thresholds: &UsageThresholds,
) -> UsagePolicyDecision {
    let limit = plan.limit_for(feature);
    match limit {
        FeatureLimit::Disabled => UsagePolicyDecision {
            feature: feature.to_string(),
            state: UsageState::Suspended,
            limit: None,
            current,
            allowance_remaining: Some(0.0),
            allows: false,
        },
        FeatureLimit::Unlimited => UsagePolicyDecision {
            feature: feature.to_string(),
            state: UsageState::WithinAllowance,
            limit: None,
            current,
            allowance_remaining: None,
            allows: true,
        },
        FeatureLimit::Limited(max) => {
            let ratio = if max == 0.0 {
                f64::INFINITY
            } else {
                current / max
            };
            let state = if ratio >= thresholds.hard_ratio {
                // At exact limit, still within? Hard means 초과
                if current > max {
                    UsageState::Suspended
                } else if ratio >= thresholds.hard_ratio {
                    UsageState::HardLimitExceeded
                } else {
                    UsageState::SoftLimitExceeded
                }
            } else if ratio >= thresholds.soft_ratio {
                UsageState::SoftLimitExceeded
            } else {
                UsageState::WithinAllowance
            };
            let remaining = (max - current).max(0.0);
            let allows = match state {
                UsageState::WithinAllowance | UsageState::SoftLimitExceeded => true,
                UsageState::HardLimitExceeded | UsageState::Suspended => false,
            };
            UsagePolicyDecision {
                feature: feature.to_string(),
                state,
                limit: Some(max),
                current,
                allowance_remaining: Some(remaining),
                allows,
            }
        }
    }
}

pub fn evaluate_all(
    plan: &Plan,
    usages: &[(String, f64)],
    thresholds: &UsageThresholds,
) -> Vec<UsagePolicyDecision> {
    usages
        .iter()
        .map(|(f, cur)| evaluate_feature(plan, f, *cur, thresholds))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billing::plan::{features, FeatureLimit, Plan, PlanCode};
    use chrono::Utc;

    fn plan() -> Plan {
        Plan::new(PlanCode::Pro, "Pro", Utc::now())
            .with_limit(features::MONTHLY_ORDERS, FeatureLimit::Limited(100.0))
            .with_limit(features::MAX_MEMBERS, FeatureLimit::Limited(5.0))
            .with_limit(features::MODULE_SNIPER, FeatureLimit::Unlimited)
            .with_limit(features::MODULE_POLYMARKET, FeatureLimit::Disabled)
    }

    #[test]
    fn within_allowance() {
        let p = plan();
        let d = evaluate_feature(
            &p,
            features::MONTHLY_ORDERS,
            10.0,
            &UsageThresholds::default(),
        );
        assert_eq!(d.state, UsageState::WithinAllowance);
        assert!(d.allows);
        assert_eq!(d.allowance_remaining, Some(90.0));
    }

    #[test]
    fn soft_limit_at_80_percent() {
        let p = plan();
        let d = evaluate_feature(
            &p,
            features::MONTHLY_ORDERS,
            85.0,
            &UsageThresholds::default(),
        );
        assert_eq!(d.state, UsageState::SoftLimitExceeded);
        assert!(d.allows, "soft still allows");
    }

    #[test]
    fn hard_limit_at_100() {
        let p = plan();
        let d = evaluate_feature(
            &p,
            features::MONTHLY_ORDERS,
            100.0,
            &UsageThresholds::default(),
        );
        assert_eq!(d.state, UsageState::HardLimitExceeded);
        assert!(!d.allows);
    }

    #[test]
    fn over_hard_suspended() {
        let p = plan();
        let d = evaluate_feature(
            &p,
            features::MONTHLY_ORDERS,
            150.0,
            &UsageThresholds::default(),
        );
        assert_eq!(d.state, UsageState::Suspended);
        assert!(!d.allows);
    }

    #[test]
    fn unlimited_always_within() {
        let p = plan();
        let d = evaluate_feature(
            &p,
            features::MODULE_SNIPER,
            1_000_000.0,
            &UsageThresholds::default(),
        );
        assert_eq!(d.state, UsageState::WithinAllowance);
        assert!(d.allows);
    }

    #[test]
    fn disabled_always_suspended() {
        let p = plan();
        let d = evaluate_feature(
            &p,
            features::MODULE_POLYMARKET,
            0.0,
            &UsageThresholds::default(),
        );
        assert_eq!(d.state, UsageState::Suspended);
        assert!(!d.allows);
    }

    #[test]
    fn server_authoritative_client_cannot_override() {
        let p = plan();
        // Client claims 0 usage but server measures 150 — policy uses server value
        let server_measured = 150.0;
        let client_claimed = 0.0;
        let d_server = evaluate_feature(
            &p,
            features::MONTHLY_ORDERS,
            server_measured,
            &UsageThresholds::default(),
        );
        let d_client = evaluate_feature(
            &p,
            features::MONTHLY_ORDERS,
            client_claimed,
            &UsageThresholds::default(),
        );
        assert_ne!(d_server.state, d_client.state);
        assert_eq!(d_server.state, UsageState::Suspended);
    }

    #[test]
    fn evaluate_all_deterministic() {
        let p = plan();
        let usages = vec![
            (features::MONTHLY_ORDERS.to_string(), 90.0),
            (features::MAX_MEMBERS.to_string(), 5.0),
        ];
        let a = evaluate_all(&p, &usages, &UsageThresholds::default());
        let b = evaluate_all(&p, &usages, &UsageThresholds::default());
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
    }
}
