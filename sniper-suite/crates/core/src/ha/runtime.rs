//! The HA runtime: worker registration + heartbeat, lease guards with
//! fencing, durable cursors and the readiness verdict
//! (TASK 6 §1, §2, §4, §10, §11, §12).
//!
//! One [`HaRuntime`] per process. It owns nothing business-related: it is
//! the ownership/durability substrate the server and the modules consult.
//!
//! * [`HaRuntime::register`] — register this worker life, publish gauges.
//! * [`HaRuntime::set_state`] / [`HaRuntime::heartbeat`] — the deterministic
//!   worker state machine plus the durable last-seen timestamp.
//! * [`HaRuntime::acquire`] — take a singleton role lease; returns a
//!   [`LeaseGuard`] carrying the fencing token.
//! * [`LeaseGuard::fence`] — re-verify ownership immediately before a
//!   money- or state-mutating step; a stale worker gets a deterministic
//!   [`FenceError`] and must stop.
//! * [`HaRuntime::cursor`] / [`HaRuntime::offer`] — durable feed cursors
//!   with duplicate suppression and gap detection.
//! * [`HaRuntime::readiness`] — the readiness verdict: never READY while a
//!   required lease is lost, recovery is pending or a dependency the worker
//!   needs is down.
//!
//! Everything fails closed: a store error is never treated as ownership.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::audit;
use super::cursor::{CursorAdvance, FeedCursor, FeedId, GapStatus};
use super::lease::{FenceError, Lease, LeaseDecision, LeaseRequest, LeaseRole};
use super::metrics;
use super::store::{HaStore, MemoryHaStore, RecoveryRecord};
use super::worker::{HaMode, WorkerHealth, WorkerRegistration, WorkerState, WorkerTransitionError};
use crate::events::EventBus;

/// Tuning of the HA runtime (mirrors `[ha]` config).
#[derive(Debug, Clone)]
pub struct HaSettings {
    /// How this worker participates in the cluster.
    pub mode: HaMode,
    /// Heartbeat period.
    pub heartbeat: ChronoDuration,
    /// A worker is stale after this long without a heartbeat.
    pub heartbeat_timeout: ChronoDuration,
    /// Lease duration granted per role.
    pub lease_ttl: ChronoDuration,
    /// Roles this worker must hold to report READY. Empty = none required
    /// (single-worker mode with no singleton work).
    pub required_roles: Vec<LeaseRole>,
}

impl Default for HaSettings {
    fn default() -> Self {
        HaSettings {
            mode: HaMode::Single,
            heartbeat: ChronoDuration::seconds(10),
            heartbeat_timeout: ChronoDuration::seconds(45),
            lease_ttl: ChronoDuration::seconds(45),
            required_roles: Vec::new(),
        }
    }
}

/// Why a worker is not ready (TASK 6 §11). Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReadyReason {
    /// The worker state itself is not a serving state.
    State(WorkerState),
    /// A required lease is not held.
    LeaseMissing(String),
    /// Durable recovery has not finished.
    RecoveryPending,
    /// A dependency the worker needs is unhealthy.
    Dependency(String),
}

impl NotReadyReason {
    /// Stable label (metrics / audit).
    pub fn as_str(&self) -> &'static str {
        match self {
            NotReadyReason::State(_) => "state",
            NotReadyReason::LeaseMissing(_) => "lease_missing",
            NotReadyReason::RecoveryPending => "recovery_pending",
            NotReadyReason::Dependency(_) => "dependency",
        }
    }
}

impl std::fmt::Display for NotReadyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotReadyReason::State(s) => write!(f, "worker state is {s}"),
            NotReadyReason::LeaseMissing(r) => write!(f, "required lease {r} is not held"),
            NotReadyReason::RecoveryPending => write!(f, "durable recovery has not completed"),
            NotReadyReason::Dependency(d) => write!(f, "dependency {d} is unhealthy"),
        }
    }
}

/// The readiness verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    /// Ready to take new work / traffic.
    pub ready: bool,
    /// Every reason it is not (empty when ready).
    pub reasons: Vec<NotReadyReason>,
    /// The worker state the verdict was taken on.
    pub state: WorkerState,
}

impl Readiness {
    /// Single-line text for probes and audit.
    pub fn detail(&self) -> String {
        if self.ready {
            format!("ready (state={})", self.state)
        } else {
            let list: Vec<String> = self.reasons.iter().map(|r| r.to_string()).collect();
            format!("not ready: {}", list.join("; "))
        }
    }
}

/// A held lease plus its fencing token. Dropping it does NOT release the
/// lease (a crash must leave it to expire, not vanish); call
/// [`HaRuntime::release`] for a clean handover.
#[derive(Debug, Clone)]
pub struct LeaseGuard {
    role: LeaseRole,
    holder: String,
    generation: i64,
}

impl LeaseGuard {
    /// The role this guard owns.
    pub fn role(&self) -> &LeaseRole {
        &self.role
    }

    /// The fencing token.
    pub fn generation(&self) -> i64 {
        self.generation
    }

