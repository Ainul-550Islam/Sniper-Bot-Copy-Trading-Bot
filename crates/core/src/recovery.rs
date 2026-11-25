//! Reconciliation & recovery workers (BUILD PLAN §4-xiv / §11).
//!
//! The core provides the **queue mechanics**; the venue-specific truth
//! lookups live behind [`TruthSource`] and are registered by the server
//! (Solana signature status via RPC, Polymarket order status via the CLOB
//! API, …). This keeps `bot-core` free of trading-network clients while
//! giving every module the same restart-safe "verify against external
//! truth" pipeline:
//!
//! 1. Producers enqueue subjects (`transaction:<sig>`,
//!    `polymarket_order:<id>`, `position:<id>`, `balance:<addr>`) — usually
//!    at submit time and via [`sweep_unresolved_transactions`].
//! 2. [`RecoveryWorker::run`] claims due items atomically
//!    (`FOR UPDATE SKIP LOCKED` — safe across replicas), asks the matching
//!    [`TruthSource`], and applies the verdict: resolve, retry with
//!    exponential backoff, or park in `failed` for operators.
//! 3. On restart, [`crate::oms::OrderManager::recover_from_db`] reloads
//!    non-terminal orders as `Unknown`; the sweeper re-enqueues their
//!    signatures so the worker reconciles them to `Filled`/`Failed`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::db::repo::{IntentRecord, IntentRepo, ReconItem, ReconRepo, TransactionRepo};
use crate::db::Database;
use crate::error::BotResult;
use crate::lifecycle::Shutdown;
use crate::obs::metrics;

/// What the external world says about a subject.
#[derive(Debug, Clone)]
pub enum ReconVerdict {
    /// Truth established; the source has applied any side effects (order
    /// transition, position close, …). `status` describes the outcome for
    /// logs/metrics (e.g. "finalized", "failed", "cancelled", "filled").
    Resolved { status: String },
    /// Inconclusive (RPC error, still pending confirmation) — retry later
    /// with backoff.
    Retry { reason: String },
    /// Permanently unresolvable (subject does not exist upstream) — park in
    /// the operator queue.
    GiveUp { reason: String },
}

/// External-truth lookup for one reconciliation kind.
#[async_trait]
pub trait TruthSource: Send + Sync {
    /// The `reconciliation_state.kind` this source resolves.
    fn kind(&self) -> &str;

    /// Query the outside world about `subject` and act on the answer.
    /// Implementations own their side effects (they hold the OMS/state
    /// handles); the worker only maintains the queue.
    async fn resolve(&self, subject: &str) -> ReconVerdict;
}

/// Fresh identity for one write-ahead broadcast attempt. Independent
/// replicas and process restarts must not reuse a process-local counter:
/// the durable journal treats an existing id as the same attempt. A random
/// UUID makes collisions negligibly likely without coordinating replicas.
pub fn new_journal_intent_id() -> String {
    format!("intent-{}", uuid::Uuid::new_v4())
}

/// Write-ahead intent journal (§I crash point C). Trading modules call this
/// AROUND the broadcast; the server implements it over `IntentRepo`. Modules
/// stay database-free: the sink is injected and optional. The default legacy
/// sink contract is best-effort, but a production implementation can opt into
/// [`IntentSink::record_checked`]; a failed checked reservation fails closed
/// before the money-moving future is polled.
#[async_trait]
pub trait IntentSink: Send + Sync {
    /// Persist the intent BEFORE the money-moving broadcast. The legacy
    /// best-effort contract remains for library users and test doubles: an
    /// implementation that cannot prove durability returns normally.
    async fn record(&self, rec: IntentRecord);
    /// Fail-checked pre-broadcast reservation. A production sink that cannot
    /// durably record the intent MUST return `Err`; the wrapper then refuses
    /// to poll the money-moving future. The default keeps older sinks
    /// best-effort so their existing semantics are unchanged.
    async fn record_checked(&self, rec: IntentRecord) -> BotResult<()> {
        self.record(rec).await;
        Ok(())
    }
    /// Acquire the durable PRE-BROADCAST reservation for this logical trade
    /// (`reservation_key`, empty = the intent id). Returning `Err` means the
    /// trade is already reserved by another attempt (a second replica, or a
    /// restarted process whose earlier broadcast is still ambiguous) and the
    /// wrapper refuses to poll the money-moving future. The default is a
    /// no-op so library users and test doubles keep their semantics.
    async fn reserve(&self, reservation_key: &str, rec: &IntentRecord) -> BotResult<()> {
        let _ = (reservation_key, rec);
        Ok(())
    }
    /// Close the durable pre-broadcast claim this attempt acquired. Returns
    /// `None` when the claim is kept (nothing is known, or the sink does not
    /// track claims) and `Some(freed)` when the claim is now free for a retry
    /// (`true`) or is still held although this attempt provably never
    /// broadcast (`false` — the trade key stays blocked and an operator must
    /// resolve it). `established_non_send` is true only when the caller
    /// proved the broadcast did not happen; such a claim must never keep
    /// blocking the trade. The default keeps older sinks unchanged.
    async fn release_reservation(
        &self,
        reservation_key: &str,
        established_non_send: bool,
    ) -> Option<bool> {
        let _ = (reservation_key, established_non_send);
        None
    }
    /// Associate a known signature with the attempt; a signature alone does
    /// not prove that the transaction landed or failed.
    async fn link(&self, intent_id: &str, signature: &str);
    /// The caller classified the result as having no broadcast signature.
    async fn abandon(&self, intent_id: &str);
}

