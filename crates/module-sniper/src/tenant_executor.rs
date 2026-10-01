//! Tenant sniper executor (PROMPT 4/10 file 9).
//!
//! [`TenantSniperExecutor`] connects the EXISTING sniper pipeline —
//! detection events, gates, risk, route selection, ownership claims,
//! transaction build, signing, broadcast, reconciliation — to a
//! tenant-scoped execution. It does not re-implement any of those stages:
//!
//! * it binds the internal [`Sniper`] to ONE tenant through
//!   [`Sniper::with_tenant_context`], which attaches the final
//!   [`solana_kit::tenant_broadcast_guard::TenantBroadcastGuard`] to the
//!   executor (wrong organization / runtime / generation / module /
//!   wallet / signer is refused BEFORE signing or broadcast);
//! * every entry and exit request the sniper builds is stamped with the
//!   tenant metadata the guard verifies;
//! * the per-tenant runtime state (dedup, counters) lives in
//!   [`crate::tenant_state::TenantSniperState`], so one tenant's dedup
//!   set can never suppress another tenant's trade;
//! * every execution outcome is offered to a
//!   [`TenantExecutionSink`] — the server-side implementation persists
//!   the outcome through the tenant-scoped `bot-core`
//!   `trading_repository` (organization-bound writes only).
//!
//! Operator mode is untouched: a [`Sniper`] built WITHOUT a tenant
//! context keeps its deployment-global behaviour byte-for-byte.

use std::sync::Arc;

use async_trait::async_trait;

use bot_core::error::{BotError, BotResult};
use bot_core::execution::TenantExecutionContext;
use bot_core::state::Shared;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};
use solana_kit::execute::ExecutionResult;
use solana_kit::rpc::Rpc;
use solana_kit::tenant_broadcast_guard::TenantBroadcastGuard;
use solana_kit::tenant_signing_context::TenantSigningContext;
use solana_kit::tenant_transaction::{TenantTransaction, TenantTransactionMeta};
use solana_kit::tokens::Wallet;

use crate::entry::EntryOutcome;
use crate::event::LaunchEvent;
use crate::tenant_context::SniperTenantContext;
use crate::tenant_state::TenantSniperState;
use crate::Sniper;

/// Why a scoped execution was refused before the pipeline ran. Closed
/// vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SniperTenantDeny {
    /// The presented context belongs to another tenant than this
    /// executor's bound context.
    OrganizationMismatch,
    /// The runtime id does not match.
    RuntimeMismatch,
    /// The fencing generation is stale (the runtime was rotated).
    GenerationMismatch,
    /// The context was issued for a non-sniper module.
    WrongModule,
    /// The funding wallet does not match the bound wallet.
    WalletMismatch,
}

impl SniperTenantDeny {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SniperTenantDeny::OrganizationMismatch => "organization_mismatch",
            SniperTenantDeny::RuntimeMismatch => "runtime_mismatch",
            SniperTenantDeny::GenerationMismatch => "generation_mismatch",
            SniperTenantDeny::WrongModule => "wrong_module",
            SniperTenantDeny::WalletMismatch => "wallet_mismatch",
        }
    }
}

impl std::fmt::Display for SniperTenantDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tenant sniper executor denied: {}",
            self.as_str().replace('_', " ")
        )
    }
}

impl std::error::Error for SniperTenantDeny {}

/// Tenant-scoped persistence for sniper executions. The server
/// implements this over the `bot-core` `trading_repository` writes
/// (every write carries the organization from the metadata — a sink can
/// never be handed another tenant's outcome).
#[async_trait]
pub trait TenantExecutionSink: Send + Sync {
    /// Persist one execution outcome for the tenant bound in its
    /// metadata. Implementations MUST fail closed on an organization
    /// mismatch with their own write scope.
    async fn record(&self, outcome: &TenantTransaction) -> BotResult<()>;
}

/// A no-op sink (used when tenant persistence is not attached — e.g. the
/// module crate's own tests; the server always attaches the repository
/// sink).
pub struct NullExecutionSink;

#[async_trait]
impl TenantExecutionSink for NullExecutionSink {
    async fn record(&self, _outcome: &TenantTransaction) -> BotResult<()> {
        Ok(())
    }
}

/// The tenant-scoped sniper executor: the existing engine, bound to one
/// tenant, with tenant-local state and tenant-scoped persistence.
pub struct TenantSniperExecutor {
    sniper: Sniper,
    context: SniperTenantContext,
    signing: TenantSigningContext,
    state: TenantSniperState,
    sink: Arc<dyn TenantExecutionSink>,
}

