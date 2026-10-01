//! Guard 5/7: wallet + signer ownership (STEP 3 file 21).
//!
//! The cross-tenant firewall. A request names a wallet LABEL and a
//! signer KEY REF; this guard resolves them through the tenant's OWN
//! binding registry slice and denies anything that is not an ACTIVE
//! binding of the REQUESTING tenant. Another tenant's wallet is not an
//! error here — it simply does not exist in this tenant's slice, which
//! is exactly the deny the audit trail should show.

use std::sync::Arc;

use bot_core::tenant::OrganizationId;

use crate::tenant::registry::TenantBindingRegistry;

use super::decision::{DenyReason, GuardOutcome};
use super::request::TenantExecutionRequest;

/// Check both bindings (wallet first — it is the cheaper lookup in
/// practice and the more common misconfiguration).
pub async fn check(
    registry: &Arc<dyn TenantBindingRegistry>,
    request: &TenantExecutionRequest,
) -> GuardOutcome {
    match wallet(registry, request.organization_id, &request.wallet_label).await {
        GuardOutcome::Allow(_) => signer(registry, request.organization_id, &request.signer).await,
        denied => denied,
    }
}

/// The wallet half of the guard.
pub async fn wallet(
    registry: &Arc<dyn TenantBindingRegistry>,
    organization_id: OrganizationId,
    label: &str,
) -> GuardOutcome {
    let binding = match registry.wallet(organization_id, label).await {
        Ok(binding) => binding,
        Err(_) => {
            return GuardOutcome::Deny(DenyReason::DependencyError {
                source: "binding_registry",
            })
        }
    };
    match binding {
        Some(binding) if binding.active => GuardOutcome::Allow("wallet_bound"),
        Some(_) => GuardOutcome::Deny(DenyReason::WalletInactive {
            label: label.to_string(),
        }),
        None => GuardOutcome::Deny(DenyReason::WalletNotBound {
            label: label.to_string(),
        }),
    }
}

/// The signer half of the guard.
pub async fn signer(
    registry: &Arc<dyn TenantBindingRegistry>,
    organization_id: OrganizationId,
    binding: &super::request::SignerBinding,
) -> GuardOutcome {
    let row = match registry
        .signer(organization_id, binding.provider, &binding.key_ref)
        .await
    {
        Ok(row) => row,
        Err(_) => {
            return GuardOutcome::Deny(DenyReason::DependencyError {
                source: "binding_registry",
            })
        }
    };
    match row {
        Some(row) if row.active => GuardOutcome::Allow("signer_bound"),
        Some(_) => GuardOutcome::Deny(DenyReason::SignerInactive {
            key_ref: binding.key_ref.clone(),
        }),
        None => GuardOutcome::Deny(DenyReason::SignerNotBound {
            key_ref: binding.key_ref.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::registry::{MemoryTenantBindingRegistry, SignerBindingRow, WalletBinding};
    use crate::tenant::request::SignerBinding as RequestSigner;
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::SignerProvider;
    use chrono::Utc;

    async fn seeded() -> (Arc<dyn TenantBindingRegistry>, OrganizationId) {
        let registry: Arc<dyn TenantBindingRegistry> = Arc::new(MemoryTenantBindingRegistry::new());
        let org = OrganizationId::new();
        registry
            .put_wallet(WalletBinding {
                organization_id: org,
                label: "main".into(),
                address: "addr-main".into(),
                active: true,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        registry
            .put_signer(SignerBindingRow {
                organization_id: org,
                provider: SignerProvider::Custody,
                key_ref: "key-7".into(),
                active: true,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        (registry, org)
    }

    fn request(org: OrganizationId, wallet: &str, key_ref: &str) -> TenantExecutionRequest {
        TenantExecutionRequest::new(
            org,
            BotModule::Copy,
            ExecutionMode::Paper,
            wallet,
            RequestSigner {
                provider: SignerProvider::Custody,
                key_ref: key_ref.into(),
            },
        )
    }

    #[tokio::test]
    async fn bound_and_active_passes() {
        let (registry, org) = seeded().await;
        assert!(check(&registry, &request(org, "main", "key-7"))
            .await
            .is_allow());
    }

    #[tokio::test]
    async fn another_tenants_wallet_is_simply_not_bound() {
        let (registry, _org) = seeded().await;
        let outsider = OrganizationId::new();
        let outcome = check(&registry, &request(outsider, "main", "key-7")).await;
        assert_eq!(
            outcome.deny_reason().unwrap().as_str(),
            "wallet_not_bound",
            "cross-tenant access resolves to not-bound, never to someone else's wallet"
        );
    }

    #[tokio::test]
    async fn unknown_and_inactive_are_distinct_denies() {
        let (registry, org) = seeded().await;
        assert_eq!(
            check(&registry, &request(org, "ghost", "key-7"))
                .await
                .deny_reason()
                .unwrap()
                .as_str(),
            "wallet_not_bound"
        );
        assert_eq!(
            check(&registry, &request(org, "main", "ghost-key"))
                .await
                .deny_reason()
                .unwrap()
                .as_str(),
            "signer_not_bound"
        );

        // Deactivate the wallet: the deny becomes inactive, not missing.
        let mut row = registry.wallet(org, "main").await.unwrap().unwrap();
        row.active = false;
        registry.put_wallet(row).await.unwrap();
        assert_eq!(
            check(&registry, &request(org, "main", "key-7"))
                .await
                .deny_reason()
                .unwrap()
                .as_str(),
            "wallet_inactive"
        );
    }
}