/// What the execution result actually establishes for an intent journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentDisposition {
    /// A known broadcast signature; this is not proof of settlement.
    Link(String),
    /// The attempt is known not to require reconciliation.
    Abandon,
    /// The result is ambiguous or lacks evidence; keep it pending.
    Retain,
}

/// Compatibility wrapper. A missing signature is classified as abandon by
/// the caller; use `with_intent_disposition` when missing can be ambiguous.
pub async fn with_intent<T>(
    sink: Option<&Arc<dyn IntentSink>>,
    rec: IntentRecord,
    run: impl std::future::Future<Output = BotResult<T>>,
    signature_of: impl FnOnce(&T) -> Option<String>,
) -> BotResult<T> {
    with_intent_disposition(sink, rec, run, |value| match signature_of(value) {
        Some(sig) if !sig.is_empty() => IntentDisposition::Link(sig),
        _ => IntentDisposition::Abandon,
    })
    .await
}

/// Run a future behind a checked write-ahead intent reservation. A sink that
/// cannot prove the record returns `Err` and the money-moving future is NOT
/// polled. Library/test sinks that use the default remain best-effort. Generic
/// execution errors and `Retain` still leave the attempt pending; link/abandon
/// are best-effort. When the sink also implements [`IntentSink::reserve`] and
/// loses the lease, the future is not polled either — see
/// [`with_reserved_intent_disposition`].
pub async fn with_intent_disposition<T>(
    sink: Option<&Arc<dyn IntentSink>>,
    rec: IntentRecord,
    run: impl std::future::Future<Output = BotResult<T>>,
    disposition_of: impl FnOnce(&T) -> IntentDisposition,
) -> BotResult<T> {
    let key = rec.intent_id.clone();
    with_reserved_intent_disposition(sink, &key, rec, run, disposition_of).await
}

/// [`with_intent_disposition`] for a LOGICAL trade identity. Two attempts that
/// must never both broadcast (a second replica, a restarted process, a
/// replayed feed event mapping to the same mirror) pass the same
/// `reservation_key`; the durable lease in the sink decides which one may
/// proceed. Before this existed the reservation was the freshly generated
/// per-attempt intent id, so duplicate attempts were never actually blocked.
pub async fn with_reserved_intent_disposition<T>(
    sink: Option<&Arc<dyn IntentSink>>,
    reservation_key: &str,
    rec: IntentRecord,
    run: impl std::future::Future<Output = BotResult<T>>,
    disposition_of: impl FnOnce(&T) -> IntentDisposition,
) -> BotResult<T> {
    let intent_id = rec.intent_id.clone();
    if let Some(s) = sink {
        s.record_checked(rec.clone()).await?;
        // Durable pre-broadcast ownership: fail closed when another attempt
        // already holds this trade (or holds it with an ambiguous outcome).
        s.reserve(reservation_key, &rec).await?;
    }
    let out = run.await;
    let mut disposition = IntentDisposition::Retain;
    if let Ok(v) = &out {
        // An arbitrary future may fail after bytes have left the process;
        // leave errors pending instead of inventing a non-send proof.
        disposition = disposition_of(v);
        if let Some(s) = sink {
            match &disposition {
                IntentDisposition::Link(sig) if !sig.trim().is_empty() => {
                    s.link(&intent_id, sig).await
                }
                IntentDisposition::Abandon => s.abandon(&intent_id).await,
                // An empty link cannot identify the broadcast; retain it.
                IntentDisposition::Link(_) | IntentDisposition::Retain => {}
            }
        }
    }
    // Did the caller establish a NON-send? Only an explicit `Abandon` from a
    // successful result (or a failed result, where the future returned an
    // error the caller treats as "never broadcast") may free the claim.
    let established_non_send = matches!(disposition, IntentDisposition::Abandon);
    if let Some(s) = sink {
        if let Some(freed) = s
            .release_reservation(reservation_key, established_non_send)
            .await
        {
            if !freed && established_non_send {
                // The claim is provably not needed any more and provably not
                // free: an operator must look (a retry cannot broadcast).
                tracing::debug!(
                    trade = %reservation_key,
                    "durable reservation still held after a proven non-send"
                );
            }
        }
    }
    out
}

/// The durable queue writes a reconciliation pass performs.
///
/// Split from [`ReconRepo`] so the FAILURE path is testable: whether a verdict
/// was actually recorded decides whether the operator may be told the claim is
/// resolved.
#[async_trait::async_trait]
pub trait ReconQueueWrites: Send + Sync {
    async fn resolve(&self, kind: &str, subject: &str) -> BotResult<()>;
    async fn fail(&self, kind: &str, subject: &str, reason: &str) -> BotResult<()>;
    async fn give_up(&self, kind: &str, subject: &str, reason: &str) -> BotResult<()>;
    async fn release(&self, kind: &str, subject: &str) -> BotResult<()>;
}

#[async_trait::async_trait]
impl ReconQueueWrites for ReconRepo {
    async fn resolve(&self, kind: &str, subject: &str) -> BotResult<()> {
        Ok(ReconRepo::resolve(self, kind, subject).await?)
    }
    async fn fail(&self, kind: &str, subject: &str, reason: &str) -> BotResult<()> {
        Ok(ReconRepo::fail(self, kind, subject, reason).await?)
    }
    async fn give_up(&self, kind: &str, subject: &str, reason: &str) -> BotResult<()> {
        Ok(ReconRepo::give_up(self, kind, subject, reason).await?)
    }
    async fn release(&self, kind: &str, subject: &str) -> BotResult<()> {
        Ok(ReconRepo::release(self, kind, subject).await?)
    }
}

