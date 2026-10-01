//! Tenant polymarket executor (PROMPT 4/10 §D).
//!
//! [`TenantPolyExecutor`] connects the EXISTING polymarket engine —
//! discovery, strategy, gates, risk, OMS idempotency, EIP-712
//! signing, posting, tracking, reconciliation — to ONE tenant. It
//! does not re-implement any pipeline stage:
//!
//! * it binds a private [`PolyBot`] instance to the tenant (the
//!   engine's internal journal stays in-memory; DURABLE tenant
//!   persistence flows exclusively through the
//!   [`TenantPolySink`], which the server implements over the
//!   organization-scoped `trading_repository` writes — a tenant
//!   execution can never land in another tenant's rows);
//! * it enforces the tenant boundary BEFORE the pipeline runs:
//!   organization / runtime / generation / module / wallet must match
//!   the bound context, a paper context never submits live orders,
//!   and a live order's maker must be the wallet bound in the
//!   context;
//! * signal dedup is tenant-local (one tenant's seen-set can never
//!   suppress another tenant's identical-looking intent);
//! * async acceptances (matched-without-hashes, `delayed`) land in
//!   the engine's async registry and are backfilled/reconciled
//!   through [`crate::backfill`] / [`crate::reconcile_async`] with
//!   every confirmed fill persisted through the sink.
//!
//! Operator mode is untouched: a [`PolyBot`] built without a tenant
//! executor keeps its deployment-global behaviour byte-for-byte.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;

use bot_core::config::PolymarketConfig;
use bot_core::error::BotResult;
use bot_core::execution::TenantExecutionContext;
use bot_core::models::{ExecutionMode, PolyMarket};
use bot_core::state::Shared;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

use crate::async_commit::AsyncOrderAcceptance;
use crate::backfill::{backfill_order, poll_once, AsyncPendingRegistry, BackfillVerdict};
use crate::error::{PolyError, PolyResult};
use crate::orders::{OrderSignal, PolyStage, RejectReason, SignalOutcome, TrackedOrder};
use crate::strategy::Quote;
use crate::tenant_context::{PolyContextError, PolyTenantContext};
use crate::PolyBot;

/// How long an async acceptance may stay pending before it is
/// surfaced as stale to reconciliation.
pub const ASYNC_STALE_AFTER_SECS: i64 = 900;

/// Bounded tenant-local dedup + counters (the polymarket analogue of
/// the sniper/copy tenant states — same FIFO-bounded discipline).
#[derive(Debug)]
pub struct TenantPolyState {
    organization: String,
    runtime: String,
    mode: ExecutionMode,
    seen: VecDeque<String>,
    cap: usize,
    considered: AtomicU64,
    duplicates: AtomicU64,
    rejected: AtomicU64,
    denied: AtomicU64,
}

const DEFAULT_DEDUP_CAP: usize = 8_192;

impl TenantPolyState {
    /// New tenant state for one (organization, runtime) pair.
    pub fn new(organization: String, runtime: String, mode: ExecutionMode) -> TenantPolyState {
        TenantPolyState {
            organization,
            runtime,
            mode,
            seen: VecDeque::new(),
            cap: DEFAULT_DEDUP_CAP,
            considered: AtomicU64::new(0),
            duplicates: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            denied: AtomicU64::new(0),
        }
    }

    /// Mark a dedup key seen; `false` when it was already seen (a
    /// duplicate). Bounded FIFO: the oldest key is forgotten at cap.
    pub fn mark_seen(&mut self, key: &str) -> bool {
        if self.seen.iter().any(|k| k == key) {
            return false;
        }
        if self.seen.len() >= self.cap {
            self.seen.pop_front();
        }
        self.seen.push_back(key.to_string());
        true
    }

    /// The acting tenant (display/observability).
    pub fn organization(&self) -> &str {
        &self.organization
    }

    /// The acting runtime (display/observability).
    pub fn runtime(&self) -> &str {
        &self.runtime
    }

