//! Distributed execution ownership (Prompt 3 §B/§D/§E/§K/§M/§N/§V).
//!
//! # The invariant
//!
//! ONE logical execution intent → AT MOST ONE ACTIVE OWNER → AT MOST ONE
//! money-moving submission, until authoritative external state proves what
//! happened. This module is the ownership half; the intent journal
//! ([`crate::recovery::with_intent`]) and the reconciliation queue
//! ([`crate::recovery`]) are the other two thirds.
//!
//! # Claim lifecycle (exact state machine)
//!
//! ```text
//!                claim(execution_id) — ATOMIC
//!   (absent) ────────────────────────────────► CLAIMED(epoch=1)
//!                                                   │  renew (owner+epoch CAS,
//!                                                   │         lease/3 cadence,
//!                                                   │         bounded count)
//!              ┌────────────────────────────────────┤
//!              │                                    │
//!     release()│ clean terminal                hand_off()│ ambiguous outcome
//!     (Confirmed/Failed/paper/no-broadcast)    (SendUnknown — reconciliation
//!              ▼                                owns the outcome now)
//!          RELEASED ──► re-acquirable              ▼
//!          (epoch+1, new owner allowed)        HANDED_OFF ──► re-acquirable
//!                                              ONLY after handoff_grace
//!   CLAIMED with lease_until < now  ══► EXPIRED (implicit)
//!          └─► takeover: CLAIMED(epoch+1, takeover_count+1,
//!                        previous_owner recorded) — the stale owner is
//!                        FENCED: every renew/verify/release CAS on
//!                        (owner_id, epoch) and fails for it.
//! ```
//!
//! Rows are NEVER deleted (auditability, §M/§S): the claim record answers
//! "which replica believed it owned this execution, when did it acquire,
//! when did it lose, who took over".
//!
//! # Fencing (§E)
//!
//! `epoch` is the fencing token. `verify`/`renew`/`release` are
//! compare-and-set on `(execution_id, owner_id, epoch, status='claimed')`:
//! a replica whose lease expired and was taken over holds a STALE epoch and
//! is rejected — [`ClaimGuard::fence`] turns that into
//! [`crate::error::BotError::ClaimRejected`] BEFORE any journal write or
//! broadcast. Fencing is lease-based (best effort at the instant of the
//! check, like all lease systems without a fenced storage layer); the
//! hard guarantees come from the combination: claim-before-broadcast +
//! intent journal + reconciliation (§I ordering: CLAIM → fence → intent →
//! broadcast → link → release/hand-off).
//!
//! # Store authority (§D/§K/§L)
//!
//! * **Postgres** (`execution_claims`, migration 0009) is the AUTHORITATIVE
//!   claim store whenever `[database]` is enabled: one atomic
//!   INSERT … ON CONFLICT … WHERE (expired|released|grace-passed) with
//!   transactional guarantees; durable audit history.
//! * **Redis** (Lua scripts over `own:claim:{execution_id}` hashes) is used
//!   only when Postgres is absent — short-lived coordination per the
//!   existing durability rule (Redis is never the sole source of truth for
//!   money STATE; a claim is a lease, not money state, and its loss on
//!   Redis restart degrades to "expired → takeover → reconcile", never to
//!   double broadcast without reconciliation).
//! * **Memory** is process-local: single-instance/paper deployments and
//!   tests. NEVER an HA safety mechanism (§K) — the server logs loudly when
//!   live trading runs on it.
//!
//! Fail-closed (§K/§L): every store method returns `Err` when the backend
//! cannot be reached or answers ambiguously; [`OwnershipRegistry::claim`]
//! propagates it and money paths MUST abort on `Err` — "could not acquire
//! ownership" is never "ownership acquired". There is no silent fallback to
//! process-local locking.
//!
//! # Renewal (§N)
//!
//! [`ClaimGuard::run_guarded`] races the work future against a renewal
//! ticker (lease/3, ± jitter): bounded (`MAX_RENEWALS`) so a hung worker
//! eventually loses its lease and gets fenced instead of holding ownership
//! forever; renewal stops at terminal states and on shutdown (the guard is
//! dropped/completed). A missed renewal is metered
//! (`bot_distributed_claim_renewal_failed_total`); after the lease lapses
//! the next [`ClaimGuard::fence`] fails closed.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::error::{BotError, BotResult};
use crate::obs::metrics;

/// Upper bound on lease renewals for ONE claim (no infinite renewal, §N).
/// With the default 45 s lease renewed every lease/3 this is ~10 minutes of
/// continuous work; beyond that the lease lapses, a takeover becomes
/// possible and the stale owner is fenced — the correct outcome for a hung
/// worker.
pub const MAX_RENEWALS: u32 = 40;

/// What is being claimed. `execution_id` is the LOGICAL identity, stable
/// across replicas for the same intent (e.g. `snipe:<mint>`,
/// `copy:<wallet>:<mint>`, `exit:<position_id>:<rule>`,
/// `poly:entry:<token_id>`, `flatten:<module>`). Never a wallet address or
/// transaction signature alone (§B).
#[derive(Debug, Clone)]
pub struct ClaimRequest {
    pub execution_id: String,
    /// Low-cardinality class: `entry` | `exit` | `poly_entry` | `flatten`.
    pub kind: String,
    pub module: String,
    pub strategy: String,
    pub symbol: String,
}

/// One ownership record as held by a store.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionClaim {
    pub execution_id: String,
    pub kind: String,
    pub module: String,
    pub strategy: String,
    pub symbol: String,
    pub owner_id: String,
    /// Fencing token: increments on every takeover/re-acquisition.
    pub epoch: i64,
    pub status: ClaimStatus,
    pub acquired_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub takeover_count: i64,
    pub previous_owner: Option<String>,
}

/// Claim record states (exact machine documented in the module header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Actively owned; lease running.
    Claimed,
    /// Work finished with a DETERMINATE outcome (confirmed, failed, paper,
    /// provably-never-broadcast). Re-acquirable immediately (a re-fired
    /// exit rule is a NEW decision on the same logical id).
    Released,
    /// Work ended AMBIGUOUSLY (e.g. `SendUnknown`): reconciliation owns the
    /// outcome. Re-acquirable only after the handoff grace — this is what
    /// prevents a second replica from resubmitting a possibly-in-flight
    /// transaction (§I case E).
    HandedOff,
}

