//! Execution pipeline: build → (fee decision) → simulate → broadcast →
//! confirm, with paper-trading, Jito bundles, latency accounting and the
//! execution lifecycle (`bot_core::execution`) threaded through every step.
//!
//! Lifecycle per attempt (states from [`ExecutionState`]):
//!
//! ```text
//! begin ─► Created ─► Validated ─► Submitted ─► Pending ─► Confirmed
//!             │           │  (paper) └──────────┐   │          ▲
//!             │           └─► Confirmed          │   ├─► Failed │ (landed, program error)
//!             │  fee veto / build / simulation   │   └─► Expired (blockhash died; rebuild ok)
//!             └─► Failed / Expired ◄─────────────┘  definite node rejection
//! ```
//!
//! * `begin` is the duplicate guard: a second run for an intent that is live
//!   or already landed is refused (`ExecStatus::Skipped` +
//!   `FailureClass::Duplicate`), a run after a definite failure re-arms it.
//! * `Submitted` is written **before** the bytes leave the process
//!   (write-ahead: a crash after this point leaves a record reconciliation
//!   can resolve from the signature).
//! * Ambiguous outcomes (transport failure, confirmation timeout) park in
//!   `Pending` and are never blind-retried; definite ones (`Expired`,
//!   rate-limit) are rebuilt with a fresh blockhash and an escalated fee.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tracing::{debug, error, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::execution::{
    self, BeginOutcome, ExecutionIntent, ExecutionLedger, ExecutionState, FailureClass,
};
use bot_core::maths;
use bot_core::models::ExecutionMode;
use bot_core::obs::metrics;

use crate::consts::JITO_BUNDLE_PATH;
use crate::fees::{FeeOracle, FeePolicy};
use crate::provider::RpcErrorClass;
use crate::rpc::{ConfirmOutcome, Rpc, RpcFailure};
use crate::tokens::Wallet;
use crate::tx::{BuiltTx, TxBuilder, TxRequest};

/// What happened to one transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub signature: String,
    pub status: ExecStatus,
    pub label: String,
    /// Total wall time from "start building" to "settled or gave up".
    pub total_ms: u64,
    pub simulate_ms: Option<u64>,
    pub send_ms: Option<u64>,
    pub confirm_ms: Option<u64>,
    pub tx_size: usize,
    /// Simulation logs, kept for the failure diagnostics.
    pub logs: Vec<String>,
    /// On failure, the reason.
    pub error: Option<String>,
    /// Whether this ran in paper mode (nothing was actually broadcast).
    pub paper: bool,
    /// How many broadcast attempts produced this result (0 = never attempted,
    /// 1 = first try succeeded/failed outright).
    pub attempts: u8,
    /// Deterministic execution-intent id this run was recorded under.
    #[serde(default)]
    pub intent_id: String,
    /// Lifecycle state of the intent when this result was produced.
    #[serde(default)]
    pub state: ExecutionState,
    /// Failure class when the attempt did not confirm (ambiguous classes
    /// included: `TransportAmbiguous`, `ConfirmationTimeout`).
    #[serde(default)]
    pub failure: Option<FailureClass>,
    /// Priority fee actually set on the broadcast transaction (µlamports/CU).
    #[serde(default)]
    pub priority_fee_micro_lamports: u64,
}

impl ExecutionResult {
    /// A blank result (`Skipped`, nothing measured) to fill in.
    pub fn empty(label: impl Into<String>, intent_id: impl Into<String>, paper: bool) -> Self {
        ExecutionResult {
            signature: String::new(),
            status: ExecStatus::Skipped,
            label: label.into(),
            total_ms: 0,
            simulate_ms: None,
            send_ms: None,
            confirm_ms: None,
            tx_size: 0,
            logs: Vec::new(),
            error: None,
            paper,
            attempts: 0,
            intent_id: intent_id.into(),
            state: ExecutionState::Created,
            failure: None,
            priority_fee_micro_lamports: 0,
        }
    }

    pub fn succeeded(&self) -> bool {
        matches!(
            self.status,
            ExecStatus::Confirmed
                | ExecStatus::Sent
                | ExecStatus::SendUnknown
                | ExecStatus::PaperFilled
        )
    }

    /// The signature as a `Signature`, when we have a real one.
    pub fn sig(&self) -> Option<Signature> {
        self.signature.parse::<Signature>().ok()
    }

    /// The signature that actually LEFT the process, for the write-ahead
    /// intent journal (§I crash point C): `None` in paper mode or when
    /// nothing was ever broadcast (simulation reject / build failure /
    /// policy veto / duplicate refusal — those results may still carry a
    /// signature for audit, but it never reached a node).
    pub fn broadcast_signature(&self) -> Option<String> {
        let left_the_process = !matches!(
            self.status,
            ExecStatus::SimulationFailed | ExecStatus::Skipped
        );
        (!self.paper && left_the_process && !self.signature.is_empty())
            .then(|| self.signature.clone())
    }

    /// Refused by the duplicate guard (an attempt for this intent was
    /// already live or had landed).
    pub fn is_duplicate(&self) -> bool {
        self.failure == Some(FailureClass::Duplicate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecStatus {
    /// Broadcast and confirmed on chain.
    Confirmed,
    /// Broadcast, confirmation still pending (timed out waiting).
    Sent,
    /// Paper mode: simulated and filled locally.
    PaperFilled,
    /// Simulation rejected it; never broadcast.
    SimulationFailed,
    /// Broadcast failed on every endpoint with a DEFINITE rejection (the
    /// network answered: blockhash expired, sanitize error, rate-limit
    /// response) — the transaction cannot have landed.
    SendFailed,
    /// Broadcast attempt ended ambiguously (transport timeout / connection
    /// loss — no answer from the network). The signed transaction MAY still
    /// reach a leader and land. Treat exactly like [`ExecStatus::Sent`]:
    /// persist the signature, enqueue reconciliation, never blind-retry with
    /// a fresh blockhash.
    SendUnknown,
    /// The transaction landed but the program returned an error.
    LandedFailed,
    /// Never attempted (disabled / risk veto).
    Skipped,
}

impl ExecStatus {
    /// True when the outcome is NOT yet proven by external state (§I):
    /// `Sent` (broadcast accepted, confirmation pending) and `SendUnknown`
    /// (transport ambiguity — the tx may still land). Distributed ownership
    /// must HAND OFF such executions to reconciliation instead of releasing
    /// the claim, so no replica resubmits while the outcome is unknown.
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, ExecStatus::Sent | ExecStatus::SendUnknown)
    }

    /// Closed-set metric label.
    pub fn label(&self) -> &'static str {
        match self {
            ExecStatus::Confirmed => "confirmed",
            ExecStatus::Sent => "sent",
            ExecStatus::PaperFilled => "paper_filled",
            ExecStatus::SimulationFailed => "simulation_failed",
            ExecStatus::SendFailed => "send_failed",
            ExecStatus::SendUnknown => "send_unknown",
            ExecStatus::LandedFailed => "landed_failed",
            ExecStatus::Skipped => "skipped",
        }
    }
}

/// Whether a broadcast failure is provably terminal or outcome-unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendFailure {
    /// The endpoint(s) answered with a rejection: the tx was not accepted
    /// and cannot land.
    Definite,
    /// No answer (timeout / transport error): the tx may have been received
    /// and forwarded before the connection broke. Conservative default.
    Ambiguous,
}

/// Classify a broadcast error string into [`SendFailure`].
///
/// Conservative by design: anything that is not a clearly node-produced
/// rejection is treated as [`SendFailure::Ambiguous`], because misclassifying
/// an ambiguous failure as definite invites a blind resubmit (a second
/// money-moving transaction) while misclassifying a definite rejection as
/// ambiguous merely delays a retry until reconciliation resolves it.
pub fn classify_send_error(err: &str) -> SendFailure {
    let lower = err.to_ascii_lowercase();
    // No response received → we cannot know whether the tx reached a leader.
    const AMBIGUOUS: &[&str] = &[
        "timed out",
        "timeout",
        "error sending request",
        "connection closed",
        "connection reset",
        "connection refused",
        "broken pipe",
        "dns error",
        "operation timed out",
    ];
    // Explicit node rejections: the RPC answered, the tx was not accepted.
    const DEFINITE: &[&str] = &[
        "blockhash",
        "block height",
        "too many requests",
        "toomanyrequests",
        "rate limit",
        "429",
        "invalid",
        "sanitize",
        "insufficient funds",
        "rejected",
        "simulation",
        "not available",
    ];
    if DEFINITE.iter().any(|m| lower.contains(m)) {
        // DEFINITE wins even if the message also mentions a timeout, because
        // a rejection proves the endpoint processed (and refused) the tx.
        return SendFailure::Definite;
    }
    if AMBIGUOUS.iter().any(|m| lower.contains(m)) {
        return SendFailure::Ambiguous;
    }
    SendFailure::Ambiguous
}

/// A classified broadcast failure: the message, whether the tx may still
/// land, and the lifecycle failure class.
#[derive(Debug, Clone)]
pub struct SendError {
    pub message: String,
    pub failure: SendFailure,
    pub class: FailureClass,
}

impl SendError {
    /// From the RPC layer's classified failure (preferred: no text parsing).
    pub fn from_rpc(f: &RpcFailure) -> Self {
        let (failure, class) = match f.class {
            RpcErrorClass::Timeout | RpcErrorClass::Transport => {
                (SendFailure::Ambiguous, FailureClass::TransportAmbiguous)
            }
            RpcErrorClass::RateLimited { .. } => (SendFailure::Definite, FailureClass::RateLimited),
            RpcErrorClass::Blockhash => (SendFailure::Definite, FailureClass::BlockhashExpired),
            RpcErrorClass::Permanent => {
                let class = match FailureClass::classify_message(&f.message) {
                    // The node answered with a rejection we do not recognise
                    // textually: still definite.
                    FailureClass::TransportAmbiguous => FailureClass::Rejected,
                    other => other,
                };
                (SendFailure::Definite, class)
            }
            // The node could not serve — but a gateway 5xx can arrive after
            // the node forwarded the tx. Trust a textual rejection, else be
            // conservative.
            RpcErrorClass::Unavailable => match classify_send_error(&f.message) {
                SendFailure::Definite => (SendFailure::Definite, FailureClass::Rejected),
                SendFailure::Ambiguous => {
                    (SendFailure::Ambiguous, FailureClass::TransportAmbiguous)
                }
            },
        };
        SendError {
            message: f.message.clone(),
            failure,
            class,
        }
    }

    /// From free text (Jito HTTP errors, legacy paths).
    pub fn from_text(message: String) -> Self {
        let failure = classify_send_error(&message);
        let class = match failure {
            SendFailure::Ambiguous => FailureClass::TransportAmbiguous,
            SendFailure::Definite => match FailureClass::classify_message(&message) {
                FailureClass::TransportAmbiguous => FailureClass::Rejected,
                other => other,
            },
        };
        SendError {
            message,
            failure,
            class,
        }
    }

    /// From a `BotError` produced by a broadcast path.
    pub fn from_error(e: &BotError) -> Self {
        SendError::from_text(e.to_string())
    }

