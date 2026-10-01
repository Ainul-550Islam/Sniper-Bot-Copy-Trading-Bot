//! The execution gateway: the ordered guard chain (STEP 3 file 24).
//!
//! [`TenantExecutionGateway::authorize`] is the ONE call every entry
//! path makes before handing work to a module engine. It runs the
//! guards in fixed order (first deny wins), records the mandated
//! authority checks in [`AUTHORITY_CHECK_ORDER`] as they pass, and on
//! full success issues a core [`TenantExecutionContext`] — the typed
//! proof the engines already require. On any deny, nothing is issued
//! and the deny reason is machine-readable.
//!
//! Fail-closed discipline: a guard DEPENDENCY error (config store,
//! binding registry, runtime registry) is a deny, never a bypass.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

use bot_core::execution::{
    AuthorityChecklist, ExecutionScope, ExecutionTrace, TenantExecutionContext,
    AUTHORITY_CHECK_ORDER,
};
use bot_core::tenant::{
    Organization, OrganizationId, TenantEntitlementView, TenantSignerRef, TenantWalletRef,
};

use crate::runtime_registry::{FenceToken, RuntimeRegistryService, TenantRuntimeRecord};
use crate::tenant_config::{ConfigCache, GlobalSafetyBounds, RuntimeOverrides};

use super::binding_guard;
use super::context::TenantContext;
use super::context_guard;
use super::decision::{DenyReason, GuardOutcome, TenantDecision};
use super::entitlement_guard;
use super::fence_guard;
use super::mode_guard;
use super::module_guard;
use super::request::TenantExecutionRequest;
use super::risk_guard;
use super::tenant_guard;

/// How the gateway learns a tenant's entitlements (the billing pipeline
/// in production; a fixed set in tests and detached deployments).
#[async_trait]
pub trait EntitlementProvider: Send + Sync {
    /// The tenant's effective entitlement view.
    async fn entitlements(
        &self,
        organization_id: OrganizationId,
    ) -> bot_core::error::BotResult<TenantEntitlementView>;
}

/// A fixed-view provider (tests, detached deployments, operator
/// overrides during incidents).
pub struct FixedEntitlementProvider {
    view: TenantEntitlementView,
}

impl FixedEntitlementProvider {
    /// Everyone gets this view.
    pub fn new(view: TenantEntitlementView) -> Self {
        FixedEntitlementProvider { view }
    }
}

#[async_trait]
impl EntitlementProvider for FixedEntitlementProvider {
    async fn entitlements(
        &self,
        _organization_id: OrganizationId,
    ) -> bot_core::error::BotResult<TenantEntitlementView> {
        Ok(self.view.clone())
    }
}

/// Everything the guards need, assembled once at startup.
pub struct ExecutionInputs {
    /// The runtime registry (fence + current runtime).
    pub runtime: Arc<RuntimeRegistryService>,
    /// The tenant configuration cache.
    pub config: Arc<ConfigCache>,
    /// The wallet/signer binding registry.
    pub bindings: Arc<dyn super::registry::TenantBindingRegistry>,
    /// The entitlement provider.
    pub entitlements: Arc<dyn EntitlementProvider>,
    /// The platform's immutable safety bounds.
    pub bounds: GlobalSafetyBounds,
    /// Runtime-level overrides for resolution (degradations). The
    /// supervisor updates these; the default is "no degradation".
    pub overrides: RuntimeOverrides,
}

impl ExecutionInputs {
    /// The standard assembly.
    pub fn new(
        runtime: Arc<RuntimeRegistryService>,
        config: Arc<ConfigCache>,
        bindings: Arc<dyn super::registry::TenantBindingRegistry>,
        entitlements: Arc<dyn EntitlementProvider>,
        bounds: GlobalSafetyBounds,
    ) -> Self {
        ExecutionInputs {
            runtime,
            config,
            bindings,
            entitlements,
            bounds,
            overrides: RuntimeOverrides::default(),
        }
    }
}

/// The gateway.
#[derive(Clone)]
pub struct TenantExecutionGateway {
    inputs: Arc<ExecutionInputs>,
}

