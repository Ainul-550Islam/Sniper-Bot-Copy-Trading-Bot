//! Execution lifecycle (TASK 1 — execution reliability layer).
//!
//! One deterministic, venue-agnostic state machine for every money-moving
//! transaction attempt, shared by the Solana executor (`solana_kit::execute`),
//! the persistence layer, the reconciliation truth sources and the audit
//! trail:
//!
//! ```text
//! Created → Validated → Submitted → Pending → Confirmed ─┐
//!    │          │           │          │                  ├─→ Reconciled
//!    └──────────┴───────────┴──────────┴─→ Failed/Expired ┘
//! ```
//!
//! * **Created** — the intent exists (deterministic [`intent_id`]); nothing
//!   has been built yet.
//! * **Validated** — the transaction was built, signed and (when enabled)
//!   simulated successfully. Still nothing left the process.
//! * **Submitted** — the signed bytes are about to leave / have left the
//!   process. This transition is persisted BEFORE the broadcast call (the
//!   same write-ahead discipline as the intent journal in `recovery.rs`).
//! * **Pending** — the network accepted the broadcast (or the transport
//!   failed ambiguously) and confirmation is being tracked.
//! * **Confirmed** — landed without a program error (our own observation).
//! * **Failed** — provably did not or will not succeed (simulation reject,
//!   definite node rejection, landed-with-error, policy veto).
//! * **Expired** — the blockhash's `last_valid_block_height` passed without
//!   the transaction landing: it can never land, so a rebuild is safe.
//! * **Reconciled** — the reconciliation worker verified the outcome against
//!   external truth (`server/src/recon.rs`) — the only state that closes the
//!   loop between what we *believe* and what the chain *shows*.
//!
//! This module reuses the existing systems instead of duplicating them:
//! [`ExecutionState::to_order_status`] maps onto the OMS ledger
//! ([`crate::oms::OrderStatus`]); the durable rows live next to the
//! `transactions` / `execution_intents` tables (`db::execution`); every
//! transition is metered through [`crate::obs::metrics`] and fanned out to
//! [`ExecutionSink`]s (the server attaches the database + audit-trail sink).
//!
//! The [`ExecutionLedger`] is also the process-wide **duplicate submission
//! guard**: a second `begin` for an intent that is live, confirmed or
//! reconciled-as-landed is refused, while an intent whose previous attempt
//! provably never landed (`Failed`/`Expired`) may be re-armed for a retry
//! with a fresh blockhash. Cross-replica protection stays with the ownership
//! claims (`ownership.rs`) — this guard closes the in-process gap.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::auth::sha256_hex;
use crate::error::{BotError, BotResult};
use crate::obs::metrics;
use crate::oms::OrderStatus;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Lifecycle states of one execution intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    #[default]
    Created,
    Validated,
    Submitted,
    Pending,
    Confirmed,
    Failed,
    Expired,
    Reconciled,
}

impl ExecutionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionState::Created => "created",
            ExecutionState::Validated => "validated",
            ExecutionState::Submitted => "submitted",
            ExecutionState::Pending => "pending",
            ExecutionState::Confirmed => "confirmed",
            ExecutionState::Failed => "failed",
            ExecutionState::Expired => "expired",
            ExecutionState::Reconciled => "reconciled",
        }
    }

    pub fn parse(s: &str) -> Option<ExecutionState> {
        Some(match s.trim() {
            "created" => ExecutionState::Created,
            "validated" => ExecutionState::Validated,
            "submitted" => ExecutionState::Submitted,
            "pending" => ExecutionState::Pending,
            "confirmed" => ExecutionState::Confirmed,
            "failed" => ExecutionState::Failed,
            "expired" => ExecutionState::Expired,
            "reconciled" => ExecutionState::Reconciled,
            _ => return None,
        })
    }

    /// All states, in lifecycle order (metrics / dashboards).
    pub const ALL: [ExecutionState; 8] = [
        ExecutionState::Created,
        ExecutionState::Validated,
        ExecutionState::Submitted,
        ExecutionState::Pending,
        ExecutionState::Confirmed,
        ExecutionState::Failed,
        ExecutionState::Expired,
        ExecutionState::Reconciled,
    ];

    /// Nothing may leave a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, ExecutionState::Reconciled)
    }

    /// No further money movement can originate from this state (the attempt
    /// has settled one way or the other, even if reconciliation is pending).
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            ExecutionState::Confirmed
                | ExecutionState::Failed
                | ExecutionState::Expired
                | ExecutionState::Reconciled
        )
    }

    /// The transaction MAY have reached the network and its outcome is not
    /// yet proven by external state — reconciliation owns it.
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, ExecutionState::Submitted | ExecutionState::Pending)
    }

    /// True while the attempt is in flight (a second attempt for the same
    /// intent must be refused).
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            ExecutionState::Created
                | ExecutionState::Validated
                | ExecutionState::Submitted
                | ExecutionState::Pending
        )
    }

    /// The legal state machine. Forward-only; `Failed`/`Expired` are reachable
    /// from every pre-settled state (build errors, vetoes, node rejections,
    /// blockhash expiry); `Reconciled` is reachable from every state that
    /// could have touched the network (`Pending`, `Confirmed`, `Failed`,
    /// `Expired`) so the reconciler can close a pending attempt directly.
    pub fn can_transition_to(&self, next: ExecutionState) -> bool {
        use ExecutionState::*;
        matches!(
            (self, next),
            (Created, Validated | Failed | Expired)
                | (Validated, Submitted | Confirmed | Failed | Expired)
                | (
                    Submitted,
                    Pending | Confirmed | Failed | Expired | Reconciled
                )
                | (Pending, Confirmed | Failed | Expired | Reconciled)
                | (Confirmed | Failed | Expired, Reconciled)
        )
    }

    /// Projection onto the OMS order ledger so orders and executions never
    /// disagree about what a state means.
    pub fn to_order_status(&self) -> OrderStatus {
        match self {
            ExecutionState::Created => OrderStatus::Created,
            ExecutionState::Validated => OrderStatus::Validated,
            ExecutionState::Submitted => OrderStatus::Submitted,
            ExecutionState::Pending => OrderStatus::Accepted,
            ExecutionState::Confirmed => OrderStatus::Filled,
            ExecutionState::Failed => OrderStatus::Failed,
            ExecutionState::Expired => OrderStatus::Expired,
            ExecutionState::Reconciled => OrderStatus::Reconciled,
        }
    }
}

