//! Wallet / signer mismatch denial tests (tenant-isolation file 69).
//!
//! Positive: a tenant's own ACTIVE wallet and signer bindings pass
//! the focused guards and the composed gateway chain.
//!
//! Negative: another tenant's wallet label / signer key ref is
//! invisible (not bound); an INACTIVE own binding is refused; a check
//! naming a foreign tenant is a context violation; and through the
//! gateway the same mismatches produce the machine-readable
//! `wallet_not_bound` / `signer_not_bound` reasons.

use std::sync::Arc;

use chrono::Utc;

use sniper_suite::tenant::context::{ContextOrigin, TenantContext};
use sniper_suite::tenant::registry::{
    MemoryTenantBindingRegistry, SignerBindingRow, TenantBindingRegistry, WalletBinding,
};
use sniper_suite::tenant::request::{SignerBinding, TenantExecutionRequest};
use sniper_suite::tenant::signer_guard::SignerGuard;
use sniper_suite::tenant::wallet_guard::WalletGuard;

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{Organization, OrganizationId, SignerProvider};

const MAIN_ADDRESS: &str = "WalletMain1111111111111111111111111111111111111";
const TEST_MODES: [ExecutionMode; 2] = [ExecutionMode::Paper, ExecutionMode::Live];

fn context_for(organization_id: OrganizationId) -> TenantContext {
    let organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
    TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap()
}

