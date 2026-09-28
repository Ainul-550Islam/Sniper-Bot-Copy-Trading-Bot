//! Strongly typed custody identifiers and records (BATCH file 06).
//!
//! ProviderType includes local, vault, kms, hsm. No private keys or raw provider credentials stored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

/// Custody profile identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CustodyProfileId(pub Uuid);

impl CustodyProfileId {
    pub fn new() -> Self {
        CustodyProfileId(Uuid::new_v4())
    }
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(CustodyProfileId)
    }
}
impl Default for CustodyProfileId {
    fn default() -> Self {
        CustodyProfileId::new()
    }
}
impl std::fmt::Display for CustodyProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Signer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SignerId(pub Uuid);

impl SignerId {
    pub fn new() -> Self {
        SignerId(Uuid::new_v4())
    }
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(SignerId)
    }
}
impl Default for SignerId {
    fn default() -> Self {
        SignerId::new()
    }
}
impl std::fmt::Display for SignerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Provider type vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Local,
    Vault,
    Kms,
    Hsm,
}

impl ProviderType {
    pub const ALL: [ProviderType; 4] = [
        ProviderType::Local,
        ProviderType::Vault,
        ProviderType::Kms,
        ProviderType::Hsm,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Local => "local",
            ProviderType::Vault => "vault",
            ProviderType::Kms => "kms",
            ProviderType::Hsm => "hsm",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim().to_ascii_lowercase())
    }
    pub fn is_remote(&self) -> bool {
        !matches!(self, ProviderType::Local)
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Custody status lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyStatus {
    Pending,
    Active,
    Suspended,
    Revoked,
    Closed,
}

impl CustodyStatus {
    pub const ALL: [CustodyStatus; 5] = [
        CustodyStatus::Pending,
        CustodyStatus::Active,
        CustodyStatus::Suspended,
        CustodyStatus::Revoked,
        CustodyStatus::Closed,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            CustodyStatus::Pending => "pending",
            CustodyStatus::Active => "active",
            CustodyStatus::Suspended => "suspended",
            CustodyStatus::Revoked => "revoked",
            CustodyStatus::Closed => "closed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }
    pub fn is_usable(&self) -> bool {
        matches!(self, CustodyStatus::Active)
    }
    #[allow(clippy::match_like_matches_macro)]
    pub fn can_transition_to(&self, to: CustodyStatus) -> bool {
        use CustodyStatus::*;
        if *self == to {
            return false;
        }
        match (self, to) {
            (Pending, Active) => true,
            (Pending, Revoked) => true,
            (Pending, Closed) => true,
            (Active, Suspended) => true,
            (Active, Revoked) => true,
            (Active, Closed) => true,
            (Suspended, Active) => true,
            (Suspended, Revoked) => true,
            (Suspended, Closed) => true,
            (Revoked, Closed) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for CustodyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Custody profile — tenant-scoped boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyProfile {
    pub id: CustodyProfileId,
    pub organization_id: OrganizationId,
    pub name: String,
    pub description: String,
    pub provider_type: ProviderType,
    pub status: CustodyStatus,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoke_reason: String,
}

impl CustodyProfile {
    pub fn new(
        organization_id: OrganizationId,
        name: impl Into<String>,
        provider_type: ProviderType,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: CustodyProfileId::new(),
            organization_id,
            name: name.into(),
            description: String::new(),
            provider_type,
            status: CustodyStatus::Pending,
            created_by: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
            revoked_at: None,
            revoke_reason: String::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == CustodyStatus::Active
    }
    pub fn summary(&self) -> String {
        format!(
            "custody_profile={} org={} provider={} status={}",
            self.id,
            self.organization_id,
            self.provider_type,
            self.status.as_str()
        )
    }
}

/// Signer record — logical signer identity with public address only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignerRecord {
    pub id: SignerId,
    pub organization_id: OrganizationId,
    pub custody_profile_id: CustodyProfileId,
    pub logical_identity: String,
    pub provider_type: ProviderType,
    /// Public address (e.g., Solana pubkey base58, EVM address). Never a secret.
    pub public_address: String,
    /// Opaque provider reference (Vault path, KMS ARN, etc.) — not a secret credential.
    pub provider_ref: Option<String>,
    /// Capabilities: which modules may use this signer (e.g., ["module.sniper"])
    pub capabilities: Vec<String>,
    pub status: CustodyStatus,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoke_reason: String,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl SignerRecord {
    pub fn new(
        organization_id: OrganizationId,
        custody_profile_id: CustodyProfileId,
        logical_identity: impl Into<String>,
        provider_type: ProviderType,
        public_address: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: SignerId::new(),
            organization_id,
            custody_profile_id,
            logical_identity: logical_identity.into(),
            provider_type,
            public_address: public_address.into(),
            provider_ref: None,
            capabilities: Vec::new(),
            status: CustodyStatus::Pending,
            created_by: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
            revoked_at: None,
            revoke_reason: String::new(),
            last_used_at: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == CustodyStatus::Active
    }
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }

    pub fn summary(&self) -> String {
        format!(
            "signer={} org={} profile={} identity={} provider={} status={} address={}",
            self.id,
            self.organization_id,
            self.custody_profile_id,
            self.logical_identity,
            self.provider_type,
            self.status.as_str(),
            self.public_address
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn provider_type_vocabulary() {
        for p in ProviderType::ALL {
            assert_eq!(ProviderType::parse(p.as_str()), Some(p));
        }
        assert_eq!(ProviderType::parse("VAULT"), Some(ProviderType::Vault));
        assert!(!ProviderType::Local.is_remote());
        assert!(ProviderType::Kms.is_remote());
    }

    #[test]
    fn custody_status_transitions() {
        assert!(CustodyStatus::Pending.can_transition_to(CustodyStatus::Active));
        assert!(CustodyStatus::Active.can_transition_to(CustodyStatus::Revoked));
        assert!(!CustodyStatus::Revoked.can_transition_to(CustodyStatus::Active));
        assert!(!CustodyStatus::Active.can_transition_to(CustodyStatus::Active));
        assert!(CustodyStatus::Active.is_usable());
        assert!(!CustodyStatus::Pending.is_usable());
    }

    #[test]
    fn signer_capability_check() {
        let now = Utc::now();
        let mut s = SignerRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            "sniper",
            ProviderType::Local,
            "Addr123",
            now,
        );
        s.capabilities = vec!["module.sniper".into()];
        assert!(s.has_capability("module.sniper"));
        assert!(!s.has_capability("module.copy"));
        assert!(!s.summary().contains("private"));
        assert!(!s.summary().contains("secret"));
    }

    #[test]
    fn ids_are_typed_and_parse() {
        let pid = CustodyProfileId::new();
        let sid = SignerId::new();
        assert_eq!(CustodyProfileId::parse(&pid.to_string()), Some(pid));
        assert_eq!(SignerId::parse(&sid.to_string()), Some(sid));
        assert_ne!(pid.to_string(), sid.to_string());
    }

    #[test]
    fn no_private_key_in_serialization() {
        let now = Utc::now();
        let p = CustodyProfile::new(OrganizationId::new(), "main", ProviderType::Vault, now);
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.to_ascii_lowercase().contains("private"));
        assert!(!json.to_ascii_lowercase().contains("secret"));
        let s = SignerRecord::new(
            OrganizationId::new(),
            p.id,
            "sniper",
            ProviderType::Vault,
            "Pubkey123",
            now,
        );
        let json2 = serde_json::to_string(&s).unwrap();
        assert!(!json2.to_ascii_lowercase().contains("private"));
    }
}
