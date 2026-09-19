//! A warm account cache for the latency-critical read path (BUILD PLAN §5).
//!
//! Sniping and copying re-read the same *semi-static* accounts on every
//! decision: the pump `Global` config (changes only when pump.fun's admin
//! touches fees), a mint's token program (never changes), an ATA's existence
//! (changes at most once). Each unnecessary `getAccountInfo` round trip costs
//! 50–200 ms — most of the 1-second snipe budget.
//!
//! The cache is deliberately conservative:
//!
//! * **TTL per lookup, not per cache** — every call site passes the staleness
//!   it can tolerate (`Duration::ZERO` = always fetch). Price-bearing accounts
//!   (bonding curves, pool state) must never be cached; nothing forces them
//!   through here.
//! * **Only positive hits are cached** — a missing account is exactly the
//!   thing that appears mid-flight (a brand-new mint, a just-created ATA), so
//!   misses always go to the network.
//! * **Bounded** — FIFO eviction past `max_entries`; a flooded key space
//!   cannot grow the process without limit.
//! * **Observable** — hit/miss/stale counters are exported to the §6 metrics
//!   registry, so the win is measurable instead of assumed.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::RwLock;

/// Cache outcome, reported via `bot_account_cache_total{outcome}`.
const METRIC: &str = "bot_account_cache_total";
const METRIC_HELP: &str = "Warm account cache lookups by outcome.";

struct Entry {
    account: Account,
    fetched_at: Instant,
}

/// A TTL + size-bounded cache of account snapshots.
///
/// Cheap to clone (all state is behind `Arc`-like shared handles: one `RwLock`
/// map plus atomic counters).
#[derive(Clone)]
pub struct AccountCache {
    entries: Arc<RwLock<HashMap<Pubkey, Entry>>>,
    /// Insertion order, for FIFO eviction.
    order: Arc<RwLock<VecDeque<Pubkey>>>,
    max_entries: usize,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    stale: Arc<AtomicU64>,
}

impl AccountCache {
    /// Create a cache holding at most `max_entries` accounts. `0` disables
    /// caching entirely (every lookup misses, every insert is dropped).
    pub fn new(max_entries: usize) -> Self {
        AccountCache {
            entries: Arc::new(RwLock::new(HashMap::new())),
            order: Arc::new(RwLock::new(VecDeque::new())),
            max_entries,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            stale: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A fresh-enough cached account, or `None`. `max_age == Duration::ZERO`
    /// never hits (the entry stays cached for other readers).
    pub async fn get(&self, key: &Pubkey, max_age: Duration) -> Option<Account> {
        if self.max_entries == 0 {
            self.count_miss();
            return None;
        }
        enum Outcome {
            Hit(Account),
            Stale,
            Miss,
        }
        let outcome = {
            let guard = self.entries.read().await;
            match guard.get(key) {
                Some(entry) if entry.fetched_at.elapsed() <= max_age => {
                    Outcome::Hit(entry.account.clone())
                }
                Some(_) => Outcome::Stale,
                None => Outcome::Miss,
            }
        };
        match outcome {
            Outcome::Hit(account) => {
                self.count_hit();
                Some(account)
            }
            Outcome::Stale => {
                self.count_stale();
                None
            }
            Outcome::Miss => {
                self.count_miss();
                None
            }
        }
    }

    /// Store a snapshot. Evicts the oldest entries past `max_entries`.
    pub async fn insert(&self, key: Pubkey, account: Account) {
        if self.max_entries == 0 {
            return;
        }
        {
            let mut entries = self.entries.write().await;
            let is_new = !entries.contains_key(&key);
            entries.insert(
                key,
                Entry {
                    account,
                    fetched_at: Instant::now(),
                },
            );
            if is_new {
                let mut order = self.order.write().await;
                order.push_back(key);
            }
        }
        self.evict().await;
    }

    /// Drop one key (e.g. after a transaction that mutated it).
    pub async fn invalidate(&self, key: &Pubkey) {
        self.entries.write().await.remove(key);
        self.order.write().await.retain(|k| k != key);
    }

    /// Drop everything (cluster switch, manual reset).
    pub async fn clear(&self) {
        self.entries.write().await.clear();
        self.order.write().await.clear();
    }

    /// Currently cached keys (including stale-not-yet-evicted ones).
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// (hits, misses, stale) since construction.
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.stale.load(Ordering::Relaxed),
        )
    }