impl ClaimStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClaimStatus::Claimed => "claimed",
            ClaimStatus::Released => "released",
            ClaimStatus::HandedOff => "handed_off",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claimed" => Some(ClaimStatus::Claimed),
            "released" => Some(ClaimStatus::Released),
            "handed_off" => Some(ClaimStatus::HandedOff),
            _ => None,
        }
    }
}

/// Result of an atomic claim attempt.
#[derive(Debug, Clone)]
pub enum ClaimDecision {
    /// This replica is now the sole active owner.
    Acquired(ExecutionClaim),
    /// Someone else holds it (active lease, or handoff grace running).
    /// Deterministic loser outcome (§G): the caller must NOT broadcast.
    Rejected {
        owner_id: String,
        epoch: i64,
        status: ClaimStatus,
        lease_until: DateTime<Utc>,
    },
}

/// Durable-store abstraction for execution ownership (§D/§V). Business
/// modules never touch raw Redis keys or SQL — only
/// [`OwnershipRegistry`]/[`ClaimGuard`].
#[async_trait]
pub trait ClaimStore: Send + Sync {
    /// `postgres` | `redis` | `memory` (metrics/logs; low cardinality).
    fn backend(&self) -> &'static str;

    /// Atomic acquisition: fresh insert, expired-lease takeover, released
    /// re-acquisition, or post-grace handoff re-acquisition — never two
    /// owners at once. `Err` = could not establish ownership (fail closed).
    async fn claim(&self, req: &ClaimRequest, ctx: &StoreContext) -> BotResult<ClaimDecision>;

    /// Extend the lease. CAS on (owner, epoch, status=claimed): `Ok(false)`
    /// means ownership was LOST (fenced/expired/taken over).
    async fn renew(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool>;

    /// Fencing check: does (owner, epoch) still hold an unexpired claim?
    async fn verify(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool>;

    /// Terminal transition (CAS on owner+epoch+claimed). `false` = not
    /// owner anymore (already fenced) — callers log, never retry blindly.
    async fn release(
        &self,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
        mode: ClaimStatus,
    ) -> BotResult<bool>;

    /// Current record, if any (operator/tooling introspection).
    async fn get(&self, execution_id: &str) -> BotResult<Option<ExecutionClaim>>;
}

/// Store-independent parameters of one claim attempt.
#[derive(Debug, Clone)]
pub struct StoreContext {
    pub owner_id: String,
    pub lease: Duration,
    pub handoff_grace: Duration,
}

/// The domain facade (§V). Cheap to clone; modules hold `Option<Arc<_>>`
/// exactly like the intent sink.
#[derive(Clone)]
pub struct OwnershipRegistry {
    store: Arc<dyn ClaimStore>,
    replica_id: String,
    lease: Duration,
    handoff_grace: Duration,
}

impl OwnershipRegistry {
    pub fn new(
        store: Arc<dyn ClaimStore>,
        replica_id: impl Into<String>,
        lease: Duration,
        handoff_grace: Duration,
    ) -> Self {
        OwnershipRegistry {
            store,
            replica_id: replica_id.into(),
            // Production floors live in HaConfig (config.rs); this clamp only
            // guards against a zero/negative Duration breaking renewal math,
            // and deliberately allows sub-second leases for deterministic
            // tests (no sleeps).
            lease: lease.max(Duration::from_millis(10)),
            handoff_grace: handoff_grace.max(Duration::from_secs(1)),
        }
    }

    pub fn replica_id(&self) -> &str {
        &self.replica_id
    }

    pub fn backend(&self) -> &'static str {
        self.store.backend()
    }

    /// Attempt to become the sole active owner of one logical execution.
    ///
    /// * `Ok(Owned(guard))` — proceed; call [`ClaimGuard::fence`] before
    ///   every money-moving continuation and `complete/release/hand_off`
    ///   at the end.
    /// * `Ok(OwnedByOther{..})` — deterministic loser (§G): skip, never
    ///   broadcast.
    /// * `Err(OwnershipUnavailable)` — FAIL CLOSED (§K): do not trade.
    pub async fn claim(
        &self,
        execution_id: impl Into<String>,
        kind: &str,
        module: &str,
        strategy: &str,
        symbol: &str,
    ) -> BotResult<ClaimOutcome> {
        let execution_id = execution_id.into();
        let req = ClaimRequest {
            execution_id: execution_id.clone(),
            kind: kind.to_string(),
            module: module.to_string(),
            strategy: strategy.to_string(),
            symbol: symbol.to_string(),
        };
        let ctx = StoreContext {
            owner_id: self.replica_id.clone(),
            lease: self.lease,
            handoff_grace: self.handoff_grace,
        };
        match self.store.claim(&req, &ctx).await {
            Ok(ClaimDecision::Acquired(claim)) => {
                meter_claim("acquired", module, kind, "");
                info!(
                    execution_id = %execution_id,
                    kind,
                    module,
                    symbol,
                    replica = %self.replica_id,
                    epoch = claim.epoch,
                    lease_until = %claim.lease_until,
                    takeover_count = claim.takeover_count,
                    previous_owner = claim.previous_owner.as_deref().unwrap_or(""),
                    backend = self.store.backend(),
                    "execution claim ACQUIRED"
                );
                Ok(ClaimOutcome::Owned(Box::new(ClaimGuard {
                    store: Arc::clone(&self.store),
                    claim,
                    lease: self.lease,
                    renewals: 0,
                    done: false,
                })))
            }
            Ok(ClaimDecision::Rejected {
                owner_id,
                epoch,
                status,
                lease_until,
            }) => {
                meter_claim("rejected", module, kind, "owned_by_other");
                debug!(
                    execution_id = %execution_id,
                    kind,
                    module,
                    symbol,
                    replica = %self.replica_id,
                    owner = %owner_id,
                    epoch,
                    status = status.as_str(),
                    "execution claim rejected — owned by another replica; NOT broadcasting"
                );
                Ok(ClaimOutcome::OwnedByOther {
                    owner_id,
                    epoch,
                    status,
                    lease_until,
                })
            }
            Err(e) => {
                meter_claim("rejected", module, kind, "store_unavailable");
                warn!(
                    execution_id = %execution_id,
                    kind,
                    module,
                    error = %e,
                    replica = %self.replica_id,
                    "ownership store unavailable — FAILING CLOSED (no execution)"
                );
                Err(BotError::ownership_unavailable(format!(
                    "claim {execution_id}: {e}"
                )))
            }
        }
    }

    /// Introspection (operators/tests).
    pub async fn get(&self, execution_id: &str) -> BotResult<Option<ExecutionClaim>> {
        self.store.get(execution_id).await
    }
}

/// What a claim attempt produced for the calling module. Deliberately NOT
/// `Clone`: a guard is singular ownership and must not be duplicated.
#[derive(Debug)]
pub enum ClaimOutcome {
    /// Boxed: the guard is far larger than the rejection payload
    /// (`clippy::large_enum_variant`).
    Owned(Box<ClaimGuard>),
    OwnedByOther {
        owner_id: String,
        epoch: i64,
        status: ClaimStatus,
        lease_until: DateTime<Utc>,
    },
}

/// Proof of ownership for one logical execution. Every money-moving
/// continuation validates the guard ([`ClaimGuard::fence`]); the terminal
/// transition is explicit ([`ClaimGuard::complete`], `release`, `hand_off`)
/// — a dropped guard without a terminal transition expires by lease, which
/// is exactly the crash semantics (§J).
pub struct ClaimGuard {
    store: Arc<dyn ClaimStore>,
    claim: ExecutionClaim,
    lease: Duration,
    renewals: u32,
    done: bool,
}

impl std::fmt::Debug for ClaimGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimGuard")
            .field("execution_id", &self.claim.execution_id)
            .field("owner_id", &self.claim.owner_id)
            .field("epoch", &self.claim.epoch)
            .field("lease_until", &self.claim.lease_until)
            .field("renewals", &self.renewals)
            .field("done", &self.done)
            .finish()
    }
}