    /// Two endpoints both failed: the combined verdict is ambiguous when
    /// EITHER attempt was (the tx may have reached a leader through it).
    pub fn combine(primary: SendError, failover: SendError) -> Self {
        let ambiguous =
            primary.failure == SendFailure::Ambiguous || failover.failure == SendFailure::Ambiguous;
        let class = if ambiguous {
            FailureClass::TransportAmbiguous
        } else if primary.class == FailureClass::BlockhashExpired
            || failover.class == FailureClass::BlockhashExpired
        {
            FailureClass::BlockhashExpired
        } else {
            failover.class
        };
        SendError {
            message: format!(
                "send failed on both endpoints: primary={}, failover={}",
                primary.message, failover.message
            ),
            failure: if ambiguous {
                SendFailure::Ambiguous
            } else {
                SendFailure::Definite
            },
            class,
        }
    }

    pub fn into_bot_error(self) -> BotError {
        BotError::solana(self.message)
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// How to broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum BroadcastMode {
    /// Plain `sendTransaction` to the RPC endpoint(s).
    #[default]
    Rpc,
    /// Jito bundle (MEV-protected, needs a tip and a block-engine URL).
    Jito,
    /// Try Jito first, fall back to plain RPC if the bundle is rejected.
    JitoThenRpc,
}

/// Execution policy.
#[derive(Debug, Clone)]
pub struct ExecPolicy {
    pub mode: ExecutionMode,
    pub broadcast: BroadcastMode,
    pub simulate_first: bool,
    /// Abort if the simulation reports a failure — always true in practice,
    /// but some operators prefer to broadcast anyway when the RPC is flaky.
    pub abort_on_simulation_failure: bool,
    pub confirm_timeout: Duration,
    /// How often to poll `getSignatureStatuses` while confirming.
    pub confirm_poll_interval: Duration,
    /// Retry the whole build+send cycle this many times on a blockhash error.
    pub max_attempts: u8,
    /// Jito block engine endpoint, e.g. `https://mainnet.block-engine.jito.wtf`.
    pub jito_url: Option<String>,
    /// Minimum priority fee that must be set before we bother simulating.
    pub min_priority_fee_micro_lamports: u64,
    /// Broadcast the signed transaction to the primary RPC and every fallback
    /// endpoint concurrently; the first acceptance wins (BUILD PLAN §5
    /// multi-RPC fan-out). Duplicates are deduped by the leader, so this
    /// trades a little egress for landing rate. Only affects `BroadcastMode::Rpc`.
    pub fanout: bool,
}

impl Default for ExecPolicy {
    fn default() -> Self {
        ExecPolicy {
            mode: ExecutionMode::Paper,
            broadcast: BroadcastMode::Rpc,
            simulate_first: true,
            abort_on_simulation_failure: true,
            confirm_timeout: Duration::from_secs(30),
            confirm_poll_interval: Duration::from_millis(400),
            max_attempts: 2,
            jito_url: None,
            min_priority_fee_micro_lamports: 0,
            fanout: false,
        }
    }
}

/// Builds, simulates and broadcasts transactions for one wallet.
pub struct Executor {
    rpc: Rpc,
    wallet: Arc<Wallet>,
    policy: ExecPolicy,
    http: reqwest::Client,
    /// Optional signer registry for transactions that need signers beyond
    /// the wallet (`TxRequest::extra_signers`). Wallet-only behaviour is
    /// unchanged when this is `None`.
    signers: Option<Arc<crate::signer::SignerRegistry>>,
    /// Priority-fee policy (bounds, escalation, adaptive mode).
    fee_policy: FeePolicy,
    /// Adaptive fee sampler (only consulted when the policy is adaptive).
    fee_oracle: Arc<FeeOracle>,
    /// Lifecycle ledger + duplicate guard. The process-wide ledger by
    /// default so every executor shares one guard.
    ledger: Arc<ExecutionLedger>,
}

impl Executor {
    pub fn new(rpc: Rpc, wallet: Arc<Wallet>, policy: ExecPolicy) -> Self {
        let fee_policy = FeePolicy::default();
        Executor {
            fee_oracle: FeeOracle::new(rpc.clone(), fee_policy),
            rpc,
            wallet,
            policy,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            signers: None,
            fee_policy,
            ledger: execution::ledger(),
        }
    }

    /// Attach the signer registry so requests with `extra_signers` can be
    /// satisfied. Explicitly optional: without it, any request needing a
    /// second signer fails with `SignerError::MissingSigner` (never a
    /// silently short signature set).
    pub fn with_signer_registry(mut self, signers: Arc<crate::signer::SignerRegistry>) -> Self {
        self.signers = Some(signers);
        self
    }

    /// Use a specific priority-fee policy (default: config defaults).
    pub fn with_fee_policy(mut self, policy: FeePolicy) -> Self {
        self.set_fee_policy(policy);
        self
    }

    /// Replace the fee policy (hot reload).
    pub fn set_fee_policy(&mut self, policy: FeePolicy) {
        self.fee_policy = policy;
        self.fee_oracle = FeeOracle::new(self.rpc.clone(), policy);
    }

    pub fn fee_policy(&self) -> &FeePolicy {
        &self.fee_policy
    }

    /// Record the lifecycle in a private ledger (tests / embedded users).
    pub fn with_ledger(mut self, ledger: Arc<ExecutionLedger>) -> Self {
        self.ledger = ledger;
        self
    }

    pub fn ledger(&self) -> &Arc<ExecutionLedger> {
        &self.ledger
    }

    /// Build a `TxBuilder` wired with this executor's wallet and (when
    /// present) its signer registry.
    fn builder(&self) -> TxBuilder<'_> {
        match &self.signers {
            Some(reg) => TxBuilder::with_registry(&self.rpc, &self.wallet, Arc::clone(reg)),
            None => TxBuilder::new(&self.rpc, &self.wallet),
        }
    }

    pub fn rpc(&self) -> &Rpc {
        &self.rpc
    }

    pub fn wallet(&self) -> &Wallet {
        &self.wallet
    }

    pub fn policy(&self) -> &ExecPolicy {
        &self.policy
    }

    pub fn set_policy(&mut self, policy: ExecPolicy) {
        self.policy = policy;
    }

    pub fn is_paper(&self) -> bool {
        self.policy.mode != ExecutionMode::Live
    }

    // ------------------------------------------------------------ intents --