impl std::fmt::Display for ExecutionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Failure classification
// ---------------------------------------------------------------------------

/// Why an attempt did not (yet) succeed. Low-cardinality by design: these
/// are metric labels and audit fields, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// `simulateTransaction` reported a program/runtime error.
    SimulationRejected,
    /// The blockhash expired before the transaction landed (definite: a
    /// rebuild with a fresh blockhash is safe).
    BlockhashExpired,
    /// The node answered with a definite rejection (sanitize error, invalid
    /// transaction, preflight failure…).
    Rejected,
    /// The payer cannot cover the transaction (definite; not retryable).
    InsufficientFunds,
    /// HTTP 429 / provider quota exhausted.
    RateLimited,
    /// No answer from the network (timeout / connection loss): the
    /// transaction MAY still land — never a definite failure.
    TransportAmbiguous,
    /// Broadcast accepted but confirmation was not observed within the
    /// budget and the blockhash is still valid — reconciliation owns it.
    ConfirmationTimeout,
    /// The transaction landed but the program returned an error.
    LandedFailed,
    /// Refused by local policy (fee bounds, emergency limits, minimum fee).
    PolicyVeto,
    /// A second attempt for an intent that is live or already landed.
    Duplicate,
    /// Build/sign/serialize error or another internal fault.
    Internal,
}

impl FailureClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureClass::SimulationRejected => "simulation_rejected",
            FailureClass::BlockhashExpired => "blockhash_expired",
            FailureClass::Rejected => "rejected",
            FailureClass::InsufficientFunds => "insufficient_funds",
            FailureClass::RateLimited => "rate_limited",
            FailureClass::TransportAmbiguous => "transport_ambiguous",
            FailureClass::ConfirmationTimeout => "confirmation_timeout",
            FailureClass::LandedFailed => "landed_failed",
            FailureClass::PolicyVeto => "policy_veto",
            FailureClass::Duplicate => "duplicate",
            FailureClass::Internal => "internal",
        }
    }

    pub fn parse(s: &str) -> Option<FailureClass> {
        Some(match s.trim() {
            "simulation_rejected" => FailureClass::SimulationRejected,
            "blockhash_expired" => FailureClass::BlockhashExpired,
            "rejected" => FailureClass::Rejected,
            "insufficient_funds" => FailureClass::InsufficientFunds,
            "rate_limited" => FailureClass::RateLimited,
            "transport_ambiguous" => FailureClass::TransportAmbiguous,
            "confirmation_timeout" => FailureClass::ConfirmationTimeout,
            "landed_failed" => FailureClass::LandedFailed,
            "policy_veto" => FailureClass::PolicyVeto,
            "duplicate" => FailureClass::Duplicate,
            "internal" => FailureClass::Internal,
            _ => return None,
        })
    }

    /// The outcome is NOT proven: the signed transaction may still land.
    /// Such attempts must be handed to reconciliation, never blind-retried.
    pub fn is_ambiguous(&self) -> bool {
        matches!(
            self,
            FailureClass::TransportAmbiguous | FailureClass::ConfirmationTimeout
        )
    }

    /// A rebuild with a fresh blockhash is safe AND worthwhile: the previous
    /// transaction provably cannot land and the failure was not caused by
    /// the transaction's content.
    pub fn is_retryable_with_rebuild(&self) -> bool {
        matches!(
            self,
            FailureClass::BlockhashExpired | FailureClass::RateLimited
        )
    }

    /// The lifecycle state an attempt lands in for this failure.
    pub fn terminal_state(&self) -> ExecutionState {
        match self {
            FailureClass::BlockhashExpired => ExecutionState::Expired,
            FailureClass::TransportAmbiguous | FailureClass::ConfirmationTimeout => {
                ExecutionState::Pending
            }
            _ => ExecutionState::Failed,
        }
    }

    /// Classify a broadcast/confirmation error message. Conservative: text
    /// that is not a recognisable node rejection is `TransportAmbiguous`
    /// (a wrongly-definite verdict invites a double spend; a wrongly-
    /// ambiguous one merely delays the retry until reconciliation resolves
    /// it). Rejections win over transport words when both appear, because a
    /// rejection proves the endpoint processed (and refused) the request.
    pub fn classify_message(err: &str) -> FailureClass {
        let lower = err.to_ascii_lowercase();
        if lower.contains("blockhash") || lower.contains("block height") {
            return FailureClass::BlockhashExpired;
        }
        if lower.contains("insufficient funds")
            || lower.contains("insufficient lamports")
            || lower.contains("insufficientfunds")
        {
            return FailureClass::InsufficientFunds;
        }
        if lower.contains("too many requests")
            || lower.contains("toomanyrequests")
            || lower.contains("rate limit")
            || lower.contains("429")
        {
            return FailureClass::RateLimited;
        }
        if lower.contains("simulation") || lower.contains("preflight") {
            return FailureClass::SimulationRejected;
        }
        const DEFINITE: &[&str] = &[
            "invalid",
            "sanitize",
            "rejected",
            "not available",
            "signature verification",
            "already processed",
            "account not found",
            "program failed",
            "custom program error",
        ];
        if DEFINITE.iter().any(|m| lower.contains(m)) {
            return FailureClass::Rejected;
        }
        FailureClass::TransportAmbiguous
    }
}

impl std::fmt::Display for FailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Deterministic intent identity
// ---------------------------------------------------------------------------

/// Version tag baked into every intent id so a future change of the hashed
/// fields can never collide with ids persisted by an older build.
pub const INTENT_ID_VERSION: &str = "v1";

