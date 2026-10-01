//! Sniper tenant context adapter (PROMPT 4/10 file 8).
//!
//! [`SniperTenantContext`] adapts the ONE core
//! [`bot_core::execution::TenantExecutionContext`] into the existing
//! sniper pipeline. It does NOT duplicate the tenant identity types —
//! every identity field delegates to the core context, which itself can
//! only exist through `TenantExecutionContext::issue` and its fail-closed
//! coherence checks.
//!
//! What the adapter adds for the sniper specifically:
//!
//! * **module enforcement** — a Copy-issued context can never drive the
//!   sniper (the module in the scope must be `Sniper`);
//! * **tenant-local dedup keys** — launch-event deduplication is scoped
//!   per organization + runtime, so the same external launch seen by two
//!   tenants (or one tenant across a runtime rotation) deduplicates
//!   independently instead of one tenant's seen-set starving the other;
//! * **paper-default policy** — a tenant-scoped sniper runs paper unless
//!   the context was issued for live mode (the operator still owns the
//!   live gates on top).

use bot_core::execution::TenantExecutionContext;
use bot_core::models::ExecutionMode;
use bot_core::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};

/// Why a context cannot drive the sniper. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SniperContextError {
    /// The context was issued for a different module.
    WrongModule(ModuleKind),
    /// A re-verification against the live runtime identity failed
    /// (rotation/fence).
    StaleRuntime,
}

impl SniperContextError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SniperContextError::WrongModule(_) => "wrong_module",
            SniperContextError::StaleRuntime => "stale_runtime",
        }
    }
}

impl std::fmt::Display for SniperContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SniperContextError::WrongModule(m) => {
                write!(f, "sniper tenant context: module {m} is not sniper")
            }
            SniperContextError::StaleRuntime => {
                write!(f, "sniper tenant context: runtime identity is stale")
            }
        }
    }
}

impl std::error::Error for SniperContextError {}

/// The sniper's view of a tenant execution context.
#[derive(Debug, Clone)]
pub struct SniperTenantContext {
    context: TenantExecutionContext,
}

impl SniperTenantContext {
    /// Adapt a core context. Fails when the context belongs to another
    /// module.
    pub fn adapt(context: TenantExecutionContext) -> Result<Self, SniperContextError> {
        if context.scope().module() != ModuleKind::Sniper {
            return Err(SniperContextError::WrongModule(context.scope().module()));
        }
        Ok(SniperTenantContext { context })
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.context.organization_id()
    }

    /// The executing runtime.
    pub fn runtime_id(&self) -> RuntimeId {
        self.context.runtime_id()
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.context.generation()
    }

    /// The execution mode this sniper may run under.
    pub fn mode(&self) -> ExecutionMode {
        self.context.scope().mode()
    }

    /// Tenant snipers default to paper: only a context explicitly issued
    /// for live mode authorizes live submissions (the operator's live
    /// gates still apply on top of this).
    pub fn is_live(&self) -> bool {
        self.context.scope().is_live()
    }

    /// The bound wallet address (public).
    pub fn wallet_address(&self) -> &str {
        self.context.wallet().address()
    }

    /// The bound signer identity (public reference).
    pub fn signer_identity(&self) -> &str {
        self.context.signer().key_ref()
    }

    /// The underlying core context.
    pub fn execution_context(&self) -> &TenantExecutionContext {
        &self.context
    }

    /// Re-verify against a freshly-resolved runtime identity.
    pub fn verify_against(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), SniperContextError> {
        self.context
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| SniperContextError::StaleRuntime)
    }

    /// A tenant-local dedup key for an external launch event. Two tenants
    /// (or one tenant across a runtime rotation) deduplicate the SAME
    /// external event independently — one tenant's seen-set can never
    /// suppress another tenant's trade.
    pub fn dedup_key(&self, event_id: &str) -> String {
        format!(
            "sniper:{}:{}:{}",
            self.context.organization_id(),
            self.context.runtime_id(),
            event_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::BotModule;
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;

    fn issued(module: BotModule, mode: ExecutionMode) -> TenantExecutionContext {
        let org = OrganizationId::new();
        // ONE runtime identity for both the authority checklist and the
        // issuance — a second `RuntimeId::new()` would never match the
        // fingerprint and the context would (correctly) fail to issue.
        let runtime = RuntimeId::new();
        let generation = RuntimeGeneration::first();
        let scope =
            bot_core::execution::ExecutionScope::new(org, runtime, generation, module, mode)
                .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet =
            TenantWalletRef::new(org, "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM").unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "sniper-key")
                .unwrap();
        TenantExecutionContext::issue(
            org,
            runtime,
            generation,
            module,
            mode,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap()
    }

    #[test]
    fn a_sniper_context_adapts() {
        let ctx = SniperTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper))
            .expect("sniper context must adapt");
        assert!(!ctx.is_live());
        assert_eq!(
            ctx.wallet_address(),
            "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM"
        );
        assert_eq!(ctx.signer_identity(), "sniper-key");
    }

    #[test]
    fn a_copy_context_is_refused() {
        let err =
            SniperTenantContext::adapt(issued(BotModule::Copy, ExecutionMode::Paper)).unwrap_err();
        assert_eq!(err.as_str(), "wrong_module");
        assert!(err.to_string().contains("copy"));
    }

    #[test]
    fn live_mode_is_explicit_not_assumed() {
        let paper =
            SniperTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper)).unwrap();
        assert!(!paper.is_live());
        let live =
            SniperTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Live)).unwrap();
        assert!(live.is_live());
    }

    #[test]
    fn dedup_keys_are_tenant_and_runtime_scoped() {
        let a =
            SniperTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper)).unwrap();
        let b =
            SniperTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper)).unwrap();
        assert_ne!(a.dedup_key("launch-1"), b.dedup_key("launch-1"));
        assert!(a.dedup_key("launch-1").starts_with("sniper:"));
        assert_eq!(a.dedup_key("launch-1"), a.dedup_key("launch-1"));
    }

    #[test]
    fn stale_runtime_fails_reverification() {
        let ctx =
            SniperTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper)).unwrap();
        let rotated = ctx.generation().next().unwrap();
        assert_eq!(
            ctx.verify_against(ctx.organization_id(), ctx.runtime_id(), rotated)
                .unwrap_err()
                .as_str(),
            "stale_runtime"
        );
        assert!(ctx
            .verify_against(ctx.organization_id(), ctx.runtime_id(), ctx.generation())
            .is_ok());
    }
}