    /// The lifecycle intent for a request. A pinned `intent_id` is used
    /// verbatim (full duplicate protection, including after a landed
    /// attempt). Otherwise the id is derived from wallet + label + content
    /// digest; when such a derived id already landed, the caller is making
    /// a new logical request with identical content, so a fresh id is
    /// minted — the derived id still guarantees that all retries of ONE run
    /// share an id and that concurrent identical runs are refused.
    async fn resolve_intent(&self, req: &TxRequest) -> ExecutionIntent {
        let wallet = self.wallet.pubkey.to_string();
        let mut id = match &req.intent_id {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                let derived = execution::intent_id(&[&wallet, &req.label, &req.intent_digest()]);
                match self.ledger.get(&derived).await {
                    Some(existing) if !existing.state.is_live() && !existing.can_rearm() => {
                        execution::intent_id(&[
                            derived.as_str(),
                            &chrono::Utc::now().timestamp_millis().to_string(),
                        ])
                    }
                    _ => derived,
                }
            }
        };
        if id.len() > 96 {
            id = execution::intent_id(&[id.as_str()]);
        }
        ExecutionIntent {
            intent_id: id,
            module: if req.module.trim().is_empty() {
                "solana".to_string()
            } else {
                req.module.clone()
            },
            label: req.label.clone(),
            wallet,
            symbol: req.symbol.clone(),
        }
    }

    /// Apply a lifecycle transition; never fails the trade (a ledger bug is
    /// logged loudly instead).
    async fn mark(&self, intent_id: &str, to: ExecutionState, reason: &str) {
        if let Err(e) = self.ledger.transition(intent_id, to, Some(reason)).await {
            error!(intent = intent_id, %to, error = %e, "execution ledger transition failed");
        }
    }

    /// Record a failure class on the intent (moves to the class's state).
    async fn mark_failed(&self, intent_id: &str, class: FailureClass, err: &str) {
        if let Err(e) = self.ledger.fail(intent_id, class, err).await {
            error!(intent = intent_id, %class, error = %e, "execution ledger fail() failed");
        }
    }

    /// Current lifecycle state of an intent (for the result).
    async fn state_of(&self, intent_id: &str) -> ExecutionState {
        self.ledger
            .get(intent_id)
            .await
            .map(|r| r.state)
            .unwrap_or_default()
    }

    /// Result for a refused duplicate: carries the live attempt's signature
    /// so the caller can attach to / reconcile the original.
    fn duplicate_result(
        &self,
        label: &str,
        record: &execution::ExecutionRecord,
        started: &Instant,
    ) -> ExecutionResult {
        let mut r = ExecutionResult::empty(label, &record.intent_id, self.is_paper());
        r.signature = record.signature.clone().unwrap_or_default();
        r.total_ms = started.elapsed().as_millis() as u64;
        r.error = Some(format!(
            "duplicate execution refused: intent {} is already {} (attempt {})",
            record.intent_id, record.state, record.attempts
        ));
        r.state = record.state;
        r.failure = Some(FailureClass::Duplicate);
        r.priority_fee_micro_lamports = record.priority_fee_micro_lamports;
        meter_attempt(ExecStatus::Skipped);
        r
    }

    // ---------------------------------------------------------------- run --

    /// Build, (simulate), broadcast and confirm.
    pub async fn run(&self, req: TxRequest) -> BotResult<ExecutionResult> {
        let started = Instant::now();
        let label = req.label.clone();
        let intent = self.resolve_intent(&req).await;

        if req.priority_fee_micro_lamports < self.policy.min_priority_fee_micro_lamports {
            let mut r = ExecutionResult::empty(&label, &intent.intent_id, self.is_paper());
            r.total_ms = started.elapsed().as_millis() as u64;
            r.error = Some(format!(
                "priority fee {} micro-lamports is below the configured minimum {}",
                req.priority_fee_micro_lamports, self.policy.min_priority_fee_micro_lamports
            ));
            r.state = ExecutionState::Failed;
            r.failure = Some(FailureClass::PolicyVeto);
            meter_attempt(ExecStatus::Skipped);
            return Ok(r);
        }

        // Duplicate guard + attempt 1.
        match self.ledger.begin(intent.clone()).await? {
            BeginOutcome::Duplicate(rec) => {
                return Ok(self.duplicate_result(&label, &rec, &started))
            }
            BeginOutcome::Fresh(_) | BeginOutcome::Rearmed(_) => {}
        }

        let max_attempts = self.policy.max_attempts.max(1);
        let mut last_error: Option<String> = None;
        let mut last_failure: Option<FailureClass> = None;
        let mut last_fee = 0u64;
        for attempt in 1..=max_attempts {
            let outcome = self
                .run_once(&req, &intent.intent_id, &started, attempt)
                .await;
            let retry_class = match outcome {
                Ok(result) => {
                    // Only a DEFINITE failure that a rebuild can fix is
                    // worth another attempt: expired blockhash or a
                    // rate-limited broadcast. Ambiguous outcomes (Sent,
                    // SendUnknown) are handed to reconciliation as-is.
                    match result.failure {
                        Some(class)
                            if matches!(result.status, ExecStatus::SendFailed)
                                && class.is_retryable_with_rebuild()
                                && attempt < max_attempts =>
                        {
                            last_error = result.error.clone();
                            last_failure = Some(class);
                            last_fee = result.priority_fee_micro_lamports;
                            Some(class)
                        }
                        _ => return Ok(result),
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    let class = FailureClass::classify_message(&msg);
                    let transient = msg.contains("blockhash")
                        || msg.contains("timeout")
                        || msg.contains("TooManyRequests")
                        || msg.contains("429");
                    // The attempt died before/while building — record it.
                    let recorded = if transient {
                        match class {
                            FailureClass::BlockhashExpired | FailureClass::RateLimited => class,
                            _ => FailureClass::Internal,
                        }
                    } else if class == FailureClass::TransportAmbiguous {
                        FailureClass::Internal
                    } else {
                        class
                    };
                    if self.state_of(&intent.intent_id).await.is_live() {
                        // `Internal` parks in Failed (definite: nothing left the process).
                        self.mark_failed(&intent.intent_id, recorded, &msg).await;
                    }
                    if transient && attempt < max_attempts {
                        warn!(label = %req.label, attempt, error = %e, "transient failure, retrying");
                        last_error = Some(msg);
                        last_failure = Some(recorded);
                        Some(recorded)
                    } else {
                        return Err(e);
                    }
                }
            };
            if let Some(class) = retry_class {
                warn!(
                    label = %req.label,
                    attempt,
                    %class,
                    "definite failure, rebuilding with a fresh blockhash"
                );
                metrics::global()
                    .counter(
                        "bot_execution_rebuilds_total",
                        "Attempts rebuilt with a fresh blockhash, by failure class.",
                        &[("class", class.as_str())],
                    )
                    .inc();
                self.rpc.invalidate_blockhash().await;
                // Re-arm the intent for the next attempt (Failed/Expired →
                // Created). A live/landed record here means another path
                // raced us: stop instead of double-sending.
                match self.ledger.begin(intent.clone()).await? {
                    BeginOutcome::Duplicate(rec) => {
                        return Ok(self.duplicate_result(&label, &rec, &started))
                    }
                    BeginOutcome::Fresh(_) | BeginOutcome::Rearmed(_) => {}
                }
            }
        }

        let mut r = ExecutionResult::empty(&label, &intent.intent_id, self.is_paper());
        r.status = ExecStatus::SendFailed;
        r.total_ms = started.elapsed().as_millis() as u64;
        r.error = Some(last_error.unwrap_or_else(|| "all attempts failed".into()));
        r.attempts = max_attempts;
        r.state = self.state_of(&intent.intent_id).await;
        r.failure = last_failure;
        r.priority_fee_micro_lamports = last_fee;
        Ok(r)
    }

    async fn run_once(
        &self,
        req: &TxRequest,
        intent_id: &str,
        started: &Instant,
        attempt: u8,
    ) -> BotResult<ExecutionResult> {
        let reg = metrics::global();
        let paper = self.is_paper();

        // ---- fee decision ----------------------------------------------------
        let mut req = req.clone();
        let quote = if self.fee_policy.adaptive && !paper {
            self.fee_oracle.quote(&writable_accounts(&req)).await
        } else {
            None
        };
        let decision =
            self.fee_policy
                .decide(req.priority_fee_micro_lamports, quote, attempt as u32);
        if decision.refused {
            let reason = decision.refusal_reason(&self.fee_policy);
            warn!(label = %req.label, %reason, "priority fee refused by policy");
            self.mark_failed(intent_id, FailureClass::PolicyVeto, &reason)
                .await;
            let mut r = ExecutionResult::empty(&req.label, intent_id, paper);
            r.total_ms = started.elapsed().as_millis() as u64;
            r.error = Some(reason);
            r.attempts = attempt;
            r.state = ExecutionState::Failed;
            r.failure = Some(FailureClass::PolicyVeto);
            r.priority_fee_micro_lamports = decision.fee;
            meter_attempt(ExecStatus::Skipped);
            return Ok(r);
        }
        if decision.fee != req.priority_fee_micro_lamports {
            debug!(
                label = %req.label,
                requested = req.priority_fee_micro_lamports,
                fee = decision.fee,
                source = decision.source.label(),
                attempt,
                "priority fee adjusted"
            );
        }
        req.priority_fee_micro_lamports = decision.fee;
        let fee = decision.fee;
        self.ledger.set_priority_fee(intent_id, fee).await;

        // ---- blockhash freshness -----------------------------------------
        // A retry never reuses a caller-pinned blockhash: the previous
        // attempt proved it dead. Otherwise the cached hash is used only
        // while younger than `max_blockhash_age`.
        let mut last_valid_block_height: Option<u64> = None;
        if attempt > 1 {
            req.blockhash = None;
        }
        if req.blockhash.is_none() {
            let bh = self
                .rpc
                .fresh_blockhash(self.fee_policy.max_blockhash_age)
                .await?;
            req.blockhash = Some(bh.blockhash);
            last_valid_block_height = Some(bh.last_valid_block_height);
        }

        // ---- build -----------------------------------------------------------
        let t = Instant::now();
        let builder = self.builder();
        let built = builder.build(&req).await?;
        observe_stage("build", t.elapsed());
        let last_valid_block_height = built.last_valid_block_height.or(last_valid_block_height);
        let signature = built.signature();
        if let Err(e) = self
            .ledger
            .attach_submission(
                intent_id,
                &signature.to_string(),
                Some(built.blockhash.to_string()),
                last_valid_block_height,
                fee,
            )
            .await
        {
            error!(intent = intent_id, error = %e, "execution ledger attach_submission failed");
        }
        self.mark(
            intent_id,
            ExecutionState::Validated,
            &format!(
                "built {} bytes, {} accounts, fee {} µlamports/CU ({})",
                built.size,
                built.account_count,
                fee,
                decision.source.label()
            ),
        )
        .await;

        // ---- simulate ------------------------------------------------------
        let mut simulate_ms = None;
        if self.policy.simulate_first {
            let t = Instant::now();
            match self.simulate(&built).await {
                Ok(sim) => {
                    simulate_ms = Some(t.elapsed().as_millis() as u64);
                    observe_stage("simulate", t.elapsed());
                    if let Some(err) = sim.error {
                        warn!(
                            label = %req.label,
                            error = %err,
                            logs = ?&sim.logs[sim.logs.len().saturating_sub(6)..],
                            "simulation failed"
                        );
                        if self.policy.abort_on_simulation_failure {
                            self.mark_failed(intent_id, FailureClass::SimulationRejected, &err)
                                .await;
                            let mut r = ExecutionResult::empty(&req.label, intent_id, paper);
                            r.signature = signature.to_string();
                            r.status = ExecStatus::SimulationFailed;
                            r.total_ms = started.elapsed().as_millis() as u64;
                            r.simulate_ms = simulate_ms;
                            r.tx_size = built.size;
                            r.logs = sim.logs;
                            r.error = Some(err);
                            r.attempts = attempt;
                            r.state = ExecutionState::Failed;
                            r.failure = Some(FailureClass::SimulationRejected);
                            r.priority_fee_micro_lamports = fee;
                            meter_attempt(ExecStatus::SimulationFailed);
                            return Ok(r);
                        }
                    } else {
                        debug!(
                            label = %req.label,
                            units = sim.units_consumed,
                            "simulation ok"
                        );
                    }
                }
                Err(e) => {
                    // A broken simulation endpoint must not stop a live trade.
                    simulate_ms = Some(t.elapsed().as_millis() as u64);
                    warn!(label = %req.label, error = %e, "simulate call failed, continuing");
                }
            }
        }

        // ---- paper mode: stop here ----------------------------------------
        if paper {
            info!(
                label = %req.label,
                size = built.size,
                attempt,
                "paper mode — transaction built but not broadcast"
            );
            self.mark(
                intent_id,
                ExecutionState::Confirmed,
                "paper fill (not broadcast)",
            )
            .await;
            let mut r = ExecutionResult::empty(&req.label, intent_id, true);
            r.signature = signature.to_string();
            r.status = ExecStatus::PaperFilled;
            r.total_ms = started.elapsed().as_millis() as u64;
            r.simulate_ms = simulate_ms;
            r.tx_size = built.size;
            r.attempts = 1;
            r.state = ExecutionState::Confirmed;
            r.priority_fee_micro_lamports = fee;
            meter_attempt(ExecStatus::PaperFilled);
            return Ok(r);
        }

        // ---- broadcast -----------------------------------------------------
        // Write-ahead: the record says "submitted" BEFORE the bytes leave.
        self.mark(
            intent_id,
            ExecutionState::Submitted,
            &format!("broadcast via {:?}", self.policy.broadcast),
        )
        .await;
        let t = Instant::now();
        let send_outcome = match self.policy.broadcast {
            BroadcastMode::Rpc => self.broadcast_rpc(&built).await,
            BroadcastMode::Jito => self.broadcast_jito(&built, req.jito_tip_lamports).await,
            BroadcastMode::JitoThenRpc => {
                match self.broadcast_jito(&built, req.jito_tip_lamports).await {
                    Ok(sig) => Ok(sig),
                    Err(e) => {
                        warn!(label = %req.label, error = %e, "jito bundle rejected, falling back to rpc");
                        self.broadcast_rpc(&built).await
                    }
                }
            }
        };
        let send_ms = t.elapsed().as_millis() as u64;
        observe_stage("send", t.elapsed());

        // Classify broadcast failures BEFORE acting on them: a definite
        // rejection can never land (safe terminal failure), while an
        // ambiguous transport failure means the signed tx may still reach a
        // leader — for those we fall through to confirmation so the outcome
        // is resolved from the chain, never assumed.
        let mut send_was_ambiguous = false;
        let signature = match send_outcome {
            Ok(sig) => {
                self.mark(
                    intent_id,
                    ExecutionState::Pending,
                    &format!("accepted in {send_ms} ms"),
                )
                .await;
                sig
            }
            Err(e) => match e.failure {
                SendFailure::Definite => {
                    self.mark_failed(intent_id, e.class, &e.message).await;
                    let state = e.class.terminal_state();
                    let mut r = ExecutionResult::empty(&req.label, intent_id, false);
                    r.signature = signature.to_string();
                    r.status = ExecStatus::SendFailed;
                    r.total_ms = started.elapsed().as_millis() as u64;
                    r.simulate_ms = simulate_ms;
                    r.send_ms = Some(send_ms);
                    r.tx_size = built.size;
                    r.error = Some(e.message);
                    r.attempts = attempt;
                    r.state = state;
                    r.failure = Some(e.class);
                    r.priority_fee_micro_lamports = fee;
                    meter_attempt(ExecStatus::SendFailed);
                    return Ok(r);
                }
                SendFailure::Ambiguous => {
                    warn!(
                        label = %req.label,
                        error = %e,
                        signature = %signature,
                        "broadcast outcome unknown (transport failure); \
                         confirming on-chain instead of assuming failure"
                    );
                    // Submitted → Pending with the ambiguous class recorded.
                    self.mark_failed(intent_id, FailureClass::TransportAmbiguous, &e.message)
                        .await;
                    send_was_ambiguous = true;
                    signature
                }
            },
        };

        // ---- confirm -------------------------------------------------------
        let t = Instant::now();
        let outcome = self
            .rpc
            .confirm_tracked(
                &signature,
                last_valid_block_height,
                self.policy.confirm_timeout,
                self.policy.confirm_poll_interval,
            )
            .await?;
        let confirm_ms = t.elapsed().as_millis() as u64;
        observe_stage("confirm", t.elapsed());

        let (status, error, logs, failure) = match outcome {
            ConfirmOutcome::Confirmed {
                logs,
                slot,
                fee: paid,
            } => {
                self.mark(
                    intent_id,
                    ExecutionState::Confirmed,
                    &format!("confirmed in slot {slot}, fee {paid} lamports"),
                )
                .await;
                (ExecStatus::Confirmed, None, logs, None)
            }
            ConfirmOutcome::Timeout => {
                let (status, msg) = if send_was_ambiguous {
                    (
                        ExecStatus::SendUnknown,
                        "broadcast failed ambiguously and confirmation timed out; \
                         the transaction may still land — reconciliation will resolve it",
                    )
                } else {
                    (
                        ExecStatus::Sent,
                        "confirmation timed out; the transaction may still land",
                    )
                };
                self.mark_failed(intent_id, FailureClass::ConfirmationTimeout, msg)
                    .await;
                (
                    status,
                    Some(msg.to_string()),
                    Vec::new(),
                    Some(FailureClass::ConfirmationTimeout),
                )
            }
            ConfirmOutcome::Expired {
                last_valid_block_height,
                block_height,
            } => {
                let msg = format!(
                    "blockhash expired before the transaction landed \
                     (last valid height {last_valid_block_height}, cluster at {block_height})"
                );
                self.mark_failed(intent_id, FailureClass::BlockhashExpired, &msg)
                    .await;
                // Definite and fee-free: reported as SendFailed so the
                // attempt loop rebuilds when it still has budget.
                (
                    ExecStatus::SendFailed,
                    Some(msg),
                    Vec::new(),
                    Some(FailureClass::BlockhashExpired),
                )
            }
            ConfirmOutcome::Failed { error, logs } => {
                self.mark_failed(intent_id, FailureClass::LandedFailed, &error)
                    .await;
                (
                    ExecStatus::LandedFailed,
                    Some(error),
                    logs,
                    Some(FailureClass::LandedFailed),
                )
            }
        };

        let total_ms = started.elapsed().as_millis() as u64;
        info!(
            label = %req.label,
            %signature,
            ?status,
            total_ms,
            send_ms,
            confirm_ms,
            "transaction settled"
        );
        reg.histogram(
            "bot_execution_total_ms",
            "End-to-end execution time (build to settled/gave up) in milliseconds.",
            &[("status", status.label())],
            metrics::LATENCY_BUCKETS_MS,
        )
        .observe(total_ms);
        meter_attempt(status);

        let mut r = ExecutionResult::empty(&req.label, intent_id, false);
        r.signature = signature.to_string();
        r.status = status;
        r.total_ms = total_ms;
        r.simulate_ms = simulate_ms;
        r.send_ms = Some(send_ms);
        r.confirm_ms = Some(confirm_ms);
        r.tx_size = built.size;
        r.logs = logs;
        r.error = error;
        r.attempts = attempt;
        r.state = self.state_of(intent_id).await;
        r.failure = failure;
        r.priority_fee_micro_lamports = fee;
        Ok(r)
    }

    /// Simulate without broadcasting.
    pub async fn simulate(&self, built: &BuiltTx) -> BotResult<SimulationResult> {
        let sim = self.rpc.simulate(&built.tx).await?;
        let value = sim.value;
        Ok(SimulationResult {
            error: value.err.map(|e| e.to_string()),
            logs: value.logs.unwrap_or_default(),
            units_consumed: value.units_consumed.unwrap_or(0),
        })
    }

    /// Simulate a request without building it into a [`BuiltTx`] first.
    pub async fn simulate_request(&self, req: &TxRequest) -> BotResult<SimulationResult> {
        let builder = self.builder();
        let built = builder.build(req).await?;
        self.simulate(&built).await
    }

    async fn broadcast_rpc(&self, built: &BuiltTx) -> Result<Signature, SendError> {
        if self.policy.fanout {
            return self.broadcast_fanout(built).await;
        }
        match self.rpc.send_transaction_classified(&built.tx).await {
            Ok(sig) => Ok(sig),
            Err(primary) => {
                let primary = SendError::from_rpc(&primary);
                // Try the failover endpoint once before giving up.
                match self.rpc.failover() {
                    Some(failover) => {
                        warn!(
                            label = %built.label,
                            error = %primary,
                            failover = failover.provider_label(),
                            "primary RPC rejected, retrying on failover"
                        );
                        failover
                            .send_transaction_classified(&built.tx)
                            .await
                            .map_err(|e| SendError::combine(primary, SendError::from_rpc(&e)))
                    }
                    None => Err(primary),
                }
            }
        }
    }

    /// Multi-RPC fan-out (BUILD PLAN §5): send the same signed transaction to
    /// the primary and every fallback endpoint concurrently; the first
    /// acceptance wins and the remaining futures are dropped. Duplicate
    /// delivery is harmless — the leader dedupes by signature — and one
    /// lagging node no longer costs the whole landing window.
    async fn broadcast_fanout(&self, built: &BuiltTx) -> Result<Signature, SendError> {
        use futures_util::stream::{FuturesUnordered, StreamExt};

        // Primary + the whole failover chain.
        let mut endpoints = vec![self.rpc.clone()];
        let mut cursor = self.rpc.clone();
        while let Some(fo) = cursor.failover() {
            cursor = fo.clone();
            endpoints.push(fo);
        }

        let reg = metrics::global();
        let mut sends = FuturesUnordered::new();
        for ep in endpoints {
            let tx = built.tx.clone();
            let label = built.label.clone();
            sends.push(async move {
                let provider = ep.provider_label().to_string();
                let started = std::time::Instant::now();
                let res = ep.send_transaction_classified(&tx).await;
                (provider, label, started.elapsed().as_millis() as u64, res)
            });
        }

        let total = sends.len();
        let mut last_err: Option<SendError> = None;
        let mut any_ambiguous = false;
        let mut accepted: Option<Signature> = None;
        while let Some((provider, label, ms, res)) = sends.next().await {
            match res {
                Ok(sig) => {
                    debug!(%provider, %label, %sig, ms, endpoints = total, "fanout: accepted");
                    accepted = Some(sig);
                    break; // remaining futures are dropped (in-flight dupes are fine)
                }
                Err(e) => {
                    warn!(%provider, %label, error = %e, "fanout: endpoint rejected");
                    let se = SendError::from_rpc(&e);
                    any_ambiguous |= se.failure == SendFailure::Ambiguous;
                    last_err = Some(se);
                }
            }
        }
        drop(sends);

        match accepted {
            Some(sig) => {
                reg.counter(
                    "bot_broadcast_fanout_total",
                    "Fan-out broadcasts by outcome.",
                    &[("outcome", "accepted")],
                )
                .inc();
                Ok(sig)
            }
            None => {
                reg.counter(
                    "bot_broadcast_fanout_total",
                    "Fan-out broadcasts by outcome.",
                    &[("outcome", "all_failed")],
                )
                .inc();
                let mut err = last_err.unwrap_or_else(|| {
                    SendError::from_text("fanout: no endpoints configured".to_string())
                });
                // If ANY endpoint failed ambiguously the tx may have reached
                // a leader through it: the fan-out as a whole is ambiguous.
                if any_ambiguous {
                    err.failure = SendFailure::Ambiguous;
                    err.class = FailureClass::TransportAmbiguous;
                }
                Err(err)
            }
        }
    }

    /// Recover the tip amount from a prebuilt transaction by reading its
    /// system-program transfer to a known Jito tip account.
    fn jito_tip_for(&self, built: &BuiltTx) -> u64 {
        for ix in &built.instructions {
            if ix.program_id != *crate::consts::SYSTEM_PROGRAM {
                continue;
            }
            if ix.data.len() < 12 || ix.data[0] != 2 {
                continue; // 2 = Transfer
            }
            let dest = ix.accounts.get(1).map(|m| m.pubkey);
            let lamports = u64::from_le_bytes(ix.data[4..12].try_into().unwrap_or([0; 8]));
            if dest.map(|d| crate::consts::JITO_TIP_ACCOUNTS.contains(&d)) == Some(true) {
                return lamports;
            }
        }
        0
    }

    /// Submit as a Jito bundle. The bundle carries exactly one transaction;
    /// Jito requires the tip transfer to be inside it (see `TxBuilder`).
    async fn broadcast_jito(
        &self,
        built: &BuiltTx,
        tip_lamports: u64,
    ) -> Result<Signature, SendError> {
        self.broadcast_jito_inner(built, tip_lamports)
            .await
            .map_err(|e| SendError::from_error(&e))
    }

    async fn broadcast_jito_inner(
        &self,
        built: &BuiltTx,
        tip_lamports: u64,
    ) -> BotResult<Signature> {
        let base = self
            .policy
            .jito_url
            .as_deref()
            .ok_or_else(|| BotError::config("jito broadcast requested but no jito_url is set"))?
            .trim_end_matches('/')
            .to_string();
        if tip_lamports == 0 {
            return Err(BotError::invalid(
                "jito bundles require a tip; set jito_tip_lamports > 0",
            ));
        }

        let b64 = base64::engine::general_purpose::STANDARD.encode(&built.bytes);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[b64], {"encoding": "base64"}]
        });

        let response = self
            .http
            .post(format!("{base}{JITO_BUNDLE_PATH}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| BotError::http(format!("jito bundle post: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| BotError::http(format!("jito bundle body: {e}")))?;
        if !status.is_success() {
            return Err(BotError::http(format!(
                "jito bundle http {status}: {}",
                truncate(&text, 300)
            )));
        }
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| BotError::encoding(format!("jito bundle json: {e}")))?;
        if let Some(err) = value.get("error") {
            return Err(BotError::solana(format!("jito bundle error: {err}")));
        }
        // sendBundle returns the bundle id, not the tx signature.
        let _bundle_id = value
            .get("result")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(built.signature())
    }

    /// Refresh the blockhash and re-sign, keeping the instruction set.
    pub async fn rebuild_with_fresh_blockhash(&self, req: &TxRequest) -> BotResult<BuiltTx> {
        let bh = self.rpc.latest_blockhash(true).await?;
        let mut req = req.clone();
        req.blockhash = Some(bh.blockhash);
        let builder = self.builder();
        let mut built = builder.build(&req).await?;
        built.last_valid_block_height = Some(bh.last_valid_block_height);
        Ok(built)
    }

    /// Send an already-signed transaction (used by the pre-signed sniper path,
    /// where the transaction was built *before* the launch was detected).
    pub async fn send_prebuilt(&self, built: &BuiltTx) -> BotResult<ExecutionResult> {
        let started = Instant::now();
        let signature = built.signature();
        if self.is_paper() {
            let mut r = ExecutionResult::empty(&built.label, "", true);
            r.signature = signature.to_string();
            r.status = ExecStatus::PaperFilled;
            r.total_ms = started.elapsed().as_millis() as u64;
            r.tx_size = built.size;
            r.attempts = 1;
            r.state = ExecutionState::Confirmed;
            meter_attempt(ExecStatus::PaperFilled);
            return Ok(r);
        }

        // One intent per signed transaction: a pinned id when the caller
        // supplied one, otherwise the signature is the identity.
        let intent = ExecutionIntent {
            intent_id: if built.intent_id.trim().is_empty() {
                execution::intent_id(&["prebuilt", &signature.to_string()])
            } else {
                built.intent_id.trim().to_string()
            },
            module: if built.module.is_empty() {
                "solana".into()
            } else {
                built.module.clone()
            },
            label: built.label.clone(),
            wallet: self.wallet.pubkey.to_string(),
            symbol: built.symbol.clone(),
        };
        let intent_id = intent.intent_id.clone();
        match self.ledger.begin(intent).await? {
            BeginOutcome::Duplicate(rec) => {
                return Ok(self.duplicate_result(&built.label, &rec, &started))
            }
            BeginOutcome::Fresh(_) | BeginOutcome::Rearmed(_) => {}
        }
        if let Err(e) = self
            .ledger
            .attach_submission(
                &intent_id,
                &signature.to_string(),
                Some(built.blockhash.to_string()),
                built.last_valid_block_height,
                0,
            )
            .await
        {
            error!(intent = %intent_id, error = %e, "execution ledger attach_submission failed");
        }
        self.mark(
            &intent_id,
            ExecutionState::Validated,
            &format!("prebuilt, {} bytes", built.size),
        )
        .await;

        // A pre-signed transaction can go stale while it sits in the queue.
        let valid = self
            .rpc
            .is_blockhash_valid(&built.blockhash)
            .await
            .unwrap_or(true);
        if !valid {
            let msg = "prebuilt transaction blockhash expired before it was sent";
            self.mark_failed(&intent_id, FailureClass::BlockhashExpired, msg)
                .await;
            let mut r = ExecutionResult::empty(&built.label, &intent_id, false);
            r.signature = signature.to_string();
            r.status = ExecStatus::SendFailed;
            r.total_ms = started.elapsed().as_millis() as u64;
            r.tx_size = built.size;
            r.error = Some(msg.into());
            r.attempts = 1;
            r.state = ExecutionState::Expired;
            r.failure = Some(FailureClass::BlockhashExpired);
            meter_attempt(ExecStatus::SendFailed);
            return Ok(r);
        }

        self.mark(
            &intent_id,
            ExecutionState::Submitted,
            &format!("broadcast via {:?}", self.policy.broadcast),
        )
        .await;
        let t = Instant::now();
        let send = match self.policy.broadcast {
            BroadcastMode::Rpc => self.broadcast_rpc(built).await,
            // A transaction signed elsewhere (`BuiltTx::from_signed`) carries
            // no instruction list, so no tip can be found and it cannot be
            // bundled: RPC is the only route that can land it.
            BroadcastMode::Jito | BroadcastMode::JitoThenRpc if built.instructions.is_empty() => {
                debug!(label = %built.label, "externally signed transaction: broadcasting via rpc");
                self.broadcast_rpc(built).await
            }
            BroadcastMode::Jito | BroadcastMode::JitoThenRpc => {
                match self.broadcast_jito(built, self.jito_tip_for(built)).await {
                    Ok(sig) => Ok(sig),
                    Err(e) if self.policy.broadcast == BroadcastMode::JitoThenRpc => {
                        warn!(error = %e, "jito rejected prebuilt, falling back to rpc");
                        self.broadcast_rpc(built).await
                    }
                    Err(e) => Err(e),
                }
            }
        };
        let send_ms = t.elapsed().as_millis() as u64;
        observe_stage("send", t.elapsed());

        let mut send_was_ambiguous = false;
        let signature = match send {
            Ok(sig) => {
                self.mark(
                    &intent_id,
                    ExecutionState::Pending,
                    &format!("accepted in {send_ms} ms"),
                )
                .await;
                sig
            }
            Err(e) => match e.failure {
                SendFailure::Definite => {
                    self.mark_failed(&intent_id, e.class, &e.message).await;
                    let mut r = ExecutionResult::empty(&built.label, &intent_id, false);
                    r.signature = signature.to_string();
                    r.status = ExecStatus::SendFailed;
                    r.total_ms = started.elapsed().as_millis() as u64;
                    r.send_ms = Some(send_ms);
                    r.tx_size = built.size;
                    r.error = Some(e.message);
                    r.attempts = 1;
                    r.state = e.class.terminal_state();
                    r.failure = Some(e.class);
                    meter_attempt(ExecStatus::SendFailed);
                    return Ok(r);
                }
                SendFailure::Ambiguous => {
                    warn!(
                        label = %built.label,
                        error = %e,
                        %signature,
                        "prebuilt broadcast outcome unknown; confirming on-chain"
                    );
                    self.mark_failed(&intent_id, FailureClass::TransportAmbiguous, &e.message)
                        .await;
                    send_was_ambiguous = true;
                    signature
                }
            },
        };

        let t = Instant::now();
        let outcome = self
            .rpc
            .confirm_tracked(
                &signature,
                built.last_valid_block_height,
                self.policy.confirm_timeout,
                self.policy.confirm_poll_interval,
            )
            .await?;
        let confirm_ms = t.elapsed().as_millis() as u64;
        observe_stage("confirm", t.elapsed());
        let (status, error, logs, failure) = match outcome {
            ConfirmOutcome::Confirmed { logs, slot, fee } => {
                self.mark(
                    &intent_id,
                    ExecutionState::Confirmed,
                    &format!("confirmed in slot {slot}, fee {fee} lamports"),
                )
                .await;
                (ExecStatus::Confirmed, None, logs, None)
            }
            ConfirmOutcome::Timeout => {
                self.mark_failed(
                    &intent_id,
                    FailureClass::ConfirmationTimeout,
                    "confirmation timed out",
                )
                .await;
                (
                    if send_was_ambiguous {
                        ExecStatus::SendUnknown
                    } else {
                        ExecStatus::Sent
                    },
                    Some("confirmation timed out".into()),
                    Vec::new(),
                    Some(FailureClass::ConfirmationTimeout),
                )
            }
            ConfirmOutcome::Expired {
                last_valid_block_height,
                block_height,
            } => {
                let msg = format!(
                    "blockhash expired before the transaction landed \
                     (last valid height {last_valid_block_height}, cluster at {block_height})"
                );
                self.mark_failed(&intent_id, FailureClass::BlockhashExpired, &msg)
                    .await;
                (
                    ExecStatus::SendFailed,
                    Some(msg),
                    Vec::new(),
                    Some(FailureClass::BlockhashExpired),
                )
            }
            ConfirmOutcome::Failed { error, logs } => {
                self.mark_failed(&intent_id, FailureClass::LandedFailed, &error)
                    .await;
                (
                    ExecStatus::LandedFailed,
                    Some(error),
                    logs,
                    Some(FailureClass::LandedFailed),
                )
            }
        };
        meter_attempt(status);

        let mut r = ExecutionResult::empty(&built.label, &intent_id, false);
        r.signature = signature.to_string();
        r.status = status;
        r.total_ms = started.elapsed().as_millis() as u64;
        r.send_ms = Some(send_ms);
        r.confirm_ms = Some(confirm_ms);
        r.tx_size = built.size;
        r.logs = logs;
        r.error = error;
        r.attempts = 1;
        r.state = self.state_of(&intent_id).await;
        r.failure = failure;
        Ok(r)
    }
}