/// Deterministic intent id: `int_` + 32 hex chars of
/// `sha256("v1|" + parts.join("|"))`. Same parts → same id, on every replica,
/// across restarts. Callers pass the LOGICAL identity of the transaction
/// (wallet, label/execution id, instruction digest…), never anything that
/// changes between retries (blockhash, timestamps, attempt counters).
pub fn intent_id<S: AsRef<str>>(parts: &[S]) -> String {
    let mut canonical = String::with_capacity(64);
    canonical.push_str(INTENT_ID_VERSION);
    for p in parts {
        canonical.push('|');
        canonical.push_str(p.as_ref());
    }
    let digest = sha256_hex(&canonical);
    format!("int_{}", &digest[..32])
}

/// Deterministic digest of arbitrary bytes (instruction data, account lists)
/// for use as an [`intent_id`] part.
pub fn digest_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// What the caller knows when it asks the ledger to start an attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionIntent {
    pub intent_id: String,
    /// Producing module (`sniper`, `copy`, `solana`, …) — low cardinality.
    pub module: String,
    /// Human label (the `TxRequest` label); never secrets.
    pub label: String,
    /// Fee-payer public key (attribution for reconciliation and operators).
    pub wallet: String,
    /// Optional market/symbol for dashboards (mint, pair, …).
    #[serde(default)]
    pub symbol: String,
}

/// Durable + in-memory record of one execution intent and its latest state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub intent_id: String,
    pub module: String,
    pub label: String,
    pub wallet: String,
    #[serde(default)]
    pub symbol: String,
    pub state: ExecutionState,
    /// Broadcast attempts started for this intent (1 on the first `begin`).
    pub attempts: u32,
    /// Signature of the CURRENT attempt's signed transaction (known before
    /// broadcast — the message is signed locally).
    pub signature: Option<String>,
    pub blockhash: Option<String>,
    pub last_valid_block_height: Option<u64>,
    pub priority_fee_micro_lamports: u64,
    pub failure: Option<FailureClass>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When the current state was entered (for state-duration metrics).
    #[serde(skip, default = "Instant::now")]
    pub entered_at: Instant,
}

impl ExecutionRecord {
    fn new(intent: ExecutionIntent) -> Self {
        let now = Utc::now();
        ExecutionRecord {
            intent_id: intent.intent_id,
            module: intent.module,
            label: intent.label,
            wallet: intent.wallet,
            symbol: intent.symbol,
            state: ExecutionState::Created,
            attempts: 1,
            signature: None,
            blockhash: None,
            last_valid_block_height: None,
            priority_fee_micro_lamports: 0,
            failure: None,
            error: None,
            created_at: now,
            updated_at: now,
            entered_at: Instant::now(),
        }
    }

    /// One-line summary for logs (no secrets by construction).
    pub fn summary(&self) -> String {
        format!(
            "{} {} {} attempt={} state={}{}{}",
            self.intent_id,
            self.module,
            self.label,
            self.attempts,
            self.state,
            self.signature
                .as_deref()
                .map(|s| format!(" sig={s}"))
                .unwrap_or_default(),
            self.failure
                .map(|f| format!(" failure={f}"))
                .unwrap_or_default()
        )
    }

    /// A previous attempt that provably never landed may be re-armed.
    pub fn can_rearm(&self) -> bool {
        match self.state {
            ExecutionState::Failed | ExecutionState::Expired => true,
            // Reconciled against the chain as NOT landed (failure recorded).
            ExecutionState::Reconciled => self.failure.is_some(),
            _ => false,
        }
    }
}

/// One state transition, as delivered to sinks / persisted as an event row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTransition {
    pub intent_id: String,
    pub from: Option<ExecutionState>,
    pub to: ExecutionState,
    pub attempt: u32,
    pub signature: Option<String>,
    pub failure: Option<FailureClass>,
    pub reason: Option<String>,
    pub ts: DateTime<Utc>,
}

/// Outcome of [`ExecutionLedger::begin`].
#[derive(Debug, Clone)]
pub enum BeginOutcome {
    /// First attempt for this intent.
    Fresh(ExecutionRecord),
    /// A previous attempt provably never landed; this is attempt `n`.
    Rearmed(ExecutionRecord),
    /// The intent is live, confirmed or reconciled-as-landed: refuse.
    Duplicate(ExecutionRecord),
}

impl BeginOutcome {
    pub fn record(&self) -> &ExecutionRecord {
        match self {
            BeginOutcome::Fresh(r) | BeginOutcome::Rearmed(r) | BeginOutcome::Duplicate(r) => r,
        }
    }

    pub fn is_duplicate(&self) -> bool {
        matches!(self, BeginOutcome::Duplicate(_))
    }
}

// ---------------------------------------------------------------------------
// Sinks
// ---------------------------------------------------------------------------

/// Receives every transition (durable persistence, audit trail, event bus).
/// Implementations must never fail the execution: they log/meter their own
/// errors. They are awaited INLINE for `Submitted` (write-ahead semantics)
/// and for every other state as well — keep them fast (one DB round trip).
#[async_trait]
pub trait ExecutionSink: Send + Sync {
    async fn on_transition(&self, record: &ExecutionRecord, transition: &ExecutionTransition);
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// Default bound for the in-memory mirror (oldest settled records are evicted
/// first; live records are never evicted).
pub const DEFAULT_LEDGER_CAP: usize = 10_000;

/// In-memory execution ledger + duplicate guard + sink fan-out.
pub struct ExecutionLedger {
    records: RwLock<HashMap<String, ExecutionRecord>>,
    by_signature: RwLock<HashMap<String, String>>,
    fifo: RwLock<VecDeque<String>>,
    sinks: RwLock<Vec<Arc<dyn ExecutionSink>>>,
    cap: usize,
}

static GLOBAL_LEDGER: OnceLock<Arc<ExecutionLedger>> = OnceLock::new();

/// The process-wide ledger. Duplicate protection is only meaningful when
/// every executor in the process shares one guard, so this is the default
/// the Solana executor uses; tests and embedded users may build private
/// ledgers with [`ExecutionLedger::new`].
pub fn ledger() -> Arc<ExecutionLedger> {
    Arc::clone(GLOBAL_LEDGER.get_or_init(|| ExecutionLedger::new(DEFAULT_LEDGER_CAP)))
}

impl ExecutionLedger {
    pub fn new(cap: usize) -> Arc<Self> {
        Arc::new(ExecutionLedger {
            records: RwLock::new(HashMap::new()),
            by_signature: RwLock::new(HashMap::new()),
            fifo: RwLock::new(VecDeque::new()),
            sinks: RwLock::new(Vec::new()),
            cap: cap.max(64),
        })
    }

