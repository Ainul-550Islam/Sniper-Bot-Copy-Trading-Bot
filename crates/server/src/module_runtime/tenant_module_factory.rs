//! Tenant module factory (PROMPT 4/10 §A file 2).
//!
//! Builds the tenant-bound TRADING ENGINES from a
//! [`TenantModuleInstance`] (derived from an issued
//! [`TenantExecutionContext`] and fenced against the tenant's live
//! [`TenantRuntimeRecord`]) instead of process-global tenant-blind
//! objects, and connects them to the tenant-scoped persistence of the
//! trading repositories:
//!
//! * the sniper executor gets a [`RepoExecutionSink`] that records
//!   every broadcast through the `bot-core`
//!   `TenantExecutionWrite` repository (`transactions` rows,
//!   organization-attributed, idempotent on the signature);
//! * the copy executor gets a [`RepoCopySink`] that records every
//!   mirror outcome through the `bot-core` `TenantCopyEventRepo`
//!   (`copy_events` rows, organization-attributed, idempotent on
//!   `(organization_id, event_id)`);
//! * both sinks verify the outcome's organization against their OWN
//!   write scope before touching the database — a handed-in outcome of
//!   another tenant fails closed (`TenantMismatch`), it is never
//!   silently re-attributed.
//!
//! The factory NEVER issues contexts itself: issuance is the gateway's
//! job (authority chain, wallet/signer binding, entitlements). The
//! factory only BUILDS engines for a context the gateway already
//! allowed, after re-checking the instance identity, the fence and the
//! module wiring. Modules whose tenant wiring is not implemented
//! (Telegram is a control plane; the staking contract is on-chain)
//! are refused with an explicit `module_not_wired` error — the
//! factory never silently falls back to a deployment-global engine.
//! Since §D the Polymarket engine is wired too: its executor gets a
//! [`RepoPolySink`] recording signals, venue orders and backfilled
//! fills through the `bot-core` organization-scoped polymarket
//! repository. Operator (deployment-global) mode never passes
//! through here — `main.rs` keeps its own path.

use std::sync::Arc;

use async_trait::async_trait;

use bot_core::error::{BotError, BotResult};
use bot_core::execution::TenantExecutionContext;
use bot_core::state::Shared;
use bot_core::tenant::OrganizationId;
use bot_core::trading_repository::copy::events::TenantCopyEventRepo;
use bot_core::trading_repository::copy::model::TenantCopyEvent;
use bot_core::trading_repository::executions::write::TenantExecutionWrite;
use bot_core::trading_repository::polymarket::model::{
    TenantPolyFill, TenantPolyOrder, TenantPolySignal,
};
use bot_core::trading_repository::polymarket::read::TenantPolyRead;
use bot_core::trading_repository::polymarket::write::TenantPolyWrite;
use bot_core::trading_repository::write_scope::TenantWriteScope;
use chrono::{DateTime, Utc};
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;

use bot_core::db::Database;
use bot_core::trading_repository::write_scope::WriteOrigin;

use crate::module_runtime::tenant_module_instance::TenantModuleInstance;
use crate::runtime_registry::model::TenantRuntimeRecord;

/// The actor label the factory's write scopes carry (audit trail).
const FACTORY_ACTOR: &str = "tenant-module-factory";

/// A built tenant engine: exactly one module's tenant-bound executor.
/// The enum is closed — there is no "generic" engine and no path that
/// builds an engine without its tenant binding.
pub enum ModuleEngine {
    /// Module 1 — the tenant's sniper executor (guarded, sink-persisted).
    Sniper(module_sniper::tenant_executor::TenantSniperExecutor),
    /// Module 2 — the tenant's copy executor (guarded, sink-persisted).
    Copy(module_copy::tenant_executor::TenantCopyExecutor),
    /// Module 3 — the tenant's polymarket executor (§D: guarded,
    /// async-commit aware, sink-persisted).
    Polymarket(module_polymarket::tenant_executor::TenantPolyExecutor),
}

impl std::fmt::Debug for ModuleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The executors hold non-Debug engines; identify by module.
        match self {
            ModuleEngine::Sniper(_) => f.debug_tuple("ModuleEngine::Sniper").finish(),
            ModuleEngine::Copy(_) => f.debug_tuple("ModuleEngine::Copy").finish(),
            ModuleEngine::Polymarket(_) => f.debug_tuple("ModuleEngine::Polymarket").finish(),
        }
    }
}

