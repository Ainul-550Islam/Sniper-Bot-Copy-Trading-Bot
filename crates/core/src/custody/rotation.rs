//! Signer/custody rotation state machine (Batch 3).
//!
//! Supports pending, active, draining, revoked, failed.
//! Ensures old signer is not revoked before replacement is safely active
//! unless explicitly forced by security policy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

use super::model::{CustodyProfileId, ProviderType, SignerId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationState {
    Pending,
    Active,
    Draining,
    Revoked,
    Failed,
}

impl RotationState {
    pub const ALL: [RotationState; 5] = [
        RotationState::Pending,
        RotationState::Active,
        RotationState::Draining,
        RotationState::Revoked,
        RotationState::Failed,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            RotationState::Pending => "pending",
            RotationState::Active => "active",
            RotationState::Draining => "draining",
            RotationState::Revoked => "revoked",
            RotationState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, RotationState::Revoked | RotationState::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationRecord {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub profile_id: CustodyProfileId,
    pub old_signer: SignerId,
    pub new_signer: SignerId,
    pub provider_type: ProviderType,
    pub state: RotationState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub failure_reason: String,
    pub force_revoked: bool,
}

impl RotationRecord {
    pub fn new(
        organization_id: OrganizationId,
        profile_id: CustodyProfileId,
        old_signer: SignerId,
        new_signer: SignerId,
        provider_type: ProviderType,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id,
            profile_id,
            old_signer,
            new_signer,
            provider_type,
            state: RotationState::Pending,
            created_at: now,
            updated_at: now,
            activated_at: None,
            revoked_at: None,
            failure_reason: String::new(),
            force_revoked: false,
        }
    }

    pub fn can_transition_to(&self, to: RotationState, force: bool) -> bool {
        use RotationState::*;
        if self.state == to {
            return false;
        }
        match (self.state, to) {
            (Pending, Active) => true,
            (Pending, Failed) => true,
            (Pending, Revoked) if force => true, // emergency revoke
            (Active, Draining) => true,
            (Active, Failed) => true,
            (Active, Revoked) if force => true,
            (Draining, Revoked) => true,
            (Draining, Active) => true, // rollback
            (Draining, Failed) => true,
            (Failed, Pending) => true, // retry
            _ => false,
        }
    }

    pub fn transition(
        &mut self,
        to: RotationState,
        now: DateTime<Utc>,
        force: bool,
    ) -> Result<(), String> {
        if !self.can_transition_to(to, force) {
            return Err(format!(
                "illegal rotation {} -> {} (force={})",
                self.state.as_str(),
                to.as_str(),
                force
            ));
        }
        // Safety: non-force must not revoke old before new active
        if to == RotationState::Revoked && !force && self.state != RotationState::Draining {
            return Err("non-force revoke requires draining state; old signer must stay valid until replacement active".into());
        }
        self.state = to;
        self.updated_at = now;
        match to {
            RotationState::Active => self.activated_at = Some(now),
            RotationState::Revoked => {
                self.revoked_at = Some(now);
                self.force_revoked = force;
            }
            RotationState::Failed => {
                // keep failure_reason set by caller
            }
            _ => {}
        }
        Ok(())
    }

    pub fn mark_failed(&mut self, reason: impl Into<String>, now: DateTime<Utc>) {
        self.state = RotationState::Failed;
        self.failure_reason = reason.into();
        self.updated_at = now;
    }

    pub fn summary(&self) -> String {
        format!(
            "rotation={} org={} profile={} {}->{} state={} force={}",
            self.id,
            self.organization_id,
            self.profile_id,
            self.old_signer,
            self.new_signer,
            self.state.as_str(),
            self.force_revoked
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::model::{CustodyProfileId, ProviderType, SignerId};
    use crate::tenant::OrganizationId;
    use chrono::Utc;

    fn rec() -> RotationRecord {
        RotationRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Vault,
            Utc::now(),
        )
    }

    #[test]
    fn normal_lifecycle() {
        let now = Utc::now();
        let mut r = rec();
        assert!(r.transition(RotationState::Active, now, false).is_ok());
        assert!(r.transition(RotationState::Draining, now, false).is_ok());
        assert!(r.transition(RotationState::Revoked, now, false).is_ok());
        assert!(r.state.is_terminal());
    }

    #[test]
    fn non_force_revoke_without_draining_rejected() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(RotationState::Active, now, false).unwrap();
        // Direct revoke from Active without force should fail
        assert!(r.transition(RotationState::Revoked, now, false).is_err());
        // With force, allowed (emergency)
        assert!(r.transition(RotationState::Revoked, now, true).is_ok());
        assert!(r.force_revoked);
    }

    #[test]
    fn emergency_revoke_from_pending_with_force() {
        let now = Utc::now();
        let mut r = rec();
        assert!(r.transition(RotationState::Revoked, now, false).is_err());
        assert!(r.transition(RotationState::Revoked, now, true).is_ok());
    }

    #[test]
    fn rollback_from_draining_to_active() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(RotationState::Active, now, false).unwrap();
        r.transition(RotationState::Draining, now, false).unwrap();
        assert!(
            r.transition(RotationState::Active, now, false).is_ok(),
            "rollback safe"
        );
    }

    #[test]
    fn failed_can_retry() {
        let now = Utc::now();
        let mut r = rec();
        r.transition(RotationState::Failed, now, false).unwrap();
        assert!(r.transition(RotationState::Pending, now, false).is_ok());
    }

    #[test]
    fn tenant_ownership_preserved() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        let r1 = RotationRecord::new(
            org1,
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Local,
            Utc::now(),
        );
        let r2 = RotationRecord::new(
            org2,
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
            ProviderType::Local,
            Utc::now(),
        );
        assert_ne!(r1.organization_id, r2.organization_id);
    }

    #[test]
    fn failed_state_preserved() {
        let now = Utc::now();
        let mut r = rec();
        r.mark_failed("provider unreachable", now);
        assert_eq!(r.state, RotationState::Failed);
        assert!(r.failure_reason.contains("unreachable"));
    }
}
