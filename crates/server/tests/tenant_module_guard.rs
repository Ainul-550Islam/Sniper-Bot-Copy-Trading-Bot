//! Module entitlement / enablement negative tests (tenant-isolation
//! file 68).
//!
//! Positive: a tenant whose plan carries the module entitlements and
//! whose config enables the module authorizes cleanly.
//!
//! Negative: a module the plan does not carry is denied at the
//! entitlement guard (`entitlement_module`); a module disabled in the
//! tenant's OWN configuration is denied at the module guard
//! (`module_disabled`); a mode outside the tenant's allowed modes is
//! denied at the mode guard (`mode_not_allowed`); live trading
//! without the live-trading entitlement is denied
//! (`entitlement_live_trading`).

use std::sync::Arc;

use chrono::Utc;

use sniper_suite::runtime_registry::store::MemoryRuntimeStore;
use sniper_suite::runtime_registry::RuntimeRegistryService;
use sniper_suite::tenant::gateway::{
    ExecutionInputs, FixedEntitlementProvider, TenantExecutionGateway,
};
use sniper_suite::tenant::registry::{
    MemoryTenantBindingRegistry, SignerBindingRow, TenantBindingRegistry, WalletBinding,
};
use sniper_suite::tenant::request::{SignerBinding, TenantExecutionRequest};
use sniper_suite::tenant_config::store::ConfigStore;
use sniper_suite::tenant_config::{
    ConfigCache, GlobalSafetyBounds, MemoryConfigStore, TenantConfigModel,
};

use bot_core::billing::entitlement::{Entitlement, EntitlementSet, EntitlementSource};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{
    ModuleDisableReason, Organization, OrganizationId, SignerProvider, TenantEntitlementView,
};

const TEST_MODES: [ExecutionMode; 2] = [ExecutionMode::Paper, ExecutionMode::Live];

fn test_bounds() -> GlobalSafetyBounds {
    GlobalSafetyBounds {
        max_position_usd: 100_000.0,
        daily_loss_usd_cap: 50_000.0,
        max_slippage_bps: 2_000,
        allowed_modes: &TEST_MODES,
    }
}

/// The entitlement view a tenant's plan grants: which modules (and
/// live trading, when `live`).
fn plan_entitlements(
    organization_id: OrganizationId,
    modules: &[&str],
    live: bool,
) -> TenantEntitlementView {
    let now = Utc::now();
    let mut features: Vec<&str> = modules.to_vec();
    if live {
        features.push("feature.live_trading");
    }
    let stored: Vec<Entitlement> = features
        .iter()
        .map(|f| Entitlement::new(organization_id, *f, None, EntitlementSource::Override, now))
        .collect();
    TenantEntitlementView::new(EntitlementSet::resolve(None, None, &stored, now))
}