    /// The worker that holds it.
    pub fn holder(&self) -> &str {
        &self.holder
    }
}

struct Inner {
    registration: Option<WorkerRegistration>,
    leases: HashMap<String, Lease>,
    cursors: HashMap<String, FeedCursor>,
    dependencies: HashMap<String, bool>,
    recovery_complete: bool,
    last_ready: Option<bool>,
}

/// The process-wide HA runtime.
pub struct HaRuntime {
    worker_id: String,
    settings: RwLock<HaSettings>,
    store: RwLock<Arc<dyn HaStore>>,
    bus: EventBus,
    inner: RwLock<Inner>,
    /// Set once the process entered graceful shutdown.
    draining: AtomicBool,
}

impl HaRuntime {
    /// A runtime for `worker_id` (the process `replica_id`) over the
    /// in-memory store; the server attaches the durable store at startup.
    pub fn new(worker_id: impl Into<String>, settings: HaSettings, bus: EventBus) -> Self {
        HaRuntime {
            worker_id: worker_id.into(),
            settings: RwLock::new(settings),
            store: RwLock::new(Arc::new(MemoryHaStore::new())),
            bus,
            inner: RwLock::new(Inner {
                registration: None,
                leases: HashMap::new(),
                cursors: HashMap::new(),
                dependencies: HashMap::new(),
                recovery_complete: false,
                last_ready: None,
            }),
            draining: AtomicBool::new(false),
        }
    }

    /// Replace the durable store (startup, before registration).
    pub async fn attach_store(&self, store: Arc<dyn HaStore>) {
        *self.store.write().await = store;
    }

    /// The store in use.
    pub async fn store(&self) -> Arc<dyn HaStore> {
        self.store.read().await.clone()
    }

    /// This worker's identity.
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// Current settings.
    pub async fn settings(&self) -> HaSettings {
        self.settings.read().await.clone()
    }

    /// Replace the settings (config reload).
    pub async fn update_settings(&self, settings: HaSettings) {
        *self.settings.write().await = settings;
    }

    /// The current registration, if registered.
    pub async fn registration(&self) -> Option<WorkerRegistration> {
        self.inner.read().await.registration.clone()
    }

    /// Current worker state (`Starting` before registration).
    pub async fn state(&self) -> WorkerState {
        self.inner
            .read()
            .await
            .registration
            .as_ref()
            .map(|r| r.state)
            .unwrap_or(WorkerState::Starting)
    }

    /// This worker's generation (`0` before registration).
    pub async fn generation(&self) -> i64 {
        self.inner
            .read()
            .await
            .registration
            .as_ref()
            .map(|r| r.generation)
            .unwrap_or(0)
    }

    /// Register this worker life. Publishes the audit record and gauges.
    pub async fn register(
        &self,
        host: &str,
        pid: i64,
        version: &str,
    ) -> crate::error::BotResult<WorkerRegistration> {
        let settings = self.settings().await;
        let store = self.store().await;
        let reg = store
            .register_worker(&self.worker_id, settings.mode.as_str(), host, pid, version)
            .await?;
        {
            let mut inner = self.inner.write().await;
            inner.registration = Some(reg.clone());
        }
        metrics::set_worker_state(reg.state);
        audit::worker_registered(&self.bus, &reg);
        info!(
            worker = %reg.worker_id,
            generation = reg.generation,
            mode = settings.mode.as_str(),
            backend = store.backend(),
            "worker registered"
        );
        Ok(reg)
    }

    /// Apply a state transition. Returns the new state, or the transition
    /// error when the move is illegal (nothing changes).
    pub async fn set_state(
        &self,
        to: WorkerState,
        detail: &str,
    ) -> Result<WorkerState, WorkerTransitionError> {
        let from = {
            let inner = self.inner.read().await;
            inner
                .registration
                .as_ref()
                .map(|r| r.state)
                .unwrap_or(WorkerState::Starting)
        };
        if from == to {
            return Ok(to);
        }
        if !from.can_transition_to(to) {
            warn!(%from, %to, "illegal worker state transition refused");
            return Err(WorkerTransitionError { from, to });
        }
        {
            let mut inner = self.inner.write().await;
            if let Some(r) = inner.registration.as_mut() {
                r.state = to;
                r.detail = detail.to_string();
            }
        }
        metrics::set_worker_state(to);
        metrics::count_state_change(from, to);
        audit::worker_state(&self.bus, &self.worker_id, from, to, detail);
        info!(%from, %to, detail, "worker state");
        // Persist immediately so operators see the state without waiting
        // for the next heartbeat tick.
        let generation = self.generation().await;
        let store = self.store().await;
        match store
            .heartbeat(&self.worker_id, generation, to, detail)
            .await
        {
            Ok(true) => metrics::count_heartbeat("ok"),
            Ok(false) => {
                metrics::count_heartbeat("stale");
                audit::heartbeat_failed(
                    &self.bus,
                    &self.worker_id,
                    generation,
                    "registration row belongs to a newer generation",
                );
            }
            Err(e) => {
                metrics::count_heartbeat("error");
                debug!(error = %e, "worker state could not be persisted");
            }
        }
        Ok(to)
    }