/// What `authorize` returns.
pub struct Authorization {
    /// The decision (allow/deny + guard notes or reason).
    pub decision: TenantDecision,
    /// The issued context, present ONLY on allow.
    pub context: Option<TenantExecutionContext>,
    /// The tenant's live runtime the context is bound to (allow only).
    pub runtime: Option<TenantRuntimeRecord>,
}

impl Authorization {
    /// Convenience: unwrap the context (panics on deny — for call sites
    /// that already matched on the decision).
    pub fn expect_context(self) -> (TenantExecutionContext, TenantRuntimeRecord) {
        let runtime = self.runtime.expect("authorization was allowed");
        (self.context.expect("authorization was allowed"), runtime)
    }
}

impl TenantExecutionGateway {
    /// Build over the assembled inputs.
    pub fn new(inputs: Arc<ExecutionInputs>) -> Self {
        TenantExecutionGateway { inputs }
    }

    /// Run the chain for an already-resolved [`TenantContext`].
    ///
    /// The typed entry path: the context guard verifies the context
    /// (principal present, request belongs to the context's tenant,
    /// lifecycle allows trading) and the full chain then runs exactly
    /// as in [`TenantExecutionGateway::authorize`]. Entry paths that
    /// hold a context (HTTP handlers via the context resolver, job
    /// tickers, recovery passes) use this method; the string-principal
    /// `authorize` remains for callers that resolved the organization
    /// through the legacy SaaS middleware path.
    pub async fn authorize_in_context(
        &self,
        context: &TenantContext,
        request: &TenantExecutionRequest,
    ) -> Authorization {
        match context_guard::check(context, request) {
            GuardOutcome::Allow(_) => {}
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }
        self.authorize(context.principal(), request, context.organization())
            .await
    }

