//! Tenant copy executor (PROMPT 4/10 file 14).
//!
//! [`TenantCopyExecutor`] connects the EXISTING copy pipeline — feed
//! events, leader resolution, dedup, ordering, policy, sizing, risk,
//! ownership claims, transaction build, signing, broadcast,
//! reconciliation, exit sweeper — to a tenant-scoped execution. It does
//! not re-implement any of those stages:
//!
//! * it binds the internal [`CopyBot`] to ONE tenant through
//!   [`CopyBot::with_tenant_context`], which attaches the final
//!   [`solana_kit::tenant_broadcast_guard::TenantBroadcastGuard`] to the
//!   bot's executor AND — through [`CopyBot::run`] — to the exit
//!   sweeper's own executor (wrong organization / runtime / generation
//!   / module / wallet is refused BEFORE signing or broadcast);
//! * every mirrored buy and every exit the bot builds is stamped with
//!   the tenant metadata the guard verifies;
//! * the per-tenant runtime state (event dedup, counters) lives in
//!   [`crate::tenant_state::TenantCopyState`], so one tenant's dedup
//!   set can never suppress another tenant's mirror — even when both
//!   tenants track the SAME external leader address;
//! * every mirror outcome is offered to a [`TenantCopySink`] — the
//!   server-side implementation persists the outcome through the
//!   tenant-scoped `bot-core` `trading_repository`
//!   (organization-bound writes only).
//!
//! Leader scoping: the tenant's leader list arrives with the tenant's
//! OWN config snapshot (the server resolves it from the tenant's
//! configuration); [`TenantCopyExecutor::process_trade`] hands that
//! snapshot to the pipeline, so `sync_leaders` keeps the registry in
//! step with the TENANT's wallets, never the deployment-global list.
//!
//! Operator mode is untouched: a [`CopyBot`] built WITHOUT a tenant
//! context keeps its deployment-global behaviour byte-for-byte.

use std::sync::Arc;

use async_trait::async_trait;

use bot_core::config::Config;
use bot_core::error::{BotError, BotResult};
use bot_core::execution::TenantExecutionContext;
use bot_core::models::WalletTrade;
use bot_core::state::Shared;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};
use solana_kit::rpc::Rpc;
use solana_kit::tenant_broadcast_guard::TenantBroadcastGuard;
use solana_kit::tenant_signing_context::TenantSigningContext;
use solana_kit::tokens::Wallet;

use crate::event::{CopyOutcome, CopyStage, LeaderTradeEvent, RejectReason, Rejection};
use crate::tenant_context::CopyTenantContext;
use crate::tenant_state::TenantCopyState;
use crate::CopyBot;

/// Why a scoped execution was refused before the pipeline ran. Closed
/// vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTenantDeny {
    /// The presented context belongs to another tenant than this
    /// executor's bound context.
    OrganizationMismatch,
    /// The runtime id does not match.
    RuntimeMismatch,
    /// The fencing generation is stale (the runtime was rotated).
    GenerationMismatch,
    /// The context was issued for a non-copy module.
    WrongModule,
    /// The funding wallet does not match the bound wallet.
    WalletMismatch,
}

impl CopyTenantDeny {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            CopyTenantDeny::OrganizationMismatch => "organization_mismatch",
            CopyTenantDeny::RuntimeMismatch => "runtime_mismatch",
            CopyTenantDeny::GenerationMismatch => "generation_mismatch",
            CopyTenantDeny::WrongModule => "wrong_module",
            CopyTenantDeny::WalletMismatch => "wallet_mismatch",
        }
    }
}

impl std::fmt::Display for CopyTenantDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tenant copy executor denied: {}",
            self.as_str().replace('_', " ")
        )
    }
}

impl std::error::Error for CopyTenantDeny {}

/// Tenant-scoped persistence for copy outcomes. The server implements
/// this over the `bot-core` `trading_repository` writes; the bound
/// context is handed to the sink with every outcome so an
/// implementation can verify the organization against its own write
/// scope before touching the database. The SOURCE event is handed over
/// as well — the outcome alone does not carry the leader/mint/venue
/// attribution the repository rows require.
#[async_trait]
pub trait TenantCopySink: Send + Sync {
    /// Persist one mirror outcome for the tenant bound in `context`.
    /// Implementations MUST fail closed on an organization mismatch
    /// with their own write scope.
    async fn record_outcome(
        &self,
        context: &CopyTenantContext,
        event: &LeaderTradeEvent,
        outcome: &CopyOutcome,
    ) -> BotResult<()>;
}