    async fn evict(&self) {
        let mut entries = self.entries.write().await;
        let mut order = self.order.write().await;
        while entries.len() > self.max_entries {
            match order.pop_front() {
                Some(oldest) => {
                    entries.remove(&oldest);
                }
                // Order queue and map disagree (should not happen); rebuild.
                None => {
                    let keys: Vec<Pubkey> = entries.keys().copied().collect();
                    for k in keys
                        .iter()
                        .take(keys.len().saturating_sub(self.max_entries))
                    {
                        entries.remove(k);
                    }
                    order.extend(entries.keys().copied());
                    break;
                }
            }
        }
    }

    fn count_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        Self::bump("hit");
    }

    fn count_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        Self::bump("miss");
    }

    fn count_stale(&self) {
        self.stale.fetch_add(1, Ordering::Relaxed);
        Self::bump("stale");
    }

    fn bump(outcome: &str) {
        // Closed label set (hit|miss|stale) — bounded cardinality, safe for
        // the process-wide registry (§6 contract).
        bot_core::obs::metrics::global()
            .counter(METRIC, METRIC_HELP, &[("outcome", outcome)])
            .inc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey::Pubkey;

    fn account(lamports: u64) -> Account {
        Account {
            lamports,
            data: vec![1, 2, 3],
            owner: Pubkey::new_unique(),
            executable: false,
            rent_epoch: 0,
        }
    }

    #[tokio::test]
    async fn fresh_entry_hits_and_stale_entry_does_not() {
        let cache = AccountCache::new(16);
        let key = Pubkey::new_unique();
        cache.insert(key, account(42)).await;

        let hit = cache.get(&key, Duration::from_secs(30)).await;
        assert_eq!(hit.map(|a| a.lamports), Some(42));

        // Zero TTL never hits — the value stays cached for other readers.
        assert!(cache.get(&key, Duration::ZERO).await.is_none());

        // Wait out a tiny TTL.
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(cache.get(&key, Duration::from_millis(10)).await.is_none());

        // An existing-but-too-old entry (including the zero-TTL lookup) is
        // accounted as *stale*, not miss: the key was present, the value was
        // just not young enough to serve.
        let (hits, misses, stale) = cache.stats();
        assert_eq!((hits, misses, stale), (1, 0, 2));
    }

    #[tokio::test]
    async fn insert_overwrites_and_keeps_one_order_entry() {
        let cache = AccountCache::new(16);
        let key = Pubkey::new_unique();
        cache.insert(key, account(1)).await;
        cache.insert(key, account(2)).await;
        assert_eq!(cache.len().await, 1);
        let got = cache.get(&key, Duration::from_secs(5)).await.unwrap();
        assert_eq!(got.lamports, 2);
    }

    #[tokio::test]
    async fn fifo_eviction_at_capacity() {
        let cache = AccountCache::new(3);
        let keys: Vec<Pubkey> = (0..4).map(|_| Pubkey::new_unique()).collect();
        for (i, k) in keys.iter().enumerate() {
            cache.insert(*k, account(i as u64)).await;
        }
        assert_eq!(cache.len().await, 3);
        // Oldest (keys[0]) evicted; the rest present.
        assert!(cache.get(&keys[0], Duration::from_secs(5)).await.is_none());
        for k in &keys[1..] {
            assert!(cache.get(k, Duration::from_secs(5)).await.is_some());
        }
    }

    #[tokio::test]
    async fn disabled_cache_always_misses_and_stores_nothing() {
        let cache = AccountCache::new(0);
        let key = Pubkey::new_unique();
        cache.insert(key, account(7)).await;
        assert!(cache.get(&key, Duration::from_secs(60)).await.is_none());
        assert_eq!(cache.len().await, 0);
    }

    #[tokio::test]
    async fn invalidate_and_clear() {
        let cache = AccountCache::new(16);
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        cache.insert(a, account(1)).await;
        cache.insert(b, account(2)).await;
        cache.invalidate(&a).await;
        assert!(cache.get(&a, Duration::from_secs(5)).await.is_none());
        assert!(cache.get(&b, Duration::from_secs(5)).await.is_some());
        cache.clear().await;
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn concurrent_readers_and_writers_stay_consistent() {
        let cache = AccountCache::new(64);
        let keys: Vec<Pubkey> = (0..8).map(|_| Pubkey::new_unique()).collect();
        let mut handles = Vec::new();
        for i in 0..8u64 {
            let cache = cache.clone();
            let keys = keys.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..50 {
                    let k = keys[i as usize % keys.len()];
                    cache.insert(k, account(i)).await;
                    let _ = cache.get(&k, Duration::from_secs(5)).await;
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(cache.len().await <= 64);
        let (hits, misses, _stale) = cache.stats();
        assert_eq!(
            hits + misses,
            8 * 50,
            "every lookup is counted exactly once"
        );
    }
}