    /// Run the chain for a request. `principal` is the authenticated
    /// actor (user id, api-key label, `job:<name>` or `recovery`) — the
    /// first authority check.
    pub async fn authorize(
        &self,
        principal: &str,
        request: &TenantExecutionRequest,
        organization: &Organization,
    ) -> Authorization {
        let mut notes: Vec<&'static str> = Vec::new();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();

        // -- authenticated_principal -------------------------------------
        if principal.trim().is_empty() {
            return self.deny(DenyReason::ContextIssue {
                reason: "missing principal",
            });
        }
        if record(&mut checklist, "authenticated_principal", now).is_err() {
            return self.deny(DenyReason::ContextIssue {
                reason: "authority checklist",
            });
        }

        // -- tenant_context ----------------------------------------------
        if organization.id != request.organization_id {
            return self.deny(DenyReason::ContextIssue {
                reason: "request tenant does not match the resolved organization",
            });
        }
        if record(&mut checklist, "tenant_context", now).is_err() {
            return self.deny(DenyReason::ContextIssue {
                reason: "authority checklist",
            });
        }

        // -- tenant_lifecycle --------------------------------------------
        match tenant_guard::check(organization) {
            GuardOutcome::Allow(note) => {
                notes.push(note);
                let _ = record(&mut checklist, "tenant_lifecycle", now);
            }
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }

        // -- runtime_exists / runtime_active / runtime_generation --------
        let runtime = match self.inputs.runtime.current(request.organization_id).await {
            Ok(Some(runtime)) => runtime,
            Ok(None) => {
                return self.deny(DenyReason::FenceFailed {
                    verdict: "no_live_runtime",
                })
            }
            Err(_) => {
                return self.deny(DenyReason::DependencyError {
                    source: "runtime_registry",
                })
            }
        };
        let _ = record(&mut checklist, "runtime_exists", now);
        if !runtime.status.is_live() {
            return self.deny(DenyReason::FenceFailed {
                verdict: "no_live_runtime",
            });
        }
        let _ = record(&mut checklist, "runtime_active", now);

        let fence_token = FenceToken {
            organization_id: request.organization_id,
            runtime_id: runtime.runtime_id,
            generation: runtime.generation,
        };
        match fence_guard::check(&self.inputs.runtime, &fence_token).await {
            GuardOutcome::Allow(note) => {
                notes.push(note);
                let _ = record(&mut checklist, "runtime_generation", now);
            }
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }

        // -- module_entitlement -------------------------------------------
        let entitlements = match self
            .inputs
            .entitlements
            .entitlements(request.organization_id)
            .await
        {
            Ok(view) => view,
            Err(_) => {
                return self.deny(DenyReason::DependencyError {
                    source: "entitlement_provider",
                })
            }
        };
        match entitlement_guard::check_module(&entitlements, request.module) {
            GuardOutcome::Allow(note) => {
                notes.push(note);
                let _ = record(&mut checklist, "module_entitlement", now);
            }
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }
        match entitlement_guard::check_mode(&entitlements, request.mode) {
            GuardOutcome::Allow(note) => notes.push(note),
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }

        // -- tenant_config (module enablement + mode under the config) ----
        let (tenant_config, _version, _origin) =
            match self.inputs.config.get(request.organization_id).await {
                Ok(pair) => pair,
                Err(_) => {
                    return self.deny(DenyReason::DependencyError {
                        source: "config_cache",
                    })
                }
            };
        let effective = crate::tenant_config::resolve(
            &self.inputs.bounds,
            Some(&tenant_config),
            Some(&self.inputs.overrides),
            None,
        );
        match module_guard::check(&effective, request.module) {
            GuardOutcome::Allow(note) => notes.push(note),
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }
        match mode_guard::check(&effective, request.mode) {
            GuardOutcome::Allow(note) => {
                notes.push(note);
                let _ = record(&mut checklist, "tenant_config", now);
            }
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }

        // -- wallet_binding / signer_binding ------------------------------
        match binding_guard::check(&self.inputs.bindings, request).await {
            GuardOutcome::Allow(note) => {
                notes.push(note);
                let _ = record(&mut checklist, "wallet_binding", now);
                let _ = record(&mut checklist, "signer_binding", now);
            }
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }

        // -- risk_permission ----------------------------------------------
        match risk_guard::check(&effective, request.size_usd, request.slippage_bps) {
            GuardOutcome::Allow(note) => {
                notes.push(note);
                let _ = record(&mut checklist, "risk_permission", now);
            }
            GuardOutcome::Deny(reason) => return self.deny(reason),
        }

        // -- issue the context ---------------------------------------------
        let wallet_row = match self
            .inputs
            .bindings
            .wallet(request.organization_id, &request.wallet_label)
            .await
        {
            Ok(Some(row)) => row,
            _ => {
                return self.deny(DenyReason::DependencyError {
                    source: "binding_registry",
                })
            }
        };
        let wallet = match TenantWalletRef::new(request.organization_id, &wallet_row.address)
            .map(|w| w.with_label(&request.wallet_label))
        {
            Ok(wallet) => wallet,
            Err(_) => {
                return self.deny(DenyReason::ContextIssue {
                    reason: "wallet address rejected",
                })
            }
        };
        let signer = match TenantSignerRef::new(
            request.organization_id,
            request.signer.provider,
            &request.signer.key_ref,
        ) {
            Ok(signer) => signer,
            Err(_) => {
                return self.deny(DenyReason::ContextIssue {
                    reason: "signer reference rejected",
                })
            }
        };
        let scope = match ExecutionScope::new(
            request.organization_id,
            runtime.runtime_id,
            runtime.generation,
            request.module,
            request.mode,
        ) {
            Ok(scope) => scope,
            Err(_) => {
                return self.deny(DenyReason::ContextIssue {
                    reason: "scope construction",
                })
            }
        };
        let authority = match checklist.finish(&scope, now) {
            Ok(authority) => authority,
            Err(_) => {
                return self.deny(DenyReason::ContextIssue {
                    reason: "authority incomplete",
                })
            }
        };
        let trace = ExecutionTrace::new(request.origin);
        let context = match TenantExecutionContext::issue(
            request.organization_id,
            runtime.runtime_id,
            runtime.generation,
            request.module,
            request.mode,
            authority,
            wallet,
            signer,
            trace,
        ) {
            Ok(context) => context,
            Err(_) => {
                return self.deny(DenyReason::ContextIssue {
                    reason: "context issuance",
                })
            }
        };

        Authorization {
            decision: TenantDecision::Allow(notes),
            context: Some(context),
            runtime: Some(runtime),
        }
    }

