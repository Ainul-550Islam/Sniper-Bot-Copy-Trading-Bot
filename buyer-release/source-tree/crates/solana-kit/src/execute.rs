//! Execution pipeline: simulate → broadcast → confirm, with paper-trading,
//! Jito bundles and latency accounting.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_sdk::hash::Hash;
use solana_sdk::signature::Signature;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::maths;
use bot_core::models::ExecutionMode;

use crate::consts::JITO_BUNDLE_PATH;
use crate::rpc::{ConfirmOutcome, Rpc};
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
}

impl ExecutionResult {
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
    /// nothing was ever broadcast (simulation reject / build failure).
    pub fn broadcast_signature(&self) -> Option<String> {
        (!self.paper && !self.signature.is_empty()).then(|| self.signature.clone())
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
}

impl Executor {
    pub fn new(rpc: Rpc, wallet: Arc<Wallet>, policy: ExecPolicy) -> Self {
        Executor {
            rpc,
            wallet,
            policy,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            signers: None,
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

    /// Build, (simulate), broadcast and confirm.
    pub async fn run(&self, req: TxRequest) -> BotResult<ExecutionResult> {
        let started = Instant::now();
        let label = req.label.clone();

        if req.priority_fee_micro_lamports < self.policy.min_priority_fee_micro_lamports {
            return Ok(ExecutionResult {
                signature: String::new(),
                status: ExecStatus::Skipped,
                label,
                total_ms: started.elapsed().as_millis() as u64,
                simulate_ms: None,
                send_ms: None,
                confirm_ms: None,
                tx_size: 0,
                logs: Vec::new(),
                error: Some(format!(
                    "priority fee {} micro-lamports is below the configured minimum {}",
                    req.priority_fee_micro_lamports, self.policy.min_priority_fee_micro_lamports
                )),
                paper: self.is_paper(),
                attempts: 0,
            });
        }

        let mut last_error: Option<String> = None;
        for attempt in 1..=self.policy.max_attempts.max(1) {
            match self.run_once(&req, &started, attempt).await {
                Ok(result) => {
                    // A stale/invalid blockhash is the only transient failure
                    // worth rebuilding for.
                    if matches!(result.status, ExecStatus::SendFailed)
                        && result
                            .error
                            .as_deref()
                            .map(|e| e.contains("blockhash"))
                            .unwrap_or(false)
                        && attempt < self.policy.max_attempts
                    {
                        warn!(label = %req.label, attempt, "blockhash expired, rebuilding");
                        self.rpc.invalidate_blockhash().await;
                        last_error = result.error;
                        continue;
                    }
                    return Ok(result);
                }
                Err(e) => {
                    let msg = e.to_string();
                    let transient = msg.contains("blockhash")
                        || msg.contains("timeout")
                        || msg.contains("TooManyRequests")
                        || msg.contains("429");
                    if transient && attempt < self.policy.max_attempts {
                        warn!(label = %req.label, attempt, error = %e, "transient failure, retrying");
                        self.rpc.invalidate_blockhash().await;
                        last_error = Some(msg);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Ok(ExecutionResult {
            signature: String::new(),
            status: ExecStatus::SendFailed,
            label,
            total_ms: started.elapsed().as_millis() as u64,
            simulate_ms: None,
            send_ms: None,
            confirm_ms: None,
            tx_size: 0,
            logs: Vec::new(),
            error: Some(last_error.unwrap_or_else(|| "all attempts failed".into())),
            paper: self.is_paper(),
            attempts: self.policy.max_attempts.max(1),
        })
    }

    async fn run_once(
        &self,
        req: &TxRequest,
        started: &Instant,
        attempt: u8,
    ) -> BotResult<ExecutionResult> {
        let builder = self.builder();
        let built = builder.build(req).await?;

        // ---- simulate ------------------------------------------------------
        let mut simulate_ms = None;
        if self.policy.simulate_first {
            let t = Instant::now();
            match self.simulate(&built).await {
                Ok(sim) => {
                    simulate_ms = Some(t.elapsed().as_millis() as u64);
                    if let Some(err) = sim.error {
                        warn!(
                            label = %req.label,
                            error = %err,
                            logs = ?&sim.logs[sim.logs.len().saturating_sub(6)..],
                            "simulation failed"
                        );
                        if self.policy.abort_on_simulation_failure {
                            return Ok(ExecutionResult {
                                signature: built.signature().to_string(),
                                status: ExecStatus::SimulationFailed,
                                label: req.label.clone(),
                                total_ms: started.elapsed().as_millis() as u64,
                                simulate_ms,
                                send_ms: None,
                                confirm_ms: None,
                                tx_size: built.size,
                                logs: sim.logs,
                                error: Some(err),
                                paper: self.is_paper(),
                                attempts: attempt,
                            });
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
        if self.is_paper() {
            info!(
                label = %req.label,
                size = built.size,
                attempt,
                "paper mode — transaction built but not broadcast"
            );
            return Ok(ExecutionResult {
                signature: built.signature().to_string(),
                status: ExecStatus::PaperFilled,
                label: req.label.clone(),
                total_ms: started.elapsed().as_millis() as u64,
                simulate_ms,
                send_ms: None,
                confirm_ms: None,
                tx_size: built.size,
                logs: Vec::new(),
                error: None,
                paper: true,
                attempts: 1,
            });
        }

        // ---- broadcast -----------------------------------------------------
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

        // Classify broadcast failures BEFORE acting on them: a definite
        // rejection can never land (safe terminal failure), while an
        // ambiguous transport failure means the signed tx may still reach a
        // leader — for those we fall through to confirmation so the outcome
        // is resolved from the chain, never assumed.
        let mut send_was_ambiguous = false;
        let signature = match send_outcome {
            Ok(sig) => sig,
            Err(e) => match classify_send_error(&e.to_string()) {
                SendFailure::Definite => {
                    return Ok(ExecutionResult {
                        signature: built.signature().to_string(),
                        status: ExecStatus::SendFailed,
                        label: req.label.clone(),
                        total_ms: started.elapsed().as_millis() as u64,
                        simulate_ms,
                        send_ms: Some(send_ms),
                        confirm_ms: None,
                        tx_size: built.size,
                        logs: Vec::new(),
                        error: Some(e.to_string()),
                        paper: false,
                        attempts: 1,
                    });
                }
                SendFailure::Ambiguous => {
                    warn!(
                        label = %req.label,
                        error = %e,
                        signature = %built.signature(),
                        "broadcast outcome unknown (transport failure); \
                         confirming on-chain instead of assuming failure"
                    );
                    send_was_ambiguous = true;
                    built.signature()
                }
            },
        };

        // ---- confirm -------------------------------------------------------
        let t = Instant::now();
        let outcome = self
            .rpc
            .confirm(
                &signature,
                self.policy.confirm_timeout,
                self.policy.confirm_poll_interval,
            )
            .await?;
        let confirm_ms = t.elapsed().as_millis() as u64;

        let (status, error, logs) = match outcome {
            ConfirmOutcome::Confirmed { logs, .. } => (ExecStatus::Confirmed, None, logs),
            ConfirmOutcome::Timeout => {
                if send_was_ambiguous {
                    (
                        ExecStatus::SendUnknown,
                        Some(
                            "broadcast failed ambiguously and confirmation timed out; \
                             the transaction may still land — reconciliation will resolve it"
                                .into(),
                        ),
                        Vec::new(),
                    )
                } else {
                    (
                        ExecStatus::Sent,
                        Some("confirmation timed out; the transaction may still land".into()),
                        Vec::new(),
                    )
                }
            }
            ConfirmOutcome::Failed { error, logs } => (ExecStatus::LandedFailed, Some(error), logs),
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

        Ok(ExecutionResult {
            signature: signature.to_string(),
            status,
            label: req.label.clone(),
            total_ms,
            simulate_ms,
            send_ms: Some(send_ms),
            confirm_ms: Some(confirm_ms),
            tx_size: built.size,
            logs,
            error,
            paper: false,
            attempts: 1,
        })
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

    async fn broadcast_rpc(&self, built: &BuiltTx) -> BotResult<Signature> {
        if self.policy.fanout {
            return self.broadcast_fanout(built).await;
        }
        match self.rpc.send_transaction(&built.tx).await {
            Ok(sig) => Ok(sig),
            Err(primary) => {
                // Try the failover endpoint once before giving up.
                match self.rpc.failover() {
                    Some(failover) => {
                        warn!(
                            label = %built.label,
                            error = %primary,
                            failover = %failover.url(),
                            "primary RPC rejected, retrying on failover"
                        );
                        failover.send_transaction(&built.tx).await.map_err(|e| {
                            BotError::solana(format!(
                                "send failed on both endpoints: primary={primary}, failover={e}"
                            ))
                        })
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
    async fn broadcast_fanout(&self, built: &BuiltTx) -> BotResult<Signature> {
        use futures_util::stream::{FuturesUnordered, StreamExt};

        // Primary + the whole failover chain.
        let mut endpoints = vec![self.rpc.clone()];
        let mut cursor = self.rpc.clone();
        while let Some(fo) = cursor.failover() {
            cursor = fo.clone();
            endpoints.push(fo);
        }

        let reg = bot_core::obs::metrics::global();
        let mut sends = FuturesUnordered::new();
        for ep in endpoints {
            let tx = built.tx.clone();
            let label = built.label.clone();
            sends.push(async move {
                let url = ep.url().to_string();
                let started = std::time::Instant::now();
                let res = ep.send_transaction(&tx).await;
                (url, label, started.elapsed().as_millis() as u64, res)
            });
        }

        let total = sends.len();
        let mut last_err: Option<BotError> = None;
        let mut accepted: Option<Signature> = None;
        while let Some((url, label, ms, res)) = sends.next().await {
            match res {
                Ok(sig) => {
                    debug!(%url, %label, %sig, ms, endpoints = total, "fanout: accepted");
                    accepted = Some(sig);
                    break; // remaining futures are dropped (in-flight dupes are fine)
                }
                Err(e) => {
                    warn!(%url, %label, error = %e, "fanout: endpoint rejected");
                    last_err = Some(e);
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
                Err(last_err.unwrap_or_else(|| {
                    BotError::solana("fanout: no endpoints configured".to_string())
                }))
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
    async fn broadcast_jito(&self, built: &BuiltTx, tip_lamports: u64) -> BotResult<Signature> {
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
        let blockhash = self.rpc.latest_blockhash(true).await?.blockhash;
        let mut req = req.clone();
        req.blockhash = Some(blockhash);
        let builder = self.builder();
        builder.build(&req).await
    }

    /// Send an already-signed transaction (used by the pre-signed sniper path,
    /// where the transaction was built *before* the launch was detected).
    pub async fn send_prebuilt(&self, built: &BuiltTx) -> BotResult<ExecutionResult> {
        let started = Instant::now();
        if self.is_paper() {
            return Ok(ExecutionResult {
                signature: built.signature().to_string(),
                status: ExecStatus::PaperFilled,
                label: built.label.clone(),
                total_ms: started.elapsed().as_millis() as u64,
                simulate_ms: None,
                send_ms: None,
                confirm_ms: None,
                tx_size: built.size,
                logs: Vec::new(),
                error: None,
                paper: true,
                attempts: 1,
            });
        }

        // A pre-signed transaction can go stale while it sits in the queue.
        let valid = self
            .rpc
            .is_blockhash_valid(&built.blockhash)
            .await
            .unwrap_or(true);
        if !valid {
            return Ok(ExecutionResult {
                signature: built.signature().to_string(),
                status: ExecStatus::SendFailed,
                label: built.label.clone(),
                total_ms: started.elapsed().as_millis() as u64,
                simulate_ms: None,
                send_ms: None,
                confirm_ms: None,
                tx_size: built.size,
                logs: Vec::new(),
                error: Some("prebuilt transaction blockhash expired before it was sent".into()),
                paper: false,
                attempts: 1,
            });
        }

        let t = Instant::now();
        let send = match self.policy.broadcast {
            BroadcastMode::Rpc => self.broadcast_rpc(built).await,
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

        let signature = match send {
            Ok(sig) => sig,
            Err(e) => {
                return Ok(ExecutionResult {
                    signature: built.signature().to_string(),
                    status: ExecStatus::SendFailed,
                    label: built.label.clone(),
                    total_ms: started.elapsed().as_millis() as u64,
                    simulate_ms: None,
                    send_ms: Some(send_ms),
                    confirm_ms: None,
                    tx_size: built.size,
                    logs: Vec::new(),
                    error: Some(e.to_string()),
                    paper: false,
                    attempts: 1,
                })
            }
        };

        let t = Instant::now();
        let outcome = self
            .rpc
            .confirm(
                &signature,
                self.policy.confirm_timeout,
                self.policy.confirm_poll_interval,
            )
            .await?;
        let confirm_ms = t.elapsed().as_millis() as u64;
        let (status, error, logs) = match outcome {
            ConfirmOutcome::Confirmed { logs, .. } => (ExecStatus::Confirmed, None, logs),
            ConfirmOutcome::Timeout => (
                ExecStatus::Sent,
                Some("confirmation timed out".into()),
                Vec::new(),
            ),
            ConfirmOutcome::Failed { error, logs } => (ExecStatus::LandedFailed, Some(error), logs),
        };

        Ok(ExecutionResult {
            signature: signature.to_string(),
            status,
            label: built.label.clone(),
            total_ms: started.elapsed().as_millis() as u64,
            simulate_ms: None,
            send_ms: Some(send_ms),
            confirm_ms: Some(confirm_ms),
            tx_size: built.size,
            logs,
            error,
            paper: false,
            attempts: 1,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            };
            assert!(!r.succeeded(), "{status:?} must not count as success");
        }
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
            label: "snipe".into(),
            instructions: vec![],
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
            label: "snipe".into(),
            instructions: vec![],
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
            label: "x".into(),
            instructions: vec![],
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

    use solana_sdk::pubkey::Pubkey;

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
        );
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
        );
        let r = executor
            .run(self_transfer_req(&wallet))
            .await
            .expect("run returns Ok");
        assert_eq!(r.status, ExecStatus::SendFailed, "{:?}", r.error);
        assert!(!r.signature.is_empty());
        assert!(!r.succeeded());
    }
}