/// Unique writable accounts of a request (the priority-fee market is per
/// writable account).
fn writable_accounts(req: &TxRequest) -> Vec<Pubkey> {
    let mut out: Vec<Pubkey> = Vec::new();
    for ix in &req.instructions {
        for meta in &ix.accounts {
            if meta.is_writable && !out.contains(&meta.pubkey) {
                out.push(meta.pubkey);
            }
        }
    }
    out
}

fn observe_stage(stage: &str, elapsed: Duration) {
    metrics::global()
        .histogram(
            "bot_execution_stage_ms",
            "Execution stage latency (build, simulate, send, confirm) in milliseconds.",
            &[("stage", stage)],
            metrics::LATENCY_BUCKETS_MS,
        )
        .observe(elapsed.as_millis() as u64);
}

fn meter_attempt(status: ExecStatus) {
    metrics::global()
        .counter(
            "bot_execution_attempts_total",
            "Execution attempts by final status.",
            &[("status", status.label())],
        )
        .inc();
}

/// Simulation output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimulationResult {
    pub error: Option<String>,
    pub logs: Vec<String>,
    pub units_consumed: u64,
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    // Step back to a char boundary; `floor_char_boundary` is still unstable.
    let mut cut = n;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

/// Estimate the SOL cost of a trade for paper-mode P&L.
pub fn paper_fill_price(sol_in: u64, slippage_pct: f64) -> u64 {
    maths::minus_pct_u64(sol_in, slippage_pct)
}

