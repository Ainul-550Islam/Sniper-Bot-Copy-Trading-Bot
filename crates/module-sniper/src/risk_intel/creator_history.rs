//! Creator-history intelligence (GAP-MAP v2, P2).
//!
//! Tracks what a creator wallet's PREVIOUS launches did, so the gate layer
//! can refuse serial ruggers. This is an in-memory ledger by design:
//! * it is populated live from launch outcomes the sniper itself observes
//!   (a launch that ends in a dev drain / rug / migration is recorded);
//! * it makes NO historical claims it cannot back with observed data — an
//!   unseen creator has an EMPTY history, which the gate treats as "unknown"
//!   (skip or accept per `strict_gates`), never as guilty.
//!
//! Persistence is deliberately out of scope here: a restart starts the
//! ledger cold, which degrades to "unknown creator" — safe in both
//! directions. A durable store is a follow-up if operators ask for it.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// How a tracked launch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchOutcome {
    /// Creator drained / dumped their bag (rug pattern).
    Rugged,
    /// Token graduated / migrated with liquidity intact.
    Migrated,
    /// Still live; not evidence either way.
    Active,
    /// Faded without a clear rug (low volume, abandoned).
    Faded,
}

impl LaunchOutcome {
    /// Only a confirmed rug counts against the creator. Everything else is
    /// either neutral (Active) or weak/ambiguous (Migrated, Faded) — the
    /// rug rate must never be inflated by non-rug endings.
    pub fn is_rug(self) -> bool {
        matches!(self, LaunchOutcome::Rugged)
    }
}

/// One observed launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchRecord {
    pub mint: Pubkey,
    pub outcome: LaunchOutcome,
    pub observed_at: DateTime<Utc>,
}

/// Per-creator aggregate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatorStats {
    /// Launches recorded for this creator.
    pub launches: u32,
    /// Of those, how many ended as rugs.
    pub rugs: u32,
}

impl CreatorStats {
    /// Rug rate in basis points (0..=10_000). Zero launches -> 0 (unknown
    /// is reported separately via `launches`, never as "0% rugs").
    pub fn rug_rate_bps(&self) -> u64 {
        if self.launches == 0 {
            return 0;
        }
        ((self.rugs as u64) * 10_000) / (self.launches as u64)
    }
}

/// The ledger.
#[derive(Debug, Default)]
pub struct CreatorLedger {
    by_creator: HashMap<Pubkey, Vec<LaunchRecord>>,
    /// Minimum recorded launches before the rug rate is considered
    /// statistically usable (a single rug from a first-time creator is a
    /// red flag but the RATE itself needs samples).
    min_samples: u32,
}

impl CreatorLedger {
    /// Build a ledger with a sample floor (`0` = use the rate from launch 1).
    pub fn new(min_samples: u32) -> Self {
        CreatorLedger {
            by_creator: HashMap::new(),
            min_samples,
        }
    }

    /// Record one launch outcome for a creator. Idempotent per mint: the
    /// FIRST terminal outcome wins (a launch cannot rug twice, and a later
    /// duplicate observation must not double-count).
    pub fn record(&mut self, creator: &Pubkey, record: LaunchRecord) {
        let entries = self.by_creator.entry(*creator).or_default();
        if entries.iter().any(|r| r.mint == record.mint) {
            return;
        }
        entries.push(record);
    }

    /// Stats for a creator (`None` = never seen).
    pub fn stats(&self, creator: &Pubkey) -> Option<CreatorStats> {
        let entries = self.by_creator.get(creator)?;
        Some(CreatorStats {
            launches: entries.len() as u32,
            rugs: entries.iter().filter(|r| r.outcome.is_rug()).count() as u32,
        })
    }

    /// The creator's rug rate in bps, ONLY when enough launches have been
    /// observed (`min_samples`). `None` = unknown — the gate decides what
    /// unknown means.
    pub fn rug_rate_bps(&self, creator: &Pubkey) -> Option<u64> {
        let stats = self.stats(creator)?;
        if stats.launches < self.min_samples.max(1) {
            return None;
        }
        Some(stats.rug_rate_bps())
    }

    /// Hard reject predicate: the creator has AT LEAST `min_rugs` recorded
    /// rugs (a count, not a rate — one confirmed rug from a repeat creator
    /// is enough for most operators; the threshold is theirs).
    pub fn is_serial_rugger(&self, creator: &Pubkey, min_rugs: u32) -> bool {
        if min_rugs == 0 {
            return false;
        }
        self.stats(creator).map(|s| s.rugs >= min_rugs).unwrap_or(false)
    }