impl ClaimGuard {
    pub fn execution_id(&self) -> &str {
        &self.claim.execution_id
    }

    pub fn epoch(&self) -> i64 {
        self.claim.epoch
    }

    pub fn owner_id(&self) -> &str {
        &self.claim.owner_id
    }

    pub fn claim(&self) -> &ExecutionClaim {
        &self.claim
    }

    /// Fencing check (§E). MUST be called immediately before any
    /// continuation that can move money (journal write, broadcast, position
    /// mutation). `Err(ClaimRejected)` = ownership lost: stop.
    pub async fn fence(&self) -> BotResult<()> {
        match self
            .store
            .verify(
                &self.claim.execution_id,
                &self.claim.owner_id,
                self.claim.epoch,
            )
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => {
                metrics::global()
                    .counter(
                        "bot_distributed_fencing_rejected_total",
                        "Money-moving continuations blocked because the claim was lost (fenced).",
                        &[("module", self.claim.module.as_str())],
                    )
                    .inc();
                warn!(
                    execution_id = %self.claim.execution_id,
                    module = %self.claim.module,
                    replica = %self.claim.owner_id,
                    epoch = self.claim.epoch,
                    "FENCED: ownership lost — blocking money-moving continuation"
                );
                Err(BotError::claim_rejected(format!(
                    "claim {} epoch {} no longer owned (fenced)",
                    self.claim.execution_id, self.claim.epoch
                )))
            }
            Err(e) => {
                metrics::global()
                    .counter(
                        "bot_distributed_fencing_rejected_total",
                        "Money-moving continuations blocked because the claim was lost (fenced).",
                        &[("module", self.claim.module.as_str())],
                    )
                    .inc();
                warn!(
                    execution_id = %self.claim.execution_id,
                    error = %e,
                    "ownership verify failed — FAILING CLOSED"
                );
                Err(BotError::ownership_unavailable(format!(
                    "verify {}: {e}",
                    self.claim.execution_id
                )))
            }
        }
    }

    /// One bounded lease renewal (§N). `Ok(false)` = ownership lost.
    pub async fn renew(&mut self) -> BotResult<bool> {
        if self.done {
            return Ok(false);
        }
        if self.renewals >= MAX_RENEWALS {
            warn!(
                execution_id = %self.claim.execution_id,
                renewals = self.renewals,
                "renewal budget exhausted — letting the lease lapse (hung-worker protection)"
            );
            return Ok(false);
        }
        self.renewals += 1;
        match self
            .store
            .renew(
                &self.claim.execution_id,
                &self.claim.owner_id,
                self.claim.epoch,
            )
            .await
        {
            Ok(true) => {
                self.claim.lease_until = Utc::now()
                    + chrono::Duration::from_std(self.lease)
                        .unwrap_or(chrono::Duration::seconds(45));
                Ok(true)
            }
            Ok(false) => {
                metrics::global()
                    .counter(
                        "bot_distributed_claim_renewal_failed_total",
                        "Lease renewals rejected (ownership lost or expired).",
                        &[("module", self.claim.module.as_str())],
                    )
                    .inc();
                warn!(
                    execution_id = %self.claim.execution_id,
                    epoch = self.claim.epoch,
                    "claim renewal REJECTED — ownership lost"
                );
                Ok(false)
            }
            Err(e) => {
                metrics::global()
                    .counter(
                        "bot_distributed_claim_renewal_failed_total",
                        "Lease renewals rejected (ownership lost or expired).",
                        &[("module", self.claim.module.as_str())],
                    )
                    .inc();
                warn!(
                    execution_id = %self.claim.execution_id,
                    error = %e,
                    "claim renewal failed (store unavailable)"
                );
                // Transient store failure: ownership MAY still be recorded;
                // the lease gives slack. The next fence decides, fail-closed.
                Err(BotError::ownership_unavailable(format!("renew: {e}")))
            }
        }
    }

    /// Run `work` under active lease renewal (every lease/3 with light
    /// jitter, bounded by [`MAX_RENEWALS`]). If a renewal is rejected the
    /// work future is NOT cancelled mid-flight (a broadcast in progress
    /// cannot be un-sent) — but the returned guard is marked fenced-lost so
    /// the caller's next `fence()`/`complete()` reports the loss. Money
    /// paths therefore call `fence()` immediately BEFORE the broadcast
    /// (inside `work`), which is where the cancellation is safe.
    pub async fn run_guarded<F>(&mut self, work: F) -> F::Output
    where
        F: Future,
    {
        let period = self.lease / 3;
        let jitter_ms = (self.claim.epoch.max(1) as u64 * 37) % 250;
        let mut ticker = tokio::time::interval(period + Duration::from_millis(jitter_ms));
        ticker.tick().await; // first tick is immediate — skip it
        tokio::pin!(work);
        loop {
            tokio::select! {
                out = &mut work => return out,
                _ = ticker.tick() => {
                    match self.renew().await {
                        Ok(_) => {}
                        Err(e) => {
                            debug!(execution_id = %self.claim.execution_id, error = %e,
                                   "renewal error during guarded work (continuing; fence decides)");
                        }
                    }
                }
            }
        }
    }

    /// Terminal transition chosen by outcome determinacy (§I/§M):
    /// `ambiguous = true` (e.g. `SendUnknown`) hands the execution to
    /// reconciliation (`handed_off`, re-acquisition blocked for the grace
    /// window); `false` releases it (`released`, re-acquirable — a re-fired
    /// exit rule is a new decision).
    pub async fn complete(&mut self, ambiguous: bool) -> bool {
        let mode = if ambiguous {
            ClaimStatus::HandedOff
        } else {
            ClaimStatus::Released
        };
        self.transition(mode).await
    }

    /// Clean terminal: work finished with a determinate outcome.
    pub async fn release(&mut self) -> bool {
        self.transition(ClaimStatus::Released).await
    }

    /// Ambiguous terminal: reconciliation owns the outcome from here.
    pub async fn hand_off(&mut self) -> bool {
        self.transition(ClaimStatus::HandedOff).await
    }

    async fn transition(&mut self, mode: ClaimStatus) -> bool {
        if self.done {
            return false;
        }
        self.done = true;
        match self
            .store
            .release(
                &self.claim.execution_id,
                &self.claim.owner_id,
                self.claim.epoch,
                mode,
            )
            .await
        {
            Ok(ok) => {
                self.claim.status = mode;
                if !ok {
                    // CAS refused: we were fenced mid-work (lease taken over,
                    // or already terminal). The journal + reconciliation own
                    // whatever left the process; meter it loudly (§E/§S).
                    metrics::global()
                        .counter(
                            "bot_distributed_claim_terminal_lost_total",
                            "Terminal transitions refused because ownership was lost mid-work (fenced).",
                            &[("module", self.claim.module.as_str())],
                        )
                        .inc();
                }
                meter_claim(
                    if mode == ClaimStatus::HandedOff {
                        "handoff"
                    } else {
                        "released"
                    },
                    &self.claim.module,
                    &self.claim.kind,
                    "",
                );
                info!(
                    execution_id = %self.claim.execution_id,
                    module = %self.claim.module,
                    replica = %self.claim.owner_id,
                    epoch = self.claim.epoch,
                    status = mode.as_str(),
                    applied = ok,
                    "execution claim terminal transition"
                );
                ok
            }
            Err(e) => {
                warn!(
                    execution_id = %self.claim.execution_id,
                    error = %e,
                    "claim release failed — lease expiry will free it (audit row keeps history)"
                );
                false
            }
        }
    }
}