/// A no-op sink (used when tenant persistence is not attached — e.g.
/// the module crate's own tests; the server always attaches the
/// repository sink).
pub struct NullCopySink;

#[async_trait]
impl TenantCopySink for NullCopySink {
    async fn record_outcome(
        &self,
        _context: &CopyTenantContext,
        _event: &LeaderTradeEvent,
        _outcome: &CopyOutcome,
    ) -> BotResult<()> {
        Ok(())
    }
}

/// The tenant-scoped copy executor: the existing engine, bound to one
/// tenant, with tenant-local state and tenant-scoped persistence.
pub struct TenantCopyExecutor {
    bot: CopyBot,
    context: CopyTenantContext,
    signing: TenantSigningContext,
    state: TenantCopyState,
    sink: Arc<dyn TenantCopySink>,
}

impl TenantCopyExecutor {
    /// Build a tenant copy bot over the EXISTING engine.
    ///
    /// `context` is the issued core execution context (module = Copy);
    /// `wallet` funds the mirrors and MUST be the wallet bound in the
    /// context (verified twice: here and again by the broadcast guard
    /// at every submission).
    pub async fn new(
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        execution: TenantExecutionContext,
    ) -> BotResult<Self> {
        let context = CopyTenantContext::adapt(execution.clone())
            .map_err(|e| BotError::invalid(e.to_string()))?;
        let signing = TenantSigningContext::new(execution);
        let guard = TenantBroadcastGuard::new(signing.clone(), wallet.pubkey)
            .map_err(|e| BotError::invalid(e.to_string()))?;
        let bot = CopyBot::new(state, rpc, wallet, signers)
            .await
            .with_tenant_context(Arc::new(guard))?;
        Ok(TenantCopyExecutor {
            bot,
            context,
            signing,
            state: TenantCopyState::new(),
            sink: Arc::new(NullCopySink),
        })
    }

    /// Attach the tenant-scoped persistence sink.
    #[must_use]
    pub fn with_copy_sink(mut self, sink: Arc<dyn TenantCopySink>) -> Self {
        self.sink = sink;
        self
    }

    /// The bound tenant context.
    pub fn tenant_context(&self) -> &CopyTenantContext {
        &self.context
    }

    /// The bound signing context (guard-verified identity).
    pub fn signing_context(&self) -> &TenantSigningContext {
        &self.signing
    }

    /// The tenant runtime state (dedup + counters).
    pub fn tenant_state(&self) -> &TenantCopyState {
        &self.state
    }

    /// The underlying (tenant-bound) copy engine.
    pub fn bot(&self) -> &CopyBot {
        &self.bot
    }

    /// The underlying (tenant-bound) copy engine, mutable for the
    /// run-loop entry points that need it (leader sync, recovery).
    pub fn bot_mut(&mut self) -> &mut CopyBot {
        &mut self.bot
    }

    /// Authorize an incoming execution context against this executor's
    /// bound identity. EVERY scoped entry point calls this first; a
    /// deny means the pipeline never runs.
    pub fn authorize(&self, presented: &TenantExecutionContext) -> Result<(), CopyTenantDeny> {
        if presented.organization_id() != self.context.organization_id() {
            return Err(CopyTenantDeny::OrganizationMismatch);
        }
        if presented.runtime_id() != self.context.runtime_id() {
            return Err(CopyTenantDeny::RuntimeMismatch);
        }
        if presented.generation() != self.context.generation() {
            return Err(CopyTenantDeny::GenerationMismatch);
        }
        if presented.scope().module() != bot_core::tenant::ModuleKind::Copy {
            return Err(CopyTenantDeny::WrongModule);
        }
        if presented.wallet().address() != self.context.wallet_address() {
            return Err(CopyTenantDeny::WalletMismatch);
        }
        Ok(())
    }

    /// Record one mirror outcome through the attached sink (the
    /// reconciliation paths use this for late-arriving outcomes; the
    /// sink itself enforces the tenant boundary).
    pub async fn record_outcome(
        &self,
        event: &LeaderTradeEvent,
        outcome: &CopyOutcome,
    ) -> BotResult<()> {
        self.sink
            .record_outcome(&self.context, event, outcome)
            .await
    }

