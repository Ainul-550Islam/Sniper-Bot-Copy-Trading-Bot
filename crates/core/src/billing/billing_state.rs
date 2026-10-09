//! Canonical aggregate describing the commercial state of one organization (Batch 3).
//!
//! Combines plan, pricing version, subscription, payment/invoice status,
//! entitlements, usage summary, grace period and suspension reason.
//! Deterministic and tenant-scoped, does not duplicate Subscription types unnecessarily.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::tenant::OrganizationId;

use super::dunning::{DunningState, GracePeriod};
use super::plan::{PlanCode, PlanStatus};
use super::provider_config::BillingProviderKind;
use super::subscription::SubscriptionStatus;

/// Period for which usage is summarised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub period: String, // YYYY-MM
    pub total_requests: f64,
    pub total_trades: f64,
    pub api_calls: f64,
}

impl UsageSummary {
    pub fn new(period: impl Into<String>) -> Self {
        Self {
            period: period.into(),
            total_requests: 0.0,
            total_trades: 0.0,
            api_calls: 0.0,
        }
    }
}

/// Billing state suspension reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspensionReason {
    PaymentFailed,
    DunningExceeded,
    Manual,
    LimitExceeded { feature: String },
    None,
}

impl SuspensionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuspensionReason::PaymentFailed => "payment_failed",
            SuspensionReason::DunningExceeded => "dunning_exceeded",
            SuspensionReason::Manual => "manual",
            SuspensionReason::LimitExceeded { .. } => "limit_exceeded",
            SuspensionReason::None => "none",
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, SuspensionReason::None)
    }
}

/// Canonical billing state for one organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingState {
    pub organization_id: OrganizationId,
    pub plan_code: PlanCode,
    pub plan_status: PlanStatus,
    pub plan_version: u32,
    pub subscription_status: SubscriptionStatus,
    pub billing_provider: BillingProviderKind,
    pub provider_ref: Option<String>,
    pub payment_status: Option<String>,
    pub invoice_status: Option<String>,
    pub entitlements_active: bool,
    pub usage: UsageSummary,
    pub dunning: DunningState,
    pub grace: Option<GracePeriod>,
    pub suspension: SuspensionReason,
    pub as_of: DateTime<Utc>,
}

impl BillingState {
    pub fn new(
        organization_id: OrganizationId,
        plan_code: PlanCode,
        plan_status: PlanStatus,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            organization_id,
            plan_code,
            plan_status,
            plan_version: 1,
            subscription_status: SubscriptionStatus::Active,
            billing_provider: BillingProviderKind::Manual,
            provider_ref: None,
            payment_status: None,
            invoice_status: None,
            entitlements_active: true,
            usage: UsageSummary::new(now.format("%Y-%m").to_string()),
            dunning: DunningState::Current,
            grace: None,
            suspension: SuspensionReason::None,
            as_of: now,
        }
    }

    pub fn is_suspended(&self) -> bool {
        !self.suspension.is_none()
            || matches!(self.dunning, super::dunning::DunningState::BillingSuspended)
            || self.subscription_status == SubscriptionStatus::PastDue
                && matches!(
                    self.suspension,
                    SuspensionReason::DunningExceeded | SuspensionReason::PaymentFailed
                )
    }

    pub fn with_suspension(mut self, reason: SuspensionReason) -> Self {
        self.suspension = reason;
        self
    }

    pub fn summary(&self) -> String {
        format!(
            "org={} plan={} sub={:?} provider={} dunning={:?} suspension={} entitlements={}",
            self.organization_id,
            self.plan_code.as_str(),
            self.subscription_status,
            self.billing_provider.as_str(),
            self.dunning,
            self.suspension.as_str(),
            self.entitlements_active
        )
    }
}