impl ModuleEngine {
    /// The module this engine runs.
    pub fn module(&self) -> bot_core::models::BotModule {
        match self {
            ModuleEngine::Sniper(_) => bot_core::models::BotModule::Sniper,
            ModuleEngine::Copy(_) => bot_core::models::BotModule::Copy,
            ModuleEngine::Polymarket(_) => bot_core::models::BotModule::Polymarket,
        }
    }
}

/// Build tenant-bound module engines over the tenant repositories.
pub struct TenantModuleFactory {
    db: Arc<Database>,
}

impl TenantModuleFactory {
    /// A factory over an ATTACHED database (the same handle the
    /// trading data plane serves reads from).
    pub fn new(db: Arc<Database>) -> Self {
        TenantModuleFactory { db }
    }

    /// The database this factory writes through.
    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    #[allow(clippy::too_many_arguments)]
    /// Build the engine a [`TenantModuleInstance`] describes.
    ///
    /// Order of checks (all fail closed):
    /// 1. the instance must be an enabled runtime module;
    /// 2. the instance's fence must verify against the LIVE runtime
    ///    record (organization, runtime id, generation, active status,
    ///    lease);
    /// 3. the issued context must match the instance identity exactly;
    /// 4. the module must actually be WIRED for tenant execution —
    ///    Sniper, Copy and Polymarket (since §D) are; Telegram is a
    ///    control plane; anything else is refused.
    pub async fn build_from_instance(
        &self,
        instance: &TenantModuleInstance,
        record: &TenantRuntimeRecord,
        now: DateTime<Utc>,
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        context: TenantExecutionContext,
    ) -> BotResult<ModuleEngine> {
        instance.authorize_engine(record, now).map_err(not_wired)?;
        instance.verify_context(&context).map_err(not_wired)?;
        match instance.module() {
            bot_core::models::BotModule::Sniper => Ok(ModuleEngine::Sniper(
                self.build_sniper(state, rpc, wallet, signers, context)
                    .await?,
            )),
            bot_core::models::BotModule::Copy => Ok(ModuleEngine::Copy(
                self.build_copy(state, rpc, wallet, signers, context)
                    .await?,
            )),
            bot_core::models::BotModule::Polymarket => Ok(ModuleEngine::Polymarket(
                self.build_polymarket(state, context).await?,
            )),
            bot_core::models::BotModule::Telegram => Err(BotError::invalid(
                "module_not_wired: Telegram is a control plane and has no tenant engine instance",
            )),
            bot_core::models::BotModule::Contract => Err(BotError::invalid(
                "module_not_wired: the staking contract is an on-chain program, not a runtime engine module",
            )),
        }
    }

    /// The tenant's sniper executor, bound to the issued context and
    /// persisted through the tenant repositories. The wallet must be
    /// the wallet bound in the context (the executor's constructor and
    /// the broadcast guard both verify this).
    pub async fn build_sniper(
        &self,
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        context: TenantExecutionContext,
    ) -> BotResult<module_sniper::tenant_executor::TenantSniperExecutor> {
        let write = self.write_scope(context.organization_id())?;
        let executor = module_sniper::tenant_executor::TenantSniperExecutor::new(
            state, rpc, wallet, signers, context,
        )
        .await?;
        Ok(
            executor.with_execution_sink(Arc::new(RepoExecutionSink::new(
                TenantExecutionWrite::new(Arc::clone(&self.db)),
                write,
            ))),
        )
    }

    /// The tenant's copy executor, bound to the issued context and
    /// persisted through the tenant repositories. The wallet must be
    /// the wallet bound in the context.
    pub async fn build_copy(
        &self,
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        context: TenantExecutionContext,
    ) -> BotResult<module_copy::tenant_executor::TenantCopyExecutor> {
        let write = self.write_scope(context.organization_id())?;
        let executor = module_copy::tenant_executor::TenantCopyExecutor::new(
            state, rpc, wallet, signers, context,
        )
        .await?;
        Ok(executor.with_copy_sink(Arc::new(RepoCopySink::new(
            TenantCopyEventRepo::new(Arc::clone(&self.db)),
            write,
        ))))
    }

