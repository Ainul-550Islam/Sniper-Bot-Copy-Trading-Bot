//! Pure custody authorization policy (BATCH file 07).
//!
//! Validates tenant owns profile, signer active, organization allowed to trade,
//! capability exists, module allowed, revoked/closed denied, provider mismatch denied.
//! Returns structured, machine-readable deny reasons.

use crate::tenant::{OrganizationId, OrganizationStatus};

use super::model::{CustodyProfile, CustodyStatus, ProviderType, SignerRecord};

/// Structured deny reasons — machine-readable, secret-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyDenyReason {
    CrossTenant,
    ProfileNotActive,
    SignerNotActive,
    SignerRevoked,
    TenantClosed,
    TenantSuspended,
    TenantPastDue,
    MissingCapability,
    ModuleNotAllowed,
    ProviderMismatch,
    NotFound,
}

impl CustodyDenyReason {
    pub const ALL: [CustodyDenyReason; 11] = [
        CustodyDenyReason::CrossTenant,
        CustodyDenyReason::ProfileNotActive,
        CustodyDenyReason::SignerNotActive,
        CustodyDenyReason::SignerRevoked,
        CustodyDenyReason::TenantClosed,
        CustodyDenyReason::TenantSuspended,
        CustodyDenyReason::TenantPastDue,
        CustodyDenyReason::MissingCapability,
        CustodyDenyReason::ModuleNotAllowed,
        CustodyDenyReason::ProviderMismatch,
        CustodyDenyReason::NotFound,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            CustodyDenyReason::CrossTenant => "cross_tenant",
            CustodyDenyReason::ProfileNotActive => "profile_not_active",
            CustodyDenyReason::SignerNotActive => "signer_not_active",
            CustodyDenyReason::SignerRevoked => "signer_revoked",
            CustodyDenyReason::TenantClosed => "tenant_closed",
            CustodyDenyReason::TenantSuspended => "tenant_suspended",
            CustodyDenyReason::TenantPastDue => "tenant_past_due",
            CustodyDenyReason::MissingCapability => "missing_capability",
            CustodyDenyReason::ModuleNotAllowed => "module_not_allowed",
            CustodyDenyReason::ProviderMismatch => "provider_mismatch",
            CustodyDenyReason::NotFound => "not_found",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }
    pub fn detail(&self) -> &'static str {
        match self {
            CustodyDenyReason::CrossTenant => "custody resource belongs to another organization",
            CustodyDenyReason::ProfileNotActive => "custody profile is not active",
            CustodyDenyReason::SignerNotActive => "signer is not active",
            CustodyDenyReason::SignerRevoked => "signer has been revoked",
            CustodyDenyReason::TenantClosed => "organization is closed",
            CustodyDenyReason::TenantSuspended => "organization is suspended",
            CustodyDenyReason::TenantPastDue => "subscription is past due",
            CustodyDenyReason::MissingCapability => "signer lacks required capability",
            CustodyDenyReason::ModuleNotAllowed => "module is not allowed for this signer",
            CustodyDenyReason::ProviderMismatch => "custody provider mismatch",
            CustodyDenyReason::NotFound => "custody profile or signer not found",
        }
    }
}

impl std::fmt::Display for CustodyDenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodyVerdict {
    Allow,
    Deny(CustodyDenyReason, String),
}

impl CustodyVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, CustodyVerdict::Allow)
    }
    pub fn deny_reason(&self) -> Option<CustodyDenyReason> {
        match self {
            CustodyVerdict::Allow => None,
            CustodyVerdict::Deny(r, _) => Some(*r),
        }
    }
}

/// Request to use a signer for a module.
#[derive(Debug, Clone)]
pub struct CustodyRequest<'a> {
    pub organization_id: OrganizationId,
    pub organization_status: OrganizationStatus,
    pub requested_module: &'a str,
    pub required_capability: &'a str,
    pub expected_provider: Option<ProviderType>,
}

impl<'a> CustodyRequest<'a> {
    pub fn new(
        organization_id: OrganizationId,
        organization_status: OrganizationStatus,
        requested_module: &'a str,
        required_capability: &'a str,
    ) -> Self {
        Self {
            organization_id,
            organization_status,
            requested_module,
            required_capability,
            expected_provider: None,
        }
    }
    pub fn with_provider(mut self, p: ProviderType) -> Self {
        self.expected_provider = Some(p);
        self
    }
}

