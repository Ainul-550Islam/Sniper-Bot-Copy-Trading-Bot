//! Durable contract of the HA layer (TASK 6 §16) and the in-memory
//! implementation used by tests, single-worker runs and no-database setups.
//!
//! The server implements [`HaStore`] over PostgreSQL
//! (`crates/server/src/ha.rs`, migration 0016). [`MemoryHaStore`] has the
//! SAME semantics — including the atomicity of `acquire_lease` — so the
//! two-worker race tests are meaningful without a database; its scope is
//! the process, which is exactly what single-worker mode needs.
//!
//! Every method answers `BotResult`: a store error is NEVER "you own it".
//! Callers fail closed ([`crate::ha::lease::FenceError::StoreUnavailable`]).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use super::cursor::{FeedCursor, FeedGap, GapStatus};
use super::lease::{next_generation, Lease, LeaseDecision, LeaseRequest, LeaseRole};
use super::recovery_plan::OrderRecoveryAction;
use super::worker::{WorkerRegistration, WorkerState};
use crate::error::{BotError, BotResult};

/// One durable recovery record: what a worker did, for which subject, when.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryRecord {
    /// Worker that performed it.
    pub worker_id: String,
    /// Generation of that worker.
    pub generation: i64,
    /// `startup` | `takeover` | `periodic`.
    pub trigger: String,
    /// Scope: `orders` | `ledger` | `cursors` | `risk` | `positions`.
    pub scope: String,
    /// Subject id (order id, cursor key, …); empty for scope-wide records.
    pub subject: String,
    /// The deterministic action.
    pub action: OrderRecoveryAction,
    /// Why (audit text).
    pub detail: String,
    /// When.
    pub ts: DateTime<Utc>,
}

impl RecoveryRecord {
    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "worker={} generation={} trigger={} scope={} subject={} action={} detail={}",
            self.worker_id,
            self.generation,
            self.trigger,
            self.scope,
            if self.subject.is_empty() {
                "-"
            } else {
                &self.subject
            },
            self.action,
            self.detail
        )
    }
}

/// Durable HA state (see the module docs).
#[async_trait]
pub trait HaStore: Send + Sync {
    /// `postgres` | `memory` (metrics / logs; low cardinality).
    fn backend(&self) -> &'static str;

    /// The SHARED clock every liveness comparison uses. Worker clocks drift
    /// independently, so heartbeat ages and lease expiry must be judged
    /// against one reference — the durable store's own clock (`now()` in
    /// PostgreSQL). Falling back to the local clock is only correct for the
    /// in-process store.
    async fn now(&self) -> BotResult<DateTime<Utc>> {
        Ok(Utc::now())
    }

    /// Register this worker life. Returns the registration with the
    /// generation the store assigned (previous generation + 1 for a known
    /// `worker_id`, else 1).
    async fn register_worker(
        &self,
        worker_id: &str,
        mode: &str,
        host: &str,
        pid: i64,
        version: &str,
    ) -> BotResult<WorkerRegistration>;

    /// Heartbeat + state update for one worker generation. `Ok(false)` when
    /// the row was taken over by a newer generation (this life is stale).
    async fn heartbeat(
        &self,
        worker_id: &str,
        generation: i64,
        state: WorkerState,
        detail: &str,
    ) -> BotResult<bool>;

    /// Every registered worker (operator view, stale detection).
    async fn workers(&self) -> BotResult<Vec<WorkerRegistration>>;

    /// Atomic lease acquisition (fresh, expired, or released → acquired;
    /// live and held by someone else → rejected).
    async fn acquire_lease(&self, req: &LeaseRequest) -> BotResult<LeaseDecision>;

