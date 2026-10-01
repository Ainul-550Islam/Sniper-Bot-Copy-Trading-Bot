//! Copy-trading tenant context adapter (PROMPT 4/10 file 12).
//!
//! [`CopyTenantContext`] adapts the ONE core
//! [`bot_core::execution::TenantExecutionContext`] into the existing
//! copy-trading pipeline. It does NOT duplicate the tenant identity
//! types — every identity field delegates to the core context.
//!
//! What the adapter adds for copy trading specifically:
//!
//! * **module enforcement** — only a Copy-issued context can drive the
//!   copy bot;
//! * **tenant-local leader keys** — the SAME external leader address may
//!   be tracked by many tenants; every leader/link/event key is scoped
//!   per organization + runtime so one tenant's leader state can never
//!   collide with (or suppress) another's;
//! * **paper-default policy** — a tenant-scoped copy bot runs paper
//!   unless the context was issued for live mode.

use bot_core::execution::TenantExecutionContext;
use bot_core::models::ExecutionMode;
use bot_core::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};

/// Why a context cannot drive the copy bot. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyContextError {
    /// The context was issued for a different module.
    WrongModule(ModuleKind),
    /// A re-verification against the live runtime identity failed.
    StaleRuntime,
}

impl CopyContextError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            CopyContextError::WrongModule(_) => "wrong_module",
            CopyContextError::StaleRuntime => "stale_runtime",
        }
    }
}

impl std::fmt::Display for CopyContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyContextError::WrongModule(m) => {
                write!(f, "copy tenant context: module {m} is not copy")
            }
            CopyContextError::StaleRuntime => {
                write!(f, "copy tenant context: runtime identity is stale")
            }
        }
    }
}

impl std::error::Error for CopyContextError {}

/// The copy bot's view of a tenant execution context.
#[derive(Debug, Clone)]
pub struct CopyTenantContext {
    context: TenantExecutionContext,
}

impl CopyTenantContext {
    /// Adapt a core context. Fails for non-Copy modules.
    pub fn adapt(context: TenantExecutionContext) -> Result<Self, CopyContextError> {
        if context.scope().module() != ModuleKind::Copy {
            return Err(CopyContextError::WrongModule(context.scope().module()));
        }
        Ok(CopyTenantContext { context })
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

    /// The execution mode this copy bot runs under.
    pub fn mode(&self) -> ExecutionMode {
        self.context.scope().mode()
    }

    /// Tenant copy bots default to paper: only a live-issued context
    /// authorizes live submissions.
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
    ) -> Result<(), CopyContextError> {
        self.context
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| CopyContextError::StaleRuntime)
    }

    /// A tenant-local key for one tracked leader address. The SAME
    /// external leader may be tracked by many tenants — this key keeps
    /// their leader state, links and event cursors independent.
    pub fn leader_key(&self, leader_address: &str) -> String {
        format!(
            "copy:{}:{}:{leader_address}",
            self.context.organization_id(),
            self.context.runtime_id()
        )
    }

    /// A tenant-local key for one mirrored event.
    pub fn event_key(&self, leader_address: &str, event_id: &str) -> String {
        format!("{}:{event_id}", self.leader_key(leader_address))
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
        let wallet = TenantWalletRef::new(org, "11111111111111111111111111111111").unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "copy-key").unwrap();
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
    fn a_copy_context_adapts_and_a_sniper_context_is_refused() {
        let ctx = CopyTenantContext::adapt(issued(BotModule::Copy, ExecutionMode::Paper))
            .expect("copy context must adapt");
        assert!(!ctx.is_live());
        let err =
            CopyTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper)).unwrap_err();
        assert_eq!(err.as_str(), "wrong_module");
    }

    #[test]
    fn the_same_external_leader_is_independent_per_tenant() {
        let a = CopyTenantContext::adapt(issued(BotModule::Copy, ExecutionMode::Paper)).unwrap();
        let b = CopyTenantContext::adapt(issued(BotModule::Copy, ExecutionMode::Paper)).unwrap();
        let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
        assert_ne!(a.leader_key(leader), b.leader_key(leader));
        assert_ne!(a.event_key(leader, "evt-1"), b.event_key(leader, "evt-1"));
        assert!(a.leader_key(leader).starts_with("copy:"));
        assert_eq!(a.event_key(leader, "evt-1"), a.event_key(leader, "evt-1"));
    }

    #[test]
    fn stale_runtime_fails_reverification() {
        let ctx = CopyTenantContext::adapt(issued(BotModule::Copy, ExecutionMode::Paper)).unwrap();
        let rotated = ctx.generation().next().unwrap();
        assert_eq!(
            ctx.verify_against(ctx.organization_id(), ctx.runtime_id(), rotated)
                .unwrap_err()
                .as_str(),
            "stale_runtime"
        );
    }
}