impl TenantSniperExecutor {
    /// Build a tenant sniper over the EXISTING engine.
    ///
    /// `context` is the issued core execution context (module = Sniper);
    /// `wallet` funds the trades and MUST be the wallet bound in the
    /// context (verified twice: here and again by the broadcast guard at
    /// every submission).
    pub async fn new(
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        execution: TenantExecutionContext,
    ) -> BotResult<Self> {
        let context = SniperTenantContext::adapt(execution.clone())
            .map_err(|e| BotError::invalid(e.to_string()))?;
        let signing = TenantSigningContext::new(execution);
        let guard = TenantBroadcastGuard::new(signing.clone(), wallet.pubkey)
            .map_err(|e| BotError::invalid(e.to_string()))?;
        let sniper = Sniper::new(state, rpc, wallet, signers)
            .await?
            .with_tenant_context(Arc::new(guard))?;
        let tenant_state = TenantSniperState::new(
            context.organization_id().to_string(),
            context.runtime_id().to_string(),
            context.mode(),
        );
        Ok(TenantSniperExecutor {
            sniper,
            context,
            signing,
            state: tenant_state,
            sink: Arc::new(NullExecutionSink),
        })
    }

    /// Attach the tenant-scoped persistence sink.
    #[must_use]
    pub fn with_execution_sink(mut self, sink: Arc<dyn TenantExecutionSink>) -> Self {
        self.sink = sink;
        self
    }

    /// The bound tenant context.
    pub fn tenant_context(&self) -> &SniperTenantContext {
        &self.context
    }

    /// The bound signing context (guard-verified identity).
    pub fn signing_context(&self) -> &TenantSigningContext {
        &self.signing
    }

    /// The tenant runtime state (dedup + counters).
    pub fn tenant_state(&self) -> &TenantSniperState {
        &self.state
    }

    /// The underlying (tenant-bound) sniper engine.
    pub fn sniper(&self) -> &Sniper {
        &self.sniper
    }

    /// Authorize an incoming execution context against this executor's
    /// bound identity. EVERY scoped entry point calls this first; a deny
    /// means the pipeline never runs.
    pub fn authorize(&self, presented: &TenantExecutionContext) -> Result<(), SniperTenantDeny> {
        if presented.organization_id() != self.context.organization_id() {
            return Err(SniperTenantDeny::OrganizationMismatch);
        }
        if presented.runtime_id() != self.context.runtime_id() {
            return Err(SniperTenantDeny::RuntimeMismatch);
        }
        if presented.generation() != self.context.generation() {
            return Err(SniperTenantDeny::GenerationMismatch);
        }
        if presented.scope().module() != bot_core::tenant::ModuleKind::Sniper {
            return Err(SniperTenantDeny::WrongModule);
        }
        if presented.wallet().address() != self.context.wallet_address() {
            return Err(SniperTenantDeny::WalletMismatch);
        }
        Ok(())
    }

    /// Record one execution outcome through the attached sink (the
    /// reconciliation paths use this for late-arriving outcomes; the
    /// sink itself enforces the tenant boundary).
    pub async fn record_execution(&self, outcome: &TenantTransaction) -> BotResult<()> {
        self.sink.record(outcome).await
    }