    /// The tenant's polymarket executor (§D), bound to the issued
    /// context and persisted through the tenant polymarket
    /// repository. The venue wallet binding (the EVM maker/funder)
    /// is verified by the executor itself before any live order.
    pub async fn build_polymarket(
        &self,
        state: Shared,
        context: TenantExecutionContext,
    ) -> BotResult<module_polymarket::tenant_executor::TenantPolyExecutor> {
        let write = self.write_scope(context.organization_id())?;
        let read = TenantPolyRead::new(Arc::clone(&self.db));
        let executor = module_polymarket::tenant_executor::TenantPolyExecutor::new(state, context)
            .await
            .map_err(|e| BotError::invalid(e.to_string()))?;
        Ok(executor.with_poly_sink(Arc::new(RepoPolySink::new(
            TenantPolyWrite::new(Arc::clone(&self.db)),
            read,
            write,
        ))))
    }

    /// A write scope for one tenant, attributed to the factory. The
    /// origin is `Job`: a tenant module engine runs as a fenced
    /// background job of the tenant's runtime, not an HTTP handler.
    fn write_scope(&self, organization_id: OrganizationId) -> BotResult<TenantWriteScope> {
        TenantWriteScope::new(organization_id, FACTORY_ACTOR, WriteOrigin::Job)
            .map_err(|e| BotError::invalid(e.to_string()))
    }
}

/// Map an instance identity error into the factory's fail-closed
/// refusal (the label is preserved for observability).
fn not_wired(e: crate::module_runtime::tenant_module_instance::InstanceIdentityError) -> BotError {
    BotError::invalid(format!("tenant module factory: {}", e))
}

/// Sniper-side persistence: every guarded broadcast becomes a
/// `transactions` row for the tenant that executed it.
pub struct RepoExecutionSink {
    executions: TenantExecutionWrite,
    write: TenantWriteScope,
}

impl RepoExecutionSink {
    /// Build the sink for ONE tenant's write scope.
    pub fn new(executions: TenantExecutionWrite, write: TenantWriteScope) -> Self {
        RepoExecutionSink { executions, write }
    }
}

/// Fail closed when the presented tenant is not the write scope's
/// tenant. Both sinks call this BEFORE any repository access — a
/// foreign outcome is refused even before a connection is needed.
fn assert_tenant_scope(
    sink: &str,
    write: &TenantWriteScope,
    presented: OrganizationId,
) -> BotResult<()> {
    if presented != write.organization_id() {
        return Err(BotError::invalid(format!(
            "{sink}: organization mismatch (sink bound to {})",
            write.organization_id()
        )));
    }
    Ok(())
}

#[async_trait]
impl module_sniper::tenant_executor::TenantExecutionSink for RepoExecutionSink {
    async fn record(
        &self,
        outcome: &solana_kit::tenant_transaction::TenantTransaction,
    ) -> BotResult<()> {
        assert_tenant_scope(
            "tenant execution sink",
            &self.write,
            outcome.organization_id(),
        )?;
        // Outcomes without a signature (denied before broadcast, pure
        // skips) never touched the chain — there is no transaction row
        // to write; the in-process execution ledger already holds them.
        let Some(signature) = outcome.signature() else {
            return Ok(());
        };
        self.executions
            .record_transaction_submitted(
                &self.write,
                "solana",
                signature,
                // The module engines trade on their own books; there is
                // no caller-facing order row to attach.
                None,
                Some(outcome.label()),
                None,
                1,
            )
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        // Precise landing semantics: only a CONFIRMED execution may be
        // recorded as landed — a SENT/UNKNOWN broadcast stays
        // `submitted` for the reconciliation matrix to resolve.
        let status = match outcome.exec_status() {
            solana_kit::execute::ExecStatus::Confirmed => "confirmed",
            solana_kit::execute::ExecStatus::SendFailed
            | solana_kit::execute::ExecStatus::SimulationFailed => "failed",
            _ => {
                if outcome.error().is_some() {
                    "failed"
                } else {
                    "submitted"
                }
            }
        };
        self.executions
            .set_transaction_status(&self.write, signature, status, None, outcome.error())
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(())
    }
}

/// Copy-side persistence: every mirror outcome becomes a `copy_events`
/// row for the tenant that processed it (idempotent on the event id,
/// so a replayed feed can never double-count).
pub struct RepoCopySink {
    events: TenantCopyEventRepo,
    write: TenantWriteScope,
}

impl RepoCopySink {
    /// Build the sink for ONE tenant's write scope.
    pub fn new(events: TenantCopyEventRepo, write: TenantWriteScope) -> Self {
        RepoCopySink { events, write }
    }
}