/// Pure policy check: tenant owns profile, signer active, tenant can trade, capability, module, provider mismatch, revoked.
pub fn check(
    profile: Option<&CustodyProfile>,
    signer: Option<&SignerRecord>,
    request: &CustodyRequest<'_>,
) -> CustodyVerdict {
    let Some(profile) = profile else {
        return CustodyVerdict::Deny(
            CustodyDenyReason::NotFound,
            "custody profile not found".into(),
        );
    };
    let Some(signer) = signer else {
        return CustodyVerdict::Deny(CustodyDenyReason::NotFound, "signer not found".into());
    };

    // Tenant ownership
    if profile.organization_id != request.organization_id
        || signer.organization_id != request.organization_id
    {
        return CustodyVerdict::Deny(
            CustodyDenyReason::CrossTenant,
            CustodyDenyReason::CrossTenant.detail().into(),
        );
    }
    if signer.custody_profile_id != profile.id {
        return CustodyVerdict::Deny(
            CustodyDenyReason::CrossTenant,
            "signer does not belong to profile".into(),
        );
    }

    // Tenant lifecycle
    match request.organization_status {
        OrganizationStatus::Closed => {
            return CustodyVerdict::Deny(
                CustodyDenyReason::TenantClosed,
                CustodyDenyReason::TenantClosed.detail().into(),
            )
        }
        OrganizationStatus::Suspended => {
            return CustodyVerdict::Deny(
                CustodyDenyReason::TenantSuspended,
                CustodyDenyReason::TenantSuspended.detail().into(),
            )
        }
        OrganizationStatus::PastDue => {
            return CustodyVerdict::Deny(
                CustodyDenyReason::TenantPastDue,
                CustodyDenyReason::TenantPastDue.detail().into(),
            )
        }
        _ => {}
    }

    // Profile must be active
    if profile.status != CustodyStatus::Active {
        return CustodyVerdict::Deny(
            CustodyDenyReason::ProfileNotActive,
            format!("profile status is {}", profile.status.as_str()),
        );
    }

    // Signer must be active
    if signer.status == CustodyStatus::Revoked || signer.status == CustodyStatus::Closed {
        return CustodyVerdict::Deny(
            CustodyDenyReason::SignerRevoked,
            format!("signer status is {}", signer.status.as_str()),
        );
    }
    if signer.status != CustodyStatus::Active {
        return CustodyVerdict::Deny(
            CustodyDenyReason::SignerNotActive,
            format!("signer status is {}", signer.status.as_str()),
        );
    }

    // Provider mismatch
    if let Some(expected) = request.expected_provider {
        if signer.provider_type != expected || profile.provider_type != expected {
            return CustodyVerdict::Deny(
                CustodyDenyReason::ProviderMismatch,
                format!(
                    "expected provider {} but profile is {} and signer is {}",
                    expected.as_str(),
                    profile.provider_type.as_str(),
                    signer.provider_type.as_str()
                ),
            );
        }
    }
    // Enforce profile and signer provider consistency
    if profile.provider_type != signer.provider_type {
        return CustodyVerdict::Deny(
            CustodyDenyReason::ProviderMismatch,
            format!(
                "profile provider {} != signer provider {}",
                profile.provider_type.as_str(),
                signer.provider_type.as_str()
            ),
        );
    }

    // Capability
    if !signer.has_capability(request.required_capability) {
        return CustodyVerdict::Deny(
            CustodyDenyReason::MissingCapability,
            format!("signer lacks capability {}", request.required_capability),
        );
    }
    if request.requested_module != request.required_capability
        && !signer.has_capability(request.requested_module)
    {
        // requested_module is the module the engine wants to use; it must also be in capabilities if distinct
        // For simplicity, we require both; callers should pass same string for single-capability cases.
        // To avoid breaking single-capability callers, we only enforce when they differ.
        return CustodyVerdict::Deny(
            CustodyDenyReason::ModuleNotAllowed,
            format!("module {} not allowed", request.requested_module),
        );
    }

    CustodyVerdict::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::model::{CustodyStatus, ProviderType, SignerRecord};
    use crate::tenant::OrganizationId;
    use chrono::Utc;

    fn active_profile(org: OrganizationId, provider: ProviderType) -> CustodyProfile {
        let mut p = CustodyProfile::new(org, "main", provider, Utc::now());
        p.status = CustodyStatus::Active;
        p
    }

    fn active_signer(
        org: OrganizationId,
        profile: &CustodyProfile,
        caps: Vec<&str>,
    ) -> SignerRecord {
        let mut s = SignerRecord::new(
            org,
            profile.id,
            "sniper",
            profile.provider_type,
            "Pubkey111",
            Utc::now(),
        );
        s.status = CustodyStatus::Active;
        s.capabilities = caps.into_iter().map(|c| c.to_string()).collect();
        s
    }

    fn request(org: OrganizationId) -> CustodyRequest<'static> {
        CustodyRequest::new(
            org,
            crate::tenant::OrganizationStatus::Active,
            "module.sniper",
            "module.sniper",
        )
    }

    #[test]
    fn allowed_when_all_conditions_met() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let signer = active_signer(org, &profile, vec!["module.sniper"]);
        let verdict = check(Some(&profile), Some(&signer), &request(org));
        assert!(verdict.is_allowed(), "{:?}", verdict);
    }

    #[test]
    fn cross_tenant_denied() {
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        let profile = active_profile(org_a, ProviderType::Local);
        let signer = active_signer(org_a, &profile, vec!["module.sniper"]);
        let verdict = check(Some(&profile), Some(&signer), &request(org_b));
        assert_eq!(verdict.deny_reason(), Some(CustodyDenyReason::CrossTenant));
    }

    #[test]
    fn revoked_signer_denied() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let mut signer = active_signer(org, &profile, vec!["module.sniper"]);
        signer.status = CustodyStatus::Revoked;
        let verdict = check(Some(&profile), Some(&signer), &request(org));
        assert_eq!(
            verdict.deny_reason(),
            Some(CustodyDenyReason::SignerRevoked)
        );
    }

    #[test]
    fn closed_tenant_denied() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let signer = active_signer(org, &profile, vec!["module.sniper"]);
        let mut req = request(org);
        req.organization_status = crate::tenant::OrganizationStatus::Closed;
        let verdict = check(Some(&profile), Some(&signer), &req);
        assert_eq!(verdict.deny_reason(), Some(CustodyDenyReason::TenantClosed));
    }

    #[test]
    fn provider_mismatch_denied() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Vault);
        let mut signer = active_signer(org, &profile, vec!["module.sniper"]);
        // Force signer to be local while profile is vault
        signer.provider_type = ProviderType::Local;
        let verdict = check(Some(&profile), Some(&signer), &request(org));
        assert_eq!(
            verdict.deny_reason(),
            Some(CustodyDenyReason::ProviderMismatch)
        );
    }

    #[test]
    fn missing_capability_denied() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let signer = active_signer(org, &profile, vec!["module.copy"]);
        let verdict = check(Some(&profile), Some(&signer), &request(org));
        assert_eq!(
            verdict.deny_reason(),
            Some(CustodyDenyReason::MissingCapability)
        );
    }

    #[test]
    fn not_found_when_missing() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let verdict = check(None, None, &request(org));
        assert_eq!(verdict.deny_reason(), Some(CustodyDenyReason::NotFound));
        let _signer = active_signer(org, &profile, vec!["module.sniper"]);
        let verdict2 = check(Some(&profile), None, &request(org));
        assert_eq!(verdict2.deny_reason(), Some(CustodyDenyReason::NotFound));
    }

    #[test]
    fn suspended_tenant_denied() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let signer = active_signer(org, &profile, vec!["module.sniper"]);
        let mut req = request(org);
        req.organization_status = crate::tenant::OrganizationStatus::Suspended;
        assert_eq!(
            check(Some(&profile), Some(&signer), &req).deny_reason(),
            Some(CustodyDenyReason::TenantSuspended)
        );
    }

    #[test]
    fn profile_not_active_denied() {
        let org = OrganizationId::new();
        let mut profile = active_profile(org, ProviderType::Local);
        profile.status = CustodyStatus::Pending;
        let signer = active_signer(org, &profile, vec!["module.sniper"]);
        // signer is pending too but profile check comes first
        let verdict = check(Some(&profile), Some(&signer), &request(org));
        assert_eq!(
            verdict.deny_reason(),
            Some(CustodyDenyReason::ProfileNotActive)
        );
    }

    #[test]
    fn expected_provider_mismatch_denied() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let signer = active_signer(org, &profile, vec!["module.sniper"]);
        let req = CustodyRequest::new(
            org,
            crate::tenant::OrganizationStatus::Active,
            "module.sniper",
            "module.sniper",
        )
        .with_provider(ProviderType::Kms);
        assert_eq!(
            check(Some(&profile), Some(&signer), &req).deny_reason(),
            Some(CustodyDenyReason::ProviderMismatch)
        );
    }

    #[test]
    fn signer_not_active_when_pending() {
        let org = OrganizationId::new();
        let profile = active_profile(org, ProviderType::Local);
        let mut signer = active_signer(org, &profile, vec!["module.sniper"]);
        signer.status = CustodyStatus::Pending;
        let verdict = check(Some(&profile), Some(&signer), &request(org));
        assert_eq!(
            verdict.deny_reason(),
            Some(CustodyDenyReason::SignerNotActive)
        );
    }
}