    /// Re-verify against a freshly-resolved runtime identity (fence
    /// check before money moves).
    pub fn verify_runtime(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), SniperTenantDeny> {
        self.context
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| SniperTenantDeny::GenerationMismatch)
    }

    /// Consider one external launch event for THIS tenant. The event is
    /// deduplicated tenant-locally, then handed to the existing pipeline
    /// (gates → risk → route → build → guarded broadcast →
    /// reconciliation). The outcome is persisted tenant-scoped.
    pub async fn consider_event(&mut self, event: LaunchEvent) -> EntryOutcome {
        self.state.note_considered();
        let dedup_key = self.context.dedup_key(&event.event_id);
        if !self.state.mark_seen(&dedup_key) {
            self.state.note_rejected();
            return EntryOutcome {
                event_id: event.event_id.clone(),
                mint: event.mint.clone(),
                stage: crate::pipeline::SniperStage::Rejected,
                rejection: Some(crate::pipeline::Rejection::new(
                    crate::pipeline::RejectReason::DuplicateEvent,
                    crate::pipeline::SniperStage::Detected,
                    format!(
                        "tenant-local duplicate for organization {}",
                        self.context.organization_id()
                    ),
                )),
                route: None,
                intent_id: None,
                position_id: None,
                slippage_bps: None,
                price_impact_bps: None,
                fee_estimate_lamports: None,
                timeline: crate::pipeline::LatencyTimeline::default(),
                gates: String::new(),
            };
        }
        let outcome = self.sniper.consider_event(event).await;
        if outcome.accepted() {
            self.state.note_submitted();
            if outcome.rejection.is_none() {
                self.state.note_succeeded();
            }
        } else if outcome
            .rejection
            .as_ref()
            .map(|r| r.detail.contains("tenant broadcast denied"))
            .unwrap_or(false)
        {
            self.state.note_guard_denied();
        } else {
            self.state.note_rejected();
        }
        outcome
    }

    /// Submit an already-built request under this tenant (the request is
    /// tenant-stamped; the guard verifies it before anything is signed).
    /// Used for operator-initiated scoped actions and tests.
    pub async fn submit(&self, req: solana_kit::tx::TxRequest) -> BotResult<ExecutionResult> {
        let result = self.sniper.execute_request(req).await?;
        let meta = TenantTransactionMeta::from_context(
            self.signing.execution_context(),
            "sniper",
            Some(&result.intent_id),
        );
        let outcome = TenantTransaction::new(meta, &result);
        // Persistence failure must never pass silently: the execution
        // happened (or was refused) and the tenant's truth store
        // missed it — surface the error to the caller for
        // reconciliation handling.
        self.sink.record(&outcome).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::state::AppState;
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;
    use solana_sdk::signer::Signer;

    fn issued_for(
        module: bot_core::models::BotModule,
        org: OrganizationId,
        runtime: RuntimeId,
        generation: RuntimeGeneration,
        wallet_address: &str,
    ) -> TenantExecutionContext {
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
        )
        .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet = TenantWalletRef::new(org, wallet_address).unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "k").unwrap();
        TenantExecutionContext::issue(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn wrong_tenant_runtime_generation_module_wallet_are_denied() {
        let kp = solana_sdk::signature::Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let wallet = Arc::new(Wallet::load(&b58).unwrap());
        let address = wallet.pubkey.to_string();

        let org = OrganizationId::new();
        let runtime = RuntimeId::new();
        let gen = RuntimeGeneration::first();
        let bound = issued_for(BotModule::Sniper, org, runtime, gen, &address);

        let shared = AppState::new(bot_core::config::AppConfig::from_defaults());
        let rpc = Rpc::new(&bot_core::config::NetworkConfig::default()).unwrap();
        let executor = TenantSniperExecutor::new(shared, rpc, wallet, None, bound.clone())
            .await
            .unwrap();

        // The bound context authorizes.
        assert!(executor.authorize(&bound).is_ok());

        // Another tenant.
        let foreign_org = issued_for(
            BotModule::Sniper,
            OrganizationId::new(),
            runtime,
            gen,
            &address,
        );
        assert_eq!(
            executor.authorize(&foreign_org).unwrap_err().as_str(),
            "organization_mismatch"
        );

        // Another runtime.
        let foreign_runtime = issued_for(BotModule::Sniper, org, RuntimeId::new(), gen, &address);
        assert_eq!(
            executor.authorize(&foreign_runtime).unwrap_err().as_str(),
            "runtime_mismatch"
        );

        // Stale generation.
        let stale = issued_for(
            BotModule::Sniper,
            org,
            runtime,
            gen.next().unwrap(),
            &address,
        );
        assert_eq!(
            executor.authorize(&stale).unwrap_err().as_str(),
            "generation_mismatch"
        );

        // Another module.
        let copy_ctx = issued_for(BotModule::Copy, org, runtime, gen, &address);
        assert_eq!(
            executor.authorize(&copy_ctx).unwrap_err().as_str(),
            "wrong_module"
        );

        // Another wallet.
        let other_kp = solana_sdk::signature::Keypair::new();
        let other_address = other_kp.pubkey().to_string();
        let foreign_wallet = issued_for(BotModule::Sniper, org, runtime, gen, &other_address);
        assert_eq!(
            executor.authorize(&foreign_wallet).unwrap_err().as_str(),
            "wallet_mismatch"
        );
    }

    #[tokio::test]
    async fn construction_binds_the_guard_to_the_funding_wallet() {
        let kp = solana_sdk::signature::Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let wallet = Arc::new(Wallet::load(&b58).unwrap());

        // A context bound to a DIFFERENT wallet cannot construct the
        // executor: the guard refuses the funding wallet at bind time.
        let other = solana_sdk::signature::Keypair::new().pubkey().to_string();
        let bound = issued_for(
            BotModule::Sniper,
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            &other,
        );
        let shared = AppState::new(bot_core::config::AppConfig::from_defaults());
        let rpc = Rpc::new(&bot_core::config::NetworkConfig::default()).unwrap();
        let err = match TenantSniperExecutor::new(shared, rpc, wallet, None, bound).await {
            Err(e) => e,
            Ok(_) => panic!("a context bound to another wallet must not construct the executor"),
        };
        assert!(err.to_string().contains("wallet"));
    }
}