#[async_trait]
impl module_copy::tenant_executor::TenantCopySink for RepoCopySink {
    async fn record_outcome(
        &self,
        context: &module_copy::tenant_context::CopyTenantContext,
        event: &module_copy::event::LeaderTradeEvent,
        outcome: &module_copy::event::CopyOutcome,
    ) -> BotResult<()> {
        assert_tenant_scope("tenant copy sink", &self.write, context.organization_id())?;
        use bot_core::models::PositionSide;
        let rec = TenantCopyEvent {
            organization_id: self.write.organization_id(),
            event_id: event.event_id.clone(),
            leader: event.leader.clone(),
            // The pipeline requires a non-empty signature: the leader's
            // own transaction signature IS the event identity here.
            signature: if event.signature.is_empty() {
                event.event_id.clone()
            } else {
                event.signature.clone()
            },
            slot: event.slot as i64,
            mint: event.mint.clone(),
            side: match event.side {
                PositionSide::Long => "buy".to_string(),
                PositionSide::Short => "sell".to_string(),
            },
            venue: event.venue.to_string(),
            token_amount: event.token_amount,
            sol_amount: event.sol_amount,
            source: event.source.as_str().to_string(),
            source_sequence: event.source_sequence as i64,
            event_at: event.block_time,
            observed_at: event.observed_at,
            stage: outcome.stage.as_str().to_string(),
            reject_reason: outcome
                .rejection
                .as_ref()
                .map(|r| r.reason.as_str().to_string()),
            detail: outcome.rejection.as_ref().map(|r| r.detail.clone()),
            intent_id: outcome.intent_id.clone(),
            position_id: outcome.position_id.clone(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        self.events
            .record(&self.write, &rec)
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(())
    }
}

/// Polymarket-side persistence (§D): signal outcomes become
/// `poly_signals` rows, tracked venue orders become `poly_orders`
/// rows, and every BACKFILLED confirmed fill becomes a `poly_fills`
/// row — all for the tenant that owns the executor, idempotent on
/// the repositories' own composite arbiters.
pub struct RepoPolySink {
    writes: TenantPolyWrite,
    read: TenantPolyRead,
    write: TenantWriteScope,
}

impl RepoPolySink {
    /// Build the sink for ONE tenant's write scope.
    pub fn new(writes: TenantPolyWrite, read: TenantPolyRead, write: TenantWriteScope) -> Self {
        RepoPolySink {
            writes,
            read,
            write,
        }
    }
}

#[async_trait]
impl module_polymarket::tenant_executor::TenantPolySink for RepoPolySink {
    async fn record_signal(
        &self,
        signal: &module_polymarket::orders::OrderSignal,
        outcome: &module_polymarket::orders::SignalOutcome,
        organization: OrganizationId,
    ) -> BotResult<()> {
        assert_tenant_scope("tenant polymarket sink", &self.write, organization)?;
        let now = Utc::now();
        let rec = TenantPolySignal {
            organization_id: self.write.organization_id(),
            signal_id: signal.signal_id.clone(),
            condition_id: signal.condition_id().to_string(),
            token_id: signal.token_id().to_string(),
            outcome: signal.decision.outcome.clone(),
            side: signal.side_str().to_string(),
            strategy: signal.strategy.to_string(),
            limit_price: signal.decision.limit_price,
            size_tokens: signal.decision.size_tokens,
            stake_usd: signal.decision.stake_usd,
            mode: signal.mode.as_str().to_string(),
            stage: outcome.stage.as_str().to_string(),
            reject_reason: outcome.reject_reason.map(|r| r.as_str().to_string()),
            detail: outcome.detail.clone(),
            order_id: outcome.order_id.clone(),
            venue_order_id: outcome.venue_order_id.clone(),
            position_id: outcome.position_id.clone(),
            created_at: signal.created_at,
            updated_at: now,
        };
        self.writes
            .record_signal(&self.write, &rec)
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(())
    }

    async fn upsert_order(
        &self,
        order: &module_polymarket::orders::TrackedOrder,
        organization: OrganizationId,
    ) -> BotResult<()> {
        assert_tenant_scope("tenant polymarket sink", &self.write, organization)?;
        let now = Utc::now();
        let rec = TenantPolyOrder {
            organization_id: self.write.organization_id(),
            venue_order_id: order.venue_order_id.clone(),
            order_id: order.order_id.clone(),
            signal_id: order.signal_id.clone(),
            condition_id: order.condition_id.clone(),
            token_id: order.token_id.clone(),
            outcome: order.outcome.clone(),
            side: if order.is_buy {
                "buy".to_string()
            } else {
                "sell".to_string()
            },
            order_type: order.order_type.clone(),
            limit_price: order.limit_price,
            size_tokens: order.size_tokens,
            size_matched: order.size_matched,
            mode: order.mode.as_str().to_string(),
            state: order.state.as_str().to_string(),
            venue_status: order.venue_status.clone(),
            expiration: order.expiration as i64,
            position_id: order.position_id.clone(),
            replica_id: FACTORY_ACTOR.to_string(),
            submitted_at: order.submitted_at,
            updated_at: now,
            closed_at: None,
        };
        self.writes
            .upsert_order(&self.write, &rec)
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(())
    }

    async fn record_fill(
        &self,
        fill: &module_polymarket::backfill::BackfilledFill,
        organization: OrganizationId,
    ) -> BotResult<()> {
        assert_tenant_scope("tenant polymarket sink", &self.write, organization)?;
        // The venue trade record names the VENUE order id; the row
        // wants the OMS order id, the token and the position. All
        // three come from this tenant's own mirror row — looked up,
        // never guessed.
        let scope = bot_core::trading_repository::query_scope::TradingQueryScope::new(
            self.write.organization_id(),
        );
        let order = self
            .read
            .order(&scope, &fill.order_id)
            .await
            .map_err(|e| BotError::db(e.to_string()))?
            .ok_or_else(|| {
                BotError::invalid(format!(
                    "tenant polymarket sink: no mirror order {id} for backfilled fill",
                    id = fill.order_id
                ))
            })?;
        let fill_id = match fill.bucket_index {
            Some(bucket) => format!("{}:{}", fill.trade_id, bucket),
            None => fill.trade_id.clone(),
        };
        let rec = TenantPolyFill {
            organization_id: self.write.organization_id(),
            fill_id,
            venue_order_id: fill.order_id.clone(),
            order_id: order.order_id.clone(),
            token_id: order.token_id.clone(),
            side: if fill.side.eq_ignore_ascii_case("BUY") {
                "buy".to_string()
            } else {
                "sell".to_string()
            },
            price: fill.price,
            size_tokens: fill.size,
            quote_usd: fill.price * fill.size,
            source: "recon".to_string(),
            position_id: order.position_id.clone(),
            ts: Utc::now(),
        };
        self.writes
            .record_fill(&self.write, &rec)
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fail-closed scope check refuses a foreign tenant BEFORE any
    /// repository access (no database is constructed at all).
    #[test]
    fn the_scope_check_refuses_foreign_tenants_without_a_database() {
        let write = TenantWriteScope::new(OrganizationId::new(), "test", WriteOrigin::Http)
            .expect("write scope");
        // The sink's own tenant passes.
        assert!(assert_tenant_scope("sink", &write, write.organization_id()).is_ok());
        // Any other tenant is refused, whichever sink asks.
        let err = assert_tenant_scope("tenant execution sink", &write, OrganizationId::new())
            .unwrap_err();
        assert!(err.to_string().contains("organization mismatch"), "{err}");
        let err =
            assert_tenant_scope("tenant copy sink", &write, OrganizationId::new()).unwrap_err();
        assert!(err.to_string().contains("organization mismatch"), "{err}");
    }

    /// Unwired modules are refused explicitly — never a silent
    /// deployment-global fallback. Since §D the wired set is Sniper,
    /// Copy and Polymarket; Telegram and the staking contract stay
    /// refused. This test needs no database: the wiring refusal
    /// labels are pure.
    #[test]
    fn unwired_modules_are_refused_without_a_database() {
        // The control-plane / on-chain modules keep their explicit
        // refusals (mirrors of the match arms in build_from_instance).
        let err = BotError::invalid(
            "module_not_wired: Telegram is a control plane and has no tenant engine instance",
        );
        assert!(err.to_string().contains("module_not_wired"));
        let err = BotError::invalid(
            "module_not_wired: the staking contract is an on-chain program, not a runtime engine module",
        );
        assert!(err.to_string().contains("module_not_wired"));
        // And the identity-error mapping preserves the label.
        let mapped = not_wired(
            crate::module_runtime::tenant_module_instance::InstanceIdentityError::ModuleDisabled,
        );
        assert!(mapped.to_string().contains("module_disabled"), "{mapped}");
    }
}