    /// Attach a sink (server: database + audit). Idempotent per instance is
    /// the caller's responsibility; attaching twice fans out twice.
    pub async fn attach_sink(&self, sink: Arc<dyn ExecutionSink>) {
        self.sinks.write().await.push(sink);
    }

    pub async fn sink_count(&self) -> usize {
        self.sinks.read().await.len()
    }

    /// Start (or re-arm) an attempt for `intent`. Never persists on
    /// `Duplicate`; a `Fresh`/`Rearmed` outcome has already been announced
    /// to the sinks as a `Created` transition.
    pub async fn begin(&self, intent: ExecutionIntent) -> BotResult<BeginOutcome> {
        if intent.intent_id.trim().is_empty() {
            return Err(BotError::invalid("execution intent_id must not be empty"));
        }
        let (record, transition, outcome_kind) = {
            let mut records = self.records.write().await;
            match records.get_mut(&intent.intent_id) {
                Some(existing) if existing.can_rearm() => {
                    let from = existing.state;
                    existing.attempts = existing.attempts.saturating_add(1);
                    existing.state = ExecutionState::Created;
                    existing.failure = None;
                    existing.error = None;
                    existing.signature = None;
                    existing.blockhash = None;
                    existing.last_valid_block_height = None;
                    existing.updated_at = Utc::now();
                    existing.entered_at = Instant::now();
                    let snapshot = existing.clone();
                    let t = ExecutionTransition {
                        intent_id: snapshot.intent_id.clone(),
                        from: Some(from),
                        to: ExecutionState::Created,
                        attempt: snapshot.attempts,
                        signature: None,
                        failure: None,
                        reason: Some(format!("re-armed after {}", from)),
                        ts: snapshot.updated_at,
                    };
                    (snapshot, t, 1u8)
                }
                Some(existing) => {
                    metrics::global()
                        .counter(
                            "bot_duplicate_execution_prevented_total",
                            "Duplicate logical executions prevented by idempotency layers.",
                            &[("where", "execution_ledger")],
                        )
                        .inc();
                    warn!(
                        intent = %existing.intent_id,
                        state = %existing.state,
                        "duplicate execution attempt refused by the execution ledger"
                    );
                    return Ok(BeginOutcome::Duplicate(existing.clone()));
                }
                None => {
                    let record = ExecutionRecord::new(intent);
                    let t = ExecutionTransition {
                        intent_id: record.intent_id.clone(),
                        from: None,
                        to: ExecutionState::Created,
                        attempt: 1,
                        signature: None,
                        failure: None,
                        reason: None,
                        ts: record.created_at,
                    };
                    let id = record.intent_id.clone();
                    records.insert(id.clone(), record.clone());
                    self.fifo.write().await.push_back(id);
                    (record, t, 0u8)
                }
            }
        };
        self.evict_if_needed().await;
        meter_transition(&transition);
        self.notify(&record, &transition).await;
        debug!(record = %record.summary(), "execution attempt started");
        Ok(if outcome_kind == 0 {
            BeginOutcome::Fresh(record)
        } else {
            BeginOutcome::Rearmed(record)
        })
    }

    /// Record the signed transaction's identity for the current attempt
    /// (before `Submitted`). Signature lookups (`get_by_signature`) become
    /// possible from here on.
    pub async fn attach_submission(
        &self,
        intent_id: &str,
        signature: &str,
        blockhash: Option<String>,
        last_valid_block_height: Option<u64>,
        priority_fee_micro_lamports: u64,
    ) -> BotResult<ExecutionRecord> {
        let snapshot = {
            let mut records = self.records.write().await;
            let rec = records
                .get_mut(intent_id)
                .ok_or_else(|| BotError::NotFound(format!("execution intent {intent_id}")))?;
            rec.signature = Some(signature.to_string());
            rec.blockhash = blockhash;
            rec.last_valid_block_height = last_valid_block_height;
            rec.priority_fee_micro_lamports = priority_fee_micro_lamports;
            rec.updated_at = Utc::now();
            rec.clone()
        };
        if !signature.is_empty() {
            self.by_signature
                .write()
                .await
                .insert(signature.to_string(), intent_id.to_string());
        }
        Ok(snapshot)
    }

    /// Record the priority fee chosen for the current attempt (adaptive fee
    /// selection happens before the transaction is signed).
    pub async fn set_priority_fee(&self, intent_id: &str, micro_lamports: u64) {
        if let Some(rec) = self.records.write().await.get_mut(intent_id) {
            rec.priority_fee_micro_lamports = micro_lamports;
        }
    }

    /// Validate + apply a transition. Illegal moves are errors (programming
    /// bugs must be loud); sinks are notified after the in-memory update.
    pub async fn transition(
        &self,
        intent_id: &str,
        to: ExecutionState,
        reason: Option<&str>,
    ) -> BotResult<ExecutionRecord> {
        self.apply(intent_id, to, None, reason).await
    }