    /// Write one heartbeat. `Ok(false)` = this life was superseded (a newer
    /// generation registered): the caller must stop mutating shared state.
    pub async fn heartbeat(&self) -> bool {
        let (generation, state, detail) = {
            let inner = self.inner.read().await;
            match inner.registration.as_ref() {
                Some(r) => (r.generation, r.state, r.detail.clone()),
                None => return false,
            }
        };
        let store = self.store().await;
        match store
            .heartbeat(&self.worker_id, generation, state, &detail)
            .await
        {
            Ok(true) => {
                metrics::count_heartbeat("ok");
                if let Some(r) = self.inner.write().await.registration.as_mut() {
                    r.last_seen_at = Utc::now();
                }
                true
            }
            Ok(false) => {
                metrics::count_heartbeat("stale");
                audit::heartbeat_failed(
                    &self.bus,
                    &self.worker_id,
                    generation,
                    "registration row belongs to a newer generation",
                );
                false
            }
            Err(e) => {
                metrics::count_heartbeat("error");
                audit::heartbeat_failed(&self.bus, &self.worker_id, generation, &e.to_string());
                warn!(error = %e, "heartbeat could not be written");
                false
            }
        }
    }

    /// Classify the registry: `(live, stale, stopped)` plus the stale rows.
    /// Detection only — nothing is taken over here (§1).
    pub async fn survey_workers(&self) -> (usize, usize, usize, Vec<WorkerRegistration>) {
        let settings = self.settings().await;
        let store = self.store().await;
        // The shared store clock is the reference: worker clocks drift.
        let now = store.now().await.unwrap_or_else(|_| Utc::now());
        let workers = store.workers().await.unwrap_or_default();
        let (mut live, mut stale, mut stopped) = (0, 0, 0);
        let mut stale_rows = Vec::new();
        for w in workers {
            match WorkerHealth::of(&w, now, settings.heartbeat_timeout) {
                WorkerHealth::Live => live += 1,
                WorkerHealth::Stale => {
                    stale += 1;
                    if w.worker_id != self.worker_id {
                        audit::stale_worker(&self.bus, &w, w.age_secs(now));
                    }
                    stale_rows.push(w);
                }
                WorkerHealth::Stopped => stopped += 1,
            }
        }
        metrics::set_workers_seen(live, stale, stopped);
        (live, stale, stopped, stale_rows)
    }

    /// Try to acquire a singleton role lease.
    pub async fn acquire(
        &self,
        role: LeaseRole,
    ) -> Result<Option<LeaseGuard>, crate::error::BotError> {
        let settings = self.settings().await;
        let store = self.store().await;
        let req = LeaseRequest {
            role: role.clone(),
            holder: self.worker_id.clone(),
            ttl: settings.lease_ttl,
        };
        match store.acquire_lease(&req).await {
            Ok(LeaseDecision::Acquired(lease)) => {
                let takeover = lease
                    .previous_holder
                    .as_deref()
                    .map(|p| p != self.worker_id)
                    .unwrap_or(false);
                metrics::count_lease_op(&role, "acquire", "ok");
                metrics::set_lease_held(&role, true);
                if takeover {
                    metrics::count_takeover(&role);
                }
                audit::lease_acquired(&self.bus, &lease, takeover);
                info!(
                    role = %role,
                    generation = lease.generation,
                    takeover,
                    "lease acquired"
                );
                let guard = LeaseGuard {
                    role: role.clone(),
                    holder: self.worker_id.clone(),
                    generation: lease.generation,
                };
                self.inner
                    .write()
                    .await
                    .leases
                    .insert(role.as_string(), lease);
                Ok(Some(guard))
            }
            Ok(LeaseDecision::Rejected { holder, .. }) => {
                metrics::count_lease_op(&role, "acquire", "rejected");
                metrics::set_lease_held(&role, false);
                debug!(role = %role, %holder, "lease held by another worker");
                Ok(None)
            }
            Err(e) => {
                metrics::count_lease_op(&role, "acquire", "error");
                metrics::set_lease_held(&role, false);
                warn!(role = %role, error = %e, "lease acquisition failed");
                Err(e)
            }
        }
    }

    /// Renew a held lease. `false` = ownership lost (the caller must stop
    /// and, if it wants the role back, re-acquire).
    pub async fn renew(&self, guard: &LeaseGuard) -> bool {
        let settings = self.settings().await;
        let store = self.store().await;
        match store
            .renew_lease(
                &guard.role,
                &guard.holder,
                guard.generation,
                settings.lease_ttl,
            )
            .await
        {
            Ok(true) => {
                metrics::count_lease_op(&guard.role, "renew", "ok");
                if let Some(l) = self
                    .inner
                    .write()
                    .await
                    .leases
                    .get_mut(&guard.role.as_string())
                {
                    l.renewed_at = Utc::now();
                    l.expires_at = Utc::now() + settings.lease_ttl;
                }
                true
            }
            Ok(false) => {
                metrics::count_lease_op(&guard.role, "renew", "lost");
                metrics::set_lease_held(&guard.role, false);
                self.inner
                    .write()
                    .await
                    .leases
                    .remove(&guard.role.as_string());
                audit::lease_lost(
                    &self.bus,
                    &guard.role,
                    &guard.holder,
                    guard.generation,
                    "renewal refused: taken over, expired or released",
                );
                warn!(role = %guard.role, "lease lost");
                false
            }
            Err(e) => {
                metrics::count_lease_op(&guard.role, "renew", "error");
                warn!(role = %guard.role, error = %e, "lease renewal failed");
                false
            }
        }
    }