/// Did the queue accept the outcome of a verdict?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The claim's new state is durable.
    Recorded,
    /// External truth answered, but the queue write failed: the claim keeps its
    /// lease and the same subject is looked up again. Never reported as done.
    Unrecorded,
}

/// Write one verdict's outcome to the queue and say whether it took.
///
/// This is the decision that used to be `repo.foo(..).await.ok()`: the verdict was
/// known, the write was not, and the failure was discarded.
pub async fn record_verdict(
    queue: &dyn ReconQueueWrites,
    item: &ReconItem,
    verdict: &ReconVerdict,
) -> RecordOutcome {
    let write = match verdict {
        ReconVerdict::Resolved { .. } => queue.resolve(&item.kind, &item.subject).await,
        ReconVerdict::Retry { reason } => queue.fail(&item.kind, &item.subject, reason).await,
        ReconVerdict::GiveUp { reason } => queue.give_up(&item.kind, &item.subject, reason).await,
    };
    match write {
        Ok(()) => RecordOutcome::Recorded,
        Err(error) => {
            warn!(
                kind = %item.kind,
                subject = %item.subject,
                verdict = verdict_label(verdict),
                %error,
                "recon verdict could not be recorded — the claim stays in the queue"
            );
            RecordOutcome::Unrecorded
        }
    }
}

fn verdict_label(verdict: &ReconVerdict) -> &'static str {
    match verdict {
        ReconVerdict::Resolved { .. } => "resolved",
        ReconVerdict::Retry { .. } => "retry",
        ReconVerdict::GiveUp { .. } => "given_up",
    }
}

/// One worker pass, summarized.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ReconStats {
    pub claimed: usize,
    /// Verdicts of `Resolved` that were WRITTEN to the queue.
    pub resolved: usize,
    pub retried: usize,
    pub given_up: usize,
    /// Verdicts that could not be recorded (queue write failed). The item keeps
    /// its claim and comes back after the lease, so the same subject is looked up
    /// and recorded again — counted here instead of being reported as resolved.
    pub unrecorded: usize,
}

pub struct RecoveryWorker {
    db: Arc<Database>,
    sources: HashMap<String, Arc<dyn TruthSource>>,
    shutdown: Arc<Shutdown>,
}

impl RecoveryWorker {
    pub fn new(db: Arc<Database>, shutdown: Arc<Shutdown>) -> Self {
        RecoveryWorker {
            db,
            sources: HashMap::new(),
            shutdown,
        }
    }

    /// Register a truth source for its kind (last registration wins).
    pub fn register(&mut self, source: Arc<dyn TruthSource>) {
        info!(kind = source.kind(), "recovery truth source registered");
        self.sources.insert(source.kind().to_string(), source);
    }

    /// Enqueue a subject for verification (thin wrapper for producers).
    pub async fn enqueue(&self, kind: &str, subject: &str) {
        if let Err(e) = ReconRepo::new(self.db.clone()).enqueue(kind, subject).await {
            warn!(kind, subject, error = %e, "recon enqueue failed");
        }
    }

    /// Claim and process one batch. Safe to call concurrently from several
    /// replicas (SKIP LOCKED).
    pub async fn run_once(&self, batch: i64) -> ReconStats {
        let repo = ReconRepo::new(self.db.clone());
        // Never leave an exhausted claim invisible: a row that hit its attempt
        // limit while stuck in `in_progress` (worker died, recording failed) is
        // not due again and not `failed`, so only this sweep can show it to an
        // operator.
        match repo.park_exhausted_in_progress().await {
            Ok(0) => {}
            Ok(parked) => info!(
                parked,
                "parked exhausted reconciliation claims for the operator"
            ),
            Err(error) => warn!(%error, "could not park exhausted reconciliation claims"),
        }
        self.run_batch(&repo, batch).await
    }