fn meter_claim(op: &str, module: &str, kind: &str, reason: &str) {
    let name = match op {
        "acquired" => "bot_distributed_claim_acquired_total",
        "rejected" => "bot_distributed_claim_rejected_total",
        "takeover" => "bot_distributed_claim_takeover_total",
        "released" | "handoff" => "bot_distributed_claim_released_total",
        _ => "bot_distributed_claim_acquired_total",
    };
    let labels: Vec<(&str, &str)> = if reason.is_empty() {
        vec![("module", module), ("kind", kind)]
    } else {
        vec![("module", module), ("kind", kind), ("reason", reason)]
    };
    metrics::global()
        .counter(
            name,
            "Distributed execution-claim transitions (low-cardinality labels only).",
            &labels,
        )
        .inc();
}

// ---------------------------------------------------------------------------
// Module-side glue: execution permits (§F/§G)
// ---------------------------------------------------------------------------

/// The outcome of requesting permission to execute ONE logical money-moving
/// intent. Business modules branch on this and never touch store internals.
#[derive(Debug)]
pub enum Permit {
    /// No [`OwnershipRegistry`] attached (single-instance / library use):
    /// proceed unguarded — exactly the pre-Prompt-3 behaviour. The server
    /// ALWAYS attaches a registry, so this never occurs in a deployed bot.
    Unmanaged,
    /// This replica is the sole active owner. `fence()` before every
    /// money-moving continuation; `finish()` at the terminal point.
    /// Boxed: `clippy::large_enum_variant`.
    Owned(Box<ClaimGuard>),
    /// Another replica owns this execution (§G deterministic loser): skip
    /// silently — never broadcast, never retry in a loop.
    Lost { owner_id: String },
}

impl Permit {
    /// Request an execution permit (§F). `Err` = ownership unavailable →
    /// FAIL CLOSED (§K): the caller must abort the money path.
    pub async fn acquire(
        registry: Option<&OwnershipRegistry>,
        execution_id: impl Into<String>,
        kind: &str,
        module: &str,
        strategy: &str,
        symbol: &str,
    ) -> BotResult<Permit> {
        let Some(reg) = registry else {
            return Ok(Permit::Unmanaged);
        };
        Ok(
            match reg
                .claim(execution_id, kind, module, strategy, symbol)
                .await?
            {
                ClaimOutcome::Owned(guard) => Permit::Owned(guard),
                ClaimOutcome::OwnedByOther { owner_id, .. } => Permit::Lost { owner_id },
            },
        )
    }