    /// The bound execution mode.
    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }

    /// Counters (all monotonically increasing).
    pub fn counters(&self) -> TenantPolyCounters {
        TenantPolyCounters {
            considered: self.considered.load(Ordering::Relaxed),
            duplicates: self.duplicates.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            denied: self.denied.load(Ordering::Relaxed),
        }
    }

    fn note_considered(&self) {
        self.considered.fetch_add(1, Ordering::Relaxed);
    }

    fn note_duplicate(&self) {
        self.duplicates.fetch_add(1, Ordering::Relaxed);
    }

    fn note_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }

    fn note_denied(&self) {
        self.denied.fetch_add(1, Ordering::Relaxed);
    }
}

/// Point-in-time tenant counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantPolyCounters {
    /// Signals offered to the executor.
    pub considered: u64,
    /// Tenant-deduplicated repeats.
    pub duplicates: u64,
    /// Pipeline rejections (any reason).
    pub rejected: u64,
    /// Tenant-boundary denials (guard refusals).
    pub denied: u64,
}

/// Why a scoped execution was refused before the pipeline ran. Closed
/// vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyTenantDeny {
    /// The presented context belongs to another tenant.
    OrganizationMismatch,
    /// The runtime id does not match.
    RuntimeMismatch,
    /// The fencing generation is stale (the runtime was rotated).
    GenerationMismatch,
    /// The context was issued for a non-polymarket module.
    WrongModule,
    /// The venue wallet (EVM maker/funder) does not match the bound
    /// wallet.
    WalletMismatch,
    /// The bound venue wallet is not a usable EVM address.
    InvalidVenueWallet,
    /// A live signal was presented under a paper-only context —
    /// refused, never silently downgraded.
    LiveRefusedUnderPaper,
    /// The engine has no signer configured, so no live order can be
    /// attributed to the bound wallet.
    NoEngineSigner,
}

impl PolyTenantDeny {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            PolyTenantDeny::OrganizationMismatch => "organization_mismatch",
            PolyTenantDeny::RuntimeMismatch => "runtime_mismatch",
            PolyTenantDeny::GenerationMismatch => "generation_mismatch",
            PolyTenantDeny::WrongModule => "wrong_module",
            PolyTenantDeny::WalletMismatch => "wallet_mismatch",
            PolyTenantDeny::InvalidVenueWallet => "invalid_venue_wallet",
            PolyTenantDeny::LiveRefusedUnderPaper => "live_refused_under_paper",
            PolyTenantDeny::NoEngineSigner => "no_engine_signer",
        }
    }
}

impl std::fmt::Display for PolyTenantDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tenant polymarket executor denied: {}",
            self.as_str().replace('_', " ")
        )
    }
}

impl std::error::Error for PolyTenantDeny {}

/// Everything that can stop a scoped polymarket execution before or
/// after the pipeline: a boundary deny, or a tenant-persistence
/// failure (which is NEVER swallowed and never mislabelled as a
/// boundary deny).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyExecutorError {
    /// The tenant boundary refused the execution.
    Denied(PolyTenantDeny),
    /// The tenant-scoped persistence sink failed after (or before)
    /// venue interaction; the venue outcome may exist — the caller
    /// must treat this as ambiguous, not rejected.
    PersistFailed(String),
}

impl std::fmt::Display for PolyExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolyExecutorError::Denied(d) => write!(f, "{d}"),
            PolyExecutorError::PersistFailed(m) => {
                write!(f, "tenant polymarket persistence failed: {m}")
            }
        }
    }
}

impl std::error::Error for PolyExecutorError {}

impl From<PolyTenantDeny> for PolyExecutorError {
    fn from(d: PolyTenantDeny) -> Self {
        PolyExecutorError::Denied(d)
    }
}

/// Tenant-scoped persistence for the polymarket engine. The server
/// implements this over the `bot-core` organization-scoped
/// `trading_repository::polymarket` writes; every record carries the
/// organization from the bound context and the repository itself
/// fails closed on a mismatch.
#[async_trait]
pub trait TenantPolySink: Send + Sync {
    /// Record one signal outcome (the `poly_signals` row).
    async fn record_signal(
        &self,
        signal: &OrderSignal,
        outcome: &SignalOutcome,
        organization: OrganizationId,
    ) -> BotResult<()>;