async fn assembled(
    organization_id: OrganizationId,
    modules: &[&str],
    live: bool,
) -> TenantExecutionGateway {
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    runtime
        .ensure_active(organization_id, "worker-test", Utc::now())
        .await
        .unwrap();
    let config_store = Arc::new(MemoryConfigStore::new());
    config_store
        .put(
            organization_id,
            &TenantConfigModel::deployment_legacy(&[ExecutionMode::Paper, ExecutionMode::Live]),
            None,
            Some("test"),
            Utc::now(),
            &test_bounds(),
        )
        .await
        .unwrap();
    let config = Arc::new(ConfigCache::new(config_store));
    let bindings: Arc<dyn TenantBindingRegistry> = Arc::new(MemoryTenantBindingRegistry::new());
    bindings
        .put_wallet(WalletBinding {
            organization_id,
            label: "main".into(),
            address: "WalletMain1111111111111111111111111111111111111".into(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    bindings
        .put_signer(SignerBindingRow {
            organization_id,
            provider: SignerProvider::Custody,
            key_ref: "key-7".into(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let inputs = ExecutionInputs::new(
        runtime,
        config,
        bindings,
        Arc::new(FixedEntitlementProvider::new(plan_entitlements(
            organization_id,
            modules,
            live,
        ))),
        test_bounds(),
    );
    TenantExecutionGateway::new(Arc::new(inputs))
}

fn request(
    organization_id: OrganizationId,
    module: BotModule,
    mode: ExecutionMode,
) -> TenantExecutionRequest {
    TenantExecutionRequest::new(
        organization_id,
        module,
        mode,
        "main",
        SignerBinding {
            provider: SignerProvider::Custody,
            key_ref: "key-7".into(),
        },
    )
    .with_size_usd(100.0)
    .with_slippage_bps(50)
}

fn organization(organization_id: OrganizationId) -> Organization {
    Organization::new(organization_id, "acme", "Acme", None, Utc::now())
}

#[tokio::test]
async fn an_entitled_tenant_with_the_module_enabled_authorizes() {
    let org = OrganizationId::new();
    let gateway = assembled(
        org,
        &["module.sniper", "module.copy", "module.polymarket"],
        true,
    )
    .await;
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Copy, ExecutionMode::Paper),
            &organization(org),
        )
        .await;
    assert!(auth.decision.is_allow(), "{:?}", auth.decision);
    let (context, _runtime) = auth.expect_context();
    assert_eq!(context.organization_id(), org);
}

#[tokio::test]
async fn a_module_the_plan_does_not_carry_is_denied() {
    let org = OrganizationId::new();
    // Plan carries ONLY copy.
    let gateway = assembled(org, &["module.copy"], true).await;
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Sniper, ExecutionMode::Paper),
            &organization(org),
        )
        .await;
    let reason = auth.decision.deny_reason().unwrap();
    assert_eq!(reason.as_str(), "entitlement_module");
    assert!(matches!(
        reason,
        sniper_suite::tenant::DenyReason::EntitlementModule { module, .. } if *module == "sniper"
    ));
    assert!(auth.context.is_none());
}

#[tokio::test]
async fn a_module_disabled_in_the_tenants_own_config_is_denied() {
    let org = OrganizationId::new();
    // Entitlements cover all trading modules...
    let gateway = assembled(
        org,
        &["module.sniper", "module.copy", "module.polymarket"],
        true,
    )
    .await;
    // ...but disabling one module is a config-store mutation the
    // gateway reads through the cache: prove the negative through the
    // plan path by removing the entitlement instead, and through the
    // config path by a model whose module is disabled.
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Polymarket, ExecutionMode::Paper),
            &organization(org),
        )
        .await;
    assert!(auth.decision.is_allow());

    // Config-side disable: a fresh store with polymarket disabled.
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    runtime
        .ensure_active(org, "worker-test", Utc::now())
        .await
        .unwrap();
    let config_store = Arc::new(MemoryConfigStore::new());
    let mut model =
        TenantConfigModel::deployment_legacy(&[ExecutionMode::Paper, ExecutionMode::Live]);
    model.disable_module(BotModule::Polymarket, ModuleDisableReason::Operator);
    config_store
        .put(org, &model, None, Some("test"), Utc::now(), &test_bounds())
        .await
        .unwrap();
    let inputs = ExecutionInputs::new(
        runtime,
        Arc::new(ConfigCache::new(config_store)),
        Arc::new(MemoryTenantBindingRegistry::new()) as Arc<dyn TenantBindingRegistry>,
        Arc::new(FixedEntitlementProvider::new(plan_entitlements(
            org,
            &["module.sniper", "module.copy", "module.polymarket"],
            true,
        ))),
        test_bounds(),
    );
    let gateway = TenantExecutionGateway::new(Arc::new(inputs));
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Polymarket, ExecutionMode::Paper),
            &organization(org),
        )
        .await;
    let reason = auth.decision.deny_reason().unwrap();
    assert_eq!(reason.as_str(), "module_disabled");
    assert!(auth.context.is_none());
}

#[tokio::test]
async fn a_mode_the_plan_does_not_allow_is_denied() {
    let org = OrganizationId::new();
    // Entitlements WITHOUT feature.live_trading.
    let gateway = assembled(org, &["module.copy"], false).await;
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Copy, ExecutionMode::Live),
            &organization(org),
        )
        .await;
    assert_eq!(
        auth.decision.deny_reason().unwrap().as_str(),
        "entitlement_live_trading"
    );
    // Paper mode still passes for the same tenant.
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Copy, ExecutionMode::Paper),
            &organization(org),
        )
        .await;
    assert!(auth.decision.is_allow(), "{:?}", auth.decision);
}

#[tokio::test]
async fn a_mode_outside_the_tenants_allowed_modes_is_denied() {
    let org = OrganizationId::new();
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    runtime
        .ensure_active(org, "worker-test", Utc::now())
        .await
        .unwrap();
    // Config allows PAPER ONLY for this tenant.
    let config_store = Arc::new(MemoryConfigStore::new());
    config_store
        .put(
            org,
            &TenantConfigModel::deployment_legacy(&[ExecutionMode::Paper]),
            None,
            Some("test"),
            Utc::now(),
            &test_bounds(),
        )
        .await
        .unwrap();
    let bindings: Arc<dyn TenantBindingRegistry> = Arc::new(MemoryTenantBindingRegistry::new());
    let inputs = ExecutionInputs::new(
        runtime,
        Arc::new(ConfigCache::new(config_store)),
        bindings,
        Arc::new(FixedEntitlementProvider::new(plan_entitlements(
            org,
            &["module.copy"],
            true,
        ))),
        test_bounds(),
    );
    let gateway = TenantExecutionGateway::new(Arc::new(inputs));
    let auth = gateway
        .authorize(
            "user:1",
            &request(org, BotModule::Copy, ExecutionMode::Live),
            &organization(org),
        )
        .await;
    assert_eq!(
        auth.decision.deny_reason().unwrap().as_str(),
        "mode_not_allowed"
    );
}