    /// Drop records older than `cutoff` (operators may not want a 2-year-old
    /// faded launch weighing on today's decision).
    pub fn forget_before(&mut self, cutoff: DateTime<Utc>) {
        for entries in self.by_creator.values_mut() {
            entries.retain(|r| r.observed_at >= cutoff);
        }
        self.by_creator.retain(|_, entries| !entries.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(mint_seed: u8, outcome: LaunchOutcome, at: i64) -> LaunchRecord {
        LaunchRecord {
            mint: Pubkey::new_from_array([mint_seed; 32]),
            outcome,
            observed_at: DateTime::from_timestamp(at, 0).unwrap(),
        }
    }

    #[test]
    fn unknown_creator_is_none_not_zero() {
        let ledger = CreatorLedger::new(1);
        assert!(ledger.stats(&Pubkey::new_unique()).is_none());
        assert!(ledger.rug_rate_bps(&Pubkey::new_unique()).is_none());
        assert!(!ledger.is_serial_rugger(&Pubkey::new_unique(), 1));
    }

    #[test]
    fn rug_rate_counts_only_confirmed_rugs() {
        let mut ledger = CreatorLedger::new(1);
        let creator = Pubkey::new_unique();
        ledger.record(&creator, rec(1, LaunchOutcome::Rugged, 100));
        ledger.record(&creator, rec(2, LaunchOutcome::Migrated, 200));
        ledger.record(&creator, rec(3, LaunchOutcome::Faded, 300));
        ledger.record(&creator, rec(4, LaunchOutcome::Active, 400));
        let stats = ledger.stats(&creator).unwrap();
        assert_eq!(stats.launches, 4);
        assert_eq!(stats.rugs, 1);
        assert_eq!(stats.rug_rate_bps(), 2_500, "1 of 4 = 25%");
    }

    #[test]
    fn duplicate_mint_is_not_double_counted() {
        let mut ledger = CreatorLedger::new(1);
        let creator = Pubkey::new_unique();
        ledger.record(&creator, rec(1, LaunchOutcome::Active, 100));
        ledger.record(&creator, rec(1, LaunchOutcome::Rugged, 200)); // dup mint
        let stats = ledger.stats(&creator).unwrap();
        assert_eq!(stats.launches, 1, "first terminal outcome wins");
        assert_eq!(stats.rugs, 0);
    }

    #[test]
    fn sample_floor_suppresses_thin_histories() {
        let mut ledger = CreatorLedger::new(3);
        let creator = Pubkey::new_unique();
        ledger.record(&creator, rec(1, LaunchOutcome::Rugged, 100));
        assert!(
            ledger.rug_rate_bps(&creator).is_none(),
            "1 launch < floor of 3 => unknown"
        );
        ledger.record(&creator, rec(2, LaunchOutcome::Rugged, 200));
        ledger.record(&creator, rec(3, LaunchOutcome::Rugged, 300));
        assert_eq!(ledger.rug_rate_bps(&creator), Some(10_000));
    }

    #[test]
    fn serial_rugger_threshold_is_a_count() {
        let mut ledger = CreatorLedger::new(1);
        let creator = Pubkey::new_unique();
        ledger.record(&creator, rec(1, LaunchOutcome::Rugged, 100));
        ledger.record(&creator, rec(2, LaunchOutcome::Rugged, 200));
        assert!(ledger.is_serial_rugger(&creator, 2));
        assert!(!ledger.is_serial_rugger(&creator, 3));
        assert!(!ledger.is_serial_rugger(&creator, 0), "0 disables");
    }

    #[test]
    fn forget_before_prunes_old_records() {
        let mut ledger = CreatorLedger::new(1);
        let creator = Pubkey::new_unique();
        ledger.record(&creator, rec(1, LaunchOutcome::Rugged, 100));
        ledger.record(&creator, rec(2, LaunchOutcome::Migrated, 500));
        ledger.forget_before(DateTime::from_timestamp(300, 0).unwrap());
        let stats = ledger.stats(&creator).unwrap();
        assert_eq!(stats.launches, 1);
        assert_eq!(stats.rugs, 0, "the old rug was forgotten");
    }
}