    /// Upsert one tracked venue order (the `poly_orders` row).
    async fn upsert_order(
        &self,
        order: &TrackedOrder,
        organization: OrganizationId,
    ) -> BotResult<()>;

    /// Record one backfilled confirmed fill (the `poly_fills` row;
    /// the fill's venue order row carries the mode).
    async fn record_fill(
        &self,
        fill: &crate::backfill::BackfilledFill,
        organization: OrganizationId,
    ) -> BotResult<()>;
}

/// A no-op sink (module-crate tests; the server always attaches the
/// repository sink).
pub struct NullPolySink;

#[async_trait]
impl TenantPolySink for NullPolySink {
    async fn record_signal(
        &self,
        _signal: &OrderSignal,
        _outcome: &SignalOutcome,
        _organization: OrganizationId,
    ) -> BotResult<()> {
        Ok(())
    }

    async fn upsert_order(
        &self,
        _order: &TrackedOrder,
        _organization: OrganizationId,
    ) -> BotResult<()> {
        Ok(())
    }

    async fn record_fill(
        &self,
        _fill: &crate::backfill::BackfilledFill,
        _organization: OrganizationId,
    ) -> BotResult<()> {
        Ok(())
    }
}

/// The tenant-scoped polymarket executor: a private engine bound to
/// one tenant, tenant-local dedup, and tenant-scoped persistence.
pub struct TenantPolyExecutor {
    bot: PolyBot,
    context: PolyTenantContext,
    state: Mutex<TenantPolyState>,
    sink: Arc<dyn TenantPolySink>,
}

impl TenantPolyExecutor {
    /// Build a tenant polymarket engine over the EXISTING `PolyBot`.
    ///
    /// The engine's internal journal is in-memory (its default); all
    /// durable writes for this tenant go through the attached sink.
    /// `execution` must be a Polymarket-module context.
    pub async fn new(state: Shared, execution: TenantExecutionContext) -> PolyResult<Self> {
        let context = PolyTenantContext::adapt(execution)
            .map_err(|e: PolyContextError| crate::error::PolyError::invalid(e.to_string()))?;
        let bot = PolyBot::new(state).await?;
        let tenant_state = TenantPolyState::new(
            context.organization_id().to_string(),
            context.runtime_id().to_string(),
            context.mode(),
        );
        Ok(TenantPolyExecutor {
            bot,
            context,
            state: Mutex::new(tenant_state),
            sink: Arc::new(NullPolySink),
        })
    }

    /// Attach the tenant-scoped persistence sink.
    #[must_use]
    pub fn with_poly_sink(mut self, sink: Arc<dyn TenantPolySink>) -> Self {
        self.sink = sink;
        self
    }