/// A pre-signed transaction waiting for a trigger.
#[derive(Debug)]
pub struct PrebuiltTx {
    pub built: BuiltTx,
    pub created_at: Instant,
    pub expires_at: Instant,
}

impl PrebuiltTx {
    pub fn new(built: BuiltTx, ttl: Duration) -> Self {
        let now = Instant::now();
        PrebuiltTx {
            built,
            created_at: now,
            expires_at: now + ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    pub fn remaining(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }

    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }
}

/// Keeps one pre-signed transaction per mint, refreshed as blockhashes roll.
pub struct PrebuiltCache {
    inner: tokio::sync::RwLock<std::collections::HashMap<String, PrebuiltTx>>,
    ttl: Duration,
}

impl PrebuiltCache {
    pub fn new(ttl: Duration) -> Self {
        PrebuiltCache {
            inner: Default::default(),
            ttl,
        }
    }

    pub async fn insert(&self, key: impl Into<String>, built: BuiltTx) {
        self.inner
            .write()
            .await
            .insert(key.into(), PrebuiltTx::new(built, self.ttl));
    }

    /// Take a transaction for immediate use, dropping expired entries.
    pub async fn take(&self, key: &str) -> Option<BuiltTx> {
        let mut map = self.inner.write().await;
        map.retain(|_, v| !v.is_expired());
        map.remove(key).map(|p| p.built)
    }