    /// True when this replica may proceed with the money path.
    pub fn proceed(&self) -> bool {
        !matches!(self, Permit::Lost { .. })
    }

    /// Fencing gate (§E): call immediately BEFORE every money-moving
    /// continuation (journal write / broadcast / position mutation).
    /// `Err` = ownership lost or unverifiable → abort.
    pub async fn fence(&self) -> BotResult<()> {
        match self {
            Permit::Owned(g) => g.fence().await,
            Permit::Unmanaged | Permit::Lost { .. } => Ok(()),
        }
    }

    /// Terminal transition (§I/§M): `ambiguous = true` when the broadcast
    /// outcome is not yet proven by external state (e.g. `Sent`/
    /// `SendUnknown`) — reconciliation owns it and re-acquisition is blocked
    /// for the handoff grace; `false` releases immediately.
    pub async fn finish(&mut self, ambiguous: bool) {
        if let Permit::Owned(g) = self {
            g.complete(ambiguous).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime flags (kill switch / module enables) — cross-replica propagation
// ---------------------------------------------------------------------------

/// One shared runtime flag row.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeFlag {
    /// `kill_switch` | `module:<name>`.
    pub flag: String,
    pub enabled: bool,
    pub reason: String,
    pub updated_by: String,
    pub updated_at: DateTime<Utc>,
}

/// Write side of runtime-flag propagation (§Q). Implemented by the server
/// over Postgres (`runtime_flags`, migration 0010 — authoritative) or Redis
/// (fallback when no DB). `AppState` calls this on every local mutation.
#[async_trait]
pub trait RuntimeFlagsWriter: Send + Sync {
    async fn write(&self, flag: &str, enabled: bool, reason: &str, updated_by: &str);
}

/// Read side used by the sync task.
#[async_trait]
pub trait RuntimeFlagsReader: Send + Sync {
    /// `Err` = store unreachable; the sync task keeps local state unchanged
    /// (a stale local view is safer than a reset one) and meters the failure.
    async fn read_all(&self) -> BotResult<Vec<RuntimeFlag>>;
}

// ---------------------------------------------------------------------------
// In-memory store (single instance / paper / tests — never HA-authoritative)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MemRow {
    claim: ExecutionClaim,
    updated_at: DateTime<Utc>,
    /// Lease length recorded at claim time (mirrors Redis `lease_span` and
    /// the PG `lease_until - claimed_at` derivation): renewals extend by
    /// exactly this, never by a drifting recomputation.
    lease_span: Duration,
}

/// Process-local [`ClaimStore`] with the SAME semantics as the durable
/// stores (atomic under one mutex, epoch fencing, handoff grace). Used when
/// neither Postgres nor Redis is configured (single-instance/paper) and in
/// unit tests. NOT an HA safety mechanism (§K).
#[derive(Default)]
pub struct MemoryClaimStore {
    rows: Mutex<std::collections::HashMap<String, MemRow>>,
    /// Test hook: pretend "now" is this far in the future (deterministic
    /// lease-expiry tests without sleeps).
    clock_skew_ms: AtomicU64,
}

impl MemoryClaimStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the store's private clock (tests only; deterministic expiry).
    pub fn advance_clock(&self, d: Duration) {
        self.clock_skew_ms
            .fetch_add(d.as_millis() as u64, Ordering::SeqCst);
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
            + chrono::Duration::milliseconds(self.clock_skew_ms.load(Ordering::SeqCst) as i64)
    }
}

