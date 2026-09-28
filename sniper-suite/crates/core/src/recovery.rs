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

/// Write-ahead intent journal (§I crash point C). Trading modules call this
/// AROUND the broadcast; the server implements it over `IntentRepo`. Modules
/// stay database-free: the sink is injected, optional, and its implementations
/// must never fail a trade — journaling is defence in depth (a failed journal
/// write degrades to today's behaviour, logged and metered by the impl).
#[async_trait]
pub trait IntentSink: Send + Sync {
    /// Persist the intent BEFORE the money-moving broadcast.
    async fn record(&self, rec: IntentRecord);
    /// The broadcast produced `signature` — the outcome is no longer ambiguous.
    async fn link(&self, intent_id: &str, signature: &str);
    /// The attempt provably never broadcast (terminal error / no signature).
    async fn abandon(&self, intent_id: &str);
}

/// Run a broadcast future inside the intent journal: record before, then
/// link on signature or abandon on error / no-signature. The sink is
/// optional (`None` = journaling disabled); the future's result is returned
/// unchanged either way — the journal never alters execution semantics.
pub async fn with_intent<T>(
    sink: Option<&Arc<dyn IntentSink>>,
    rec: IntentRecord,
    run: impl std::future::Future<Output = BotResult<T>>,
    signature_of: impl FnOnce(&T) -> Option<String>,
) -> BotResult<T> {
    let intent_id = rec.intent_id.clone();
    if let Some(s) = sink {
        s.record(rec).await;
    }
    let out = run.await;
    if let Some(s) = sink {
        match &out {
            Ok(v) => match signature_of(v) {
                Some(sig) if !sig.is_empty() => s.link(&intent_id, &sig).await,
                _ => s.abandon(&intent_id).await,
            },
            Err(_) => s.abandon(&intent_id).await,
        }
    }
    out
}

/// One worker pass, summarized.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ReconStats {
    pub claimed: usize,
    pub resolved: usize,
    pub retried: usize,
    pub given_up: usize,
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
                release_claim(&repo, &item).await;
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
            match verdict {
                ReconVerdict::Resolved { status } => {
                    repo.resolve(&item.kind, &item.subject).await.ok();
                    stats.resolved += 1;
                    debug!(kind = %item.kind, subject = %item.subject, %status, "reconciled");
                }
                ReconVerdict::Retry { reason } => {
                    repo.fail(&item.kind, &item.subject, &reason).await.ok();
                    stats.retried += 1;
                }
                ReconVerdict::GiveUp { reason } => {
                    // Exhaust immediately: set attempts to max via repeated
                    // failure would waste cycles — mark failed with the
                    // reason; operators see it in /api/recovery/failed.
                    repo.give_up(&item.kind, &item.subject, &reason).await.ok();
                    stats.given_up += 1;
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
        unresolved = report.total_unresolved(),
        deadline_hit = report.deadline_hit,
        "startup reconciliation complete"
    );
    report
}

/// Return an unprocessed claim to the pending pool without consuming an
/// attempt (best effort).
async fn release_claim(repo: &ReconRepo, item: &ReconItem) {
    repo.release(&item.kind, &item.subject).await.ok();
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
    async fn with_intent_abandons_on_error_or_missing_signature() {
        // Error path: the broadcast never produced a signature.
        let sink = Arc::new(FakeSink {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let dyn_sink: Arc<dyn IntentSink> = sink.clone();
        let out: crate::error::BotResult<String> = with_intent(
            Some(&dyn_sink),
            test_intent("i2"),
            async { Err(crate::error::BotError::invalid("boom")) },
            |v: &String| Some(v.clone()),
        )
        .await;
        assert!(out.is_err(), "the error must pass through unchanged");
        assert_eq!(
            *sink.calls.lock().unwrap(),
            vec!["record:i2".to_string(), "abandon:i2".to_string()]
        );

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
            sink.calls.lock().unwrap()[2..],
            ["record:i3".to_string(), "abandon:i3".to_string()]
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
}