    pub async fn peek(&self, key: &str) -> Option<Hash> {
        self.inner.read().await.get(key).map(|p| p.built.blockhash)
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    pub async fn prune(&self) -> usize {
        let mut map = self.inner.write().await;
        let before = map.len();
        map.retain(|_, v| !v.is_expired());
        before - map.len()
    }

    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

/// Build an [`ExecPolicy`] from the operator config.
///
/// Live execution is downgraded to simulate when the explicit
/// `allow_live_trading` gate is closed. Shared by every trading module so the
/// policy is consistent across the suite.
pub fn exec_policy_from_config(cfg: &bot_core::config::Config) -> ExecPolicy {
    let ex = &cfg.execution;
    let mode = if ex.mode.is_live() && !ex.allow_live_trading {
        // Live requested but the safety gate is closed: simulate only.
        ExecutionMode::Simulate
    } else {
        ex.mode
    };
    let broadcast = if ex.use_jito {
        BroadcastMode::JitoThenRpc
    } else {
        BroadcastMode::Rpc
    };
    let jito_url = if ex.use_jito {
        Some(ex.jito_block_engine_url.clone())
    } else {
        None
    };
    ExecPolicy {
        mode,
        broadcast,
        simulate_first: ex.simulate_first,
        abort_on_simulation_failure: ex.abort_on_simulation_failure,
        confirm_timeout: Duration::from_millis(ex.confirm_timeout_ms),
        confirm_poll_interval: Duration::from_millis(ex.confirm_poll_ms.max(50)),
        max_attempts: u8::try_from(ex.send_retries.clamp(1, 5)).unwrap_or(2),
        jito_url,
        min_priority_fee_micro_lamports: 0,
        fanout: ex.broadcast_fanout,
    }
}

/// Build the [`FeePolicy`] that belongs next to [`exec_policy_from_config`].
pub fn fee_policy_from_config(cfg: &bot_core::config::Config) -> FeePolicy {
    FeePolicy::from_config(&cfg.execution)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn res(signature: &str, paper: bool) -> ExecutionResult {
        ExecutionResult {
            signature: signature.into(),
            status: ExecStatus::Sent,
            label: "t".into(),
            total_ms: 0,
            simulate_ms: None,
            send_ms: None,
            confirm_ms: None,
            tx_size: 0,
            logs: vec![],
            error: None,
            paper,
            attempts: 1,
            intent_id: String::new(),
            state: ExecutionState::Pending,
            failure: None,
            priority_fee_micro_lamports: 0,
        }
    }

    #[test]
    fn broadcast_signature_only_for_real_broadcasts() {
        // The intent journal (§I) links ONLY signatures that left the
        // process: paper fills and empty signatures must map to None so the
        // intent is abandoned, not linked to a phantom.
        assert_eq!(
            res("5hX7", false).broadcast_signature().as_deref(),
            Some("5hX7")
        );
        assert_eq!(res("", false).broadcast_signature(), None);
        assert_eq!(res("5hX7", true).broadcast_signature(), None);
        // Signed but never sent: simulation reject / duplicate refusal carry
        // a signature for audit only.
        let mut sim = res("5hX7", false);
        sim.status = ExecStatus::SimulationFailed;
        assert_eq!(sim.broadcast_signature(), None);
        let mut dup = res("5hX7", false);
        dup.status = ExecStatus::Skipped;
        assert_eq!(dup.broadcast_signature(), None);
        // A definite node rejection DID leave the process.
        let mut rejected = res("5hX7", false);
        rejected.status = ExecStatus::SendFailed;
        assert_eq!(rejected.broadcast_signature().as_deref(), Some("5hX7"));
    }

    #[test]
    fn status_success_semantics() {
        assert!(ExecutionResult {
            signature: String::new(),
            status: ExecStatus::Confirmed,
            label: "x".into(),
            total_ms: 0,
            simulate_ms: None,
            send_ms: None,
            confirm_ms: None,
            tx_size: 0,
            logs: vec![],
            error: None,
            paper: false,
            attempts: 1,
            intent_id: String::new(),
            state: ExecutionState::Confirmed,
            failure: None,
            priority_fee_micro_lamports: 0,
        }
        .succeeded());

        for status in [
            ExecStatus::SimulationFailed,
            ExecStatus::SendFailed,
            ExecStatus::LandedFailed,
            ExecStatus::Skipped,
        ] {
            let r = ExecutionResult {
                signature: String::new(),
                status,
                label: "x".into(),
                total_ms: 0,
                simulate_ms: None,
                send_ms: None,
                confirm_ms: None,
                tx_size: 0,
                logs: vec![],
                error: None,
                paper: false,
                attempts: 1,
                intent_id: String::new(),
                state: ExecutionState::Failed,
                failure: None,
                priority_fee_micro_lamports: 0,
            };
            assert!(!r.succeeded(), "{status:?} must not count as success");
        }
    }

    #[test]
    fn execution_result_serde_is_backward_compatible() {
        // A result serialised by the previous version (no lifecycle fields)
        // must still deserialise: the new fields default.
        let legacy = json!({
            "signature": "abc", "status": "sent", "label": "t", "total_ms": 1,
            "simulate_ms": null, "send_ms": 1, "confirm_ms": null, "tx_size": 10,
            "logs": [], "error": null, "paper": false, "attempts": 1
        });
        let r: ExecutionResult = serde_json::from_value(legacy).unwrap();
        assert_eq!(r.state, ExecutionState::Created);
        assert!(r.failure.is_none());
        assert_eq!(r.intent_id, "");
        assert!(!r.is_duplicate());
        let e = ExecutionResult::empty("l", "int_1", true);
        assert_eq!(e.status, ExecStatus::Skipped);
        assert!(e.paper && e.intent_id == "int_1");
    }

    #[test]
    fn default_policy_is_paper() {
        let p = ExecPolicy::default();
        assert_eq!(p.mode, ExecutionMode::Paper, "paper must be the default");
        assert_eq!(p.broadcast, BroadcastMode::Rpc);
        assert!(p.simulate_first);
        assert!(p.abort_on_simulation_failure);
        assert_eq!(p.max_attempts, 2);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
        // A multibyte char must not be split.
        let s = "日本語のテキストです";
        let t = truncate(s, 4);
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 5);
    }

    #[tokio::test]
    async fn prebuilt_cache_expires_entries() {
        let cache = PrebuiltCache::new(Duration::from_millis(20));
        let built = BuiltTx {
            tx: dummy_tx(),
            bytes: vec![],
            size: 0,
            account_count: 0,
            blockhash: Hash::default(),
            last_valid_block_height: None,
            label: "snipe".into(),
            instructions: vec![],
            intent_id: String::new(),
            module: String::new(),
            symbol: String::new(),
        };
        cache.insert("mint1", built.clone()).await;
        assert_eq!(cache.len().await, 1);
        assert!(cache.peek("mint1").await.is_some());

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            cache.take("mint1").await.is_none(),
            "an expired prebuilt transaction must not be handed out"
        );
        assert_eq!(cache.len().await, 0);
    }

    #[tokio::test]
    async fn prebuilt_cache_take_removes_the_entry() {
        let cache = PrebuiltCache::new(Duration::from_secs(60));
        let built = BuiltTx {
            tx: dummy_tx(),
            bytes: vec![],
            size: 0,
            account_count: 0,
            blockhash: Hash::default(),
            last_valid_block_height: None,
            label: "snipe".into(),
            instructions: vec![],
            intent_id: String::new(),
            module: String::new(),
            symbol: String::new(),
        };
        cache.insert("k", built).await;
        assert!(cache.take("k").await.is_some());
        assert!(cache.take("k").await.is_none(), "take must be once-only");
    }