#[async_trait]
impl ClaimStore for MemoryClaimStore {
    fn backend(&self) -> &'static str {
        "memory"
    }

    async fn claim(&self, req: &ClaimRequest, ctx: &StoreContext) -> BotResult<ClaimDecision> {
        let now = self.now();
        let lease_until =
            now + chrono::Duration::from_std(ctx.lease).unwrap_or(chrono::Duration::seconds(45));
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        match rows.get(&req.execution_id) {
            None => {
                let claim = ExecutionClaim {
                    execution_id: req.execution_id.clone(),
                    kind: req.kind.clone(),
                    module: req.module.clone(),
                    strategy: req.strategy.clone(),
                    symbol: req.symbol.clone(),
                    owner_id: ctx.owner_id.clone(),
                    epoch: 1,
                    status: ClaimStatus::Claimed,
                    acquired_at: now,
                    lease_until,
                    takeover_count: 0,
                    previous_owner: None,
                };
                rows.insert(
                    req.execution_id.clone(),
                    MemRow {
                        claim: claim.clone(),
                        updated_at: now,
                        lease_span: ctx.lease,
                    },
                );
                Ok(ClaimDecision::Acquired(claim))
            }
            Some(row) => {
                let c = &row.claim;
                let reacquirable = match c.status {
                    ClaimStatus::Claimed => c.lease_until <= now,
                    ClaimStatus::Released => true,
                    ClaimStatus::HandedOff => {
                        row.updated_at
                            + chrono::Duration::from_std(ctx.handoff_grace)
                                .unwrap_or(chrono::Duration::seconds(900))
                            <= now
                    }
                };
                if !reacquirable {
                    return Ok(ClaimDecision::Rejected {
                        owner_id: c.owner_id.clone(),
                        epoch: c.epoch,
                        status: c.status,
                        lease_until: c.lease_until,
                    });
                }
                let takeover = c.status == ClaimStatus::Claimed;
                let claim = ExecutionClaim {
                    execution_id: req.execution_id.clone(),
                    kind: req.kind.clone(),
                    module: req.module.clone(),
                    strategy: req.strategy.clone(),
                    symbol: req.symbol.clone(),
                    owner_id: ctx.owner_id.clone(),
                    epoch: c.epoch + 1,
                    status: ClaimStatus::Claimed,
                    acquired_at: now,
                    lease_until,
                    takeover_count: c.takeover_count + if takeover { 1 } else { 0 },
                    previous_owner: Some(c.owner_id.clone()),
                };
                if takeover {
                    meter_claim("takeover", &req.module, &req.kind, "");
                    metrics::global()
                        .counter(
                            "bot_distributed_claim_expired_total",
                            "Claims found expired and taken over by a new replica.",
                            &[("module", req.module.as_str())],
                        )
                        .inc();
                }
                rows.insert(
                    req.execution_id.clone(),
                    MemRow {
                        claim: claim.clone(),
                        updated_at: now,
                        lease_span: ctx.lease,
                    },
                );
                Ok(ClaimDecision::Acquired(claim))
            }
        }
    }

    async fn renew(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool> {
        let now = self.now();
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let Some(row) = rows.get_mut(execution_id) else {
            return Ok(false);
        };
        let span = row.lease_span;
        let c = &mut row.claim;
        if c.owner_id != owner_id || c.epoch != epoch || c.status != ClaimStatus::Claimed {
            return Ok(false);
        }
        if c.lease_until <= now {
            return Ok(false); // expired: renewals do not resurrect leases
        }
        c.lease_until =
            now + chrono::Duration::from_std(span).unwrap_or(chrono::Duration::seconds(45));
        row.updated_at = now;
        Ok(true)
    }

    async fn verify(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool> {
        let now = self.now();
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        Ok(match rows.get(execution_id) {
            Some(row) => {
                let c = &row.claim;
                c.owner_id == owner_id
                    && c.epoch == epoch
                    && c.status == ClaimStatus::Claimed
                    && c.lease_until > now
            }
            None => false,
        })
    }

    async fn release(
        &self,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
        mode: ClaimStatus,
    ) -> BotResult<bool> {
        debug_assert!(mode != ClaimStatus::Claimed);
        let now = self.now();
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let Some(row) = rows.get_mut(execution_id) else {
            return Ok(false);
        };
        let c = &mut row.claim;
        if c.owner_id != owner_id || c.epoch != epoch || c.status != ClaimStatus::Claimed {
            return Ok(false);
        }
        c.status = mode;
        row.updated_at = now;
        Ok(true)
    }

    async fn get(&self, execution_id: &str) -> BotResult<Option<ExecutionClaim>> {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        Ok(rows.get(execution_id).map(|r| r.claim.clone()))
    }
}

/// In-memory runtime flags (single instance/tests).
#[derive(Default)]
pub struct MemoryFlags {
    rows: Mutex<std::collections::HashMap<String, RuntimeFlag>>,
}

#[async_trait]
impl RuntimeFlagsWriter for MemoryFlags {
    async fn write(&self, flag: &str, enabled: bool, reason: &str, updated_by: &str) {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).insert(
            flag.to_string(),
            RuntimeFlag {
                flag: flag.to_string(),
                enabled,
                reason: reason.to_string(),
                updated_by: updated_by.to_string(),
                updated_at: Utc::now(),
            },
        );
    }
}

#[async_trait]
impl RuntimeFlagsReader for MemoryFlags {
    async fn read_all(&self) -> BotResult<Vec<RuntimeFlag>> {
        Ok(self
            .rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn registry(store: Arc<MemoryClaimStore>, replica: &str, lease: Duration) -> OwnershipRegistry {
        OwnershipRegistry::new(store, replica, lease, Duration::from_secs(900))
    }

    #[tokio::test]
    async fn two_replicas_one_winner() {
        // §G/§W1: concurrent claims on the same logical id → exactly one
        // owner; the loser gets a deterministic OwnedByOther (no broadcast).
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "replica-A", Duration::from_secs(60));
        let b = registry(store.clone(), "replica-B", Duration::from_secs(60));
        let (ra, rb) = tokio::join!(
            a.claim("snipe:MINT", "entry", "sniper", "pump", "MINT"),
            b.claim("snipe:MINT", "entry", "sniper", "pump", "MINT")
        );
        let (ra, rb) = (ra.unwrap(), rb.unwrap());
        let owned =
            matches!(ra, ClaimOutcome::Owned(_)) as u8 + matches!(rb, ClaimOutcome::Owned(_)) as u8;
        assert_eq!(owned, 1, "exactly one owner");
        let loser = if matches!(ra, ClaimOutcome::Owned(_)) {
            rb
        } else {
            ra
        };
        match loser {
            ClaimOutcome::OwnedByOther { status, .. } => assert_eq!(status, ClaimStatus::Claimed),
            _ => panic!("loser must see OwnedByOther"),
        }
    }

    #[tokio::test]
    async fn three_replicas_one_winner() {
        // §W2
        let store = Arc::new(MemoryClaimStore::new());
        let regs: Vec<_> = (0..3)
            .map(|i| registry(store.clone(), &format!("r{i}"), Duration::from_secs(60)))
            .collect();
        let mut owned = 0;
        for r in &regs {
            if matches!(
                r.claim("copy:w:m", "entry", "copy", "mirror", "m")
                    .await
                    .unwrap(),
                ClaimOutcome::Owned(_)
            ) {
                owned += 1;
            }
        }
        assert_eq!(owned, 1);
    }

    #[tokio::test]
    async fn expired_lease_takeover_and_fencing() {
        // §W7/§W8/§W9: lease expiry → takeover with epoch+1 → stale owner
        // is fenced on verify/renew/release. Deterministic via the store's
        // injected clock (no sleeps).
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "A", Duration::from_secs(30));
        let b = registry(store.clone(), "B", Duration::from_secs(30));

        let mut ga = match a.claim("x", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!("A must acquire"),
        };
        // While the lease runs, B is rejected.
        assert!(matches!(
            b.claim("x", "entry", "sniper", "", "").await.unwrap(),
            ClaimOutcome::OwnedByOther { .. }
        ));
        assert!(ga.fence().await.is_ok());

        // A stalls past its lease (injected clock).
        store.advance_clock(Duration::from_secs(31));
        assert!(ga.fence().await.is_err(), "expired lease must fence A");
        assert!(!ga.renew().await.unwrap(), "expired lease must not renew");

        // B takes over: epoch 2, previous owner recorded (audit, §S).
        let gb = match b.claim("x", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!("B must take over the expired lease"),
        };
        assert_eq!(gb.epoch(), 2);
        assert_eq!(gb.claim().takeover_count, 1);
        assert_eq!(gb.claim().previous_owner.as_deref(), Some("A"));

        // A wakes up: every continuation is rejected (§E).
        assert!(ga.fence().await.is_err());
        assert!(!ga.release().await, "stale owner cannot release");
        assert!(!ga.hand_off().await);
        // B still owns it.
        assert!(gb.fence().await.is_ok());
        let rec = store.get("x").await.unwrap().unwrap();
        assert_eq!(rec.owner_id, "B");
        assert_eq!(rec.epoch, 2);
    }