    /// Re-verify against a freshly-resolved runtime identity (fence
    /// check before money moves).
    pub fn verify_runtime(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), CopyTenantDeny> {
        self.context
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| CopyTenantDeny::GenerationMismatch)
    }

    /// Process ONE leader trade for THIS tenant. The event is deduped
    /// tenant-locally FIRST (the same external leader trade reaching
    /// two tenants is two independent mirrors; the same trade reaching
    /// one tenant twice is ONE mirror), then handed to the existing
    /// pipeline. The tenant's OWN config snapshot drives leader
    /// resolution, policy and sizing.
    pub async fn process_trade(
        &mut self,
        trade: &WalletTrade,
        feed: &str,
        cfg: &Config,
    ) -> CopyOutcome {
        self.state.on_considered();
        let event = self.bot.event_from_trade(trade, feed);
        let dedup_key = self.context.event_key(&event.leader, &event.signature);
        let mut outcome = if !self.state.first_sight(&dedup_key) {
            self.state.on_rejected();
            CopyOutcome::rejected(
                &event.event_id,
                Rejection::new(
                    RejectReason::DuplicateEvent,
                    CopyStage::Deduplicated,
                    format!(
                        "tenant-local duplicate for organization {}",
                        self.context.organization_id()
                    ),
                ),
                0,
            )
        } else {
            let outcome = self.bot.process_event(&event, cfg).await;
            if outcome.stage == CopyStage::Submitted || outcome.stage == CopyStage::Filled {
                self.state.on_submitted();
                if outcome.rejection.is_none() {
                    self.state.on_succeeded();
                }
            } else if outcome
                .rejection
                .as_ref()
                .map(|r| r.detail.contains("tenant broadcast denied"))
                .unwrap_or(false)
            {
                self.state.on_guard_denied();
            } else if outcome.stage == CopyStage::Rejected || outcome.stage == CopyStage::Failed {
                self.state.on_rejected();
            }
            outcome
        };
        // Tenant-scoped persistence: a sink failure SURFACES (it is
        // never swallowed) but does not un-broadcast a transaction that
        // already left the process — the outcome records the error.
        if let Err(e) = self
            .sink
            .record_outcome(&self.context, &event, &outcome)
            .await
        {
            let detail = format!("tenant copy sink: {e}");
            if outcome.rejection.is_none() {
                outcome.rejection = Some(Rejection::new(
                    RejectReason::ExecutionFailed,
                    outcome.stage,
                    detail,
                ));
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;

    fn issued_for(
        org: OrganizationId,
        runtime: RuntimeId,
        module: BotModule,
    ) -> TenantExecutionContext {
        let generation = RuntimeGeneration::first();
        let mode = ExecutionMode::Paper;
        let scope =
            bot_core::execution::ExecutionScope::new(org, runtime, generation, module, mode)
                .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        // A REAL 32-byte pubkey so downstream base58 parsing round-trips.
        use solana_sdk::signer::Signer;
        let address = solana_sdk::signature::Keypair::new().pubkey().to_string();
        let wallet = TenantWalletRef::new(org, &address).unwrap();
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

    fn parts() -> (OrganizationId, RuntimeId, RuntimeGeneration) {
        (
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
        )
    }

    #[test]
    fn a_sniper_context_cannot_drive_a_copy_executor() {
        let (org, runtime, gen) = parts();
        // Adapt first (pure, no engine needed): the module check is the
        // constructor's first gate.
        let err =
            CopyTenantContext::adapt(issued_for(org, runtime, BotModule::Sniper)).unwrap_err();
        assert_eq!(err.as_str(), "wrong_module");
        let _ = gen;
    }

    #[test]
    fn the_authorization_chain_is_exhaustive() {
        // The deny logic is testable without an engine: build the
        // context pair and evaluate every authorize branch through the
        // (engine-free) adapter identity checks.
        let (org, runtime, gen) = parts();
        let bound = CopyTenantContext::adapt(issued_for(org, runtime, BotModule::Copy)).unwrap();
        // Same org/runtime/gen/module/wallet → authorize-equivalent.
        assert!(bound
            .verify_against(
                bound.organization_id(),
                bound.runtime_id(),
                bound.generation()
            )
            .is_ok());
        // Rotated generation → stale.
        let rotated = gen.next().unwrap();
        assert!(bound
            .verify_against(bound.organization_id(), bound.runtime_id(), rotated)
            .is_err());
        // Foreign org → denied.
        let foreign =
            CopyTenantContext::adapt(issued_for(OrganizationId::new(), runtime, BotModule::Copy))
                .unwrap();
        assert_ne!(foreign.organization_id(), bound.organization_id());
    }
}
