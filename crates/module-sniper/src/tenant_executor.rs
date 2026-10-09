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

// ---------------------------------------------------------------------------
// Wallet pools (GAP-MAP v2 P2; durable schema in migration 0050)
// ---------------------------------------------------------------------------

/// How a pool assigns buys to its members. Mirrors `wallet_pools.allocation`
/// in migration 0050.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolAllocation {
    /// Next active member in deterministic rotation order; the cursor is
    /// persisted server-side (`wallet_pools.rotation_cursor`) so rotation
    /// survives restarts.
    RoundRobin,
    /// Members chosen proportionally to `weight` via a smooth weighted
    /// round-robin (nginx-style): deterministic, no RNG, exact long-run
    /// proportions.
    WeightedSplit,
}

impl PoolAllocation {
    /// Parse the SQL column value. Unknown values fail closed.
    pub fn from_column(value: &str) -> BotResult<Self> {
        match value {
            "round_robin" => Ok(Self::RoundRobin),
            "weighted_split" => Ok(Self::WeightedSplit),
            other => Err(BotError::invalid(format!(
                "wallet pool: unknown allocation '{other}'"
            ))),
        }
    }

    /// The SQL column value.
    pub fn as_str(&self) -> &'static str {
        match self {
            PoolAllocation::RoundRobin => "round_robin",
            PoolAllocation::WeightedSplit => "weighted_split",
        }
    }
}

/// One pool member (projection of an ACTIVE `wallet_pool_members` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolMember {
    /// Wallet address of the tenant-bound custody signer.
    pub wallet_address: String,
    /// Selection weight for `weighted_split` (1..=1000 per the schema);
    /// ignored by round-robin.
    pub weight: u32,
    /// Smooth-round-robin bookkeeping (not persisted).
    current_weight: i64,
    /// How many positions this wallet currently holds open. Positions stay
    /// with their ORIGINAL wallet for their whole life; only NEW buys are
    /// re-assigned.
    open_positions: u32,
}

/// In-memory selector over one tenant's wallet pool. The server builds this
/// from the pool + member rows at executor startup and re-builds it when
/// membership changes (changes are immediate by design: a removed member
/// stops receiving new orders but keeps its open positions).
#[derive(Debug, Clone)]
pub struct WalletPoolSelector {
    allocation: PoolAllocation,
    members: Vec<PoolMember>,
    /// Index of the LAST member used by round-robin (mirrors the persisted
    /// cursor so a restart continues where it left off).
    rotation_cursor: usize,
    /// Hard cap of concurrently open positions per wallet. `None` = no cap
    /// (the risk engine still applies its global exposure limits).
    max_open_per_wallet: Option<u32>,
}

/// The outcome of a pool selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolPick {
    /// Wallet address that should execute the buy.
    pub wallet_address: String,
    /// Positions that wallet held open BEFORE this pick.
    pub open_before: u32,
}

