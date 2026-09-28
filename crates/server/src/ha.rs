//! Server wiring of the TASK 6 HA layer.
//!
//! * [`DbHaStore`] — the PostgreSQL implementation of
//!   [`bot_core::ha::HaStore`] (migration 0016, [`HaRepo`]).
//! * [`attach`] — installs the durable store BEFORE registration and
//!   recovery (with no database the in-memory store stays: identical
//!   semantics, process scope, which is exactly single-worker mode).
//! * [`register_and_recover`] — register this worker life, move it through
//!   `Starting → Recovering`, publish the recovery records for what the
//!   TASK 1–5 restore paths decided, then `Ready`.
//! * [`spawn_heartbeat`] — heartbeat + stale-worker survey + readiness
//!   refresh.
//! * [`LeasedWorker`] — run a periodic job under a singleton role lease:
//!   acquire, renew, fence before every tick, step down on loss.
//! * [`shutdown`] — graceful shutdown (§10): stop accepting, persist
//!   cursors, release leases, audit, mark stopped.
//!
//! Nothing here decides trades or books money; it only owns *who may run
//! what* and *what a restart must do*.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tracing::{debug, info, warn};

use bot_core::db::ha::HaRepo;
use bot_core::db::Database;
use bot_core::error::BotResult;
use bot_core::ha::{
    audit as ha_audit, metrics as ha_metrics, plan_order_recovery, FeedCursor, FeedGap, GapStatus,
    HaStore, Lease, LeaseDecision, LeaseGuard, LeaseRequest, LeaseRole, LocalOrderEvidence,
    OrderRecoveryAction, RecoveryRecord, VenueOrderEvidence, WorkerRegistration, WorkerState,
};
use bot_core::lifecycle::Shutdown;
use bot_core::oms::OrderStatus;
use bot_core::state::Shared;

/// PostgreSQL-backed HA store.
pub struct DbHaStore {
    db: Arc<Database>,
}

impl DbHaStore {
    /// Store over `db`.
    pub fn new(db: Arc<Database>) -> Self {
        DbHaStore { db }
    }

    fn repo(&self) -> HaRepo {
        HaRepo::new(self.db.clone())
    }
}