    /// Extend a lease; CAS on `(holder, generation)`. `Ok(false)` = lost.
    async fn renew_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
        ttl: ChronoDuration,
    ) -> BotResult<bool>;

    /// Fencing check: is `(holder, generation)` still the live owner?
    async fn verify_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> BotResult<bool>;

    /// Release a lease; CAS on `(holder, generation)`. `Ok(false)` = the
    /// caller was already fenced (it must not clear the new owner).
    async fn release_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> BotResult<bool>;

    /// Current record for a role.
    async fn get_lease(&self, role: &LeaseRole) -> BotResult<Option<Lease>>;

    /// Every lease (operator view).
    async fn leases(&self) -> BotResult<Vec<Lease>>;

    /// Persist one cursor (upsert by `feed:scope`).
    async fn save_cursor(&self, cursor: &FeedCursor) -> BotResult<()>;

    /// Load one cursor.
    async fn load_cursor(&self, key: &str) -> BotResult<Option<FeedCursor>>;

    /// Every cursor (operator view, lag gauges).
    async fn cursors(&self) -> BotResult<Vec<FeedCursor>>;

    /// Record a detected gap (append-only).
    async fn record_gap(&self, gap: &FeedGap) -> BotResult<()>;

    /// Gaps, newest first, optionally only unresolved ones.
    async fn gaps(&self, unresolved_only: bool, limit: usize) -> BotResult<Vec<FeedGap>>;

    /// Resolve a gap (backfilled or explicitly accepted).
    async fn resolve_gap(
        &self,
        feed: &str,
        scope: &str,
        from_position: u64,
        status: GapStatus,
    ) -> BotResult<bool>;

    /// Append one recovery record.
    async fn record_recovery(&self, record: &RecoveryRecord) -> BotResult<()>;

    /// Recovery records, newest first.
    async fn recovery_records(&self, limit: usize) -> BotResult<Vec<RecoveryRecord>>;
}

#[derive(Default)]
struct MemoryInner {
    workers: HashMap<String, WorkerRegistration>,
    leases: HashMap<String, Lease>,
    cursors: HashMap<String, FeedCursor>,
    gaps: Vec<FeedGap>,
    recoveries: Vec<RecoveryRecord>,
    /// Test clock offset, applied to every time comparison.
    clock_skew: ChronoDuration,
    unavailable: bool,
}

/// In-memory HA store: same semantics, process scope.
pub struct MemoryHaStore {
    inner: Mutex<MemoryInner>,
}

impl Default for MemoryHaStore {
    fn default() -> Self {
        MemoryHaStore::new()
    }
}

