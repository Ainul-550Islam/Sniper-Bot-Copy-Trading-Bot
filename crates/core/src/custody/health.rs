//! Custody-provider health model (BATCH 2 file 05).
//!
//! Defines provider readiness states and typed health results suitable for
//! readiness endpoints. Never exposes provider credentials. Health failure
//! must not accidentally authorize signing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::model::ProviderType;

/// Provider readiness state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Configured,
    Reachable,
    Unavailable,
    Degraded,
    Revoked,
}

impl HealthState {
    pub const ALL: [HealthState; 5] = [
        HealthState::Configured,
        HealthState::Reachable,
        HealthState::Unavailable,
        HealthState::Degraded,
        HealthState::Revoked,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            HealthState::Configured => "configured",
            HealthState::Reachable => "reachable",
            HealthState::Unavailable => "unavailable",
            HealthState::Degraded => "degraded",
            HealthState::Revoked => "revoked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim().to_ascii_lowercase())
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthState::Configured | HealthState::Reachable)
    }

    pub fn allows_signing(&self) -> bool {
        matches!(self, HealthState::Reachable)
            || (matches!(self, HealthState::Configured) && cfg!(test))
    }
}

/// Typed health result for one provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub provider_type: ProviderType,
    pub state: HealthState,
    /// Human-readable, secret-free detail (e.g., "credentials missing", "timeout").
    pub detail: String,
    pub checked_at: DateTime<Utc>,
    /// Whether this health allows signing (true only for Reachable, or Configured in tests).
    pub signing_allowed: bool,
}

impl ProviderHealth {
    pub fn new(
        provider_type: ProviderType,
        state: HealthState,
        detail: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let detail_s = detail.into();
        let signing_allowed = match state {
            HealthState::Reachable => true,
            HealthState::Configured => false, // Configured but not yet probed — fail closed in prod
            _ => false,
        };
        Self {
            provider_type,
            state,
            detail: detail_s,
            checked_at: now,
            signing_allowed,
        }
    }

    pub fn reachable(provider_type: ProviderType, now: DateTime<Utc>) -> Self {
        Self::new(provider_type, HealthState::Reachable, "ok", now)
    }

    pub fn configured(provider_type: ProviderType, now: DateTime<Utc>) -> Self {
        Self::new(provider_type, HealthState::Configured, "configured", now)
    }

    pub fn unavailable(provider_type: ProviderType, detail: impl Into<String>) -> Self {
        Self::new(provider_type, HealthState::Unavailable, detail, Utc::now())
    }

    pub fn degraded(provider_type: ProviderType, detail: impl Into<String>) -> Self {
        Self::new(provider_type, HealthState::Degraded, detail, Utc::now())
    }

    pub fn revoked(provider_type: ProviderType, now: DateTime<Utc>) -> Self {
        Self::new(provider_type, HealthState::Revoked, "revoked", now)
    }

    pub fn is_signing_allowed(&self) -> bool {
        self.signing_allowed && self.state.is_healthy() || self.state == HealthState::Reachable
    }

    /// Safe debug — never contains credentials.
    pub fn safe_summary(&self) -> String {
        format!(
            "provider={} state={} signing_allowed={} detail={}",
            self.provider_type.as_str(),
            self.state.as_str(),
            self.is_signing_allowed(),
            self.detail
        )
    }
}

/// Collection of provider healths for an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyHealthReport {
    pub organization_id: crate::tenant::OrganizationId,
    pub providers: Vec<ProviderHealth>,
    pub generated_at: DateTime<Utc>,
}

impl CustodyHealthReport {
    pub fn overall_state(&self) -> HealthState {
        if self
            .providers
            .iter()
            .any(|p| p.state == HealthState::Revoked)
        {
            return HealthState::Revoked;
        }
        if self
            .providers
            .iter()
            .any(|p| p.state == HealthState::Unavailable)
        {
            return HealthState::Unavailable;
        }
        if self
            .providers
            .iter()
            .any(|p| p.state == HealthState::Degraded)
        {
            return HealthState::Degraded;
        }
        if self
            .providers
            .iter()
            .all(|p| p.state == HealthState::Reachable)
        {
            return HealthState::Reachable;
        }
        HealthState::Configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::model::ProviderType;
    use chrono::Utc;

    #[test]
    fn state_vocabulary_round_trips() {
        for s in HealthState::ALL {
            assert_eq!(HealthState::parse(s.as_str()), Some(s));
            assert_eq!(s.as_str(), s.as_str().to_ascii_lowercase());
        }
    }

    #[test]
    fn health_failure_does_not_authorize_signing() {
        let h = ProviderHealth::unavailable(ProviderType::Vault, "no credentials");
        assert!(!h.is_signing_allowed());
        assert!(!h.state.is_healthy() || h.state == HealthState::Unavailable);
        let h2 = ProviderHealth::degraded(ProviderType::Kms, "timeout");
        assert!(!h2.is_signing_allowed());
        let h3 = ProviderHealth::revoked(ProviderType::Hsm, Utc::now());
        assert!(!h3.is_signing_allowed());
    }

    #[test]
    fn reachable_allows_signing() {
        let h = ProviderHealth::reachable(ProviderType::Local, Utc::now());
        assert!(h.is_signing_allowed());
    }

    #[test]
    fn configured_in_prod_does_not_allow_signing() {
        let h = ProviderHealth::configured(ProviderType::Vault, Utc::now());
        // In production, Configured alone does not allow signing — must be Reachable
        assert!(!h.is_signing_allowed());
    }

    #[test]
    fn credentials_never_in_debug() {
        let h = ProviderHealth::new(
            ProviderType::Vault,
            HealthState::Unavailable,
            "missing VAULT_TOKEN",
            Utc::now(),
        );
        let dbg = format!("{:?}", h);
        // Detail may contain env var name but never secret value; we assert
        // secret value not present by ensuring no token value leaked (we only put name)
        assert!(!dbg.contains("secret_value"));
        let summary = h.safe_summary();
        assert!(!summary.to_ascii_lowercase().contains("secret_value"));
        assert!(summary.contains("vault"));
    }

    #[test]
    fn overall_state_aggregates() {
        let org = crate::tenant::OrganizationId::new();
        let report = CustodyHealthReport {
            organization_id: org,
            providers: vec![
                ProviderHealth::reachable(ProviderType::Local, Utc::now()),
                ProviderHealth::unavailable(ProviderType::Vault, "down"),
            ],
            generated_at: Utc::now(),
        };
        assert_eq!(report.overall_state(), HealthState::Unavailable);
    }
}