    /// Fencing check (§12): re-verify ownership immediately before a
    /// mutation. `Err` means the caller is stale and must not proceed.
    pub async fn fence(&self, guard: &LeaseGuard) -> Result<(), FenceError> {
        let store = self.store().await;
        match store
            .verify_lease(&guard.role, &guard.holder, guard.generation)
            .await
        {
            Ok(true) => {
                metrics::count_lease_op(&guard.role, "verify", "ok");
                Ok(())
            }
            Ok(false) => {
                let actual = store
                    .get_lease(&guard.role)
                    .await
                    .ok()
                    .flatten()
                    .map(|l| (l.holder, l.generation));
                let err = match &actual {
                    Some((holder, gen)) if holder == &guard.holder && *gen == guard.generation => {
                        FenceError::Expired {
                            role: guard.role.as_string(),
                            presented: guard.generation,
                        }
                    }
                    _ => FenceError::Fenced {
                        role: guard.role.as_string(),
                        presented: guard.generation,
                        actual,
                    },
                };
                metrics::count_lease_op(&guard.role, "verify", "lost");
                metrics::count_fenced(guard.role.kind(), err.reason());
                metrics::set_lease_held(&guard.role, false);
                self.inner
                    .write()
                    .await
                    .leases
                    .remove(&guard.role.as_string());
                audit::fenced(&self.bus, &err);
                warn!(role = %guard.role, error = %err, "fenced mutation refused");
                Err(err)
            }
            Err(e) => {
                let err = FenceError::StoreUnavailable {
                    role: guard.role.as_string(),
                    detail: e.to_string(),
                };
                metrics::count_lease_op(&guard.role, "verify", "error");
                metrics::count_fenced(guard.role.kind(), err.reason());
                audit::fenced(&self.bus, &err);
                Err(err)
            }
        }
    }

    /// Run `work` only while the lease is verifiably held. The fence is
    /// checked BEFORE the work; the caller re-fences inside long work.
    pub async fn guarded<F, T>(&self, guard: &LeaseGuard, work: F) -> Result<T, FenceError>
    where
        F: std::future::Future<Output = T>,
    {
        self.fence(guard).await?;
        Ok(work.await)
    }

    /// Release a lease cleanly (graceful shutdown, role handover).
    pub async fn release(&self, guard: &LeaseGuard) -> bool {
        let store = self.store().await;
        let ok = store
            .release_lease(&guard.role, &guard.holder, guard.generation)
            .await
            .unwrap_or(false);
        metrics::count_lease_op(&guard.role, "release", if ok { "ok" } else { "lost" });
        metrics::set_lease_held(&guard.role, false);
        self.inner
            .write()
            .await
            .leases
            .remove(&guard.role.as_string());
        if ok {
            audit::lease_released(&self.bus, &guard.role, &guard.holder, guard.generation);
        }
        ok
    }

    /// How often a holder should renew, derived from the configured lease
    /// TTL (a third of it, floored at one second).
    pub async fn renew_every(&self) -> ChronoDuration {
        super::lease::renew_interval(self.settings.read().await.lease_ttl)
    }

    /// Leases this worker currently believes it holds.
    pub async fn held_roles(&self) -> Vec<String> {
        let mut v: Vec<String> = self.inner.read().await.leases.keys().cloned().collect();
        v.sort();
        v
    }

    // ------------------------------------------------------------ cursors --

    /// Load (or create) a cursor for `feed`/`scope`.
    pub async fn cursor(&self, feed: FeedId, scope: &str) -> FeedCursor {
        let key = if scope.is_empty() {
            feed.as_str().to_string()
        } else {
            format!("{}:{}", feed.as_str(), scope)
        };
        if let Some(c) = self.inner.read().await.cursors.get(&key) {
            return c.clone();
        }
        let store = self.store().await;
        let loaded = store.load_cursor(&key).await.ok().flatten();
        let cursor = loaded.unwrap_or_else(|| FeedCursor::new(feed, scope, &self.worker_id));
        self.inner.write().await.cursors.insert(key, cursor.clone());
        cursor
    }