impl MemoryHaStore {
    /// Empty store.
    pub fn new() -> Self {
        MemoryHaStore {
            inner: Mutex::new(MemoryInner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Move the store's clock forward (tests: expire leases and heartbeats
    /// without sleeping).
    pub fn advance_clock(&self, by: ChronoDuration) {
        let mut inner = self.lock();
        inner.clock_skew += by;
    }

    /// Simulate an unreachable backend (tests only).
    pub fn set_unavailable(&self, on: bool) {
        let mut inner = self.lock();
        inner.unavailable = on;
    }

    fn now(inner: &MemoryInner) -> DateTime<Utc> {
        Utc::now() + inner.clock_skew
    }

    fn guard(inner: &MemoryInner, op: &str) -> BotResult<()> {
        if inner.unavailable {
            return Err(BotError::db(format!("ha store unavailable ({op})")));
        }
        Ok(())
    }
}

#[async_trait]
impl HaStore for MemoryHaStore {
    fn backend(&self) -> &'static str {
        "memory"
    }

    async fn now(&self) -> BotResult<DateTime<Utc>> {
        let inner = self.lock();
        Self::guard(&inner, "now")?;
        Ok(Self::now(&inner))
    }

    async fn register_worker(
        &self,
        worker_id: &str,
        mode: &str,
        host: &str,
        pid: i64,
        version: &str,
    ) -> BotResult<WorkerRegistration> {
        let mut inner = self.lock();
        Self::guard(&inner, "register_worker")?;
        let now = Self::now(&inner);
        let generation = inner
            .workers
            .get(worker_id)
            .map(|w| w.generation + 1)
            .unwrap_or(1);
        let mode = super::worker::HaMode::parse(mode).unwrap_or(super::worker::HaMode::Single);
        let reg = WorkerRegistration::new(worker_id, generation, mode, host, pid, version, now);
        inner.workers.insert(worker_id.to_string(), reg.clone());
        Ok(reg)
    }

    async fn heartbeat(
        &self,
        worker_id: &str,
        generation: i64,
        state: WorkerState,
        detail: &str,
    ) -> BotResult<bool> {
        let mut inner = self.lock();
        Self::guard(&inner, "heartbeat")?;
        let now = Self::now(&inner);
        let Some(w) = inner.workers.get_mut(worker_id) else {
            return Ok(false);
        };
        if w.generation != generation {
            return Ok(false);
        }
        w.state = state;
        w.detail = detail.to_string();
        w.last_seen_at = now;
        Ok(true)
    }

    async fn workers(&self) -> BotResult<Vec<WorkerRegistration>> {
        let inner = self.lock();
        Self::guard(&inner, "workers")?;
        let mut out: Vec<_> = inner.workers.values().cloned().collect();
        out.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
        Ok(out)
    }

    async fn acquire_lease(&self, req: &LeaseRequest) -> BotResult<LeaseDecision> {
        let mut inner = self.lock();
        Self::guard(&inner, "acquire_lease")?;
        let now = Self::now(&inner);
        let key = req.role.as_string();
        if let Some(existing) = inner.leases.get(&key) {
            if existing.is_live(now) && existing.holder != req.holder {
                return Ok(LeaseDecision::Rejected {
                    holder: existing.holder.clone(),
                    generation: existing.generation,
                    expires_at: existing.expires_at,
                });
            }
        }
        let previous = inner.leases.get(&key).cloned();
        let generation = next_generation(previous.as_ref().map(|l| l.generation));
        let takeover = previous
            .as_ref()
            .map(|l| l.holder != req.holder && !l.released)
            .unwrap_or(false);
        let lease = Lease {
            role: req.role.clone(),
            holder: req.holder.clone(),
            generation,
            acquired_at: now,
            expires_at: now + req.ttl,
            renewed_at: now,
            takeover_count: previous.as_ref().map(|l| l.takeover_count).unwrap_or(0)
                + i64::from(takeover),
            previous_holder: previous.as_ref().map(|l| l.holder.clone()),
            released: false,
        };
        inner.leases.insert(key, lease.clone());
        Ok(LeaseDecision::Acquired(lease))
    }

    async fn renew_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
        ttl: ChronoDuration,
    ) -> BotResult<bool> {
        let mut inner = self.lock();
        Self::guard(&inner, "renew_lease")?;
        let now = Self::now(&inner);
        let Some(l) = inner.leases.get_mut(&role.as_string()) else {
            return Ok(false);
        };
        if l.holder != holder || l.generation != generation || l.released || l.expires_at <= now {
            return Ok(false);
        }
        l.expires_at = now + ttl;
        l.renewed_at = now;
        Ok(true)
    }

    async fn verify_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> BotResult<bool> {
        let inner = self.lock();
        Self::guard(&inner, "verify_lease")?;
        let now = Self::now(&inner);
        Ok(inner
            .leases
            .get(&role.as_string())
            .map(|l| l.holder == holder && l.generation == generation && l.is_live(now))
            .unwrap_or(false))
    }

    async fn release_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> BotResult<bool> {
        let mut inner = self.lock();
        Self::guard(&inner, "release_lease")?;
        let now = Self::now(&inner);
        let Some(l) = inner.leases.get_mut(&role.as_string()) else {
            return Ok(false);
        };
        if l.holder != holder || l.generation != generation || l.released {
            return Ok(false);
        }
        l.released = true;
        l.expires_at = now;
        Ok(true)
    }

    async fn get_lease(&self, role: &LeaseRole) -> BotResult<Option<Lease>> {
        let inner = self.lock();
        Self::guard(&inner, "get_lease")?;
        Ok(inner.leases.get(&role.as_string()).cloned())
    }

    async fn leases(&self) -> BotResult<Vec<Lease>> {
        let inner = self.lock();
        Self::guard(&inner, "leases")?;
        let mut out: Vec<_> = inner.leases.values().cloned().collect();
        out.sort_by_key(|l| l.role.as_string());
        Ok(out)
    }

    async fn save_cursor(&self, cursor: &FeedCursor) -> BotResult<()> {
        let mut inner = self.lock();
        Self::guard(&inner, "save_cursor")?;
        inner.cursors.insert(cursor.key(), cursor.clone());
        Ok(())
    }

    async fn load_cursor(&self, key: &str) -> BotResult<Option<FeedCursor>> {
        let inner = self.lock();
        Self::guard(&inner, "load_cursor")?;
        Ok(inner.cursors.get(key).cloned())
    }

    async fn cursors(&self) -> BotResult<Vec<FeedCursor>> {
        let inner = self.lock();
        Self::guard(&inner, "cursors")?;
        let mut out: Vec<_> = inner.cursors.values().cloned().collect();
        out.sort_by_key(|c| c.key());
        Ok(out)
    }

    async fn record_gap(&self, gap: &FeedGap) -> BotResult<()> {
        let mut inner = self.lock();
        Self::guard(&inner, "record_gap")?;
        inner.gaps.push(gap.clone());
        Ok(())
    }

    async fn gaps(&self, unresolved_only: bool, limit: usize) -> BotResult<Vec<FeedGap>> {
        let inner = self.lock();
        Self::guard(&inner, "gaps")?;
        Ok(inner
            .gaps
            .iter()
            .rev()
            .filter(|g| !unresolved_only || g.status == GapStatus::Detected)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn resolve_gap(
        &self,
        feed: &str,
        scope: &str,
        from_position: u64,
        status: GapStatus,
    ) -> BotResult<bool> {
        let mut inner = self.lock();
        Self::guard(&inner, "resolve_gap")?;
        let mut hit = false;
        for g in inner.gaps.iter_mut() {
            if g.feed.as_str() == feed && g.scope == scope && g.from_position == from_position {
                g.status = status;
                hit = true;
            }
        }
        Ok(hit)
    }

    async fn record_recovery(&self, record: &RecoveryRecord) -> BotResult<()> {
        let mut inner = self.lock();
        Self::guard(&inner, "record_recovery")?;
        inner.recoveries.push(record.clone());
        Ok(())
    }

    async fn recovery_records(&self, limit: usize) -> BotResult<Vec<RecoveryRecord>> {
        let inner = self.lock();
        Self::guard(&inner, "recovery_records")?;
        Ok(inner.recoveries.iter().rev().take(limit).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::cursor::FeedId;

    fn req(role: LeaseRole, holder: &str, ttl: i64) -> LeaseRequest {
        LeaseRequest {
            role,
            holder: holder.into(),
            ttl: ChronoDuration::seconds(ttl),
        }
    }

    #[tokio::test]
    async fn registration_increments_the_generation_per_life() {
        let s = MemoryHaStore::new();
        let a = s
            .register_worker("w1", "single", "h", 1, "0.1.0")
            .await
            .unwrap();
        assert_eq!(a.generation, 1);
        assert_eq!(a.state, WorkerState::Starting);
        let b = s
            .register_worker("w1", "single", "h", 2, "0.1.0")
            .await
            .unwrap();
        assert_eq!(b.generation, 2, "a restart is a new generation");
        // The old generation can no longer heartbeat.
        assert!(!s
            .heartbeat("w1", 1, WorkerState::Running, "stale life")
            .await
            .unwrap());
        assert!(s
            .heartbeat("w1", 2, WorkerState::Running, "live")
            .await
            .unwrap());
        assert!(!s
            .heartbeat("ghost", 1, WorkerState::Running, "")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn only_one_worker_holds_a_lease_at_a_time() {
        let s = MemoryHaStore::new();
        let role = LeaseRole::Recovery;
        let a = s.acquire_lease(&req(role.clone(), "w1", 30)).await.unwrap();
        assert!(a.is_acquired());
        let g1 = a.acquired().unwrap().generation;
        let b = s.acquire_lease(&req(role.clone(), "w2", 30)).await.unwrap();
        match b {
            LeaseDecision::Rejected {
                holder, generation, ..
            } => {
                assert_eq!(holder, "w1");
                assert_eq!(generation, g1);
            }
            LeaseDecision::Acquired(_) => panic!("two holders"),
        }
        // w1 renews; w2 cannot.
        assert!(s
            .renew_lease(&role, "w1", g1, ChronoDuration::seconds(30))
            .await
            .unwrap());
        assert!(!s
            .renew_lease(&role, "w2", g1, ChronoDuration::seconds(30))
            .await
            .unwrap());
        assert!(!s
            .renew_lease(&role, "w1", g1 + 5, ChronoDuration::seconds(30))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn expiry_enables_takeover_and_fences_the_old_holder() {
        let s = MemoryHaStore::new();
        let role = LeaseRole::Reconciliation;
        let g1 = s
            .acquire_lease(&req(role.clone(), "w1", 10))
            .await
            .unwrap()
            .acquired()
            .unwrap()
            .generation;
        assert!(s.verify_lease(&role, "w1", g1).await.unwrap());
        // w1 dies; the lease expires.
        s.advance_clock(ChronoDuration::seconds(11));
        assert!(!s.verify_lease(&role, "w1", g1).await.unwrap());
        let takeover = s.acquire_lease(&req(role.clone(), "w2", 10)).await.unwrap();
        let l = takeover.acquired().expect("takeover");
        assert_eq!(l.holder, "w2");
        assert!(l.generation > g1, "fencing token increased");
        assert_eq!(l.previous_holder.as_deref(), Some("w1"));
        assert_eq!(l.takeover_count, 1);
        // The stale holder is fenced on every path.
        assert!(!s.verify_lease(&role, "w1", g1).await.unwrap());
        assert!(!s
            .renew_lease(&role, "w1", g1, ChronoDuration::seconds(10))
            .await
            .unwrap());
        assert!(
            !s.release_lease(&role, "w1", g1).await.unwrap(),
            "a fenced worker must not release the new owner's lease"
        );
        assert!(s.verify_lease(&role, "w2", l.generation).await.unwrap());
    }

    #[tokio::test]
    async fn release_makes_the_role_immediately_acquirable() {
        let s = MemoryHaStore::new();
        let role = LeaseRole::Feed("copy_logs".into());
        let g1 = s
            .acquire_lease(&req(role.clone(), "w1", 60))
            .await
            .unwrap()
            .acquired()
            .unwrap()
            .generation;
        assert!(s.release_lease(&role, "w1", g1).await.unwrap());
        assert!(!s.verify_lease(&role, "w1", g1).await.unwrap());
        let b = s.acquire_lease(&req(role.clone(), "w2", 60)).await.unwrap();
        assert!(b.is_acquired(), "a released lease is free immediately");
        assert!(b.acquired().unwrap().generation > g1);
    }

    #[tokio::test]
    async fn store_failures_never_look_like_ownership() {
        let s = MemoryHaStore::new();
        let role = LeaseRole::StateSync;
        s.set_unavailable(true);
        assert!(s.acquire_lease(&req(role.clone(), "w1", 30)).await.is_err());
        assert!(s.verify_lease(&role, "w1", 1).await.is_err());
        assert!(s
            .heartbeat("w1", 1, WorkerState::Running, "")
            .await
            .is_err());
        s.set_unavailable(false);
        assert!(s.acquire_lease(&req(role, "w1", 30)).await.is_ok());
    }

    #[tokio::test]
    async fn cursors_gaps_and_recovery_records_round_trip() {
        let s = MemoryHaStore::new();
        let mut c = FeedCursor::new(FeedId::PolymarketUser, "", "w1");
        c.offer(7, None, "w1", Utc::now());
        s.save_cursor(&c).await.unwrap();
        let back = s.load_cursor(&c.key()).await.unwrap().expect("saved");
        assert_eq!(back.position, Some(7));
        assert_eq!(s.cursors().await.unwrap().len(), 1);

        let gap = FeedGap {
            feed: FeedId::PolymarketUser,
            scope: String::new(),
            from_position: 3,
            to_position: 5,
            detected_at: Utc::now(),
            worker_id: "w1".into(),
            status: GapStatus::Detected,
        };
        s.record_gap(&gap).await.unwrap();
        assert_eq!(s.gaps(true, 10).await.unwrap().len(), 1);
        assert!(s
            .resolve_gap("polymarket_user", "", 3, GapStatus::Backfilled)
            .await
            .unwrap());
        assert!(s.gaps(true, 10).await.unwrap().is_empty());
        assert_eq!(s.gaps(false, 10).await.unwrap().len(), 1);
        assert!(!s
            .resolve_gap("polymarket_user", "", 99, GapStatus::Backfilled)
            .await
            .unwrap());

        s.record_recovery(&RecoveryRecord {
            worker_id: "w1".into(),
            generation: 1,
            trigger: "startup".into(),
            scope: "orders".into(),
            subject: "ord-1".into(),
            action: OrderRecoveryAction::HoldAmbiguous,
            detail: "unknown submit".into(),
            ts: Utc::now(),
        })
        .await
        .unwrap();
        let recs = s.recovery_records(10).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert!(recs[0].summary().contains("hold_ambiguous"));
    }
}