async fn seeded() -> (Arc<MemoryTenantBindingRegistry>, OrganizationId) {
    let registry = Arc::new(MemoryTenantBindingRegistry::new());
    let org = OrganizationId::new();
    registry
        .put_wallet(WalletBinding {
            organization_id: org,
            label: "main".into(),
            address: MAIN_ADDRESS.into(),
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

fn signer_binding(key_ref: &str) -> SignerBinding {
    SignerBinding {
        provider: SignerProvider::Custody,
        key_ref: key_ref.into(),
    }
}

#[tokio::test]
async fn the_own_active_wallet_and_signer_pass() {
    let (registry, org) = seeded().await;
    let wallet_guard = WalletGuard::new(registry.clone());
    let signer_guard = SignerGuard::new(registry.clone());
    let context = context_for(org);

    let wallet_outcome = wallet_guard.check(&context, "main", org).await;
    assert!(
        wallet_outcome.is_allow(),
        "{:?}",
        wallet_outcome.deny_reason()
    );

    let signer_outcome = signer_guard
        .check(&context, &signer_binding("key-7"), org)
        .await;
    assert!(
        signer_outcome.is_allow(),
        "{:?}",
        signer_outcome.deny_reason()
    );

    // Resolve helpers hand back the OWN bindings.
    let wallet = wallet_guard
        .resolve(&context, "main")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wallet.address, MAIN_ADDRESS);
    assert_eq!(wallet.organization_id, org);
    let signer = signer_guard
        .resolve(&context, &signer_binding("key-7"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(signer.key_ref, "key-7");
}

#[tokio::test]
async fn another_tenants_wallet_label_is_not_bound_here() {
    let (registry, _org) = seeded().await;
    let wallet_guard = WalletGuard::new(registry);
    let other = OrganizationId::new();
    // Tenant B has NO bindings; the label tenant A bound does not
    // exist in B's slice.
    let outcome = wallet_guard.check(&context_for(other), "main", other).await;
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "wallet_not_bound");
}

#[tokio::test]
async fn another_tenants_signer_is_not_bound_here() {
    let (registry, _org) = seeded().await;
    let signer_guard = SignerGuard::new(registry);
    let other = OrganizationId::new();
    let outcome = signer_guard
        .check(&context_for(other), &signer_binding("key-7"), other)
        .await;
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "signer_not_bound");
}

#[tokio::test]
async fn a_check_naming_a_foreign_tenant_is_a_context_violation() {
    let (registry, org) = seeded().await;
    let wallet_guard = WalletGuard::new(registry.clone());
    let signer_guard = SignerGuard::new(registry);
    let foreign = OrganizationId::new();

    let wallet_outcome = wallet_guard.check(&context_for(org), "main", foreign).await;
    assert_eq!(
        wallet_outcome.deny_reason().unwrap().as_str(),
        "context_issue"
    );

    let signer_outcome = signer_guard
        .check(&context_for(org), &signer_binding("key-7"), foreign)
        .await;
    assert_eq!(
        signer_outcome.deny_reason().unwrap().as_str(),
        "context_issue"
    );
}

#[tokio::test]
async fn an_inactive_own_wallet_is_refused() {
    let (registry, org) = seeded().await;
    registry
        .put_wallet(WalletBinding {
            organization_id: org,
            label: "cold".into(),
            address: MAIN_ADDRESS.into(),
            active: false,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let wallet_guard = WalletGuard::new(registry);
    let outcome = wallet_guard.check(&context_for(org), "cold", org).await;
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "wallet_inactive");
}

#[tokio::test]
async fn an_inactive_own_signer_is_refused() {
    let (registry, org) = seeded().await;
    registry
        .put_signer(SignerBindingRow {
            organization_id: org,
            provider: SignerProvider::Custody,
            key_ref: "key-old".into(),
            active: false,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let signer_guard = SignerGuard::new(registry);
    let outcome = signer_guard
        .check(&context_for(org), &signer_binding("key-old"), org)
        .await;
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "signer_inactive");
}

#[tokio::test]
async fn through_the_gateway_a_missing_wallet_binding_is_denied() {
    use bot_core::billing::entitlement::{Entitlement, EntitlementSet, EntitlementSource};
    use bot_core::tenant::TenantEntitlementView;
    use sniper_suite::runtime_registry::store::MemoryRuntimeStore;
    use sniper_suite::runtime_registry::RuntimeRegistryService;
    use sniper_suite::tenant::gateway::{
        ExecutionInputs, FixedEntitlementProvider, TenantExecutionGateway,
    };
    use sniper_suite::tenant_config::store::ConfigStore;
    use sniper_suite::tenant_config::{
        ConfigCache, GlobalSafetyBounds, MemoryConfigStore, TenantConfigModel,
    };

    let org = OrganizationId::new();
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    runtime
        .ensure_active(org, "worker-test", Utc::now())
        .await
        .unwrap();
    let bounds = GlobalSafetyBounds {
        max_position_usd: 100_000.0,
        daily_loss_usd_cap: 50_000.0,
        max_slippage_bps: 2_000,
        allowed_modes: &TEST_MODES,
    };
    let config_store = Arc::new(MemoryConfigStore::new());
    config_store
        .put(
            org,
            &TenantConfigModel::deployment_legacy(&TEST_MODES),
            None,
            Some("test"),
            Utc::now(),
            &bounds,
        )
        .await
        .unwrap();
    // Bindings: only the SIGNER (no wallet) — the wallet guard must
    // deny before anything runs.
    let bindings: Arc<dyn TenantBindingRegistry> = Arc::new(MemoryTenantBindingRegistry::new());
    bindings
        .put_signer(SignerBindingRow {
            organization_id: org,
            provider: SignerProvider::Custody,
            key_ref: "key-7".into(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let now = Utc::now();
    let features = ["module.copy", "feature.live_trading"];
    let stored: Vec<Entitlement> = features
        .iter()
        .map(|f| Entitlement::new(org, *f, None, EntitlementSource::Override, now))
        .collect();
    let view = TenantEntitlementView::new(EntitlementSet::resolve(None, None, &stored, now));

    let inputs = ExecutionInputs::new(
        runtime,
        Arc::new(ConfigCache::new(config_store)),
        bindings,
        Arc::new(FixedEntitlementProvider::new(view)),
        bounds,
    );
    let gateway = TenantExecutionGateway::new(Arc::new(inputs));
    let organization = Organization::new(org, "acme", "Acme", None, Utc::now());
    let request = TenantExecutionRequest::new(
        org,
        BotModule::Copy,
        ExecutionMode::Paper,
        "ghost-wallet",
        signer_binding("key-7"),
    );
    let auth = gateway.authorize("user:1", &request, &organization).await;
    assert_eq!(
        auth.decision.deny_reason().unwrap().as_str(),
        "wallet_not_bound"
    );
    assert!(auth.context.is_none());
}
