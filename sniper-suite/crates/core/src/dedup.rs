//! Restart-safe deduplication (BUILD PLAN §4-iii / §7).
//!
//! Two layers, one contract — `mark()` returns `true` for the FIRST arrival
//! of a key (the caller may act) and `false` for every repeat:
//!
//! * **L1** — the in-memory bounded set that has always guarded the feeds.
//!   Zero-latency, process-lifetime only.
//! * **L2** — optional shared/durable window behind L1:
//!   * `redis`   — `SET NX PX` (fast, survives restarts while Redis retains
//!     data; a flushed Redis only widens the dedup window's blind spot to
//!     what L1 still remembers),
//!   * `postgres`— `INSERT … ON CONFLICT DO NOTHING` on `dedup_keys`
//!     (fully restart-safe; the production default when the DB is on).
//!
//! Degradation rule: if L2 errors or times out, the decision falls back to
//! the L1 verdict and a `bot_dedup_l2_degraded_total` counter is raised —
//! trading continues (availability), operators see the degradation (health).
//!
//! This module never stores financial state; keys are opaque strings
//! (signatures, mints, webhook ids) and values are constant "1"s.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::db::repo::DedupRepo;
use crate::db::Database;
use crate::obs::metrics;
use crate::redis_kv::RedisKv;
use crate::state::BoundedSet;

/// Where the durable (L2) dedup window lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupBackend {
    Memory,
    Redis,
    Postgres,
}

impl DedupBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            DedupBackend::Memory => "memory",
            DedupBackend::Redis => "redis",
            DedupBackend::Postgres => "postgres",
        }
    }

    pub fn parse(s: &str) -> DedupBackend {
        match s {
            "redis" => DedupBackend::Redis,
            "postgres" => DedupBackend::Postgres,
            _ => DedupBackend::Memory,
        }
    }
}

/// The dedup facade shared by every feed/consumer.
pub struct DedupStore {
    backend: DedupBackend,
    redis: Option<RedisKv>,
    db: Option<Arc<Database>>,
    ttl: Duration,
    cap: usize,
    l1: RwLock<HashMap<String, BoundedSet>>,
}

impl DedupStore {
    /// Build from resolved components. `backend` names the L2 layer; when the
    /// named layer's client is missing the store silently behaves as
    /// `Memory` (config validation catches the mismatch earlier).
    pub fn new(
        backend: DedupBackend,
        redis: Option<RedisKv>,
        db: Option<Arc<Database>>,
        ttl: Duration,
        cap: usize,
    ) -> Arc<Self> {
        let effective = match backend {
            DedupBackend::Redis if redis.is_none() => DedupBackend::Memory,
            DedupBackend::Postgres if db.is_none() => DedupBackend::Memory,
            b => b,
        };
        Arc::new(DedupStore {
            backend: effective,
            redis,
            db,
            ttl,
            cap: cap.max(64),
            l1: RwLock::new(HashMap::new()),
        })
    }

    pub fn backend(&self) -> DedupBackend {
        self.backend
    }

    fn redis_key(ns: &str, key: &str) -> String {
        format!("dedup:{ns}:{key}")
    }

    /// First-arrival check. `true` = newly marked (act on it), `false` =
    /// duplicate. Matches `AppState::mark_signature_seen` semantics exactly.
    pub async fn mark(&self, ns: &str, key: &str) -> bool {
        // L1: definitive when it says "seen".
        let l1_new = {
            let mut map = self.l1.write().await;
            let set = map.entry(ns.to_string()).or_default();
            set.insert(key, self.cap)
        };
        if !l1_new {
            Self::meter(ns, "l1", "duplicate");
            return false;
        }
        match self.backend {
            DedupBackend::Memory => {
                Self::meter(ns, "l1", "new");
                true
            }
            DedupBackend::Redis => {
                let Some(redis) = &self.redis else {
                    Self::meter(ns, "l1", "new");
                    return true;
                };
                match redis
                    .set_nx_ttl(&Self::redis_key(ns, key), "1", self.ttl)
                    .await
                {
                    Ok(true) => {
                        Self::meter(ns, "redis", "new");
                        true
                    }
                    Ok(false) => {
                        // Seen before this process's lifetime (restart case).
                        Self::meter(ns, "redis", "duplicate");
                        false
                    }
                    Err(e) => {
                        Self::meter(ns, "redis", "degraded");
                        self.degraded(ns, &e.to_string());
                        true // L1 said new; keep trading.
                    }
                }
            }
            DedupBackend::Postgres => {
                let Some(db) = &self.db else {
                    Self::meter(ns, "l1", "new");
                    return true;
                };
                match DedupRepo::new(db.clone()).mark(ns, key, self.ttl).await {
                    Ok(true) => {
                        Self::meter(ns, "postgres", "new");
                        true
                    }
                    Ok(false) => {
                        Self::meter(ns, "postgres", "duplicate");
                        false
                    }
                    Err(e) => {
                        Self::meter(ns, "postgres", "degraded");
                        self.degraded(ns, &e.to_string());
                        true
                    }
                }
            }
        }
    }