    /// The tenant's local state, locked. Lock poisoning is a real
    /// failure (reported as `PersistFailed`, never silently ignored).
    fn locked_state(&self) -> Result<MutexGuard<'_, TenantPolyState>, PolyExecutorError> {
        self.state
            .lock()
            .map_err(|_| PolyExecutorError::PersistFailed("tenant state lock poisoned".into()))
    }

    /// The bound tenant context.
    pub fn tenant_context(&self) -> &PolyTenantContext {
        &self.context
    }

    /// The underlying (tenant-bound) engine.
    pub fn bot(&self) -> &PolyBot {
        &self.bot
    }

    /// The engine's async pending registry (acceptances still owed
    /// settlement facts).
    pub fn async_pending(&self) -> &Arc<AsyncPendingRegistry> {
        self.bot.async_pending()
    }

    /// Point-in-time tenant counters. `None` only if the state lock
    /// is poisoned (a real failure, surfaced rather than zeroed).
    pub fn counters(&self) -> Option<TenantPolyCounters> {
        self.state.lock().ok().map(|s| s.counters())
    }

    /// Authorize an incoming execution context against this
    /// executor's bound identity. EVERY scoped entry point calls this
    /// first; a deny means the pipeline never runs.
    pub fn authorize(&self, presented: &TenantExecutionContext) -> Result<(), PolyTenantDeny> {
        if presented.organization_id() != self.context.organization_id() {
            return Err(PolyTenantDeny::OrganizationMismatch);
        }
        if presented.runtime_id() != self.context.runtime_id() {
            return Err(PolyTenantDeny::RuntimeMismatch);
        }
        if presented.generation() != self.context.generation() {
            return Err(PolyTenantDeny::GenerationMismatch);
        }
        if presented.scope().module() != bot_core::tenant::ModuleKind::Polymarket {
            return Err(PolyTenantDeny::WrongModule);
        }
        if presented.wallet().address() != self.context.wallet_address() {
            return Err(PolyTenantDeny::WalletMismatch);
        }
        Ok(())
    }

    /// Re-verify against a freshly-resolved runtime identity (fence
    /// check before money moves).
    pub fn verify_runtime(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), PolyTenantDeny> {
        self.context
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| PolyTenantDeny::GenerationMismatch)
    }

    /// The tenant-boundary gate every signal passes BEFORE the
    /// pipeline: mode policy and venue-wallet binding.
    async fn gate_signal(&self, signal: &OrderSignal) -> Result<(), PolyTenantDeny> {
        // A paper context never submits live orders — refused, never
        // downgraded.
        if signal.mode == ExecutionMode::Live && !self.context.is_live() {
            return Err(PolyTenantDeny::LiveRefusedUnderPaper);
        }
        // The venue wallet must be a usable EVM address for this
        // module, whatever the mode.
        self.context
            .validate_venue_wallet()
            .map_err(|_| PolyTenantDeny::InvalidVenueWallet)?;
        // For live orders the funds that move are the engine signer's
        // (or its configured funder's): that address MUST be the
        // wallet bound in the context — the same discipline the
        // sniper's broadcast guard applies on Solana.
        if signal.mode == ExecutionMode::Live {
            let Some(signer) = self.bot.signer_address() else {
                return Err(PolyTenantDeny::NoEngineSigner);
            };
            let maker = self
                .bot
                .configured_funder()
                .await
                .unwrap_or_else(|| signer.to_string());
            if !self.context.wallet_is(&maker) {
                return Err(PolyTenantDeny::WalletMismatch);
            }
        }
        Ok(())
    }

    /// Consider one order signal for THIS tenant: tenant dedup, the
    /// boundary gate, then the EXISTING pipeline (gates → risk → OMS
    /// idempotency → signing → posting → tracking). The outcome is
    /// persisted tenant-scoped through the sink.
    pub async fn process_signal(
        &self,
        signal: &OrderSignal,
        market: &PolyMarket,
        quotes: &HashMap<String, Quote>,
        poly: &PolymarketConfig,
    ) -> Result<SignalOutcome, PolyExecutorError> {
        {
            let state = self.locked_state()?;
            state.note_considered();
        }
        // Tenant-local dedup on the signal's intent key.
        let dedup_key = self.context.dedup_key(&signal.intent_key());
        {
            let mut state = self.locked_state()?;
            if !state.mark_seen(&dedup_key) {
                state.note_duplicate();
                return Ok(duplicate_outcome(signal));
            }
        }
        // Boundary gate (mode + wallet) — a deny never reaches the
        // pipeline.
        if let Err(deny) = self.gate_signal(signal).await {
            if let Ok(state) = self.state.lock() {
                state.note_denied();
            }
            return Err(PolyExecutorError::Denied(deny));
        }
        // The existing engine pipeline.
        let outcome = self.bot.process_signal(signal, market, quotes, poly).await;
        if outcome.stage == PolyStage::Rejected || outcome.stage == PolyStage::Failed {
            if let Ok(state) = self.state.lock() {
                state.note_rejected();
            }
        }
        // Tenant-scoped persistence: the signal outcome always; the
        // venue order row when one exists. Sink failures propagate
        // as `PersistFailed` — a persistence failure is NEVER
        // swallowed and NEVER mislabelled as a boundary deny (the
        // venue outcome may exist; the caller treats this as
        // ambiguous, not rejected).
        if let Err(e) = self
            .sink
            .record_signal(signal, &outcome, self.context.organization_id())
            .await
        {
            return Err(PolyExecutorError::PersistFailed(e.to_string()));
        }
        if outcome.venue_order_id.is_some() {
            for tracked in self.bot.tracked_orders().await {
                if Some(tracked.venue_order_id.as_str()) == outcome.venue_order_id.as_deref() {
                    if let Err(e) = self
                        .sink
                        .upsert_order(&tracked, self.context.organization_id())
                        .await
                    {
                        return Err(PolyExecutorError::PersistFailed(e.to_string()));
                    }
                    break;
                }
            }
        }
        Ok(outcome)
    }

    /// One async backfill pass over the pending registry: for each
    /// acceptance still owed settlement facts, poll the venue and
    /// persist every newly confirmed fill through the sink. Returns
    /// the verdicts in registry order.
    pub async fn backfill_pending(
        &self,
        timeout: Duration,
        interval: Duration,
    ) -> PolyResult<Vec<(String, BackfillVerdict)>> {
        let client = self.bot.authed_clob_client().await?;
        let mut out = Vec::new();
        for pending in self.async_pending().pending() {
            let verdict = backfill_order(&client, &pending, timeout, interval).await?;
            match &verdict {
                BackfillVerdict::Settled { fills, .. } => {
                    for fill in fills {
                        self.sink
                            .record_fill(fill, self.context.organization_id())
                            .await
                            .map_err(|e| PolyError::invalid(e.to_string()))?;
                    }
                    self.async_pending().remove(&pending.order_id);
                }
                BackfillVerdict::OrderGoneOnVenue => {
                    self.async_pending().remove(&pending.order_id);
                }
                BackfillVerdict::StillPending { .. } => {}
            }
            out.push((pending.order_id.clone(), verdict));
        }
        Ok(out)
    }

    /// One reconciliation pass over the async registry (no waiting):
    /// findings are journaled through the engine's store exactly like
    /// the base reconciler (see [`crate::reconcile_async`]).
    pub async fn reconcile_async(
        &self,
        replica_id: &str,
    ) -> PolyResult<Vec<crate::reconcile_async::AsyncReconFinding>> {
        let client = self.bot.authed_clob_client().await?;
        let store = self.bot.journal_store();
        crate::reconcile_async::reconcile_async_commitments(
            &client,
            self.async_pending(),
            store.as_ref(),
            replica_id,
            chrono::Utc::now(),
        )
        .await
    }

    /// A single venue poll for one pending order (exposed for
    /// operators/tests; `backfill_pending` covers the engine loop).
    pub async fn poll_pending_once(&self, order_id: &str) -> PolyResult<Option<BackfillVerdict>> {
        let client = self.bot.authed_clob_client().await?;
        let pending = self
            .async_pending()
            .pending()
            .into_iter()
            .find(|p| p.order_id == order_id);
        match pending {
            Some(p) => Ok(Some(poll_once(&client, &p).await?)),
            None => Ok(None),
        }
    }

    /// Record a raw acceptance into the async registry (used by
    /// callers that posted orders through other paths).
    pub fn record_async_acceptance(&self, acceptance: &AsyncOrderAcceptance) -> PolyResult<bool> {
        self.async_pending().record(
            acceptance,
            chrono::Utc::now(),
            chrono::Duration::seconds(ASYNC_STALE_AFTER_SECS),
        )
    }
}

/// Build the manual outcome for a tenant-deduplicated repeat.
fn duplicate_outcome(signal: &OrderSignal) -> SignalOutcome {
    SignalOutcome {
        signal_id: signal.signal_id.clone(),
        stage: PolyStage::Idempotent,
        reject_reason: Some(RejectReason::DuplicateIntent),
        detail: "tenant-local duplicate intent: already processed by this tenant".to_string(),
        order_id: None,
        venue_order_id: None,
        position_id: None,
        approved_stake: None,
        size_tokens: None,
        total_ms: 0,
    }
}