    #[tokio::test]
    async fn release_is_reacquirable_handoff_is_graced() {
        // §I case E / §M: a RELEASED claim (determinate outcome) can be
        // re-acquired (re-fired exit rule = new decision); a HANDED_OFF
        // claim (ambiguous outcome) blocks re-acquisition during the grace
        // window so nobody resubmits a possibly-in-flight transaction.
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "A", Duration::from_secs(60));
        let b = registry(store.clone(), "B", Duration::from_secs(60));

        let mut g = match a
            .claim("exit:p:sl", "exit", "sniper", "", "p")
            .await
            .unwrap()
        {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        g.release().await;
        assert!(
            matches!(
                b.claim("exit:p:sl", "exit", "sniper", "", "p")
                    .await
                    .unwrap(),
                ClaimOutcome::Owned(_)
            ),
            "released claims are re-acquirable"
        );

        let mut g2 = match b
            .claim("exit:p2:sl", "exit", "sniper", "", "p2")
            .await
            .unwrap()
        {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        g2.hand_off().await;
        assert!(
            matches!(
                a.claim("exit:p2:sl", "exit", "sniper", "", "p2")
                    .await
                    .unwrap(),
                ClaimOutcome::OwnedByOther {
                    status: ClaimStatus::HandedOff,
                    ..
                }
            ),
            "handed-off claims block re-acquisition during grace"
        );
        // After the grace window, re-acquisition is allowed (reconciliation
        // had its chance; the module re-decides from fresh state).
        store.advance_clock(Duration::from_secs(901));
        assert!(matches!(
            a.claim("exit:p2:sl", "exit", "sniper", "", "p2")
                .await
                .unwrap(),
            ClaimOutcome::Owned(_)
        ));
    }

    #[tokio::test]
    async fn renewal_extends_lease_and_is_bounded() {
        // §W10/§W11 + §N bounded renewals.
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "A", Duration::from_millis(60));
        let mut g = match a.claim("r", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        assert!(g.renew().await.unwrap());
        // Bounded: burn the budget, then renewals refuse (lease lapses →
        // hung-worker protection).
        g.renewals = MAX_RENEWALS;
        assert!(!g.renew().await.unwrap());
        // After the (unrenewed) lease lapses, the owner is fenced.
        store.advance_clock(Duration::from_millis(120));
        assert!(g.fence().await.is_err());
    }

    #[tokio::test]
    async fn run_guarded_renews_during_slow_work() {
        // §N: slow work keeps its lease via the guarded renewal ticker.
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "A", Duration::from_millis(150));
        let mut g = match a.claim("slow", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        let out = g
            .run_guarded(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                42
            })
            .await;
        assert_eq!(out, 42);
        assert!(g.renewals > 0, "the ticker must have renewed");
        assert!(g.fence().await.is_ok(), "still owned after guarded work");
        g.release().await;
    }