    /// Read-only membership check (no marking). Checks L1, then L2 when the
    /// key is not in L1.
    pub async fn contains(&self, ns: &str, key: &str) -> bool {
        {
            let map = self.l1.read().await;
            if let Some(set) = map.get(ns) {
                if set.contains(key) {
                    return true;
                }
            }
        }
        match self.backend {
            DedupBackend::Memory => false,
            DedupBackend::Redis => {
                let Some(redis) = &self.redis else {
                    return false;
                };
                redis
                    .get(&Self::redis_key(ns, key))
                    .await
                    .map(|v| v.is_some())
                    .unwrap_or(false)
            }
            DedupBackend::Postgres => {
                let Some(db) = &self.db else {
                    return false;
                };
                DedupRepo::new(db.clone())
                    .exists(ns, key)
                    .await
                    .unwrap_or(false)
            }
        }
    }

    /// Remove a key from every layer (used by `forget_signature` when a
    /// transaction provably did NOT land and may be retried).
    pub async fn forget(&self, ns: &str, key: &str) {
        {
            let mut map = self.l1.write().await;
            if let Some(set) = map.get_mut(ns) {
                set.remove(key);
            }
        }
        match self.backend {
            DedupBackend::Memory => {}
            DedupBackend::Redis => {
                if let Some(redis) = &self.redis {
                    if let Err(e) = redis.del(&Self::redis_key(ns, key)).await {
                        debug!(ns, error = %e, "redis dedup forget failed");
                    }
                }
            }
            DedupBackend::Postgres => {
                if let Some(db) = &self.db {
                    if let Err(e) = DedupRepo::new(db.clone()).forget(ns, key).await {
                        debug!(ns, error = %e, "postgres dedup forget failed");
                    }
                }
            }
        }
    }

    /// L1 size for one namespace (gauge/diagnostic parity with the old
    /// `seen_launch_count`).
    pub async fn len(&self, ns: &str) -> usize {
        let map = self.l1.read().await;
        map.get(ns).map(|s| s.len()).unwrap_or(0)
    }

    fn meter(ns: &str, layer: &str, result: &str) {
        metrics::global()
            .counter(
                "bot_dedup_marks_total",
                "Dedup first-arrival checks by namespace, layer and result.",
                &[("ns", ns), ("layer", layer), ("result", result)],
            )
            .inc();
    }

    fn degraded(&self, ns: &str, err: &str) {
        metrics::global()
            .counter(
                "bot_dedup_l2_degraded_total",
                "Dedup checks that fell back to L1 because the L2 backend failed.",
                &[("backend", self.backend.as_str())],
            )
            .inc();
        warn!(
            ns,
            backend = self.backend.as_str(),
            error = err,
            "dedup L2 degraded — using L1 verdict"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> Arc<DedupStore> {
        DedupStore::new(
            DedupBackend::Memory,
            None,
            None,
            Duration::from_secs(60),
            128,
        )
    }

    #[tokio::test]
    async fn first_arrival_wins_exactly_once() {
        let d = mem_store();
        assert!(d.mark("sig", "abc").await, "first arrival acts");
        assert!(!d.mark("sig", "abc").await, "repeat is a duplicate");
        assert!(d.mark("sig", "def").await, "different key is independent");
    }

    #[tokio::test]
    async fn namespaces_are_isolated() {
        let d = mem_store();
        assert!(d.mark("sig", "k").await);
        assert!(
            d.mark("launch", "k").await,
            "same key in another namespace is new"
        );
    }

    #[tokio::test]
    async fn contains_does_not_mark() {
        let d = mem_store();
        assert!(!d.contains("sig", "x").await);
        assert!(d.mark("sig", "x").await, "contains did not pre-mark");
        assert!(d.contains("sig", "x").await);
    }

    #[tokio::test]
    async fn forget_allows_reprocessing() {
        let d = mem_store();
        assert!(d.mark("sig", "r").await);
        d.forget("sig", "r").await;
        assert!(
            d.mark("sig", "r").await,
            "after forget the key is new again"
        );
    }

    #[tokio::test]
    async fn l1_is_bounded_per_namespace() {
        let d = DedupStore::new(
            DedupBackend::Memory,
            None,
            None,
            Duration::from_secs(60),
            64,
        );
        for i in 0..200 {
            d.mark("sig", &format!("k{i}")).await;
        }
        assert!(d.len("sig").await <= 64, "L1 stays under cap");
        // The most recent key survives; the oldest was evicted.
        assert!(d.contains("sig", "k199").await);
        assert!(!d.contains("sig", "k0").await);
    }

    #[test]
    fn backend_parses_and_degrades_to_memory_without_clients() {
        assert_eq!(DedupBackend::parse("redis"), DedupBackend::Redis);
        assert_eq!(DedupBackend::parse("postgres"), DedupBackend::Postgres);
        assert_eq!(DedupBackend::parse("junk"), DedupBackend::Memory);
        let d = DedupStore::new(DedupBackend::Redis, None, None, Duration::from_secs(60), 64);
        assert_eq!(d.backend(), DedupBackend::Memory, "no client => memory");
    }
}