    /// Offer a sequenced item to a cursor. Persists the cursor, records and
    /// audits any gap, and returns what the caller should do.
    pub async fn offer(
        &self,
        feed: FeedId,
        scope: &str,
        position: u64,
        event_at: Option<chrono::DateTime<Utc>>,
    ) -> CursorAdvance {
        let mut cursor = self.cursor(feed, scope).await;
        let advance = cursor.offer(position, event_at, &self.worker_id, Utc::now());
        metrics::count_replay_event(feed, advance.as_str());
        if let CursorAdvance::Gap(gap) = &advance {
            metrics::count_feed_gap(feed);
            audit::feed_gap(&self.bus, gap);
            warn!(feed = %feed, from = gap.from_position, to = gap.to_position, "feed gap detected");
            let store = self.store().await;
            if let Err(e) = store.record_gap(gap).await {
                warn!(error = %e, "feed gap could not be journaled");
            }
        }
        if advance.should_process() {
            self.persist_cursor(&cursor).await;
        }
        self.inner
            .write()
            .await
            .cursors
            .insert(cursor.key(), cursor.clone());
        metrics::set_cursor(feed, cursor.lag_secs(Utc::now()), cursor.position);
        advance
    }

    /// Offer an opaque (signature-style) item to a cursor.
    pub async fn offer_token(
        &self,
        feed: FeedId,
        scope: &str,
        token: &str,
        event_at: Option<chrono::DateTime<Utc>>,
    ) -> CursorAdvance {
        let mut cursor = self.cursor(feed, scope).await;
        let advance = cursor.offer_token(token, event_at, &self.worker_id, Utc::now());
        metrics::count_replay_event(feed, advance.as_str());
        if advance.should_process() {
            self.persist_cursor(&cursor).await;
        }
        self.inner
            .write()
            .await
            .cursors
            .insert(cursor.key(), cursor.clone());
        metrics::set_cursor(feed, cursor.lag_secs(Utc::now()), cursor.position);
        advance
    }

    async fn persist_cursor(&self, cursor: &FeedCursor) {
        let store = self.store().await;
        if let Err(e) = store.save_cursor(cursor).await {
            debug!(cursor = %cursor.key(), error = %e, "cursor could not be persisted");
        }
    }

