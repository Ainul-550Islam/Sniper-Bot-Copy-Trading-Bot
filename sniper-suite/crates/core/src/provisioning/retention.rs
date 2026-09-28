//! Customer-data retention policies and purge eligibility (BATCH file 10).
//!
//! Distinguish operational data, credentials, sessions, API keys, control-plane audit,
//! financial/accounting records, legal/compliance records.
//! Only purge data when policy explicitly allows it. Provide deterministic eligibility checks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::tenant::OrganizationId;

/// Category of data under retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionCategory {
    Operational,
    Credentials,
    Sessions,
    ApiKeys,
    ControlPlaneAudit,
    FinancialAccounting,
    LegalCompliance,
}

impl RetentionCategory {
    pub const ALL: [RetentionCategory; 7] = [
        RetentionCategory::Operational,
        RetentionCategory::Credentials,
        RetentionCategory::Sessions,
        RetentionCategory::ApiKeys,
        RetentionCategory::ControlPlaneAudit,
        RetentionCategory::FinancialAccounting,
        RetentionCategory::LegalCompliance,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            RetentionCategory::Operational => "operational",
            RetentionCategory::Credentials => "credentials",
            RetentionCategory::Sessions => "sessions",
            RetentionCategory::ApiKeys => "api_keys",
            RetentionCategory::ControlPlaneAudit => "control_plane_audit",
            RetentionCategory::FinancialAccounting => "financial_accounting",
            RetentionCategory::LegalCompliance => "legal_compliance",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        // Accept both snake and the DB values without _audit
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "operational" => Some(RetentionCategory::Operational),
            "credentials" => Some(RetentionCategory::Credentials),
            "sessions" => Some(RetentionCategory::Sessions),
            "api_keys" | "apikeys" => Some(RetentionCategory::ApiKeys),
            "control_plane_audit" | "controlplaneaudit" => {
                Some(RetentionCategory::ControlPlaneAudit)
            }
            "financial_accounting" | "financial" => Some(RetentionCategory::FinancialAccounting),
            "legal_compliance" | "legal" => Some(RetentionCategory::LegalCompliance),
            _ => None,
        }
    }
    /// Whether this category is purgeable at all (financial truth is never automatically purged).
    pub fn purge_allowed_default(&self) -> bool {
        match self {
            RetentionCategory::Operational => true,
            RetentionCategory::Credentials => true,
            RetentionCategory::Sessions => true,
            RetentionCategory::ApiKeys => true,
            RetentionCategory::ControlPlaneAudit => false,
            RetentionCategory::FinancialAccounting => false,
            RetentionCategory::LegalCompliance => false,
        }
    }
    /// Default retention days (from migration 0021 seed)
    pub fn default_retention_days(&self) -> i32 {
        match self {
            RetentionCategory::Operational => 90,
            RetentionCategory::Credentials => 0,
            RetentionCategory::Sessions => 0,
            RetentionCategory::ApiKeys => 0,
            RetentionCategory::ControlPlaneAudit => 2555,
            RetentionCategory::FinancialAccounting => 2555,
            RetentionCategory::LegalCompliance => 2555,
        }
    }
}

/// Policy for one category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub category: RetentionCategory,
    pub retention_days: i32,
    pub purge_allowed: bool,
    pub description: String,
}

impl RetentionPolicy {
    pub fn new(category: RetentionCategory) -> Self {
        Self {
            category,
            retention_days: category.default_retention_days(),
            purge_allowed: category.purge_allowed_default(),
            description: String::new(),
        }
    }
    pub fn with_retention(mut self, days: i32) -> Self {
        self.retention_days = days;
        self
    }
    pub fn with_purge(mut self, allowed: bool) -> Self {
        self.purge_allowed = allowed;
        self
    }
}

/// Per-tenant retention state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionState {
    pub organization_id: OrganizationId,
    pub category: RetentionCategory,
    pub retention_deadline: DateTime<Utc>,
    pub purged: bool,
    pub purged_at: Option<DateTime<Utc>>,
}

impl RetentionState {
    pub fn new(
        organization_id: OrganizationId,
        category: RetentionCategory,
        deadline: DateTime<Utc>,
    ) -> Self {
        Self {
            organization_id,
            category,
            retention_deadline: deadline,
            purged: false,
            purged_at: None,
        }
    }

    /// Is this state eligible for purge at `now` under `policy`?
    pub fn is_purge_eligible(&self, policy: &RetentionPolicy, now: DateTime<Utc>) -> bool {
        is_purge_eligible(policy, self, now)
    }
}

/// Deterministic eligibility check: only purge when policy allows AND deadline passed AND not already purged.
pub fn is_purge_eligible(
    policy: &RetentionPolicy,
    state: &RetentionState,
    now: DateTime<Utc>,
) -> bool {
    if state.category != policy.category {
        return false;
    }
    if !policy.purge_allowed {
        return false;
    }
    if state.purged {
        return false;
    }
    now >= state.retention_deadline
}