    #[tokio::test]
    async fn registry_fails_closed_on_store_errors() {
        // §K/§W12-14: an unreachable store is NEVER "ownership acquired".
        struct FailingStore;
        #[async_trait]
        impl ClaimStore for FailingStore {
            fn backend(&self) -> &'static str {
                "memory"
            }
            async fn claim(
                &self,
                _r: &ClaimRequest,
                _c: &StoreContext,
            ) -> BotResult<ClaimDecision> {
                Err(BotError::db("connection refused"))
            }
            async fn renew(&self, _i: &str, _o: &str, _e: i64) -> BotResult<bool> {
                Err(BotError::db("down"))
            }
            async fn verify(&self, _i: &str, _o: &str, _e: i64) -> BotResult<bool> {
                Err(BotError::db("down"))
            }
            async fn release(
                &self,
                _i: &str,
                _o: &str,
                _e: i64,
                _m: ClaimStatus,
            ) -> BotResult<bool> {
                Err(BotError::db("down"))
            }
            async fn get(&self, _i: &str) -> BotResult<Option<ExecutionClaim>> {
                Err(BotError::db("down"))
            }
        }
        let reg = OwnershipRegistry::new(
            Arc::new(FailingStore),
            "A",
            Duration::from_secs(30),
            Duration::from_secs(60),
        );
        let err = reg.claim("x", "entry", "sniper", "", "").await.unwrap_err();
        assert!(
            matches!(err, BotError::OwnershipUnavailable(_)),
            "must fail closed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn memory_flags_roundtrip() {
        let f = MemoryFlags::default();
        RuntimeFlagsWriter::write(&f, "kill_switch", true, "test", "repA").await;
        let rows = RuntimeFlagsReader::read_all(&f).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].enabled);
        assert_eq!(rows[0].updated_by, "repA");
    }

    #[tokio::test]
    async fn permit_glue_skip_fence_finish() {
        // §F/§G glue: Unmanaged proceeds and fences trivially; Lost never
        // proceeds; Owned fences and finishes through to the store.
        assert!(Permit::Unmanaged.proceed());
        Permit::Unmanaged.fence().await.unwrap();
        let lost = Permit::Lost {
            owner_id: "B".into(),
        };
        assert!(!lost.proceed());
        lost.fence().await.unwrap(); // no-op, must not error

        let store = Arc::new(MemoryClaimStore::new());
        let reg = registry(store.clone(), "A", Duration::from_secs(60));
        let mut p = Permit::acquire(Some(&reg), "g1", "entry", "sniper", "", "S")
            .await
            .unwrap();
        assert!(p.proceed());
        p.fence().await.unwrap();
        p.finish(false).await;
        assert_eq!(
            store.get("g1").await.unwrap().unwrap().status,
            ClaimStatus::Released
        );
        // A second replica now sees Lost with the owner recorded.
        let reg_b = registry(store.clone(), "B", Duration::from_secs(60));
        // (released → re-acquirable, so B actually acquires; Lost is covered
        // by two_replicas_one_winner — here assert re-acquisition works.)
        assert!(matches!(
            Permit::acquire(Some(&reg_b), "g1", "entry", "sniper", "", "S")
                .await
                .unwrap(),
            Permit::Owned(_)
        ));
    }

    #[tokio::test]
    async fn same_owner_reclaim_of_active_claim_is_rejected() {
        // A replica cannot "re-decide" an execution it already owns: the
        // active claim rejects EVERYONE including its owner (prevents two
        // concurrent tasks inside one replica from both executing).
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "A", Duration::from_secs(60));
        let _g = match a.claim("e", "exit", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!("first claim must acquire"),
        };
        match a.claim("e", "exit", "sniper", "", "").await.unwrap() {
            ClaimOutcome::OwnedByOther { owner_id, .. } => assert_eq!(owner_id, "A"),
            _ => panic!("active claim must reject even its own owner"),
        }
    }

    #[tokio::test]
    async fn handoff_blocks_even_the_owner() {
        // §I case E: after an ambiguous terminal, NOBODY (not even the same
        // replica) may re-acquire before the grace window — reconciliation
        // owns the outcome.
        let store = Arc::new(MemoryClaimStore::new());
        let a = registry(store.clone(), "A", Duration::from_secs(60));
        let mut g = match a.claim("h", "exit", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        g.hand_off().await;
        assert!(matches!(
            a.claim("h", "exit", "sniper", "", "").await.unwrap(),
            ClaimOutcome::OwnedByOther {
                status: ClaimStatus::HandedOff,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn epoch_increments_across_repeated_takeovers() {
        // §S/§T: ownership history is reconstructable — each takeover bumps
        // epoch and takeover_count and records the previous owner; every
        // stale generation stays fenced.
        let store = Arc::new(MemoryClaimStore::new());
        let mk = |n: &str| registry(store.clone(), n, Duration::from_millis(50));
        let (a, b, c) = (mk("A"), mk("B"), mk("C"));

        let g1 = match a.claim("t", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        assert_eq!(g1.epoch(), 1);

        store.advance_clock(Duration::from_millis(60));
        let g2 = match b.claim("t", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!("B must take over A's expired lease"),
        };
        assert_eq!(g2.epoch(), 2);
        assert_eq!(g2.claim().takeover_count, 1);
        assert_eq!(g2.claim().previous_owner.as_deref(), Some("A"));

        store.advance_clock(Duration::from_millis(60));
        let g3 = match c.claim("t", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!("C must take over B's expired lease"),
        };
        assert_eq!(g3.epoch(), 3);
        assert_eq!(g3.claim().takeover_count, 2);
        assert_eq!(g3.claim().previous_owner.as_deref(), Some("B"));

        // Both stale generations are fenced; only the current one verifies.
        assert!(g1.fence().await.is_err());
        assert!(g2.fence().await.is_err());
        assert!(g3.fence().await.is_ok());
    }

    #[tokio::test]
    async fn fencing_fails_closed_on_store_errors() {
        // §Y: a store that starts failing mid-flight must abort the money
        // path (fence/renew propagate Err), never silently proceed.
        struct FlakyStore {
            inner: MemoryClaimStore,
            fail: AtomicBool,
        }
        #[async_trait]
        impl ClaimStore for FlakyStore {
            fn backend(&self) -> &'static str {
                "memory"
            }
            async fn claim(&self, r: &ClaimRequest, c: &StoreContext) -> BotResult<ClaimDecision> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(BotError::db("flaky: claim"));
                }
                self.inner.claim(r, c).await
            }
            async fn renew(&self, i: &str, o: &str, e: i64) -> BotResult<bool> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(BotError::db("flaky: renew"));
                }
                self.inner.renew(i, o, e).await
            }
            async fn verify(&self, i: &str, o: &str, e: i64) -> BotResult<bool> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(BotError::db("flaky: verify"));
                }
                self.inner.verify(i, o, e).await
            }
            async fn release(&self, i: &str, o: &str, e: i64, m: ClaimStatus) -> BotResult<bool> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(BotError::db("flaky: release"));
                }
                self.inner.release(i, o, e, m).await
            }
            async fn get(&self, i: &str) -> BotResult<Option<ExecutionClaim>> {
                self.inner.get(i).await
            }
        }
        let store = Arc::new(FlakyStore {
            inner: MemoryClaimStore::new(),
            fail: AtomicBool::new(false),
        });
        let reg = OwnershipRegistry::new(
            store.clone() as Arc<dyn ClaimStore>,
            "A",
            Duration::from_secs(60),
            Duration::from_secs(60),
        );
        let mut g = match reg.claim("f", "entry", "sniper", "", "").await.unwrap() {
            ClaimOutcome::Owned(g) => g,
            _ => panic!(),
        };
        g.fence().await.unwrap();

        store.fail.store(true, Ordering::SeqCst);
        assert!(matches!(
            g.fence().await.unwrap_err(),
            BotError::OwnershipUnavailable(_)
        ));
        assert!(g.renew().await.is_err());
        // New claims fail closed too.
        assert!(reg.claim("f2", "entry", "sniper", "", "").await.is_err());
        // Release failure is tolerated (returns false; lease expiry frees it).
        assert!(!g.release().await);
    }

    #[test]
    fn claim_status_strings_are_stable() {
        // DB/Redis wire format contract.
        assert_eq!(ClaimStatus::Claimed.as_str(), "claimed");
        assert_eq!(ClaimStatus::Released.as_str(), "released");
        assert_eq!(ClaimStatus::HandedOff.as_str(), "handed_off");
        assert_eq!(
            ClaimStatus::parse("handed_off"),
            Some(ClaimStatus::HandedOff)
        );
        assert_eq!(ClaimStatus::parse("bogus"), None);
    }
}