    #[tokio::test]
    async fn prebuilt_cache_prune_reports_how_many_were_dropped() {
        let cache = PrebuiltCache::new(Duration::from_millis(10));
        let built = BuiltTx {
            tx: dummy_tx(),
            bytes: vec![],
            size: 0,
            account_count: 0,
            blockhash: Hash::default(),
            last_valid_block_height: None,
            label: "x".into(),
            instructions: vec![],
            intent_id: String::new(),
            module: String::new(),
            symbol: String::new(),
        };
        for i in 0..3 {
            cache.insert(format!("k{i}"), built.clone()).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(cache.prune().await, 3);
        assert_eq!(cache.len().await, 0);
    }

    #[test]
    fn paper_fill_price_applies_slippage_downwards() {
        assert_eq!(paper_fill_price(1_000_000_000, 0.0), 1_000_000_000);
        assert_eq!(paper_fill_price(1_000_000_000, 1.0), 990_000_000);
    }

    fn dummy_tx() -> solana_sdk::transaction::VersionedTransaction {
        use solana_sdk::message::VersionedMessage;
        use solana_sdk::signature::Keypair;
        use solana_sdk::signer::Signer;
        use solana_system_interface::instruction as system_instruction;

        let kp = Keypair::new();
        let msg = solana_sdk::message::v0::Message::try_compile(
            &kp.pubkey(),
            &[system_instruction::transfer(
                &kp.pubkey(),
                &Pubkey::new_unique(),
                1,
            )],
            &[],
            Hash::default(),
        )
        .unwrap();
        solana_sdk::transaction::VersionedTransaction::try_new(VersionedMessage::V0(msg), &[&kp])
            .unwrap()
    }

    // ---- Prompt 2 §V13-15: broadcast failure classification ----------------

    #[test]
    fn classify_send_error_matrix() {
        // Definite: the node answered with a rejection.
        for msg in [
            "Transaction precompile verification failure BlockhashNotFound",
            "blockhash not found",
            "TooManyRequests: rate limit exceeded",
            "HTTP 429 Too Many Requests",
            "invalid transaction: Versioned transaction message is not sanitized",
            "insufficient funds for rent",
            "simulation failed",
        ] {
            assert_eq!(
                classify_send_error(msg),
                SendFailure::Definite,
                "must be definite: {msg}"
            );
        }
        // Ambiguous: no answer — the tx may have reached a leader.
        for msg in [
            "error sending request for url (http://x/): connection closed before message completed",
            "operation timed out",
            "Timed out while waiting for response",
            "connection reset by peer",
            "broken pipe",
            "dns error: failed to lookup host",
        ] {
            assert_eq!(
                classify_send_error(msg),
                SendFailure::Ambiguous,
                "must be ambiguous: {msg}"
            );
        }
        // Conservative default: unknown text is ambiguous, never definite.
        assert_eq!(
            classify_send_error("something nobody has seen"),
            SendFailure::Ambiguous
        );
        // A rejection that also mentions a timeout is still definite (the
        // endpoint answered).
        assert_eq!(
            classify_send_error("timeout while rejecting: blockhash expired"),
            SendFailure::Definite
        );
    }

    #[test]
    fn send_error_maps_rpc_classes_to_lifecycle_classes() {
        let f = |class: RpcErrorClass, message: &str| RpcFailure {
            class,
            message: message.into(),
            attempts: 1,
            provider: "0:x".into(),
        };
        let e = SendError::from_rpc(&f(RpcErrorClass::Timeout, "sendTransaction: timed out"));
        assert_eq!(e.failure, SendFailure::Ambiguous);
        assert_eq!(e.class, FailureClass::TransportAmbiguous);

        let e = SendError::from_rpc(&f(RpcErrorClass::Blockhash, "Blockhash not found"));
        assert_eq!(e.failure, SendFailure::Definite);
        assert_eq!(e.class, FailureClass::BlockhashExpired);
        assert!(e.class.is_retryable_with_rebuild());

        let e = SendError::from_rpc(&f(
            RpcErrorClass::RateLimited { retry_after: None },
            "http 429",
        ));
        assert_eq!(e.class, FailureClass::RateLimited);

        let e = SendError::from_rpc(&f(RpcErrorClass::Permanent, "insufficient funds for rent"));
        assert_eq!(e.class, FailureClass::InsufficientFunds);
        let e = SendError::from_rpc(&f(RpcErrorClass::Permanent, "weird node text"));
        assert_eq!(e.class, FailureClass::Rejected, "permanent stays definite");

        let e = SendError::from_rpc(&f(RpcErrorClass::Unavailable, "503 service unavailable"));
        assert_eq!(
            e.failure,
            SendFailure::Ambiguous,
            "a gateway 5xx may follow a forward"
        );

        // Combining: one ambiguous leg makes the whole verdict ambiguous.
        let c = SendError::combine(
            SendError::from_text("blockhash not found".into()),
            SendError::from_text("operation timed out".into()),
        );
        assert_eq!(c.failure, SendFailure::Ambiguous);
        assert!(c.message.contains("primary=") && c.message.contains("failover="));
        let c = SendError::combine(
            SendError::from_text("blockhash not found".into()),
            SendError::from_text("invalid transaction".into()),
        );
        assert_eq!(c.failure, SendFailure::Definite);
        assert_eq!(
            c.class,
            FailureClass::BlockhashExpired,
            "a rebuildable leg wins"
        );
    }

    /// Mock endpoint that accepts TCP connections and drops them without a
    /// response — a transport-level ambiguity (V13/V15: RPC timeout / lost
    /// connection must never be reported as a definite failure).
    async fn spawn_drop_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                // Accepting and immediately dropping the socket yields a
                // "connection closed" transport error client-side.
                let _ = listener.accept().await;
            }
        });
        addr
    }

    /// Mock JSON-RPC endpoint answering every call with an error payload —
    /// a definite rejection (the node processed and refused the request).
    async fn spawn_jsonrpc_error_server(message: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let body = format!(
            "{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-32002,\"message\":\"{message}\"}},\"id\":1}}"
        );
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            }
        });
        addr
    }

    fn live_policy(confirm_timeout: Duration) -> ExecPolicy {
        ExecPolicy {
            mode: ExecutionMode::Live,
            broadcast: BroadcastMode::Rpc,
            simulate_first: false,
            abort_on_simulation_failure: true,
            confirm_timeout,
            confirm_poll_interval: Duration::from_millis(250),
            max_attempts: 1,
            jito_url: None,
            min_priority_fee_micro_lamports: 0,
            fanout: false,
        }
    }

    fn mock_rpc(url: &str) -> Rpc {
        Rpc::with_urls(
            url.to_string(),
            String::new(),
            Vec::new(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
            1,
            Duration::from_secs(2),
        )
        .expect("rpc builds")
    }

    fn self_transfer_req(wallet: &Wallet) -> TxRequest {
        TxRequest {
            instructions: vec![solana_system_interface::instruction::transfer(
                &wallet.pubkey,
                &wallet.pubkey,
                0,
            )],
            label: "recon-classification-test".into(),
            // Pre-seeded blockhash: the builder never touches the network,
            // so the ONLY RPC traffic is the broadcast + confirm under test.
            blockhash: Some(solana_sdk::hash::Hash::new_unique()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn transport_failure_yields_send_unknown_not_send_failed() {
        // V13/V15: RPC timeout / lost connection before an answer is an
        // AMBIGUOUS outcome — the signed tx may still land. The executor must
        // report SendUnknown, carry the signature, and count as "succeeded"
        // so the module persists the claim instead of dropping it.
        let url = spawn_drop_server().await;
        let wallet = Arc::new(Wallet::generate());
        let executor = Executor::new(
            mock_rpc(&url),
            Arc::clone(&wallet),
            live_policy(Duration::from_secs(2)),
        )
        .with_ledger(ExecutionLedger::new(64));
        let r = executor
            .run(self_transfer_req(&wallet))
            .await
            .expect("run returns Ok");
        assert_eq!(r.status, ExecStatus::SendUnknown, "{:?}", r.error);
        assert!(!r.signature.is_empty(), "signature must survive ambiguity");
        assert!(r.succeeded(), "ambiguous sends ride the claim path");
        assert!(r
            .error
            .as_deref()
            .is_some_and(|e| e.contains("may still land")));
        assert_eq!(r.attempts, 1);
        // Lifecycle: parked in Pending with the ambiguous class, never Failed.
        assert_eq!(r.state, ExecutionState::Pending);
        assert_eq!(r.failure, Some(FailureClass::ConfirmationTimeout));
        let rec = executor.ledger().get(&r.intent_id).await.expect("recorded");
        assert_eq!(rec.state, ExecutionState::Pending);
        assert_eq!(rec.signature.as_deref(), Some(r.signature.as_str()));
        assert_eq!(rec.attempts, 1);
    }

    #[tokio::test]
    async fn definite_rejection_yields_send_failed() {
        // V14-adjacent: an explicit node rejection (blockhash) is terminal —
        // no claim needed, and the signature is still recorded for audit.
        let url = spawn_jsonrpc_error_server(
            "Transaction precompile verification failure BlockhashNotFound",
        )
        .await;
        let wallet = Arc::new(Wallet::generate());
        let executor = Executor::new(
            mock_rpc(&url),
            Arc::clone(&wallet),
            live_policy(Duration::from_secs(2)),
        )
        .with_ledger(ExecutionLedger::new(64));
        let r = executor
            .run(self_transfer_req(&wallet))
            .await
            .expect("run returns Ok");
        assert_eq!(r.status, ExecStatus::SendFailed, "{:?}", r.error);
        assert!(!r.signature.is_empty());
        assert!(!r.succeeded());
        assert_eq!(r.failure, Some(FailureClass::BlockhashExpired));
        assert_eq!(
            r.state,
            ExecutionState::Expired,
            "blockhash rejection settles as Expired"
        );
        let rec = executor.ledger().get(&r.intent_id).await.expect("recorded");
        assert!(rec.can_rearm(), "a definite failure may be retried later");
    }

    #[tokio::test]
    async fn duplicate_run_is_refused_while_the_first_attempt_is_pending() {
        // Duplicate-retry injection: the first run parks in Pending
        // (ambiguous). A second run with the SAME pinned intent id must be
        // refused without touching the network.
        let url = spawn_drop_server().await;
        let wallet = Arc::new(Wallet::generate());
        let executor = Executor::new(
            mock_rpc(&url),
            Arc::clone(&wallet),
            live_policy(Duration::from_millis(600)),
        )
        .with_ledger(ExecutionLedger::new(64));
        let mut req = self_transfer_req(&wallet);
        req.intent_id = Some("int_dup_test".into());
        let first = executor.run(req.clone()).await.unwrap();
        assert_eq!(first.status, ExecStatus::SendUnknown);
        assert_eq!(first.intent_id, "int_dup_test");

        let started = Instant::now();
        let second = executor.run(req).await.unwrap();
        assert!(second.is_duplicate(), "{second:?}");
        assert_eq!(second.status, ExecStatus::Skipped);
        assert!(!second.succeeded());
        assert_eq!(
            second.signature, first.signature,
            "points at the live attempt"
        );
        assert_eq!(second.state, ExecutionState::Pending);
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "refused locally"
        );
    }

    #[tokio::test]
    async fn definite_failure_is_rearmed_and_retried_with_escalated_fee() {
        // Stale-blockhash injection with retry budget: attempt 1 is rejected
        // (BlockhashNotFound), the executor re-arms the intent and attempt 2
        // rebuilds — which needs a fresh blockhash from the (rejecting) mock
        // and therefore ends as a transient error. The ledger must show two
        // attempts and the escalated fee.
        let url = spawn_jsonrpc_error_server("Blockhash not found").await;
        let wallet = Arc::new(Wallet::generate());
        let mut policy = live_policy(Duration::from_secs(1));
        policy.max_attempts = 2;
        let executor = Executor::new(mock_rpc(&url), Arc::clone(&wallet), policy)
            .with_ledger(ExecutionLedger::new(64))
            .with_fee_policy(FeePolicy {
                escalation_pct: 100,
                ..FeePolicy::default()
            });
        let mut req = self_transfer_req(&wallet);
        req.priority_fee_micro_lamports = 1_000;
        req.intent_id = Some("int_rearm_test".into());
        let outcome = executor.run(req).await;
        let rec = executor
            .ledger()
            .get("int_rearm_test")
            .await
            .expect("recorded");
        assert_eq!(rec.attempts, 2, "re-armed once: {outcome:?}");
        assert_eq!(
            rec.priority_fee_micro_lamports, 2_000,
            "fee escalated by 100%"
        );
        assert!(
            rec.state == ExecutionState::Failed || rec.state == ExecutionState::Expired,
            "second attempt settled definitively: {rec:?}"
        );
        assert!(!rec.state.is_live());
    }

    #[tokio::test]
    async fn fee_above_emergency_limit_is_vetoed_before_any_network_io() {
        let url = spawn_drop_server().await;
        let wallet = Arc::new(Wallet::generate());
        let executor = Executor::new(
            mock_rpc(&url),
            Arc::clone(&wallet),
            live_policy(Duration::from_secs(1)),
        )
        .with_ledger(ExecutionLedger::new(64))
        .with_fee_policy(FeePolicy {
            max_micro_lamports: 1_000,
            emergency_max_micro_lamports: 2_000,
            ..FeePolicy::default()
        });
        let mut req = self_transfer_req(&wallet);
        req.priority_fee_micro_lamports = 5_000;
        let started = Instant::now();
        let r = executor.run(req).await.unwrap();
        assert_eq!(r.status, ExecStatus::Skipped);
        assert_eq!(r.failure, Some(FailureClass::PolicyVeto));
        assert_eq!(r.state, ExecutionState::Failed);
        assert!(r.signature.is_empty(), "nothing was signed");
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(r.error.as_deref().unwrap_or("").contains("emergency"));
    }

    #[tokio::test]
    async fn paper_mode_records_a_confirmed_lifecycle_without_broadcast() {
        let url = spawn_drop_server().await;
        let wallet = Arc::new(Wallet::generate());
        let mut policy = live_policy(Duration::from_secs(1));
        policy.mode = ExecutionMode::Paper;
        let executor = Executor::new(mock_rpc(&url), Arc::clone(&wallet), policy)
            .with_ledger(ExecutionLedger::new(64));
        let r = executor.run(self_transfer_req(&wallet)).await.unwrap();
        assert_eq!(r.status, ExecStatus::PaperFilled);
        assert!(r.paper && r.broadcast_signature().is_none());
        assert_eq!(r.state, ExecutionState::Confirmed);
        let rec = executor.ledger().get(&r.intent_id).await.unwrap();
        assert_eq!(rec.state, ExecutionState::Confirmed);
    }

    // ------------------------------------------------------------------
    // Scripted mock node: answers per JSON-RPC method so a whole lifecycle
    // (blockhash → send → confirm / expire) can be driven offline.
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct MockNode {
        /// Blockhash / lastValidBlockHeight returned by getLatestBlockhash,
        /// popped front-to-back (the last one repeats).
        blockhashes: std::sync::Mutex<std::collections::VecDeque<(Hash, u64)>>,
        /// getBlockHeight answer.
        block_height: std::sync::atomic::AtomicU64,
        /// simulateTransaction: `Some(err)` = rejected with this error.
        simulate_error: std::sync::Mutex<Option<String>>,
        /// getTransaction: `true` = report the last sent tx as confirmed.
        confirm: std::sync::atomic::AtomicBool,
        sends: std::sync::atomic::AtomicUsize,
        simulations: std::sync::atomic::AtomicUsize,
        last_sent_b64: std::sync::Mutex<Option<String>>,
    }

    impl MockNode {
        fn respond(&self, req: &serde_json::Value) -> serde_json::Value {
            let method = req["method"].as_str().unwrap_or_default();
            match method {
                "getLatestBlockhash" => {
                    let mut q = self.blockhashes.lock().unwrap();
                    let (hash, lvbh) = if q.len() > 1 {
                        q.pop_front().unwrap()
                    } else {
                        q.front()
                            .copied()
                            .unwrap_or((Hash::new_unique(), u64::MAX / 2))
                    };
                    json!({"context": {"slot": 1}, "value": {"blockhash": hash.to_string(), "lastValidBlockHeight": lvbh}})
                }
                "isBlockhashValid" => json!({"context": {"slot": 1}, "value": true}),
                "getBlockHeight" => json!(self.block_height.load(Ordering::SeqCst)),
                "simulateTransaction" => {
                    self.simulations.fetch_add(1, Ordering::SeqCst);
                    match self.simulate_error.lock().unwrap().clone() {
                        Some(err) => json!({"context": {"slot": 1}, "value": {
                            "err": {"InstructionError": [0, {"Custom": 6001}]},
                            "logs": ["Program log: Instruction: Buy", format!("Program log: {err}")],
                            "unitsConsumed": 1200
                        }}),
                        None => {
                            json!({"context": {"slot": 1}, "value": {"err": null, "logs": ["Program log: ok"], "unitsConsumed": 1200}})
                        }
                    }
                }
                "sendTransaction" => {
                    self.sends.fetch_add(1, Ordering::SeqCst);
                    let b64 = req["params"][0].as_str().unwrap_or_default().to_string();
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&b64)
                        .unwrap_or_default();
                    // Wire format: compact-u16 signature count (1 byte for
                    // one signer) followed by the 64-byte signature.
                    let sig = bytes.get(1..65).map(bs58::encode).map(|e| e.into_string());
                    *self.last_sent_b64.lock().unwrap() = Some(b64);
                    json!(sig.unwrap_or_default())
                }
                "getTransaction" => {
                    if self.confirm.load(Ordering::SeqCst) {
                        match self.last_sent_b64.lock().unwrap().clone() {
                            // `EncodedConfirmedTransactionWithStatusMeta`
                            // flattens the inner struct: `transaction` and
                            // `meta` sit next to `slot`.
                            Some(b64) => json!({
                                "slot": 42,
                                "blockTime": null,
                                "transaction": [b64, "base64"],
                                "meta": {
                                    "err": null, "status": {"Ok": null}, "fee": 5000,
                                    "preBalances": [], "postBalances": [],
                                    "innerInstructions": [], "logMessages": ["Program log: landed"],
                                    "preTokenBalances": [], "postTokenBalances": [], "rewards": []
                                }
                            }),
                            None => serde_json::Value::Null,
                        }
                    } else {
                        serde_json::Value::Null
                    }
                }
                _ => json!(null),
            }
        }
    }

    async fn spawn_mock_node(node: Arc<MockNode>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let node = node.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let mut n = 0usize;
                    // Read until the JSON body is complete.
                    loop {
                        let Ok(r) = sock.read(&mut buf[n..]).await else {
                            return;
                        };
                        if r == 0 {
                            break;
                        }
                        n += r;
                        let text = String::from_utf8_lossy(&buf[..n]);
                        if let Some(idx) = text.find("\r\n\r\n") {
                            let body = &text[idx + 4..];
                            let want = text
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if body.len() >= want {
                                break;
                            }
                        }
                        if n == buf.len() {
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let body = text.split("\r\n\r\n").nth(1).unwrap_or("{}");
                    let req: serde_json::Value = serde_json::from_str(body).unwrap_or(json!({}));
                    let id = req.get("id").cloned().unwrap_or(json!(1));
                    let result = node.respond(&req);
                    let out = json!({"jsonrpc": "2.0", "result": result, "id": id}).to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                        out.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        addr
    }

    /// A request WITHOUT a pinned blockhash so the builder fetches one from
    /// the mock (and records its expiry height).
    fn unpinned_transfer_req(wallet: &Wallet) -> TxRequest {
        TxRequest {
            instructions: vec![solana_system_interface::instruction::transfer(
                &wallet.pubkey,
                &wallet.pubkey,
                0,
            )],
            label: "lifecycle-test".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn failed_simulation_aborts_before_any_broadcast() {
        // Failed-simulation injection: the node rejects the preflight; the
        // executor must settle the intent as Failed(SimulationRejected) and
        // never call sendTransaction.
        let node = Arc::new(MockNode::default());
        *node.simulate_error.lock().unwrap() = Some("slippage exceeded".into());
        let url = spawn_mock_node(node.clone()).await;
        let wallet = Arc::new(Wallet::generate());
        let mut policy = live_policy(Duration::from_secs(1));
        policy.simulate_first = true;
        policy.abort_on_simulation_failure = true;
        let executor = Executor::new(mock_rpc(&url), Arc::clone(&wallet), policy)
            .with_ledger(ExecutionLedger::new(64));
        let r = executor.run(unpinned_transfer_req(&wallet)).await.unwrap();
        assert_eq!(r.status, ExecStatus::SimulationFailed, "{:?}", r.error);
        assert_eq!(r.failure, Some(FailureClass::SimulationRejected));
        assert_eq!(r.state, ExecutionState::Failed);
        assert!(r.logs.iter().any(|l| l.contains("slippage exceeded")));
        assert_eq!(node.simulations.load(Ordering::SeqCst), 1);
        assert_eq!(
            node.sends.load(Ordering::SeqCst),
            0,
            "nothing was broadcast"
        );
        assert!(r.broadcast_signature().is_none());
        let rec = executor.ledger().get(&r.intent_id).await.unwrap();
        assert_eq!(rec.state, ExecutionState::Failed);
        assert!(rec.can_rearm());
    }

    #[tokio::test]
    async fn confirmation_timeout_parks_the_intent_in_pending() {
        // Confirmation-timeout injection: the node accepts the transaction
        // but never reports it (getTransaction → null) and the block height
        // stays inside the validity window. After `confirm_timeout` the
        // outcome is ambiguous: `Sent`, Pending, ConfirmationTimeout.
        let node = Arc::new(MockNode::default());
        node.blockhashes
            .lock()
            .unwrap()
            .push_back((Hash::new_unique(), 10_000));
        node.block_height.store(100, Ordering::SeqCst);
        let url = spawn_mock_node(node.clone()).await;
        let wallet = Arc::new(Wallet::generate());
        let mut policy = live_policy(Duration::from_millis(700));
        policy.confirm_poll_interval = Duration::from_millis(50);
        let executor = Executor::new(mock_rpc(&url), Arc::clone(&wallet), policy)
            .with_ledger(ExecutionLedger::new(64));
        let started = Instant::now();
        let r = executor.run(unpinned_transfer_req(&wallet)).await.unwrap();
        assert_eq!(r.status, ExecStatus::Sent, "{:?}", r.error);
        assert_eq!(r.failure, Some(FailureClass::ConfirmationTimeout));
        assert_eq!(r.state, ExecutionState::Pending);
        assert!(r.succeeded(), "ambiguous → claim path");
        assert!(r.confirm_ms.is_some_and(|ms| ms >= 600));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(node.sends.load(Ordering::SeqCst), 1);
        let rec = executor.ledger().get(&r.intent_id).await.unwrap();
        assert_eq!(rec.state, ExecutionState::Pending);
        assert_eq!(rec.last_valid_block_height, Some(10_000));
        assert!(rec.state.is_live(), "stays live for reconciliation / dedup");
    }

    #[tokio::test]
    async fn stale_blockhash_is_detected_and_rebuilt_with_a_fresh_one() {
        // Stale-blockhash injection at the executor level: the first
        // blockhash expires at height 100 while the cluster is already at
        // 150, and the transaction never appears — `confirm_tracked` reports
        // Expired, the executor re-arms and rebuilds with the SECOND
        // blockhash (valid until 1_000_000), which the node then confirms.
        let node = Arc::new(MockNode::default());
        {
            let mut q = node.blockhashes.lock().unwrap();
            q.push_back((Hash::new_unique(), 100));
            q.push_back((Hash::new_unique(), 1_000_000));
        }
        node.block_height.store(150, Ordering::SeqCst);
        let url = spawn_mock_node(node.clone()).await;
        let wallet = Arc::new(Wallet::generate());
        let mut policy = live_policy(Duration::from_secs(5));
        policy.confirm_poll_interval = Duration::from_millis(50);
        policy.max_attempts = 2;
        let executor = Executor::new(mock_rpc(&url), Arc::clone(&wallet), policy)
            .with_ledger(ExecutionLedger::new(64));
        // Flip the node to "confirmed" once the second send has happened.
        let flip = node.clone();
        tokio::spawn(async move {
            loop {
                if flip.sends.load(Ordering::SeqCst) >= 2 {
                    flip.confirm.store(true, Ordering::SeqCst);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let mut req = unpinned_transfer_req(&wallet);
        req.intent_id = Some("int_stale_rebuild".into());
        let started = Instant::now();
        let r = executor.run(req).await.unwrap();
        assert_eq!(r.status, ExecStatus::Confirmed, "{:?}", r.error);
        assert_eq!(r.state, ExecutionState::Confirmed);
        assert_eq!(r.attempts, 2, "one rebuild");
        assert_eq!(node.sends.load(Ordering::SeqCst), 2);
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "expiry must short-circuit the 5 s confirmation timeout"
        );
        let rec = executor.ledger().get("int_stale_rebuild").await.unwrap();
        assert_eq!(rec.state, ExecutionState::Confirmed);
        assert_eq!(rec.attempts, 2);
        assert_eq!(
            rec.last_valid_block_height,
            Some(1_000_000),
            "rebuilt on the fresh blockhash"
        );
        assert_eq!(rec.signature.as_deref(), Some(r.signature.as_str()));
        assert!(
            metrics::global()
                .counter(
                    "bot_execution_rebuilds_total",
                    "",
                    &[("class", "blockhash_expired")]
                )
                .get()
                >= 1
        );
    }

    #[tokio::test]
    async fn derived_intent_ids_are_stable_for_identical_requests() {
        let url = spawn_drop_server().await;
        let wallet = Arc::new(Wallet::generate());
        let executor = Executor::new(
            mock_rpc(&url),
            Arc::clone(&wallet),
            live_policy(Duration::from_secs(1)),
        )
        .with_ledger(ExecutionLedger::new(64));
        let a = executor.resolve_intent(&self_transfer_req(&wallet)).await;
        let b = executor.resolve_intent(&self_transfer_req(&wallet)).await;
        assert_eq!(a.intent_id, b.intent_id, "same wallet + label + content");
        assert!(a.intent_id.starts_with("int_"));
        assert_eq!(a.module, "solana");
        let mut other = self_transfer_req(&wallet);
        other.label = "other".into();
        let c = executor.resolve_intent(&other).await;
        assert_ne!(c.intent_id, a.intent_id);
        let mut pinned = self_transfer_req(&wallet);
        pinned.intent_id = Some("  int_pinned ".into());
        assert_eq!(
            executor.resolve_intent(&pinned).await.intent_id,
            "int_pinned"
        );
    }
}