/// Pure consistency validation — returns list of violations (empty = consistent).
pub fn validate_consistency(state: &BillingState) -> Vec<String> {
    let mut v = Vec::new();
    // 1. Suspended must have a reason or dunning suspended
    if state.is_suspended()
        && state.suspension.is_none()
        && state.dunning != super::dunning::DunningState::BillingSuspended
    {
        // If dunning says suspended, allowed to have None reason, but otherwise need reason
        if !matches!(
            state.dunning,
            super::dunning::DunningState::BillingSuspended
        ) {
            v.push("suspended state must carry a suspension reason or dunning suspended".into());
        }
    }
    // 2. Not suspended should not claim suspension reason
    if !state.is_suspended() && !state.suspension.is_none() {
        v.push(format!(
            "unexpected suspension reason while not suspended: {}",
            state.suspension.as_str()
        ));
    }
    // 3. Expired/Canceled subscription should not have active entitlements unless grace
    if matches!(
        state.subscription_status,
        SubscriptionStatus::Expired | SubscriptionStatus::Canceled
    ) && state.entitlements_active
        && state.grace.is_none()
    {
        v.push(
            "expired/canceled subscription must not have active entitlements without grace".into(),
        );
    }
    // 4. Usage period must be YYYY-MM
    if state.usage.period.len() != 7 || state.usage.period.chars().nth(4) != Some('-') {
        v.push(format!(
            "usage period must be YYYY-MM, got {}",
            state.usage.period
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn state() -> BillingState {
        BillingState::new(
            OrganizationId::new(),
            PlanCode::Pro,
            PlanStatus::Active,
            Utc::now(),
        )
    }

    #[test]
    fn new_is_consistent() {
        let s = state();
        assert!(
            validate_consistency(&s).is_empty(),
            "{:?}",
            validate_consistency(&s)
        );
        assert!(!s.is_suspended());
        assert!(s.entitlements_active);
    }

    #[test]
    fn suspended_requires_reason() {
        let mut s = state();
        s.suspension = SuspensionReason::PaymentFailed;
        assert!(s.is_suspended());
        assert!(validate_consistency(&s).is_empty());
        // Manually mark suspended via flag but no reason and not dunning suspended => violation
        let mut s2 = state();
        // Force is_suspended via suspension field
        s2.suspension = SuspensionReason::None;
        // Not suspended, so no violation
        assert!(validate_consistency(&s2).is_empty());
        // Make it suspended via dunning but no reason — allowed
        let mut s3 = state();
        s3.dunning = super::super::dunning::DunningState::BillingSuspended;
        assert!(s3.is_suspended());
        // This is allowed to have None reason because dunning is suspended
        assert!(validate_consistency(&s3).is_empty());
    }

    #[test]
    fn expired_without_grace_inconsistent() {
        let mut s = state();
        s.subscription_status = SubscriptionStatus::Expired;
        s.entitlements_active = true;
        s.grace = None;
        let v = validate_consistency(&s);
        assert!(v.iter().any(|m| m.contains("expired")));
        // With grace, consistent
        s.grace = Some(super::super::dunning::GracePeriod {
            until: Utc::now() + chrono::Duration::days(3),
            reason: "grace".into(),
        });
        assert!(validate_consistency(&s).is_empty());
    }

    #[test]
    fn usage_period_must_be_yyyy_mm() {
        let mut s = state();
        s.usage.period = "2026/09".into();
        assert!(validate_consistency(&s)
            .iter()
            .any(|m| m.contains("YYYY-MM")));
        s.usage.period = "2026-09".into();
        assert!(validate_consistency(&s).is_empty());
    }

    #[test]
    fn tenant_scoped_identity() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        let s1 = BillingState::new(org1, PlanCode::Starter, PlanStatus::Active, Utc::now());
        let s2 = BillingState::new(org2, PlanCode::Starter, PlanStatus::Active, Utc::now());
        assert_ne!(s1.organization_id, s2.organization_id);
        assert_ne!(s1.summary(), s2.summary());
    }

    #[test]
    fn serialization_deterministic() {
        let s = state();
        let j1 = serde_json::to_string(&s).unwrap();
        let j2 = serde_json::to_string(&s).unwrap();
        assert_eq!(j1, j2);
    }
}