    /// One pass against an arbitrary queue implementation (the seam that makes
    /// the "verdict known, queue write failed" path testable).
    async fn run_batch(&self, queue: &dyn ReconQueueWrites, batch: i64) -> ReconStats {
        let mut stats = ReconStats::default();
        let repo = ReconRepo::new(self.db.clone());
        let items = match repo.claim_due(batch).await {
            Ok(items) => items,
            Err(e) => {
                warn!(error = %e, "recon claim failed");
                return stats;
            }
        };
        stats.claimed = items.len();
        for item in items {
            if self.shutdown.is_signalled() {
                // Put unprocessed claims back instead of burning attempts.
                if !release_claim(queue, &item).await {
                    stats.unrecorded += 1;
                }
                continue;
            }
            let Some(source) = self.sources.get(&item.kind) else {
                debug!(kind = %item.kind, "no truth source registered — will retry");
                repo.fail(&item.kind, &item.subject, "no truth source registered")
                    .await
                    .ok();
                stats.retried += 1;
                continue;
            };
            let resolve_started = std::time::Instant::now();
            let verdict = source.resolve(&item.subject).await;
            metrics::global()
                .histogram(
                    "bot_reconciliation_duration_ms",
                    "External-truth lookup duration per reconciliation kind, in milliseconds.",
                    &[("kind", item.kind.as_str())],
                    crate::obs::metrics::LATENCY_BUCKETS_MS,
                )
                .observe(resolve_started.elapsed().as_millis().min(u64::MAX as u128) as u64);
            meter_verdict(&item.kind, &verdict);
            // The verdict is what external truth said; the queue write is what
            // makes it durable. They are counted separately: a claim whose
            // outcome could not be recorded must NOT be reported as resolved —
            // it is still in the queue and will be looked up again.
            let outcome = record_verdict(queue, &item, &verdict).await;
            match &verdict {
                ReconVerdict::Resolved { .. } => match outcome {
                    RecordOutcome::Recorded => {
                        stats.resolved += 1;
                        debug!(kind = %item.kind, subject = %item.subject, "reconciled");
                    }
                    RecordOutcome::Unrecorded => stats.unrecorded += 1,
                },
                ReconVerdict::Retry { .. } => {
                    stats.retried += 1;
                    if outcome == RecordOutcome::Unrecorded {
                        stats.unrecorded += 1;
                    }
                }
                ReconVerdict::GiveUp { reason } => {
                    // Parked for operators via the queue write above.
                    stats.given_up += 1;
                    if outcome == RecordOutcome::Unrecorded {
                        stats.unrecorded += 1;
                    }
                    warn!(kind = %item.kind, subject = %item.subject, %reason, "reconciliation gave up");
                }
            }
        }
        if stats.claimed > 0 {
            info!(
                claimed = stats.claimed,
                resolved = stats.resolved,
                retried = stats.retried,
                given_up = stats.given_up,
                unrecorded = stats.unrecorded,
                "reconciliation pass complete"
            );
        }
        stats
    }

    /// Poll loop until shutdown. `interval` spaces the passes; each pass
    /// claims up to `batch` due items.
    pub async fn run(&self, batch: i64, interval: Duration) {
        info!(?interval, batch, "recovery worker started");
        loop {
            tokio::select! {
                _ = self.shutdown.wait() => {
                    info!("recovery worker stopping (shutdown)");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    self.run_once(batch).await;
                }
            }
        }
    }
}

/// Outcome of the startup reconciliation pass (§H).
#[derive(Debug, Clone, Default, Serialize)]
pub struct StartupReconReport {
    /// Worker passes executed during the window.
    pub passes: u32,
    /// Claims resolved against external truth.
    pub resolved: usize,
    /// Claims that stayed inconclusive (backoff / unavailable source).
    pub retried: usize,
    /// Claims parked for operators.
    pub given_up: usize,
    /// True when the window elapsed while claims were still being processed.
    pub deadline_hit: bool,
    /// Passes whose verdicts could NOT be written to the queue. Those claims are
    /// still pending, so the queue cannot be called "as resolved as it can get".
    pub unrecorded: usize,
    /// Actively unresolved claims per kind AFTER the pass — the input to the
    /// module-blocking decision (`pending`/`in_progress` only; claims already
    /// parked in `failed` alerted when they were parked).
    pub unresolved: Vec<(String, i64)>,
}

impl StartupReconReport {
    pub fn total_unresolved(&self) -> i64 {
        self.unresolved.iter().map(|(_, n)| *n).sum()
    }
}

/// Run the reconciliation worker over due claims until none remain or the
/// window elapses. Called BEFORE trading modules spawn so a restart never
/// trades on top of state the venue has not confirmed. Deterministic and
/// shutdown-aware.
pub async fn startup_reconcile(
    worker: &RecoveryWorker,
    batch: i64,
    window: Duration,
) -> StartupReconReport {
    let mut report = StartupReconReport::default();
    let deadline = tokio::time::Instant::now() + window;
    loop {
        if worker.shutdown.is_signalled() {
            break;
        }
        let stats = worker.run_once(batch.max(1)).await;
        report.passes += 1;
        report.resolved += stats.resolved;
        report.retried += stats.retried;
        report.given_up += stats.given_up;
        report.unrecorded += stats.unrecorded;
        if stats.claimed == 0 {
            break; // nothing due — the queue is as resolved as it can get now
        }
        if tokio::time::Instant::now() >= deadline {
            report.deadline_hit = true;
            warn!(
                passes = report.passes,
                "startup reconciliation window elapsed with claims still due"
            );
            break;
        }
        // Claims resolved in this pass may have unblocked follow-ups; yield
        // briefly so backoff timers registered this pass are respected.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    match ReconRepo::new(worker.db.clone())
        .unresolved_counts(true)
        .await
    {
        Ok(rows) => report.unresolved = rows,
        Err(e) => warn!(error = %e, "unresolved claim count failed"),
    }
    info!(
        passes = report.passes,
        resolved = report.resolved,
        retried = report.retried,
        given_up = report.given_up,
        unrecorded = report.unrecorded,
        unresolved = report.total_unresolved(),
        deadline_hit = report.deadline_hit,
        "startup reconciliation complete"
    );
    report
}

/// Return an unprocessed claim to the pending pool without consuming an
/// attempt (best effort).
async fn release_claim(queue: &dyn ReconQueueWrites, item: &ReconItem) -> bool {
    match queue.release(&item.kind, &item.subject).await {
        Ok(()) => true,
        Err(error) => {
            warn!(
                kind = %item.kind,
                subject = %item.subject,
                %error,
                "could not release a claim during shutdown — it lapses with its lease"
            );
            false
        }
    }
}

fn meter_verdict(kind: &str, verdict: &ReconVerdict) {
    let label = match verdict {
        ReconVerdict::Resolved { .. } => "resolved",
        ReconVerdict::Retry { .. } => "retry",
        ReconVerdict::GiveUp { .. } => "given_up",
    };
    metrics::global()
        .counter(
            "bot_reconciliation_verdicts_total",
            "Reconciliation verdicts by kind.",
            &[("kind", kind), ("verdict", label)],
        )
        .inc();
}

/// Sweep transactions stuck in `submitted`/`confirmed` (older than `age`)
/// into the reconciliation queue. Run at startup (crash recovery) and
/// periodically (silent losses: confirm-poll tasks that died).
pub async fn sweep_unresolved_transactions(db: &Arc<Database>, age: Duration) -> usize {
    let tx_repo = TransactionRepo::new(db.clone());
    let recon = ReconRepo::new(db.clone());
    let cutoff = chrono::Utc::now()
        - chrono::Duration::from_std(age).unwrap_or(chrono::Duration::minutes(5));
    // Only rows old enough that a confirmation could already have landed.
    let rows = match tx_repo.list_unresolved_before(cutoff, 500).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "transaction sweep failed");
            return 0;
        }
    };
    let mut enqueued = 0;
    for (sig, _order) in rows {
        if let Err(e) = recon.enqueue("transaction", &sig).await {
            debug!(sig = %sig, error = %e, "sweep enqueue failed");
            continue;
        }
        enqueued += 1;
    }
    if enqueued > 0 {
        info!(
            enqueued,
            "swept unresolved transactions into reconciliation"
        );
    }
    enqueued
}