    fn deny(&self, reason: DenyReason) -> Authorization {
        Authorization {
            decision: TenantDecision::Deny(reason),
            context: None,
            runtime: None,
        }
    }
}

fn record(
    checklist: &mut AuthorityChecklist,
    name: &'static str,
    at: chrono::DateTime<Utc>,
) -> Result<(), ()> {
    checklist.record(name, at).map_err(|_| ())
}

/// The full mandated order, re-exported for the observability module.
pub const GUARD_CHAIN_ORDER: [&str; 11] = AUTHORITY_CHECK_ORDER;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use crate::tenant::registry::{
        MemoryTenantBindingRegistry, SignerBindingRow, TenantBindingRegistry, WalletBinding,
    };
    use crate::tenant::request::SignerBinding;
    use crate::tenant_config::store::ConfigStore;
    use crate::tenant_config::{MemoryConfigStore, TenantConfigModel};
    use bot_core::billing::entitlement::{Entitlement, EntitlementSet, EntitlementSource};
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::{OrganizationStatus, SignerProvider};
    use chrono::Utc;

    /// Test bounds: paper AND live allowed (Live.as_str() is "LIVE" —
    /// the platform's warning-style convention).
    const TEST_MODES: [ExecutionMode; 2] = [ExecutionMode::Paper, ExecutionMode::Live];

    fn test_bounds() -> GlobalSafetyBounds {
        GlobalSafetyBounds {
            max_position_usd: 100_000.0,
            daily_loss_usd_cap: 50_000.0,
            max_slippage_bps: 2_000,
            allowed_modes: &TEST_MODES,
        }
    }

    fn all_access_entitlements() -> TenantEntitlementView {
        let org = OrganizationId::new();
        let now = Utc::now();
        let features = [
            "module.sniper",
            "module.copy",
            "module.polymarket",
            "feature.live_trading",
        ];
        let stored: Vec<Entitlement> = features
            .iter()
            .map(|f| Entitlement::new(org, *f, None, EntitlementSource::Override, now))
            .collect();
        TenantEntitlementView::new(EntitlementSet::resolve(None, None, &stored, now))
    }

    async fn assembled(org: OrganizationId) -> (TenantExecutionGateway, Organization) {
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        runtime
            .ensure_active(org, "worker-test", Utc::now())
            .await
            .unwrap();
        let config_store = Arc::new(MemoryConfigStore::new());
        config_store
            .put(
                org,
                &TenantConfigModel::deployment_legacy(&[ExecutionMode::Paper, ExecutionMode::Live]),
                None,
                Some("test"),
                Utc::now(),
                &test_bounds(),
            )
            .await
            .unwrap();
        let config = Arc::new(ConfigCache::new(config_store));
        let bindings = Arc::new(MemoryTenantBindingRegistry::new());
        bindings
            .put_wallet(WalletBinding {
                organization_id: org,
                label: "main".into(),
                address: "WalletMain1111111111111111111111111111111111111".into(),
                active: true,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
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

        let inputs = ExecutionInputs::new(
            runtime,
            config,
            bindings,
            Arc::new(FixedEntitlementProvider::new(all_access_entitlements())),
            test_bounds(),
        );
        let org_model = Organization::new(org, "test-org", "Test Org", None, Utc::now());
        (TenantExecutionGateway::new(Arc::new(inputs)), org_model)
    }

    fn request(org: OrganizationId) -> TenantExecutionRequest {
        TenantExecutionRequest::new(
            org,
            BotModule::Copy,
            ExecutionMode::Paper,
            "main",
            SignerBinding {
                provider: SignerProvider::Custody,
                key_ref: "key-7".into(),
            },
        )
        .with_size_usd(100.0)
        .with_slippage_bps(50)
    }

    #[tokio::test]
    async fn a_fully_authorized_request_issues_a_context() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let auth = gateway
            .authorize("user-1", &request(org), &organization)
            .await;

        assert!(auth.decision.is_allow(), "{:?}", auth.decision);
        let (context, runtime) = auth.expect_context();
        assert_eq!(context.organization_id(), org);
        assert_eq!(context.scope().module(), BotModule::Copy);
        assert_eq!(context.scope().mode(), ExecutionMode::Paper);
        assert_eq!(context.wallet().label(), Some("main"));
        assert!(runtime.status.is_live());
        // Every mandated authority check is recorded, in order.
        let names: Vec<&str> = context
            .authority()
            .checks()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, GUARD_CHAIN_ORDER.to_vec());
    }

    #[tokio::test]
    async fn a_suspended_tenant_is_denied_before_anything_runs() {
        let org = OrganizationId::new();
        let (gateway, _organization) = assembled(org).await;
        let mut suspended = Organization::new(org, "test-org", "Test Org", None, Utc::now());
        suspended.status = OrganizationStatus::Suspended;
        let auth = gateway.authorize("user-1", &request(org), &suspended).await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "tenant_state"
        );
        assert!(auth.context.is_none());
    }

    #[tokio::test]
    async fn a_missing_wallet_binding_denies_at_the_binding_guard() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let mut req = request(org);
        req.wallet_label = "ghost".into();
        let auth = gateway.authorize("user-1", &req, &organization).await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "wallet_not_bound"
        );
    }

    #[tokio::test]
    async fn an_oversized_request_denies_at_the_risk_guard() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let req = request(org).with_size_usd(1_000_000.0);
        let auth = gateway.authorize("user-1", &req, &organization).await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "position_size_exceeds"
        );
    }

    #[tokio::test]
    async fn a_tenant_without_a_live_runtime_denies_at_the_fence() {
        let org = OrganizationId::new();
        // Assemble WITHOUT registering a runtime for the tenant.
        let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )));
        let config_store = Arc::new(MemoryConfigStore::new());
        let config = Arc::new(ConfigCache::new(config_store));
        let bindings = Arc::new(MemoryTenantBindingRegistry::new());
        let inputs = ExecutionInputs::new(
            runtime,
            config,
            bindings,
            Arc::new(FixedEntitlementProvider::new(all_access_entitlements())),
            test_bounds(),
        );
        let gateway = TenantExecutionGateway::new(Arc::new(inputs));
        let organization = Organization::new(org, "test-org", "Test Org", None, Utc::now());
        let auth = gateway
            .authorize("user-1", &request(org), &organization)
            .await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "fence_failed"
        );
    }

    #[tokio::test]
    async fn an_empty_principal_never_authorizes() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let auth = gateway.authorize("   ", &request(org), &organization).await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "context_issue"
        );
    }

    #[tokio::test]
    async fn a_mismatched_organization_header_is_denied() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let auth = gateway
            .authorize("user-1", &request(OrganizationId::new()), &organization)
            .await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "context_issue"
        );
    }

    #[tokio::test]
    async fn authorize_in_context_issues_the_same_proof() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let context = crate::tenant::context::TenantContext::new(
            organization.clone(),
            "user:42",
            crate::tenant::context::ContextOrigin::Http,
            Utc::now(),
        )
        .unwrap();
        let auth = gateway.authorize_in_context(&context, &request(org)).await;
        assert!(auth.decision.is_allow(), "{:?}", auth.decision);
        let (proof, runtime) = auth.expect_context();
        assert_eq!(proof.organization_id(), org);
        assert!(runtime.status.is_live());
    }

    #[tokio::test]
    async fn authorize_in_context_denies_a_foreign_request_at_the_context_guard() {
        let org = OrganizationId::new();
        let (gateway, organization) = assembled(org).await;
        let context = crate::tenant::context::TenantContext::new(
            organization,
            "user:42",
            crate::tenant::context::ContextOrigin::Http,
            Utc::now(),
        )
        .unwrap();
        // The request names ANOTHER tenant: the context guard denies
        // before any downstream guard runs.
        let auth = gateway
            .authorize_in_context(&context, &request(OrganizationId::new()))
            .await;
        assert_eq!(
            auth.decision.deny_reason().unwrap().as_str(),
            "context_issue"
        );
        assert!(auth.context.is_none());
    }
}