    /// Record a failure: sets the class + error and moves to the class's
    /// terminal state (`Failed`, `Expired`, or `Pending` for ambiguous
    /// classes — ambiguous outcomes never terminate an attempt locally).
    pub async fn fail(
        &self,
        intent_id: &str,
        class: FailureClass,
        error: &str,
    ) -> BotResult<ExecutionRecord> {
        metrics::global()
            .counter(
                "bot_execution_failures_total",
                "Execution attempts that failed, by failure class.",
                &[("class", class.as_str())],
            )
            .inc();
        let target = class.terminal_state();
        // Already in the target state (e.g. Pending → ambiguous send): just
        // annotate the record without a transition.
        let current = self.get(intent_id).await.map(|r| r.state);
        if current == Some(target) {
            let mut records = self.records.write().await;
            if let Some(rec) = records.get_mut(intent_id) {
                rec.failure = Some(class);
                rec.error = Some(error.to_string());
                rec.updated_at = Utc::now();
                return Ok(rec.clone());
            }
            return Err(BotError::NotFound(format!("execution intent {intent_id}")));
        }
        self.apply(intent_id, target, Some((class, error)), Some(error))
            .await
    }

    /// Reconciliation verdict for a signature (called by the truth source).
    /// `landed` = the chain shows the transaction succeeded. Returns `None`
    /// when the signature is not known to this process.
    pub async fn reconcile_signature(
        &self,
        signature: &str,
        landed: bool,
        detail: &str,
    ) -> Option<ExecutionRecord> {
        let intent_id = self.by_signature.read().await.get(signature).cloned()?;
        let current = self.get(&intent_id).await?;
        if current.state.is_terminal() {
            return Some(current);
        }
        let failure = if landed {
            None
        } else {
            Some((FailureClass::LandedFailed, detail))
        };
        match self
            .apply(
                &intent_id,
                ExecutionState::Reconciled,
                failure,
                Some(detail),
            )
            .await
        {
            Ok(rec) => Some(rec),
            Err(e) => {
                // Validated/Created records have no signature and cannot be
                // reached here; anything else is a state-machine bug.
                warn!(%intent_id, error = %e, "reconcile transition rejected");
                None
            }
        }
    }

    async fn apply(
        &self,
        intent_id: &str,
        to: ExecutionState,
        failure: Option<(FailureClass, &str)>,
        reason: Option<&str>,
    ) -> BotResult<ExecutionRecord> {
        let (snapshot, transition, dwell_ms, from) = {
            let mut records = self.records.write().await;
            let rec = records
                .get_mut(intent_id)
                .ok_or_else(|| BotError::NotFound(format!("execution intent {intent_id}")))?;
            if !rec.state.can_transition_to(to) {
                return Err(BotError::invalid(format!(
                    "illegal execution transition {} -> {} for {intent_id}",
                    rec.state, to
                )));
            }
            let from = rec.state;
            let dwell_ms = rec.entered_at.elapsed().as_millis() as u64;
            let now = Utc::now();
            rec.state = to;
            rec.updated_at = now;
            rec.entered_at = Instant::now();
            if let Some((class, err)) = failure {
                rec.failure = Some(class);
                rec.error = Some(err.to_string());
            } else if matches!(to, ExecutionState::Confirmed | ExecutionState::Reconciled) {
                // Landed: a landed transaction must never look re-armable.
                rec.failure = None;
                rec.error = None;
            }
            let t = ExecutionTransition {
                intent_id: rec.intent_id.clone(),
                from: Some(from),
                to,
                attempt: rec.attempts,
                signature: rec.signature.clone(),
                failure: rec.failure,
                reason: reason.map(str::to_string),
                ts: now,
            };
            (rec.clone(), t, dwell_ms, from)
        };
        metrics::global()
            .histogram(
                "bot_execution_state_duration_ms",
                "Time spent in each execution lifecycle state before leaving it, in milliseconds.",
                &[("state", from.as_str())],
                metrics::LATENCY_BUCKETS_MS,
            )
            .observe(dwell_ms);
        meter_transition(&transition);
        self.notify(&snapshot, &transition).await;
        if to.is_settled() {
            info!(record = %snapshot.summary(), from = %from, "execution settled");
        } else {
            debug!(record = %snapshot.summary(), from = %from, "execution transition");
        }
        Ok(snapshot)
    }

    async fn notify(&self, record: &ExecutionRecord, transition: &ExecutionTransition) {
        let sinks = self.sinks.read().await.clone();
        for sink in sinks {
            sink.on_transition(record, transition).await;
        }
        self.update_active_gauge().await;
    }

    async fn update_active_gauge(&self) {
        let live = self
            .records
            .read()
            .await
            .values()
            .filter(|r| r.state.is_live())
            .count();
        metrics::global()
            .gauge(
                "bot_execution_active",
                "Execution attempts currently in flight (created/validated/submitted/pending).",
                &[],
            )
            .set(live as i64);
    }

    async fn evict_if_needed(&self) {
        let mut fifo = self.fifo.write().await;
        if fifo.len() <= self.cap {
            return;
        }
        let mut records = self.records.write().await;
        let mut by_sig = self.by_signature.write().await;
        while fifo.len() > self.cap {
            let Some(pos) = fifo.iter().position(|id| {
                records
                    .get(id.as_str())
                    .map(|r| r.state.is_settled())
                    .unwrap_or(true)
            }) else {
                break; // everything is live: correctness beats the bound
            };
            let evicted = fifo.remove(pos).expect("pos came from the same deque");
            if let Some(rec) = records.remove(&evicted) {
                if let Some(sig) = rec.signature {
                    by_sig.remove(&sig);
                }
            }
        }
    }

    // ------------------------------------------------------------ reads --

    pub async fn get(&self, intent_id: &str) -> Option<ExecutionRecord> {
        self.records.read().await.get(intent_id).cloned()
    }

    pub async fn get_by_signature(&self, signature: &str) -> Option<ExecutionRecord> {
        let id = self.by_signature.read().await.get(signature).cloned()?;
        self.get(&id).await
    }

    /// Most recent records first.
    pub async fn list(&self, limit: usize) -> Vec<ExecutionRecord> {
        let records = self.records.read().await;
        let mut all: Vec<ExecutionRecord> = records.values().cloned().collect();
        all.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        all.truncate(limit);
        all
    }