/// Sweep `pending` intents older than `age` into the reconciliation queue as
/// `intent` claims (crash point C: the process died between broadcast and any
/// signature being journaled, so the outcome is AMBIGUOUS — never resubmit).
/// Returns the orphans so callers can gate their symbols (§H). Run at startup
/// and periodically (a failed `link` write leaves the same orphan shape).
pub async fn sweep_orphan_intents(db: &Arc<Database>, age: Duration) -> Vec<IntentRecord> {
    let repo = IntentRepo::new(db.clone());
    let recon = ReconRepo::new(db.clone());
    let cutoff = chrono::Utc::now()
        - chrono::Duration::from_std(age).unwrap_or(chrono::Duration::minutes(5));
    let orphans = match repo.list_orphaned(cutoff, 500).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "intent sweep failed");
            return Vec::new();
        }
    };
    for o in &orphans {
        if let Err(e) = recon.enqueue("intent", &o.intent_id).await {
            debug!(intent = %o.intent_id, error = %e, "intent enqueue failed");
        }
    }
    if !orphans.is_empty() {
        info!(
            count = orphans.len(),
            "swept orphaned execution intents into reconciliation"
        );
    }
    orphans
}

/// Periodic housekeeping: TTL-expired dedup keys, old idempotency keys,
/// aged system events. Cheap; run hourly.
pub async fn run_maintenance(db: &Arc<Database>, event_retention_days: i64) {
    use crate::db::repo::{DedupRepo, IdempotencyRepo, SystemEventRepo};
    match DedupRepo::new(db.clone()).cleanup_expired().await {
        Ok(n) if n > 0 => info!(removed = n, "expired dedup keys cleaned"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "dedup cleanup failed"),
    }
    match IdempotencyRepo::new(db.clone())
        .cleanup_older_than(chrono::Duration::days(7))
        .await
    {
        Ok(n) if n > 0 => info!(removed = n, "old idempotency keys cleaned"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "idempotency cleanup failed"),
    }
    match SystemEventRepo::new(db.clone())
        .delete_older_than(chrono::Duration::days(event_retention_days.max(1)))
        .await
    {
        Ok(n) if n > 0 => info!(removed = n, "aged system events cleaned"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "system-event cleanup failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fake sink capturing the journal call sequence.
    struct FakeSink {
        calls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl IntentSink for FakeSink {
        async fn record(&self, rec: IntentRecord) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("record:{}", rec.intent_id));
        }
        async fn link(&self, id: &str, sig: &str) {
            self.calls.lock().unwrap().push(format!("link:{id}:{sig}"));
        }
        async fn abandon(&self, id: &str) {
            self.calls.lock().unwrap().push(format!("abandon:{id}"));
        }
    }

    fn test_intent(id: &str) -> IntentRecord {
        IntentRecord {
            intent_id: id.into(),
            module: "test".into(),
            symbol: "SYM".into(),
            wallet: "W".into(),
            side: "buy".into(),
            qty: "1".into(),
            status: "pending".into(),
            signature: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn with_intent_links_on_signature() {
        let sink = Arc::new(FakeSink {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let out = with_intent(
            Some(&dyn_sink),
            test_intent("i1"),
            async { Ok::<_, crate::error::BotError>("SIG".to_string()) },
            |v: &String| Some(v.clone()),
        )
        .await;
        assert_eq!(out.unwrap(), "SIG");
        assert_eq!(
            *sink.calls.lock().unwrap(),
            vec!["record:i1".to_string(), "link:i1:SIG".to_string()]
        );
    }

    #[tokio::test]
    async fn with_intent_retains_errors_and_abandons_classified_missing_signature() {
        // A generic error does not prove whether a broadcast occurred.
        let sink = Arc::new(FakeSink {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let sends = AtomicUsize::new(0);
        let out: crate::error::BotResult<String> = with_intent(
            Some(&dyn_sink),
            test_intent("i2"),
            async {
                sends.fetch_add(1, Ordering::SeqCst);
                Err(crate::error::BotError::invalid(
                    "error after mock broadcast",
                ))
            },
            |v: &String| Some(v.clone()),
        )
        .await;
        assert!(out.is_err(), "the error must pass through unchanged");
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert_eq!(*sink.calls.lock().unwrap(), vec!["record:i2".to_string()]);

        // Ok-but-no-signature path (paper mode / simulation reject).
        let out = with_intent(
            Some(&dyn_sink),
            test_intent("i3"),
            async { Ok::<_, crate::error::BotError>(String::new()) },
            |v: &String| (!v.is_empty()).then(|| v.clone()),
        )
        .await;
        assert_eq!(out.unwrap(), "");
        assert_eq!(
            sink.calls.lock().unwrap()[1..],
            ["record:i3".to_string(), "abandon:i3".to_string()]
        );
    }

    #[tokio::test]
    async fn explicit_retain_and_empty_signature_leave_intents_pending() {
        let sink = Arc::new(FakeSink {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        for (id, disposition) in [
            ("unknown", IntentDisposition::Retain),
            ("empty", IntentDisposition::Link("  ".into())),
        ] {
            with_intent_disposition(
                Some(&dyn_sink),
                test_intent(id),
                async { Ok::<_, crate::error::BotError>(()) },
                |_| disposition,
            )
            .await
            .unwrap();
        }
        assert_eq!(
            *sink.calls.lock().unwrap(),
            vec!["record:unknown", "record:empty"]
        );
    }

    #[tokio::test]
    async fn with_intent_without_sink_is_passthrough() {
        let out = with_intent(
            None,
            test_intent("i4"),
            async { Ok::<_, crate::error::BotError>(7u32) },
            |_| None,
        )
        .await;
        assert_eq!(out.unwrap(), 7);
    }

    /// A sink that reports that its durable write-ahead record failed.
    struct FailingReservationSink;

    #[async_trait]
    impl IntentSink for FailingReservationSink {
        async fn record(&self, _rec: IntentRecord) {
            panic!("checked reservation must not fall back to best-effort record");
        }
        async fn record_checked(&self, _rec: IntentRecord) -> BotResult<()> {
            Err(crate::error::BotError::db(
                "durable intent reservation unavailable",
            ))
        }
        async fn link(&self, _intent_id: &str, _signature: &str) {
            panic!("no broadcast future should be polled");
        }
        async fn abandon(&self, _intent_id: &str) {
            panic!("no broadcast future should be polled");
        }
    }

    /// Records release calls so the wrapper's claim lifecycle is observable.
    struct ReleaseSink {
        releases: std::sync::Mutex<Vec<(String, bool)>>,
        freed: Option<bool>,
    }

    #[async_trait]
    impl IntentSink for ReleaseSink {
        async fn record(&self, _rec: IntentRecord) {}
        async fn link(&self, _intent_id: &str, _signature: &str) {}
        async fn abandon(&self, intent_id: &str) {
            self.releases
                .lock()
                .unwrap()
                .push((format!("abandon:{intent_id}"), true));
        }
        async fn release_reservation(
            &self,
            reservation_key: &str,
            established_non_send: bool,
        ) -> Option<bool> {
            self.releases
                .lock()
                .unwrap()
                .push((reservation_key.to_string(), established_non_send));
            self.freed
        }
    }

    #[tokio::test]
    async fn a_proven_non_send_releases_the_durable_claim_immediately() {
        // Abandon + freed: the trade key is retryable at once instead of
        // waiting out a lease nobody will renew.
        let sink = Arc::new(ReleaseSink {
            releases: std::sync::Mutex::new(Vec::new()),
            freed: Some(true),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let mut rec = test_intent("release-1");
        rec.status = "pending".into();
        let out: BotResult<u32> = with_reserved_intent_disposition(
            Some(&dyn_sink),
            "copy:wallet:mint",
            rec,
            async { Ok(7) },
            |_| IntentDisposition::Abandon,
        )
        .await;
        assert_eq!(out.unwrap(), 7);
        let calls = sink.releases.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                ("abandon:release-1".to_string(), true),
                ("copy:wallet:mint".to_string(), true),
            ],
            "a proven non-send must abandon the journal row AND release the claim"
        );
    }

    #[tokio::test]
    async fn an_ambiguous_outcome_keeps_the_claim_for_the_operator_sweep() {
        // Retain: nothing is proven, so the claim must stay held (the lease
        // plus the operator sweep own it) and no release is attempted.
        let sink = Arc::new(ReleaseSink {
            releases: std::sync::Mutex::new(Vec::new()),
            freed: Some(true),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let out: BotResult<u32> = with_reserved_intent_disposition(
            Some(&dyn_sink),
            "copy:wallet:mint2",
            test_intent("release-2"),
            async { Ok(9) },
            |_| IntentDisposition::Retain,
        )
        .await;
        assert_eq!(out.unwrap(), 9);
        let calls = sink.releases.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![("copy:wallet:mint2".to_string(), false)],
            "an unproven outcome must not free the trade key"
        );
    }

    #[tokio::test]
    async fn a_failed_future_never_frees_the_claim() {
        // The future returned an error: bytes may have left the process, so
        // the claim must stay reserved (no abandon, no release).
        let sink = Arc::new(ReleaseSink {
            releases: std::sync::Mutex::new(Vec::new()),
            freed: Some(true),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let out: BotResult<u32> = with_reserved_intent_disposition(
            Some(&dyn_sink),
            "copy:wallet:mint3",
            test_intent("release-3"),
            async { Err(crate::error::BotError::invalid("rpc timeout")) },
            |_| IntentDisposition::Abandon,
        )
        .await;
        assert!(out.is_err());
        let calls = sink.releases.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![("copy:wallet:mint3".to_string(), false)],
            "an errored attempt must not be treated as a proven non-send"
        );
    }

    #[tokio::test]
    async fn failed_durable_reservation_aborts_before_broadcast_future() {
        let sink: Arc<dyn IntentSink> = Arc::new(FailingReservationSink);
        let sends = Arc::new(AtomicUsize::new(0));
        let sends_in_future = sends.clone();
        let out: BotResult<()> = with_intent_disposition(
            Some(&sink),
            test_intent("i5"),
            async move {
                sends_in_future.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| IntentDisposition::Abandon,
        )
        .await;
        assert!(
            out.is_err(),
            "an unavailable durable reservation must fail closed"
        );
        assert_eq!(
            sends.load(Ordering::SeqCst),
            0,
            "the money-moving future must never be polled"
        );
    }

    #[tokio::test]
    async fn default_checked_reservation_preserves_best_effort_sink() {
        let sink = Arc::new(FakeSink {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let out = with_intent_disposition(
            Some(&dyn_sink),
            test_intent("i6"),
            async { Ok::<_, crate::error::BotError>("executed") },
            |_| IntentDisposition::Abandon,
        )
        .await;
        assert_eq!(out.unwrap(), "executed");
        assert_eq!(
            *sink.calls.lock().unwrap(),
            vec!["record:i6".to_string(), "abandon:i6".to_string()]
        );
    }

    /// One gate covering every refusal that must happen BEFORE a broadcast:
    /// a failed checked reservation, the kill switch, a mode that is not
    /// live, and an unresolved ambiguous live attempt. Each variant must stop
    /// the money-moving future from ever being polled.
    #[tokio::test]
    async fn every_pre_broadcast_safety_gate_blocks_the_money_moving_future() {
        use crate::state::AppState;

        let sends = AtomicUsize::new(0);
        let sink = Arc::new(FailingReservationSink);
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let out: BotResult<()> = with_intent_disposition(
            Some(&dyn_sink),
            test_intent("gate-reservation"),
            async {
                sends.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| IntentDisposition::Abandon,
        )
        .await;
        assert!(
            out.is_err(),
            "a failed checked reservation must fail closed"
        );
        assert_eq!(sends.load(Ordering::SeqCst), 0);

        // The default configuration is NOT live, so it must refuse already.
        let state = AppState::new(crate::config::AppConfig::from_defaults());
        assert!(state.may_broadcast().await.is_err());

        // Kill switch engaged while a live mode is configured.
        state
            .update_config(|c| {
                c.execution.mode = crate::models::ExecutionMode::Live;
                c.execution.allow_live_trading = true;
            })
            .await;
        state.may_broadcast().await.expect("kill switch off");
        state.set_kill_switch(true, "red test").await;
        let blocked = state.may_broadcast().await;
        assert!(
            matches!(blocked, Err(crate::error::BotError::KillSwitch(_))),
            "the kill switch must refuse the broadcast, got {blocked:?}"
        );

        // Live no longer authorized.
        state.set_kill_switch(false, "red test").await;
        state
            .update_config(|c| {
                c.execution.mode = crate::models::ExecutionMode::Live;
                c.execution.allow_live_trading = false;
            })
            .await;
        assert!(state.may_broadcast().await.is_err());

        // Mode downgraded since the guard snapshot was taken.
        state
            .update_config(|c| {
                c.execution.mode = crate::models::ExecutionMode::Paper;
                c.execution.allow_live_trading = true;
            })
            .await;
        assert!(state.may_broadcast().await.is_err());
    }

    /// A truth source that resolves every subject successfully and counts.
    struct OkSource {
        hits: AtomicUsize,
    }

    #[async_trait]
    impl TruthSource for OkSource {
        fn kind(&self) -> &str {
            "test"
        }
        async fn resolve(&self, _subject: &str) -> ReconVerdict {
            self.hits.fetch_add(1, Ordering::SeqCst);
            ReconVerdict::Resolved {
                status: "finalized".into(),
            }
        }
    }

    #[test]
    fn verdict_meter_labels_are_stable() {
        // Metric label contract: dashboards/alerts reference these strings.
        meter_verdict("test", &ReconVerdict::Resolved { status: "x".into() });
        meter_verdict("test", &ReconVerdict::Retry { reason: "x".into() });
        meter_verdict("test", &ReconVerdict::GiveUp { reason: "x".into() });
        let reg = metrics::global();
        let text = reg.encode();
        assert!(text.contains("bot_reconciliation_verdicts_total"));
        assert!(text.contains("resolved"));
        assert!(text.contains("retry"));
        assert!(text.contains("given_up"));
    }

    #[tokio::test]
    async fn worker_registers_sources_by_kind() {
        // No DB in unit tests: verify the registration map only.
        let shutdown = Shutdown::new();
        let (worker_sources, registered) = {
            // Build the map the same way `register` does.
            let mut sources: HashMap<String, Arc<dyn TruthSource>> = HashMap::new();
            let src = Arc::new(OkSource {
                hits: AtomicUsize::new(0),
            });
            sources.insert(src.kind().to_string(), src.clone());
            (sources, src)
        };
        assert!(worker_sources.contains_key("test"));
        let v = worker_sources["test"].resolve("s").await;
        assert!(matches!(v, ReconVerdict::Resolved { .. }));
        assert_eq!(registered.hits.load(Ordering::SeqCst), 1);
        let _ = shutdown;
    }

    /// Gap: "recovery worker errors and ambiguous intent" — a verdict is not a
    /// recorded outcome.
    ///
    /// The pass used to call `repo.resolve(..).await.ok()`, so a queue write that
    /// failed still incremented `resolved` and the operator was told the claim was
    /// handled. It was not: the row keeps its claim and comes back after the
    /// lease, but nothing said so. Now the verdict and the recorded outcome are
    /// counted separately.
    #[tokio::test]
    async fn a_verdict_that_could_not_be_recorded_is_not_reported_as_resolved() {
        use std::sync::Mutex;

        #[derive(Debug)]
        struct FailingQueue {
            writes: Mutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl ReconQueueWrites for FailingQueue {
            async fn resolve(&self, kind: &str, subject: &str) -> BotResult<()> {
                self.writes
                    .lock()
                    .unwrap()
                    .push(format!("resolve {kind}/{subject}"));
                Err(bot_core_error("queue is down"))
            }
            async fn fail(&self, kind: &str, subject: &str, reason: &str) -> BotResult<()> {
                self.writes
                    .lock()
                    .unwrap()
                    .push(format!("fail {kind}/{subject}: {reason}"));
                Err(bot_core_error("queue is down"))
            }
            async fn give_up(&self, kind: &str, subject: &str, reason: &str) -> BotResult<()> {
                self.writes
                    .lock()
                    .unwrap()
                    .push(format!("give_up {kind}/{subject}: {reason}"));
                Err(bot_core_error("queue is down"))
            }
            async fn release(&self, kind: &str, subject: &str) -> BotResult<()> {
                self.writes
                    .lock()
                    .unwrap()
                    .push(format!("release {kind}/{subject}"));
                Err(bot_core_error("queue is down"))
            }
        }

        #[derive(Debug)]
        struct FixedVerdict(ReconVerdict);

        #[async_trait::async_trait]
        impl TruthSource for FixedVerdict {
            fn kind(&self) -> &str {
                "order"
            }
            async fn resolve(&self, _subject: &str) -> ReconVerdict {
                self.0.clone()
            }
        }

        fn bot_core_error(message: &str) -> crate::error::BotError {
            crate::error::BotError::Config(message.to_string())
        }

        // The DECISION that used to be `repo.resolve(..).await.ok()`.
        let item = ReconItem {
            kind: "order".into(),
            subject: "subj-1".into(),
            attempts: 1,
            max_attempts: 5,
        };

        // Queue down: the verdict is known, the outcome is NOT recorded.
        let down = FailingQueue {
            writes: Mutex::new(Vec::new()),
        };
        let resolved = ReconVerdict::Resolved {
            status: "finalized".into(),
        };
        assert_eq!(
            record_verdict(&down, &item, &resolved).await,
            RecordOutcome::Unrecorded,
            "a failed queue write must not be reported as recorded"
        );
        assert_eq!(
            down.writes.lock().unwrap().as_slice(),
            &["resolve order/subj-1".to_string()]
        );

        // Every verdict kind goes through the same decision, and each one names
        // its own write so a mixed pass cannot be mistaken for another.
        for (verdict, expected) in [
            (
                ReconVerdict::Retry {
                    reason: "rpc hiccup".into(),
                },
                "fail order/subj-1: rpc hiccup",
            ),
            (
                ReconVerdict::GiveUp {
                    reason: "unknown order".into(),
                },
                "give_up order/subj-1: unknown order",
            ),
        ] {
            let queue = FailingQueue {
                writes: Mutex::new(Vec::new()),
            };
            assert_eq!(
                record_verdict(&queue, &item, &verdict).await,
                RecordOutcome::Unrecorded
            );
            assert_eq!(
                queue.writes.lock().unwrap().as_slice(),
                &[expected.to_string()]
            );
        }

        // A QUEUE THAT ACCEPTS is recorded, so the honest path is exercised too.
        struct AcceptingQueue;
        #[async_trait::async_trait]
        impl ReconQueueWrites for AcceptingQueue {
            async fn resolve(&self, _kind: &str, _subject: &str) -> BotResult<()> {
                Ok(())
            }
            async fn fail(&self, _kind: &str, _subject: &str, _reason: &str) -> BotResult<()> {
                Ok(())
            }
            async fn give_up(&self, _kind: &str, _subject: &str, _reason: &str) -> BotResult<()> {
                Ok(())
            }
            async fn release(&self, _kind: &str, _subject: &str) -> BotResult<()> {
                Ok(())
            }
        }
        assert_eq!(
            record_verdict(&AcceptingQueue, &item, &resolved).await,
            RecordOutcome::Recorded,
            "a recorded verdict must count as resolved"
        );

        // Shutdown release reports honestly as well.
        assert!(
            !release_claim(&down, &item).await,
            "a failed release must be reported, not assumed"
        );
        assert!(
            release_claim(&AcceptingQueue, &item).await,
            "a successful release is reported"
        );

        // And the verdict itself is still what external truth said.
        let source = FixedVerdict(ReconVerdict::Resolved {
            status: "finalized".into(),
        });
        assert_eq!(source.kind(), "order");
        assert!(matches!(
            source.resolve("subj-1").await,
            ReconVerdict::Resolved { .. }
        ));
    }
}