#[async_trait]
impl HaStore for DbHaStore {
    fn backend(&self) -> &'static str {
        "postgres"
    }

    async fn now(&self) -> BotResult<DateTime<Utc>> {
        self.repo()
            .now()
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn register_worker(
        &self,
        worker_id: &str,
        mode: &str,
        host: &str,
        pid: i64,
        version: &str,
    ) -> BotResult<WorkerRegistration> {
        self.repo()
            .register_worker(worker_id, mode, host, pid, version)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn heartbeat(
        &self,
        worker_id: &str,
        generation: i64,
        state: WorkerState,
        detail: &str,
    ) -> BotResult<bool> {
        self.repo()
            .heartbeat(worker_id, generation, state, detail)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn workers(&self) -> BotResult<Vec<WorkerRegistration>> {
        self.repo()
            .workers()
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn acquire_lease(&self, req: &LeaseRequest) -> BotResult<LeaseDecision> {
        self.repo()
            .acquire_lease(req)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn renew_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
        ttl: ChronoDuration,
    ) -> BotResult<bool> {
        self.repo()
            .renew_lease(role, holder, generation, ttl)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn verify_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> BotResult<bool> {
        self.repo()
            .verify_lease(role, holder, generation)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn release_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> BotResult<bool> {
        self.repo()
            .release_lease(role, holder, generation)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn get_lease(&self, role: &LeaseRole) -> BotResult<Option<Lease>> {
        self.repo()
            .get_lease(role)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn leases(&self) -> BotResult<Vec<Lease>> {
        self.repo()
            .leases()
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn save_cursor(&self, cursor: &FeedCursor) -> BotResult<()> {
        self.repo()
            .save_cursor(cursor)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn load_cursor(&self, key: &str) -> BotResult<Option<FeedCursor>> {
        self.repo()
            .load_cursor(key)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn cursors(&self) -> BotResult<Vec<FeedCursor>> {
        self.repo()
            .cursors()
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn record_gap(&self, gap: &FeedGap) -> BotResult<()> {
        self.repo()
            .record_gap(gap)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn gaps(&self, unresolved_only: bool, limit: usize) -> BotResult<Vec<FeedGap>> {
        self.repo()
            .gaps(unresolved_only, limit as i64)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn resolve_gap(
        &self,
        feed: &str,
        scope: &str,
        from_position: u64,
        status: GapStatus,
    ) -> BotResult<bool> {
        self.repo()
            .resolve_gap(feed, scope, from_position, status)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn record_recovery(&self, record: &RecoveryRecord) -> BotResult<()> {
        self.repo()
            .record_recovery(record)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }

    async fn recovery_records(&self, limit: usize) -> BotResult<Vec<RecoveryRecord>> {
        self.repo()
            .recovery_records(limit as i64)
            .await
            .map_err(|e| bot_core::error::BotError::db(e.to_string()))
    }
}

/// Install the durable HA store when a database is attached. Must run
/// before [`register_and_recover`].
pub async fn attach(state: &Shared, db: Option<&Arc<Database>>) {
    if let Some(db) = db {
        state
            .ha()
            .attach_store(Arc::new(DbHaStore::new(db.clone())))
            .await;
        state.ha().set_dependency("database", true).await;
        info!("HA store attached (postgres): workers, leases, cursors, recovery journal");
    } else {
        state.ha().set_dependency("database", true).await;
        info!("HA store in memory (no database configured) — single-worker semantics");
    }
}

/// Register this worker life and walk it through recovery.
///
/// The TASK 1–5 restore paths (`persist::restore`, the execution ledger
/// rehydration, `accounting::recover`) remain the ones that rebuild state;
/// this function records WHAT they decided in the durable recovery journal
/// with the deterministic §6 vocabulary, so a post-mortem can see one action
/// per order.
pub async fn register_and_recover(
    state: &Shared,
    version: &str,
    ambiguous_intents: &[String],
) -> WorkerState {
    let ha = state.ha();
    let host = hostname();
    let pid = std::process::id() as i64;
    if let Err(e) = ha.register(&host, pid, version).await {
        warn!(error = %e, "worker registration failed — continuing with local identity only");
    }
    let generation = ha.generation().await;
    ha_audit::recovery_started(&state.events, ha.worker_id(), "startup");
    if ha
        .set_state(WorkerState::Recovering, "rebuilding durable state")
        .await
        .is_err()
    {
        warn!("worker could not enter the recovering state");
    }

    // 1. Orders: one deterministic action per unfinished order.
    let mut actions = 0usize;
    if let Some(mgr) = state.orders() {
        for order in mgr.list(5_000).await {
            let local = match order.status {
                OrderStatus::Created | OrderStatus::Validated | OrderStatus::Queued => {
                    LocalOrderEvidence::JournaledNotSent
                }
                OrderStatus::Submitted | OrderStatus::Unknown => {
                    LocalOrderEvidence::SubmittedUnknown
                }
                OrderStatus::Accepted | OrderStatus::PartiallyFilled => LocalOrderEvidence::Open,
                OrderStatus::Filled
                | OrderStatus::Failed
                | OrderStatus::Cancelled
                | OrderStatus::Expired
                | OrderStatus::Reconciled => LocalOrderEvidence::Terminal,
            };
            if local == LocalOrderEvidence::Terminal {
                continue; // nothing to decide; reconciliation owns late truth
            }
            // At startup the venue has not been read yet: that is exactly
            // `Unavailable`, which never finalizes anything.
            let plan = plan_order_recovery(local, VenueOrderEvidence::Unavailable);
            ha.record_recovery(RecoveryRecord {
                worker_id: ha.worker_id().to_string(),
                generation,
                trigger: "startup".into(),
                scope: "orders".into(),
                subject: order.id.clone(),
                action: plan.action,
                detail: plan.reason.clone(),
                ts: Utc::now(),
            })
            .await;
            actions += 1;
        }
    }

    // 2. Ambiguous execution intents the ledger rehydration handed over.
    for intent in ambiguous_intents {
        ha.record_recovery(RecoveryRecord {
            worker_id: ha.worker_id().to_string(),
            generation,
            trigger: "startup".into(),
            scope: "orders".into(),
            subject: intent.clone(),
            action: OrderRecoveryAction::HoldAmbiguous,
            detail:
                "execution ledger restart: submission outcome unknown, handed to reconciliation"
                    .into(),
            ts: Utc::now(),
        })
        .await;
        actions += 1;
    }

    // 3. Ledger / positions / risk: TASK 5 recovery already rebuilt them;
    //    record the scope-level outcome so the journal is complete.
    let book = state.ledger().book().await;
    ha.record_recovery(RecoveryRecord {
        worker_id: ha.worker_id().to_string(),
        generation,
        trigger: "startup".into(),
        scope: "ledger".into(),
        subject: String::new(),
        action: OrderRecoveryAction::NoAction,
        detail: format!(
            "global ledger rebuilt from the journal: {} aggregated positions, {} events",
            book.open_count(),
            state.ledger().len().await
        ),
        ts: Utc::now(),
    })
    .await;

    // 4. Cursors: nothing to replay at startup, but the journal records the
    //    durable positions the worker resumed from.
    let cursors = ha.store().await.cursors().await.unwrap_or_default();
    for c in &cursors {
        ha_metrics::set_cursor(c.feed, c.lag_secs(Utc::now()), c.position);
    }
    ha.record_recovery(RecoveryRecord {
        worker_id: ha.worker_id().to_string(),
        generation,
        trigger: "startup".into(),
        scope: "cursors".into(),
        subject: String::new(),
        action: OrderRecoveryAction::NoAction,
        detail: format!("resumed {} durable feed cursors", cursors.len()),
        ts: Utc::now(),
    })
    .await;

    ha.set_recovery_complete(true).await;
    let summary = format!(
        "order_actions={actions} cursors={} open_positions={}",
        cursors.len(),
        book.open_count()
    );
    ha_audit::recovery_completed(&state.events, ha.worker_id(), &summary);
    let _ = ha.set_state(WorkerState::Ready, "recovery complete").await;
    ha.refresh_readiness().await;
    info!(%summary, "HA recovery complete");
    WorkerState::Ready
}

/// Heartbeat + stale-worker survey + readiness refresh.
pub fn spawn_heartbeat(
    state: Shared,
    shutdown: Arc<Shutdown>,
    period: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(period.max(Duration::from_secs(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = ticker.tick() => {
                    let ha = state.ha();
                    if !ha.heartbeat().await {
                        // A newer generation owns our row, or the store is
                        // down: we must not keep acting as owner.
                        let _ = ha.set_state(WorkerState::LeaseLost, "heartbeat refused").await;
                    }
                    ha.survey_workers().await;
                    ha.refresh_readiness().await;
                }
            }
        }
        debug!("heartbeat loop stopped");
    })
}

/// A periodic job that may only run on ONE worker at a time.
///
/// The loop acquires the role lease, renews it on schedule, and fences
/// immediately before every tick: a worker that lost ownership steps down
/// deterministically instead of continuing to mutate shared state.
pub struct LeasedWorker {
    state: Shared,
    role: LeaseRole,
    interval: Duration,
}

impl LeasedWorker {
    /// A job for `role` that ticks every `interval`.
    pub fn new(state: Shared, role: LeaseRole, interval: Duration) -> Self {
        LeasedWorker {
            state,
            role,
            interval,
        }
    }

    /// Spawn the loop. `job` runs only while the lease is verifiably held.
    pub fn spawn<F, Fut>(self, shutdown: Arc<Shutdown>, job: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(Shared, LeaseGuard) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            let ha = self.state.ha().clone();
            let mut ticker = tokio::time::interval(self.interval.max(Duration::from_secs(1)));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut guard: Option<LeaseGuard> = None;
            let mut renew_at = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _ = shutdown.wait() => break,
                    _ = ticker.tick() => {
                        // 1. Make sure we own the role.
                        if guard.is_none() {
                            match ha.acquire(self.role.clone()).await {
                                Ok(Some(g)) => {
                                    info!(role = %self.role, generation = g.generation(), "leased worker active");
                                    renew_at = tokio::time::Instant::now()
                                        + renew_period(&ha).await;
                                    guard = Some(g);
                                }
                                Ok(None) => continue, // another worker owns it
                                Err(_) => continue,   // store down: fail closed
                            }
                        }
                        let Some(g) = guard.clone() else { continue };

                        // 2. Renew on schedule.
                        if tokio::time::Instant::now() >= renew_at {
                            if ha.renew(&g).await {
                                renew_at = tokio::time::Instant::now() + renew_period(&ha).await;
                            } else {
                                warn!(role = %self.role, "lease lost; stepping down");
                                guard = None;
                                continue;
                            }
                        }

                        // 3. Fence immediately before the work.
                        match ha.fence(&g).await {
                            Ok(()) => job(self.state.clone(), g).await,
                            Err(e) => {
                                warn!(role = %self.role, error = %e, "fenced: skipping this tick");
                                guard = None;
                            }
                        }
                    }
                }
            }
            // Release on the way out so a standby can take over at once.
            if let Some(g) = guard {
                ha.release(&g).await;
            }
            debug!(role = %self.role, "leased worker stopped");
        })
    }
}

async fn renew_period(ha: &Arc<bot_core::ha::HaRuntime>) -> Duration {
    ha.renew_every()
        .await
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(10))
}

/// Graceful shutdown of the HA layer (§10).
pub async fn shutdown(state: &Shared, reason: &str) {
    state.ha().shutdown(reason).await;
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown".into())
}