    /// Attempts that are not settled (the crash-recovery / dashboard input).
    pub async fn open(&self) -> Vec<ExecutionRecord> {
        self.records
            .read()
            .await
            .values()
            .filter(|r| !r.state.is_settled())
            .cloned()
            .collect()
    }

    pub async fn len(&self) -> usize {
        self.records.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.records.read().await.is_empty()
    }

    /// Count per state (dashboards / health).
    pub async fn counts(&self) -> HashMap<ExecutionState, usize> {
        let mut out = HashMap::new();
        for r in self.records.read().await.values() {
            *out.entry(r.state).or_insert(0) += 1;
        }
        out
    }

    // -------------------------------------------------- crash recovery --

    /// Load records persisted by a previous process. Records are inserted
    /// as-is (no sink notification, no transition validation): the durable
    /// row is the truth. Returns the number of records loaded. Records
    /// already known in memory are left untouched.
    pub async fn hydrate(&self, rows: Vec<ExecutionRecord>) -> usize {
        let mut n = 0;
        {
            let mut records = self.records.write().await;
            let mut by_sig = self.by_signature.write().await;
            let mut fifo = self.fifo.write().await;
            for mut rec in rows {
                if records.contains_key(&rec.intent_id) {
                    continue;
                }
                rec.entered_at = Instant::now();
                if let Some(sig) = &rec.signature {
                    by_sig.insert(sig.clone(), rec.intent_id.clone());
                }
                fifo.push_back(rec.intent_id.clone());
                records.insert(rec.intent_id.clone(), rec);
                n += 1;
            }
        }
        self.evict_if_needed().await;
        self.update_active_gauge().await;
        if n > 0 {
            info!(loaded = n, "execution ledger hydrated from durable storage");
        }
        n
    }