/// Batch eligibility: which categories are purgeable now?
pub fn eligible_categories(
    policies: &[RetentionPolicy],
    states: &[RetentionState],
    now: DateTime<Utc>,
) -> Vec<RetentionCategory> {
    let mut out = Vec::new();
    for state in states {
        if let Some(policy) = policies.iter().find(|p| p.category == state.category) {
            if is_purge_eligible(policy, state, now) {
                out.push(state.category);
            }
        }
    }
    out
}

/// Categories that are NEVER purgeable regardless of deadline (financial truth etc.)
pub fn never_purge_categories() -> Vec<RetentionCategory> {
    RetentionCategory::ALL
        .iter()
        .copied()
        .filter(|c| !c.purge_allowed_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::OrganizationId;
    use chrono::Utc;

    #[test]
    fn purge_eligibility_is_deterministic() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let policy = RetentionPolicy::new(RetentionCategory::Operational);
        let state = RetentionState::new(
            org,
            RetentionCategory::Operational,
            now - chrono::Duration::days(1),
        );
        assert!(
            state.is_purge_eligible(&policy, now),
            "deadline passed and purge allowed"
        );

        // Not yet eligible
        let future = RetentionState::new(
            org,
            RetentionCategory::Operational,
            now + chrono::Duration::days(10),
        );
        assert!(!future.is_purge_eligible(&policy, now));

        // Financial truth never purgeable even after deadline
        let fin_policy = RetentionPolicy::new(RetentionCategory::FinancialAccounting);
        let fin_state = RetentionState::new(
            org,
            RetentionCategory::FinancialAccounting,
            now - chrono::Duration::days(100),
        );
        assert!(
            !fin_state.is_purge_eligible(&fin_policy, now),
            "financial truth must never be purged automatically"
        );
    }

    #[test]
    fn never_purge_includes_financial_and_audit() {
        let never = never_purge_categories();
        assert!(never.contains(&RetentionCategory::FinancialAccounting));
        assert!(never.contains(&RetentionCategory::ControlPlaneAudit));
        assert!(never.contains(&RetentionCategory::LegalCompliance));
        assert!(!never.contains(&RetentionCategory::Operational));
        assert!(!never.contains(&RetentionCategory::Credentials));
    }

    #[test]
    fn category_parsing() {
        assert_eq!(
            RetentionCategory::parse("operational"),
            Some(RetentionCategory::Operational)
        );
        assert_eq!(
            RetentionCategory::parse("api_keys"),
            Some(RetentionCategory::ApiKeys)
        );
        assert_eq!(
            RetentionCategory::parse("FINANCIAL_ACCOUNTING"),
            Some(RetentionCategory::FinancialAccounting)
        );
        assert_eq!(RetentionCategory::parse("unknown"), None);
    }

    #[test]
    fn already_purged_not_eligible() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let policy = RetentionPolicy::new(RetentionCategory::Credentials);
        let mut state = RetentionState::new(
            org,
            RetentionCategory::Credentials,
            now - chrono::Duration::days(1),
        );
        state.purged = true;
        state.purged_at = Some(now);
        assert!(!is_purge_eligible(&policy, &state, now));
    }

    #[test]
    fn eligible_categories_batch() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let policies = vec![
            RetentionPolicy::new(RetentionCategory::Operational),
            RetentionPolicy::new(RetentionCategory::FinancialAccounting),
            RetentionPolicy::new(RetentionCategory::Credentials),
        ];
        let states = vec![
            RetentionState::new(
                org,
                RetentionCategory::Operational,
                now - chrono::Duration::days(1),
            ),
            RetentionState::new(
                org,
                RetentionCategory::FinancialAccounting,
                now - chrono::Duration::days(1),
            ),
            RetentionState::new(
                org,
                RetentionCategory::Credentials,
                now + chrono::Duration::days(1),
            ),
        ];
        let eligible = eligible_categories(&policies, &states, now);
        assert_eq!(eligible, vec![RetentionCategory::Operational]);
    }

    #[test]
    fn policy_mismatch_not_eligible() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let policy = RetentionPolicy::new(RetentionCategory::Operational);
        let state = RetentionState::new(
            org,
            RetentionCategory::Credentials,
            now - chrono::Duration::days(1),
        );
        assert!(
            !is_purge_eligible(&policy, &state, now),
            "category mismatch must not be eligible"
        );
    }

    #[test]
    fn default_retention_days_match_migration() {
        assert_eq!(RetentionCategory::Operational.default_retention_days(), 90);
        assert_eq!(
            RetentionCategory::FinancialAccounting.default_retention_days(),
            2555
        );
        assert_eq!(RetentionCategory::Credentials.default_retention_days(), 0);
    }
}