impl WalletPoolSelector {
    /// Build from the allocation column and members (already filtered to
    /// `status = 'active'`, ordered deterministically by member id).
    pub fn new(
        allocation: PoolAllocation,
        members: Vec<(String, u32)>,
        rotation_cursor: usize,
        max_open_per_wallet: Option<u32>,
    ) -> BotResult<Self> {
        if members.is_empty() {
            return Err(BotError::invalid(
                "wallet pool: at least one active member is required",
            ));
        }
        for (address, weight) in &members {
            if address.trim().is_empty() {
                return Err(BotError::invalid("wallet pool: empty member address"));
            }
            if *weight == 0 {
                return Err(BotError::invalid(format!(
                    "wallet pool: member {address} has weight 0"
                )));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (address, _) in &members {
            if !seen.insert(address.as_str()) {
                return Err(BotError::invalid(format!(
                    "wallet pool: duplicate member {address}"
                )));
            }
        }
        Ok(Self {
            allocation,
            members: members
                .into_iter()
                .map(|(wallet_address, weight)| PoolMember {
                    wallet_address,
                    weight,
                    current_weight: 0,
                    open_positions: 0,
                })
                .collect(),
            rotation_cursor,
            max_open_per_wallet,
        })
    }

    /// The allocation strategy.
    pub fn allocation(&self) -> PoolAllocation {
        self.allocation
    }

    /// Active member count.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// True with no members (cannot happen through `new`, kept for callers
    /// filtering membership live).
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The persisted round-robin cursor (for the server to write back to
    /// `wallet_pools.rotation_cursor`).
    pub fn rotation_cursor(&self) -> usize {
        self.rotation_cursor
    }

    /// Open positions currently tracked for one wallet.
    pub fn open_positions(&self, wallet_address: &str) -> u32 {
        self.members
            .iter()
            .find(|m| m.wallet_address == wallet_address)
            .map(|m| m.open_positions)
            .unwrap_or(0)
    }

    /// Pick the wallet for the NEXT buy. Fails closed when every member is
    /// at its per-wallet position cap. Deterministic for both strategies.
    pub fn select_for_buy(&mut self) -> BotResult<PoolPick> {
        let eligible = self
            .members
            .iter()
            .filter(|m| match self.max_open_per_wallet {
                None => true,
                Some(cap) => m.open_positions < cap,
            })
            .count();
        if eligible == 0 {
            return Err(BotError::risk(
                "wallet pool: every member is at its per-wallet position cap",
            ));
        }
        let index = match self.allocation {
            PoolAllocation::RoundRobin => self.round_robin_pick(),
            PoolAllocation::WeightedSplit => self.weighted_pick(),
        };
        let member = &mut self.members[index];
        let pick = PoolPick {
            wallet_address: member.wallet_address.clone(),
            open_before: member.open_positions,
        };
        member.open_positions += 1;
        Ok(pick)
    }

    /// Round-robin: walk forward from the cursor, skipping wallets at their
    /// cap, then advance the cursor past the picked member.
    fn round_robin_pick(&mut self) -> usize {
        let n = self.members.len();
        let start = self.rotation_cursor % n;
        let mut i = start;
        loop {
            let eligible = match self.max_open_per_wallet {
                None => true,
                Some(cap) => self.members[i].open_positions < cap,
            };
            if eligible {
                self.rotation_cursor = (i + 1) % n;
                return i;
            }
            i = (i + 1) % n;
        }
    }

    /// Smooth weighted round-robin (nginx-style): add each member's weight
    /// to its current weight, pick the max among cap-eligible members, then
    /// subtract the total weight from the pick. Deterministic, no RNG, and
    /// the long-run share of each member equals weight/total exactly.
    fn weighted_pick(&mut self) -> usize {
        let total: i64 = self.members.iter().map(|m| i64::from(m.weight)).sum();
        for member in &mut self.members {
            member.current_weight += i64::from(member.weight);
        }
        let mut best: Option<usize> = None;
        for (i, member) in self.members.iter().enumerate() {
            let capped = self
                .max_open_per_wallet
                .map(|cap| member.open_positions >= cap)
                .unwrap_or(false);
            if capped {
                continue;
            }
            match best {
                None => best = Some(i),
                Some(b) if member.current_weight > self.members[b].current_weight => {
                    best = Some(i)
                }
                _ => {}
            }
        }
        // `select_for_buy` guarantees at least one eligible member.
        let index = best.expect("weighted pick: caller guarantees an eligible member");
        self.members[index].current_weight -= total;
        index
    }

    /// A position on `wallet_address` was opened externally (recovery /
    /// re-adoption after restart): sync the bookkeeping.
    pub fn note_open(&mut self, wallet_address: &str) {
        if let Some(member) = self
            .members
            .iter_mut()
            .find(|m| m.wallet_address == wallet_address)
        {
            member.open_positions += 1;
        }
    }

    /// A position closed — frees capacity on that wallet.
    pub fn note_closed(&mut self, wallet_address: &str) {
        if let Some(member) = self
            .members
            .iter_mut()
            .find(|m| m.wallet_address == wallet_address)
        {
            member.open_positions = member.open_positions.saturating_sub(1);
        }
    }

    /// Remove a member (immediate: it stops receiving new orders; its open
    /// positions are unaffected and still closeable). Returns true when the
    /// member existed.
    pub fn remove_member(&mut self, wallet_address: &str) -> bool {
        let before = self.members.len();
        self.members.retain(|m| m.wallet_address != wallet_address);
        if self.members.len() != before {
            self.rotation_cursor = 0;
            true
        } else {
            false
        }
    }
}

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
    /// Optional wallet pool for spreading buys across several of the
    /// tenant's custody wallets (GAP-MAP v2 P2, migration 0050). When
    /// present, each NEW buy is assigned a pool wallet by
    /// [`Self::select_wallet_for_buy`]; the server routes that buy through
    /// the pool member's own signing context. When absent, buys use the
    /// single bound funding wallet exactly as before.
    pool: Option<WalletPoolSelector>,
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
            pool: None,
        })
    }

    /// Attach the tenant-scoped persistence sink.
    #[must_use]
    pub fn with_execution_sink(mut self, sink: Arc<dyn TenantExecutionSink>) -> Self {
        self.sink = sink;
        self
    }

    /// Attach a wallet pool: new buys are spread across the pool members
    /// (round-robin or weighted, per migration 0050) instead of always
    /// using the single bound funding wallet.
    #[must_use]
    pub fn with_wallet_pool(mut self, pool: WalletPoolSelector) -> Self {
        self.pool = Some(pool);
        self
    }

    /// The attached pool, if any.
    pub fn wallet_pool(&self) -> Option<&WalletPoolSelector> {
        self.pool.as_ref()
    }

    /// Mutable access to the pool (the sweep/exit path calls
    /// [`WalletPoolSelector::note_closed`] when a position exits).
    pub fn wallet_pool_mut(&mut self) -> Option<&mut WalletPoolSelector> {
        self.pool.as_mut()
    }

    /// Pick the wallet that should execute the NEXT buy for this tenant.
    /// Fails closed (typed risk error) when no pool is attached AND the
    /// caller explicitly asked for pool distribution — callers that do not
    /// configure a pool simply keep using the bound funding wallet.
    pub fn select_wallet_for_buy(&mut self) -> BotResult<PoolPick> {
        match self.pool.as_mut() {
            Some(pool) => pool.select_for_buy(),
            None => Err(BotError::invalid(
                "tenant sniper: no wallet pool attached — buys use the bound funding wallet",
            )),
        }
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

    // ---------------- wallet pool selector ----------------

    fn pool(allocation: PoolAllocation, members: &[(&str, u32)], cap: Option<u32>) -> WalletPoolSelector {
        WalletPoolSelector::new(
            allocation,
            members.iter().map(|(a, w)| (a.to_string(), *w)).collect(),
            0,
            cap,
        )
        .unwrap()
    }

    #[test]
    fn pool_rejects_empty_duplicate_and_zero_weight() {
        assert!(WalletPoolSelector::new(PoolAllocation::RoundRobin, vec![], 0, None).is_err());
        assert!(WalletPoolSelector::new(
            PoolAllocation::RoundRobin,
            vec![("A".into(), 1), ("A".into(), 1)],
            0,
            None,
        )
        .is_err());
        assert!(WalletPoolSelector::new(
            PoolAllocation::RoundRobin,
            vec![("A".into(), 0)],
            0,
            None,
        )
        .is_err());
        assert!(WalletPoolSelector::new(
            PoolAllocation::RoundRobin,
            vec![("".into(), 1)],
            0,
            None,
        )
        .is_err());
    }

    #[test]
    fn allocation_column_round_trips_and_fails_closed() {
        assert_eq!(PoolAllocation::from_column("round_robin").unwrap(), PoolAllocation::RoundRobin);
        assert_eq!(PoolAllocation::from_column("weighted_split").unwrap(), PoolAllocation::WeightedSplit);
        assert!(PoolAllocation::from_column("random").is_err());
        assert_eq!(PoolAllocation::RoundRobin.as_str(), "round_robin");
        assert_eq!(PoolAllocation::WeightedSplit.as_str(), "weighted_split");
    }

    #[test]
    fn round_robin_rotates_and_resumes_from_cursor() {
        let mut sel = pool(
            PoolAllocation::RoundRobin,
            &[("A", 1), ("B", 1), ("C", 1)],
            None,
        );
        let got: Vec<String> = (0..6)
            .map(|_| sel.select_for_buy().unwrap().wallet_address)
            .collect();
        assert_eq!(got, vec!["A", "B", "C", "A", "B", "C"]);
        assert_eq!(sel.rotation_cursor(), 0, "cursor wraps to the head");

        // Restart with a persisted cursor: rotation continues mid-cycle.
        let mut resumed = WalletPoolSelector::new(
            PoolAllocation::RoundRobin,
            vec![("A".into(), 1), ("B".into(), 1), ("C".into(), 1)],
            2,
            None,
        )
        .unwrap();
        assert_eq!(resumed.select_for_buy().unwrap().wallet_address, "C");
        assert_eq!(resumed.select_for_buy().unwrap().wallet_address, "A");
    }

    #[test]
    fn round_robin_skips_wallets_at_their_cap() {
        let mut sel = pool(
            PoolAllocation::RoundRobin,
            &[("A", 1), ("B", 1), ("C", 1)],
            Some(1),
        );
        assert_eq!(sel.select_for_buy().unwrap().wallet_address, "A");
        assert_eq!(sel.select_for_buy().unwrap().wallet_address, "B");
        assert_eq!(sel.select_for_buy().unwrap().wallet_address, "C");
        // Every wallet now holds one open position = the cap.
        let err = sel.select_for_buy().unwrap_err();
        assert!(err.to_string().contains("cap"));
        // Freeing one wallet reopens selection there.
        sel.note_closed("B");
        assert_eq!(sel.select_for_buy().unwrap().wallet_address, "B");
    }

    #[test]
    fn weighted_split_matches_weights_exactly_over_a_cycle() {
        // Weights 5/3/2 -> over 10 buys the shares are exactly 5/3/2.
        let mut sel = pool(
            PoolAllocation::WeightedSplit,
            &[("A", 5), ("B", 3), ("C", 2)],
            None,
        );
        let mut counts = std::collections::HashMap::new();
        for _ in 0..10 {
            let pick = sel.select_for_buy().unwrap();
            *counts.entry(pick.wallet_address).or_insert(0) += 1;
        }
        assert_eq!(counts.get("A").copied().unwrap_or(0), 5);
        assert_eq!(counts.get("B").copied().unwrap_or(0), 3);
        assert_eq!(counts.get("C").copied().unwrap_or(0), 2);
    }

    #[test]
    fn weighted_split_never_starves_a_small_weight() {
        // Smooth round-robin interleaves instead of front-loading the big
        // member: the first three picks of a 2/1 pool must contain both.
        let mut sel = pool(PoolAllocation::WeightedSplit, &[("BIG", 2), ("small", 1)], None);
        let picks: Vec<String> = (0..3)
            .map(|_| sel.select_for_buy().unwrap().wallet_address)
            .collect();
        assert!(picks.contains(&"BIG".to_string()));
        assert!(picks.contains(&"small".to_string()));
    }

    #[test]
    fn remove_member_is_immediate_for_new_orders_only() {
        let mut sel = pool(
            PoolAllocation::RoundRobin,
            &[("A", 1), ("B", 1)],
            None,
        );
        let pick = sel.select_for_buy().unwrap();
        assert_eq!(pick.wallet_address, "A");
        assert_eq!(sel.open_positions("A"), 1);
        assert!(sel.remove_member("A"));
        assert_eq!(sel.len(), 1);
        // New orders only go to B now.
        assert_eq!(sel.select_for_buy().unwrap().wallet_address, "B");
        assert!(!sel.remove_member("A"), "already removed");
    }

    #[test]
    fn open_position_bookkeeping_tracks_open_and_close() {
        let mut sel = pool(
            PoolAllocation::RoundRobin,
            &[("A", 1), ("B", 1)],
            None,
        );
        sel.note_open("A"); // recovered position after restart
        sel.note_open("A");
        assert_eq!(sel.open_positions("A"), 2);
        sel.note_closed("A");
        assert_eq!(sel.open_positions("A"), 1);
        // Closing below zero saturates — bookkeeping can never wrap.
        sel.note_closed("B");
        assert_eq!(sel.open_positions("B"), 0);
        assert_eq!(sel.open_positions("missing"), 0);
    }

    #[tokio::test]
    async fn executor_without_pool_refuses_pool_selection() {
        let kp = solana_sdk::signature::Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let wallet = Arc::new(Wallet::load(&b58).unwrap());
        let address = wallet.pubkey.to_string();
        let bound = issued_for(
            BotModule::Sniper,
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            &address,
        );
        let shared = AppState::new(bot_core::config::AppConfig::from_defaults());
        let rpc = Rpc::new(&bot_core::config::NetworkConfig::default()).unwrap();
        let mut executor = TenantSniperExecutor::new(shared, rpc, wallet, None, bound)
            .await
            .unwrap();
        assert!(executor.wallet_pool().is_none());
        assert!(executor.select_wallet_for_buy().is_err());

        // Attaching a pool enables distribution.
        executor = executor.with_wallet_pool(pool(
            PoolAllocation::RoundRobin,
            &[("W1", 1), ("W2", 1)],
            None,
        ));
        assert_eq!(executor.select_wallet_for_buy().unwrap().wallet_address, "W1");
        assert_eq!(executor.select_wallet_for_buy().unwrap().wallet_address, "W2");
    }
}