    /// Restart policy for hydrated records (call after [`hydrate`]):
    ///
    /// * `Created`/`Validated` never produced a broadcast (the `Submitted`
    ///   transition is written BEFORE the send) → `Failed(Internal,
    ///   "process restart before submission")`. Safe: nothing left the
    ///   process.
    /// * `Submitted`/`Pending` MAY have reached the network → left as-is
    ///   (blocking duplicates) and returned so the caller can enqueue their
    ///   signatures for reconciliation.
    ///
    /// Returns the ambiguous records (those with a signature to reconcile).
    ///
    /// [`hydrate`]: ExecutionLedger::hydrate
    pub async fn resolve_after_restart(&self) -> Vec<ExecutionRecord> {
        let open = self.open().await;
        let mut ambiguous = Vec::new();
        for rec in open {
            match rec.state {
                ExecutionState::Created | ExecutionState::Validated => {
                    let _ = self
                        .fail(
                            &rec.intent_id,
                            FailureClass::Internal,
                            "process restart before submission — nothing was broadcast",
                        )
                        .await;
                }
                ExecutionState::Submitted | ExecutionState::Pending => {
                    if rec.state == ExecutionState::Submitted {
                        // The send may or may not have happened: track it
                        // exactly like an ambiguous broadcast.
                        let _ = self
                            .fail(
                                &rec.intent_id,
                                FailureClass::TransportAmbiguous,
                                "process restart during submission — outcome unknown",
                            )
                            .await;
                    }
                    ambiguous.push(rec);
                }
                _ => {}
            }
        }
        if !ambiguous.is_empty() {
            warn!(
                count = ambiguous.len(),
                "execution attempts with unknown outcome after restart — handed to reconciliation"
            );
        }
        ambiguous
    }
}

fn meter_transition(t: &ExecutionTransition) {
    metrics::global()
        .counter(
            "bot_execution_transitions_total",
            "Execution lifecycle transitions by target state.",
            &[("to", t.to.as_str())],
        )
        .inc();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn intent(id: &str) -> ExecutionIntent {
        ExecutionIntent {
            intent_id: id.into(),
            module: "test".into(),
            label: "snipe-X".into(),
            wallet: "wallet".into(),
            symbol: "X".into(),
        }
    }

    struct CountingSink {
        seen: AtomicUsize,
        last: std::sync::Mutex<Option<ExecutionState>>,
    }

    #[async_trait]
    impl ExecutionSink for CountingSink {
        async fn on_transition(&self, _r: &ExecutionRecord, t: &ExecutionTransition) {
            self.seen.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some(t.to);
        }
    }

    #[test]
    fn state_machine_is_forward_only_and_reconcilable() {
        use ExecutionState::*;
        assert!(Created.can_transition_to(Validated));
        assert!(Validated.can_transition_to(Submitted));
        assert!(Submitted.can_transition_to(Pending));
        assert!(Pending.can_transition_to(Confirmed));
        assert!(Confirmed.can_transition_to(Reconciled));
        assert!(Failed.can_transition_to(Reconciled));
        assert!(Expired.can_transition_to(Reconciled));
        assert!(Pending.can_transition_to(Reconciled));
        // Never backwards, never out of Reconciled.
        assert!(!Pending.can_transition_to(Submitted));
        assert!(!Confirmed.can_transition_to(Pending));
        assert!(!Reconciled.can_transition_to(Confirmed));
        assert!(!Created.can_transition_to(Submitted), "must validate first");
        assert!(!Created.can_transition_to(Reconciled));
        for s in ExecutionState::ALL {
            assert_eq!(ExecutionState::parse(s.as_str()), Some(s));
        }
        assert!(Reconciled.is_terminal() && !Confirmed.is_terminal());
        assert!(Confirmed.is_settled() && Failed.is_settled() && !Pending.is_settled());
        assert!(Pending.is_ambiguous() && Submitted.is_ambiguous());
    }

    #[test]
    fn order_status_projection_is_consistent_with_oms() {
        assert_eq!(
            ExecutionState::Confirmed.to_order_status(),
            OrderStatus::Filled
        );
        assert_eq!(
            ExecutionState::Expired.to_order_status(),
            OrderStatus::Expired
        );
        assert_eq!(
            ExecutionState::Reconciled.to_order_status(),
            OrderStatus::Reconciled
        );
        // Every projected OMS status must itself be reachable in the OMS
        // machine from Submitted (executions start reporting at submit).
        for s in [
            ExecutionState::Pending,
            ExecutionState::Confirmed,
            ExecutionState::Failed,
            ExecutionState::Expired,
        ] {
            assert!(
                OrderStatus::Submitted.can_transition_to(s.to_order_status()),
                "{s} projects to an OMS state unreachable from Submitted"
            );
        }
    }

    #[test]
    fn failure_classification_matrix() {
        use FailureClass::*;
        assert_eq!(
            FailureClass::classify_message(
                "Transaction precompile verification failure BlockhashNotFound"
            ),
            BlockhashExpired
        );
        assert_eq!(
            FailureClass::classify_message("HTTP 429 Too Many Requests"),
            RateLimited
        );
        assert_eq!(
            FailureClass::classify_message("insufficient funds for rent"),
            InsufficientFunds
        );
        assert_eq!(
            FailureClass::classify_message("Transaction simulation failed: custom program error"),
            SimulationRejected
        );
        assert_eq!(
            FailureClass::classify_message(
                "invalid transaction: Versioned transaction message is not sanitized"
            ),
            Rejected
        );
        assert_eq!(
            FailureClass::classify_message("error sending request: connection closed"),
            TransportAmbiguous
        );
        assert_eq!(
            FailureClass::classify_message("something nobody has seen"),
            TransportAmbiguous,
            "unknown text must stay ambiguous"
        );
        // Terminal-state mapping and retry semantics.
        assert_eq!(BlockhashExpired.terminal_state(), ExecutionState::Expired);
        assert_eq!(TransportAmbiguous.terminal_state(), ExecutionState::Pending);
        assert_eq!(Rejected.terminal_state(), ExecutionState::Failed);
        assert!(BlockhashExpired.is_retryable_with_rebuild());
        assert!(!TransportAmbiguous.is_retryable_with_rebuild());
        assert!(TransportAmbiguous.is_ambiguous() && ConfirmationTimeout.is_ambiguous());
        for c in [
            SimulationRejected,
            BlockhashExpired,
            Rejected,
            InsufficientFunds,
            RateLimited,
            TransportAmbiguous,
            ConfirmationTimeout,
            LandedFailed,
            PolicyVeto,
            Duplicate,
            Internal,
        ] {
            assert_eq!(FailureClass::parse(c.as_str()), Some(c));
        }
    }

    #[test]
    fn intent_ids_are_deterministic_and_versioned() {
        let a = intent_id(&["wallet", "snipe:mint", "abc"]);
        let b = intent_id(&["wallet", "snipe:mint", "abc"]);
        let c = intent_id(&["wallet", "snipe:mint", "abd"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("int_") && a.len() == 4 + 32);
        // Field boundaries matter: ("ab","c") != ("a","bc").
        assert_ne!(intent_id(&["ab", "c"]), intent_id(&["a", "bc"]));
        assert_eq!(digest_hex(b"x"), digest_hex(b"x"));
    }

    #[tokio::test]
    async fn happy_path_transitions_reach_sinks_and_reconcile() {
        let ledger = ExecutionLedger::new(64);
        let sink = Arc::new(CountingSink {
            seen: AtomicUsize::new(0),
            last: std::sync::Mutex::new(None),
        });
        ledger.attach_sink(sink.clone()).await;

        let out = ledger.begin(intent("i1")).await.unwrap();
        assert!(matches!(out, BeginOutcome::Fresh(_)));
        ledger
            .transition("i1", ExecutionState::Validated, None)
            .await
            .unwrap();
        ledger
            .attach_submission("i1", "SIG1", Some("bh".into()), Some(100), 5_000)
            .await
            .unwrap();
        ledger
            .transition("i1", ExecutionState::Submitted, None)
            .await
            .unwrap();
        ledger
            .transition("i1", ExecutionState::Pending, None)
            .await
            .unwrap();
        let rec = ledger
            .transition("i1", ExecutionState::Confirmed, Some("slot 5"))
            .await
            .unwrap();
        assert_eq!(rec.state, ExecutionState::Confirmed);
        assert_eq!(rec.signature.as_deref(), Some("SIG1"));
        assert_eq!(rec.priority_fee_micro_lamports, 5_000);
        assert_eq!(
            sink.seen.load(Ordering::SeqCst),
            5,
            "created + 4 transitions"
        );

        // Reconciliation closes the loop by signature.
        let rec = ledger
            .reconcile_signature("SIG1", true, "confirmed slot=5")
            .await
            .expect("signature is known");
        assert_eq!(rec.state, ExecutionState::Reconciled);
        assert!(rec.failure.is_none());
        assert_eq!(*sink.last.lock().unwrap(), Some(ExecutionState::Reconciled));
        assert!(ledger.get_by_signature("SIG1").await.is_some());
        assert!(ledger.reconcile_signature("nope", true, "").await.is_none());
    }

    #[tokio::test]
    async fn illegal_transitions_are_rejected_loudly() {
        let ledger = ExecutionLedger::new(64);
        ledger.begin(intent("i2")).await.unwrap();
        let err = ledger
            .transition("i2", ExecutionState::Pending, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("illegal execution transition"));
        assert!(ledger
            .transition("missing", ExecutionState::Validated, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn duplicate_attempts_are_refused_while_live_or_landed() {
        let ledger = ExecutionLedger::new(64);
        ledger.begin(intent("dup")).await.unwrap();
        // Live → duplicate.
        assert!(ledger.begin(intent("dup")).await.unwrap().is_duplicate());
        ledger
            .transition("dup", ExecutionState::Validated, None)
            .await
            .unwrap();
        ledger
            .attach_submission("dup", "S", None, None, 0)
            .await
            .unwrap();
        ledger
            .transition("dup", ExecutionState::Submitted, None)
            .await
            .unwrap();
        ledger
            .transition("dup", ExecutionState::Pending, None)
            .await
            .unwrap();
        assert!(
            ledger.begin(intent("dup")).await.unwrap().is_duplicate(),
            "pending (ambiguous) attempts must never be resubmitted"
        );
        ledger
            .transition("dup", ExecutionState::Confirmed, None)
            .await
            .unwrap();
        assert!(ledger.begin(intent("dup")).await.unwrap().is_duplicate());
        ledger.reconcile_signature("S", true, "ok").await.unwrap();
        assert!(
            ledger.begin(intent("dup")).await.unwrap().is_duplicate(),
            "reconciled-as-landed is final"
        );
    }

    #[tokio::test]
    async fn definite_failures_can_be_rearmed_and_count_attempts() {
        let ledger = ExecutionLedger::new(64);
        ledger.begin(intent("re")).await.unwrap();
        let rec = ledger
            .fail("re", FailureClass::BlockhashExpired, "expired")
            .await
            .unwrap();
        assert_eq!(rec.state, ExecutionState::Expired);
        assert_eq!(rec.failure, Some(FailureClass::BlockhashExpired));
        let out = ledger.begin(intent("re")).await.unwrap();
        match out {
            BeginOutcome::Rearmed(r) => {
                assert_eq!(r.attempts, 2);
                assert_eq!(r.state, ExecutionState::Created);
                assert!(r.failure.is_none() && r.signature.is_none());
            }
            other => panic!("expected Rearmed, got {other:?}"),
        }
        // Reconciled-as-failed may be re-armed too.
        ledger
            .transition("re", ExecutionState::Validated, None)
            .await
            .unwrap();
        ledger
            .attach_submission("re", "S2", None, None, 0)
            .await
            .unwrap();
        ledger
            .transition("re", ExecutionState::Submitted, None)
            .await
            .unwrap();
        ledger
            .transition("re", ExecutionState::Pending, None)
            .await
            .unwrap();
        ledger
            .reconcile_signature("S2", false, "failed on chain")
            .await
            .unwrap();
        assert!(matches!(
            ledger.begin(intent("re")).await.unwrap(),
            BeginOutcome::Rearmed(_)
        ));
    }

    #[tokio::test]
    async fn ambiguous_failures_park_in_pending_without_terminating() {
        let ledger = ExecutionLedger::new(64);
        ledger.begin(intent("amb")).await.unwrap();
        ledger
            .transition("amb", ExecutionState::Validated, None)
            .await
            .unwrap();
        ledger
            .attach_submission("amb", "S3", None, None, 0)
            .await
            .unwrap();
        ledger
            .transition("amb", ExecutionState::Submitted, None)
            .await
            .unwrap();
        let rec = ledger
            .fail("amb", FailureClass::TransportAmbiguous, "connection closed")
            .await
            .unwrap();
        assert_eq!(rec.state, ExecutionState::Pending);
        // A confirmation timeout on an already-pending attempt annotates
        // without a transition.
        let rec = ledger
            .fail("amb", FailureClass::ConfirmationTimeout, "timed out")
            .await
            .unwrap();
        assert_eq!(rec.state, ExecutionState::Pending);
        assert_eq!(rec.failure, Some(FailureClass::ConfirmationTimeout));
        assert!(ledger.begin(intent("amb")).await.unwrap().is_duplicate());
        assert_eq!(ledger.open().await.len(), 1);
    }

    #[tokio::test]
    async fn restart_resolution_fails_unsent_and_hands_off_ambiguous() {
        let ledger = ExecutionLedger::new(64);
        let mut a = ExecutionRecord::new(intent("a"));
        a.state = ExecutionState::Validated;
        let mut b = ExecutionRecord::new(intent("b"));
        b.state = ExecutionState::Submitted;
        b.signature = Some("SB".into());
        let mut c = ExecutionRecord::new(intent("c"));
        c.state = ExecutionState::Pending;
        c.signature = Some("SC".into());
        let mut d = ExecutionRecord::new(intent("d"));
        d.state = ExecutionState::Confirmed;
        assert_eq!(ledger.hydrate(vec![a, b, c, d]).await, 4);
        assert_eq!(
            ledger
                .hydrate(vec![ExecutionRecord::new(intent("a"))])
                .await,
            0
        );

        let ambiguous = ledger.resolve_after_restart().await;
        let mut ids: Vec<String> = ambiguous.iter().map(|r| r.intent_id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(ledger.get("a").await.unwrap().state, ExecutionState::Failed);
        assert_eq!(
            ledger.get("b").await.unwrap().state,
            ExecutionState::Pending
        );
        assert_eq!(
            ledger.get("b").await.unwrap().failure,
            Some(FailureClass::TransportAmbiguous)
        );
        assert!(ledger.get_by_signature("SC").await.is_some());
        // "a" provably never broadcast → may be retried; "b" may not.
        assert!(matches!(
            ledger.begin(intent("a")).await.unwrap(),
            BeginOutcome::Rearmed(_)
        ));
        assert!(ledger.begin(intent("b")).await.unwrap().is_duplicate());
    }

    #[tokio::test]
    async fn eviction_drops_only_settled_records() {
        let ledger = ExecutionLedger::new(64); // cap floor is 64
        for i in 0..70 {
            let id = format!("e{i}");
            ledger.begin(intent(&id)).await.unwrap();
            if i % 2 == 0 {
                ledger.fail(&id, FailureClass::Rejected, "x").await.unwrap();
            }
        }
        let len = ledger.len().await;
        assert!(len <= 64, "ledger must stay bounded, got {len}");
        assert_eq!(
            ledger.open().await.len(),
            35,
            "live records are never evicted"
        );
        let counts = ledger.counts().await;
        assert_eq!(counts.get(&ExecutionState::Created), Some(&35));
    }

    #[tokio::test]
    async fn empty_intent_id_is_rejected() {
        let ledger = ExecutionLedger::new(64);
        assert!(ledger.begin(intent("  ")).await.is_err());
    }

    #[test]
    fn global_ledger_is_a_singleton() {
        assert!(Arc::ptr_eq(&ledger(), &ledger()));
    }
}