    /// Deliberately rewind a cursor for replay / backfill (audited).
    pub async fn replay_from(&self, feed: FeedId, scope: &str, position: Option<u64>) {
        let mut cursor = self.cursor(feed, scope).await;
        cursor.rewind_to(position, Utc::now());
        self.persist_cursor(&cursor).await;
        self.inner
            .write()
            .await
            .cursors
            .insert(cursor.key(), cursor.clone());
        audit::feed_replay(
            &self.bus,
            feed.as_str(),
            &format!(
                "scope={} rewound_to={}",
                if scope.is_empty() { "-" } else { scope },
                position
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "start".into())
            ),
        );
    }

    /// Mark a journaled gap resolved.
    pub async fn resolve_gap(
        &self,
        feed: FeedId,
        scope: &str,
        from_position: u64,
        status: GapStatus,
    ) -> bool {
        let store = self.store().await;
        let ok = store
            .resolve_gap(feed.as_str(), scope, from_position, status)
            .await
            .unwrap_or(false);
        if ok {
            audit::feed_replay(
                &self.bus,
                feed.as_str(),
                &format!("gap_from={from_position} resolved={}", status.as_str()),
            );
        }
        ok
    }

    /// Every cursor this worker has touched.
    pub async fn cursors(&self) -> Vec<FeedCursor> {
        let mut v: Vec<FeedCursor> = self.inner.read().await.cursors.values().cloned().collect();
        v.sort_by_key(|c| c.key());
        v
    }

    // --------------------------------------------------------- recovery ---

    /// Journal + audit one recovery action.
    pub async fn record_recovery(&self, record: RecoveryRecord) {
        metrics::count_recovery_action(&record.scope, record.action.as_str());
        audit::recovery_action(&self.bus, &record);
        let store = self.store().await;
        if let Err(e) = store.record_recovery(&record).await {
            metrics::count_recovery_failure(&record.scope, "journal_error");
            warn!(error = %e, "recovery record could not be journaled");
        }
    }

    /// Mark durable recovery finished (or not).
    pub async fn set_recovery_complete(&self, complete: bool) {
        self.inner.write().await.recovery_complete = complete;
    }

    /// Has recovery completed in this life?
    pub async fn recovery_complete(&self) -> bool {
        self.inner.read().await.recovery_complete
    }

    // -------------------------------------------------------- readiness ---

    /// Report a dependency's health (`database`, `redis`, `feeds`, …).
    pub async fn set_dependency(&self, name: &str, healthy: bool) {
        self.inner
            .write()
            .await
            .dependencies
            .insert(name.to_string(), healthy);
    }

    /// The readiness verdict (§11). READY requires: a serving worker state,
    /// completed recovery, every required lease held, every reported
    /// dependency healthy.
    pub async fn readiness(&self) -> Readiness {
        let settings = self.settings().await;
        let inner = self.inner.read().await;
        let state = inner
            .registration
            .as_ref()
            .map(|r| r.state)
            .unwrap_or(WorkerState::Starting);
        let mut reasons = Vec::new();
        if !state.is_ready() {
            reasons.push(NotReadyReason::State(state));
        }
        if !inner.recovery_complete {
            reasons.push(NotReadyReason::RecoveryPending);
        }
        for role in &settings.required_roles {
            if !inner.leases.contains_key(&role.as_string()) {
                reasons.push(NotReadyReason::LeaseMissing(role.as_string()));
            }
        }
        for (name, healthy) in &inner.dependencies {
            if !healthy {
                reasons.push(NotReadyReason::Dependency(name.clone()));
            }
        }
        Readiness {
            ready: reasons.is_empty(),
            reasons,
            state,
        }
    }

    /// Re-verify every REQUIRED lease against the durable store and drop
    /// the ones this worker no longer owns. Readiness must never be decided
    /// from a local cache: between two ticks another worker may have taken
    /// the role over, and a worker that reports READY without owning its
    /// singleton work is exactly the split-brain this layer exists to
    /// prevent.
    async fn verify_required_leases(&self) {
        let settings = self.settings.read().await.clone();
        if settings.required_roles.is_empty() {
            return;
        }
        let store = self.store().await;
        for role in &settings.required_roles {
            let held = {
                let inner = self.inner.read().await;
                inner.leases.get(&role.as_string()).cloned()
            };
            let Some(lease) = held else { continue };
            let still_ours = store
                .verify_lease(role, &lease.holder, lease.generation)
                .await
                .unwrap_or(false);
            if !still_ours {
                metrics::count_lease_op(role, "verify", "lost");
                metrics::set_lease_held(role, false);
                self.inner.write().await.leases.remove(&role.as_string());
                audit::lease_lost(
                    &self.bus,
                    role,
                    &lease.holder,
                    lease.generation,
                    "readiness check: the lease is no longer held",
                );
                warn!(role = %role, "required lease lost — worker is not ready");
            }
        }
    }

    /// Compute readiness, publish the gauge and audit a change.
    pub async fn refresh_readiness(&self) -> Readiness {
        self.verify_required_leases().await;
        let verdict = self.readiness().await;
        let changed = {
            let mut inner = self.inner.write().await;
            let changed = inner.last_ready != Some(verdict.ready);
            inner.last_ready = Some(verdict.ready);
            changed
        };
        metrics::set_readiness(verdict.ready, changed);
        if changed {
            audit::readiness(&self.bus, &self.worker_id, verdict.ready, &verdict.detail());
            info!(ready = verdict.ready, detail = %verdict.detail(), "readiness changed");
        }
        verdict
    }

    // --------------------------------------------------------- shutdown ---

    /// Has graceful shutdown started?
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Graceful shutdown (§10): stop accepting work, persist cursors,
    /// release every held lease, mark the worker stopped, audit each phase.
    /// Returns how many leases were released.
    pub async fn shutdown(&self, reason: &str) -> usize {
        if self.draining.swap(true, Ordering::SeqCst) {
            return 0;
        }
        audit::shutdown(&self.bus, &self.worker_id, "draining", reason);
        let _ = self.set_state(WorkerState::Draining, reason).await;
        self.refresh_readiness().await;

        // 1. Persist every cursor we advanced.
        let cursors: Vec<FeedCursor> = self.inner.read().await.cursors.values().cloned().collect();
        let mut persisted = 0usize;
        for c in &cursors {
            let store = self.store().await;
            if store.save_cursor(c).await.is_ok() {
                persisted += 1;
            }
        }
        audit::shutdown(
            &self.bus,
            &self.worker_id,
            "cursors_persisted",
            &format!("cursors={persisted}/{}", cursors.len()),
        );

        // 2. Release every lease so a standby can take over immediately.
        let held: Vec<(String, i64)> = {
            let inner = self.inner.read().await;
            inner
                .leases
                .values()
                .map(|l| (l.role.as_string(), l.generation))
                .collect()
        };
        let mut released = 0usize;
        for (role_key, generation) in held {
            let Some(role) = LeaseRole::parse(&role_key) else {
                continue;
            };
            let guard = LeaseGuard {
                role,
                holder: self.worker_id.clone(),
                generation,
            };
            if self.release(&guard).await {
                released += 1;
            }
        }
        audit::shutdown(
            &self.bus,
            &self.worker_id,
            "leases_released",
            &format!("released={released}"),
        );

        // 3. Terminal state.
        let _ = self.set_state(WorkerState::Stopped, reason).await;
        self.refresh_readiness().await;
        audit::shutdown(&self.bus, &self.worker_id, "stopped", reason);
        info!(released, persisted, reason, "graceful shutdown complete");
        released
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::store::MemoryHaStore;

    async fn runtime(
        id: &str,
        store: Arc<MemoryHaStore>,
        required: Vec<LeaseRole>,
    ) -> Arc<HaRuntime> {
        let rt = Arc::new(HaRuntime::new(
            id,
            HaSettings {
                mode: HaMode::ActiveActive,
                required_roles: required,
                ..Default::default()
            },
            EventBus::new(256),
        ));
        rt.attach_store(store).await;
        rt
    }

    #[tokio::test]
    async fn registration_heartbeat_and_state_machine() {
        let store = Arc::new(MemoryHaStore::new());
        let rt = HaRuntime::new("w1", HaSettings::default(), EventBus::new(64));
        rt.attach_store(store.clone()).await;
        let reg = rt.register("host", 1, "0.1.0").await.unwrap();
        assert_eq!(reg.generation, 1);
        assert_eq!(rt.state().await, WorkerState::Starting);
        assert!(rt.heartbeat().await);

        rt.set_state(WorkerState::Recovering, "rebuilding")
            .await
            .unwrap();
        rt.set_state(WorkerState::Ready, "recovered").await.unwrap();
        assert_eq!(rt.state().await, WorkerState::Ready);
        // Illegal jumps are refused and change nothing.
        let err = rt
            .set_state(WorkerState::Starting, "nope")
            .await
            .unwrap_err();
        assert_eq!(err.from, WorkerState::Ready);
        assert_eq!(rt.state().await, WorkerState::Ready);
    }

    #[tokio::test]
    async fn two_workers_race_for_one_lease_and_exactly_one_wins() {
        let store = Arc::new(MemoryHaStore::new());
        let a = runtime("w-a", store.clone(), vec![]).await;
        let b = runtime("w-b", store.clone(), vec![]).await;
        a.register("h", 1, "v").await.unwrap();
        b.register("h", 2, "v").await.unwrap();

        let role = LeaseRole::Recovery;
        let (ra, rb) = tokio::join!(a.acquire(role.clone()), b.acquire(role.clone()));
        let winners = [ra.unwrap(), rb.unwrap()];
        let held: Vec<_> = winners.iter().flatten().collect();
        assert_eq!(held.len(), 1, "exactly one worker may hold the lease");

        let guard = held[0].clone();
        // The winner can fence; a forged older generation cannot.
        assert!(a.fence(&guard).await.is_ok() || b.fence(&guard).await.is_ok());
        let stale = LeaseGuard {
            role: role.clone(),
            holder: "w-b".into(),
            generation: guard.generation() - 1,
        };
        let err = b.fence(&stale).await.unwrap_err();
        assert_eq!(err.reason(), "fenced");
    }

    #[tokio::test]
    async fn takeover_after_expiry_fences_the_old_owner() {
        let store = Arc::new(MemoryHaStore::new());
        let a = runtime("w-a", store.clone(), vec![]).await;
        let b = runtime("w-b", store.clone(), vec![]).await;
        a.register("h", 1, "v").await.unwrap();
        b.register("h", 2, "v").await.unwrap();
        let role = LeaseRole::Reconciliation;
        let ga = a.acquire(role.clone()).await.unwrap().expect("w-a wins");
        assert!(a.fence(&ga).await.is_ok());
        // w-a dies: the lease expires.
        store.advance_clock(ChronoDuration::seconds(60));
        assert!(b.acquire(role.clone()).await.unwrap().is_some());
        // The old owner is deterministically fenced and cannot renew.
        let err = a.fence(&ga).await.unwrap_err();
        assert_eq!(err.reason(), "fenced");
        assert!(!a.renew(&ga).await);
        // …and its guarded work never runs.
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = Arc::clone(&ran);
        let res = a
            .guarded(&ga, async move {
                r2.store(true, Ordering::SeqCst);
            })
            .await;
        assert!(res.is_err());
        assert!(!ran.load(Ordering::SeqCst), "fenced work must not run");
    }

    #[tokio::test]
    async fn store_failure_fails_closed() {
        let store = Arc::new(MemoryHaStore::new());
        let a = runtime("w-a", store.clone(), vec![]).await;
        a.register("h", 1, "v").await.unwrap();
        let g = a.acquire(LeaseRole::StateSync).await.unwrap().unwrap();
        store.set_unavailable(true);
        let err = a.fence(&g).await.unwrap_err();
        assert_eq!(err.reason(), "store_unavailable");
        assert!(a.acquire(LeaseRole::Recovery).await.is_err());
    }

    #[tokio::test]
    async fn readiness_requires_state_recovery_leases_and_dependencies() {
        let store = Arc::new(MemoryHaStore::new());
        let role = LeaseRole::AccountingMaintenance;
        let rt = runtime("w1", store.clone(), vec![role.clone()]).await;
        rt.register("h", 1, "v").await.unwrap();
        // Starting + no recovery + no lease.
        let r = rt.readiness().await;
        assert!(!r.ready);
        assert!(r
            .reasons
            .iter()
            .any(|x| matches!(x, NotReadyReason::State(_))));
        assert!(r.reasons.contains(&NotReadyReason::RecoveryPending));
        assert!(r
            .reasons
            .iter()
            .any(|x| matches!(x, NotReadyReason::LeaseMissing(_))));

        rt.set_state(WorkerState::Recovering, "").await.unwrap();
        rt.set_recovery_complete(true).await;
        rt.set_state(WorkerState::Ready, "").await.unwrap();
        rt.acquire(role.clone()).await.unwrap().unwrap();
        rt.set_dependency("database", true).await;
        assert!(rt.refresh_readiness().await.ready);

        // A dependency failing removes readiness.
        rt.set_dependency("database", false).await;
        let r = rt.refresh_readiness().await;
        assert!(!r.ready);
        assert!(r.detail().contains("database"));
        rt.set_dependency("database", true).await;

        // Losing the lease removes readiness even though the state is Ready.
        store.advance_clock(ChronoDuration::seconds(120));
        let other = runtime("w2", store.clone(), vec![]).await;
        other.register("h", 9, "v").await.unwrap();
        other.acquire(role.clone()).await.unwrap().unwrap();
        let g = LeaseGuard {
            role: role.clone(),
            holder: "w1".into(),
            generation: 1,
        };
        assert!(rt.fence(&g).await.is_err());
        let r = rt.refresh_readiness().await;
        assert!(!r.ready, "{}", r.detail());
    }

    #[tokio::test]
    async fn cursors_persist_suppress_duplicates_and_report_gaps() {
        let store = Arc::new(MemoryHaStore::new());
        let rt = runtime("w1", store.clone(), vec![]).await;
        rt.register("h", 1, "v").await.unwrap();
        assert_eq!(
            rt.offer(FeedId::PolymarketUser, "", 1, None).await,
            CursorAdvance::Advanced
        );
        assert_eq!(
            rt.offer(FeedId::PolymarketUser, "", 1, None).await,
            CursorAdvance::Duplicate
        );
        let adv = rt.offer(FeedId::PolymarketUser, "", 4, None).await;
        assert!(matches!(adv, CursorAdvance::Gap(_)));
        assert_eq!(store.gaps(true, 10).await.unwrap().len(), 1);
        // The cursor survives a "restart": a new runtime loads it.
        let rt2 = runtime("w2", store.clone(), vec![]).await;
        rt2.register("h", 2, "v").await.unwrap();
        let c = rt2.cursor(FeedId::PolymarketUser, "").await;
        assert_eq!(c.position, Some(4));
        assert_eq!(
            rt2.offer(FeedId::PolymarketUser, "", 4, None).await,
            CursorAdvance::Duplicate,
            "a replayed event after the restart is suppressed"
        );
        // Deliberate replay.
        rt2.replay_from(FeedId::PolymarketUser, "", Some(2)).await;
        assert_eq!(
            rt2.offer(FeedId::PolymarketUser, "", 3, None).await,
            CursorAdvance::Advanced
        );
        assert!(
            rt2.resolve_gap(FeedId::PolymarketUser, "", 2, GapStatus::Backfilled)
                .await
        );
    }

    #[tokio::test]
    async fn graceful_shutdown_persists_releases_and_stops() {
        let store = Arc::new(MemoryHaStore::new());
        let rt = runtime("w1", store.clone(), vec![]).await;
        rt.register("h", 1, "v").await.unwrap();
        rt.set_state(WorkerState::Recovering, "").await.unwrap();
        rt.set_recovery_complete(true).await;
        rt.set_state(WorkerState::Ready, "").await.unwrap();
        let role = LeaseRole::Feed("copy_logs".into());
        rt.acquire(role.clone()).await.unwrap().unwrap();
        rt.offer_token(FeedId::CopyLogs, "wallet", "sig-1", None)
            .await;

        let released = rt.shutdown("sigterm").await;
        assert_eq!(released, 1);
        assert_eq!(rt.state().await, WorkerState::Stopped);
        assert!(rt.is_draining());
        assert!(!rt.readiness().await.ready);
        // The lease is free for a standby immediately.
        let standby = runtime("w2", store.clone(), vec![]).await;
        standby.register("h", 2, "v").await.unwrap();
        assert!(standby.acquire(role).await.unwrap().is_some());
        // The cursor was persisted.
        assert!(store
            .load_cursor("copy_logs:wallet")
            .await
            .unwrap()
            .is_some());
        // Shutting down twice is a no-op.
        assert_eq!(rt.shutdown("again").await, 0);
    }

    #[tokio::test]
    async fn stale_workers_are_detected_but_never_silently_taken_over() {
        let store = Arc::new(MemoryHaStore::new());
        let a = runtime("w-a", store.clone(), vec![]).await;
        let b = runtime("w-b", store.clone(), vec![]).await;
        a.register("h", 1, "v").await.unwrap();
        b.register("h", 2, "v").await.unwrap();
        a.heartbeat().await;
        b.heartbeat().await;
        let (live, stale, _stopped, rows) = b.survey_workers().await;
        assert_eq!((live, stale), (2, 0));
        assert!(rows.is_empty());
        store.advance_clock(ChronoDuration::seconds(120));
        // b heartbeats; a does not.
        b.heartbeat().await;
        let (live, stale, _stopped, rows) = b.survey_workers().await;
        assert_eq!((live, stale), (1, 1));
        assert_eq!(rows[0].worker_id, "w-a");
        // Detection alone changed no ownership.
        assert!(store.leases().await.unwrap().is_empty());
    }
}
