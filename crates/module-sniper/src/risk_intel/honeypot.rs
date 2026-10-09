//! Honeypot / sell-path intelligence (GAP-MAP v2, P2).
//!
//! The authoritative honeypot check is the venue sell simulation —
//! `solana_kit::token_safety::classify_sell_probe` turns one
//! `simulateTransaction` result into a [`SellProbeOutcome`], and the P1 work
//! already wires that probe into the entry path (`simulate_sell`).
//!
//! This module adds the two things a repeated gate needs on top:
//! * a **TTL cache** keyed by mint, so a launch that survives 20 gate
//!   evaluations in a second is probed ONCE, not 20 times (each probe costs
//!   a full RPC round trip); and
//! * a **decision layer** that maps the three outcomes onto gate behaviour
//!   without ever guessing: `Sellable` passes, `NotSellable` rejects, and
//!   `Unknown` stays UNKNOWN — strict gating decides whether unknown is a
//!   skip or a reject. A honeypot verdict is never invented from a probe
//!   that did not run.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use solana_kit::token_safety::SellProbeOutcome;

/// A cached probe verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CachedProbe {
    outcome: SellProbeOutcome,
    probed_at: DateTime<Utc>,
}

/// TTL cache of sell-probe verdicts.
#[derive(Debug)]
pub struct SellProbeCache {
    entries: HashMap<Pubkey, CachedProbe>,
    ttl: Duration,
}

impl SellProbeCache {
    /// Build a cache with a verdict TTL. A zero TTL disables caching
    /// (every lookup misses) — useful for tests and paranoid profiles.
    pub fn new(ttl: Duration) -> Self {
        SellProbeCache {
            entries: HashMap::new(),
            ttl,
        }
    }

    /// Look up a fresh verdict for `mint` at `now`. Expired entries are
    /// treated as misses (and dropped), so a stale probe can never gate a
    /// new decision.
    pub fn get(&mut self, mint: &Pubkey, now: DateTime<Utc>) -> Option<&SellProbeOutcome> {
        if self.ttl.is_zero() {
            return None;
        }
        // Drop-if-expired, then borrow.
        if let Some(entry) = self.entries.get(mint) {
            let age = now.signed_duration_since(entry.probed_at);
            let expired = age.to_std().map(|d| d > self.ttl).unwrap_or(true);
            if expired {
                self.entries.remove(mint);
                return None;
            }
        }
        self.entries.get(mint).map(|e| &e.outcome)
    }

    /// Store a fresh verdict.
    pub fn insert(&mut self, mint: Pubkey, outcome: SellProbeOutcome, at: DateTime<Utc>) {
        self.entries.insert(mint, CachedProbe { outcome, probed_at: at });
    }

    /// Forget every expired entry (call periodically; cheap).
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let ttl = self.ttl;
        if ttl.is_zero() {
            self.entries.clear();
            return;
        }
        self.entries.retain(|_, entry| {
            now.signed_duration_since(entry.probed_at)
                .to_std()
                .map(|d| d <= ttl)
                .unwrap_or(false)
        });
    }

    /// Number of live (stored, not necessarily fresh) entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no verdicts are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The gate decision derived from a probe outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoneypotVerdict {
    /// Sell path confirmed intact.
    Sellable,
    /// Sell path confirmed broken — reject the entry.
    Honeypot,
    /// The probe could not produce a verdict (or was not run). This is a
    /// skip, not a pass: the caller's `strict_gates` setting decides whether
    /// an unknown sell path is acceptable.
    Unknown,
}

impl HoneypotVerdict {
    /// True when this verdict alone is enough to REJECT an entry.
    pub fn rejects(self) -> bool {
        matches!(self, HoneypotVerdict::Honeypot)
    }

    /// True when this verdict positively CONFIRMS sellability.
    pub fn confirms_sellable(self) -> bool {
        matches!(self, HoneypotVerdict::Sellable)
    }
}

/// Map a probe outcome to a gate verdict. Total and pure.
pub fn decide(outcome: &SellProbeOutcome) -> HoneypotVerdict {
    match outcome {
        SellProbeOutcome::Sellable => HoneypotVerdict::Sellable,
        SellProbeOutcome::NotSellable(_) => HoneypotVerdict::Honeypot,
        SellProbeOutcome::Unknown(_) => HoneypotVerdict::Unknown,
    }
}

/// Apply `strict_gates` to a verdict: in strict mode an UNKNOWN sell path
/// rejects; otherwise it is a skip (returns false = don't reject).
pub fn rejects_with(verdict: HoneypotVerdict, strict_gates: bool) -> bool {
    match verdict {
        HoneypotVerdict::Honeypot => true,
        HoneypotVerdict::Unknown => strict_gates,
        HoneypotVerdict::Sellable => false,
    }
}

/// Readable reason for telemetry / gate summaries.
pub fn reason(verdict: HoneypotVerdict, outcome: Option<&SellProbeOutcome>) -> String {
    match verdict {
        HoneypotVerdict::Sellable => "sell probe: path intact".to_string(),
        HoneypotVerdict::Honeypot => {
            let detail = match outcome {
                Some(SellProbeOutcome::NotSellable(d)) => d.clone(),
                _ => "sell path broken".to_string(),
            };
            format!("sell probe: honeypot — {detail}")
        }
        HoneypotVerdict::Unknown => {
            let detail = match outcome {
                Some(SellProbeOutcome::Unknown(d)) => format!(" ({d})"),
                Some(_) | None => String::new(),
            };
            format!("sell probe: no verdict{detail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn decision_mapping_is_total() {
        assert_eq!(decide(&SellProbeOutcome::Sellable), HoneypotVerdict::Sellable);
        assert_eq!(
            decide(&SellProbeOutcome::NotSellable("frozen".into())),
            HoneypotVerdict::Honeypot
        );
        assert_eq!(
            decide(&SellProbeOutcome::Unknown("transport".into())),
            HoneypotVerdict::Unknown
        );
    }

    #[test]
    fn strictness_controls_unknown() {
        assert!(!rejects_with(HoneypotVerdict::Unknown, false), "lenient skips");
        assert!(rejects_with(HoneypotVerdict::Unknown, true), "strict rejects");
        assert!(rejects_with(HoneypotVerdict::Honeypot, false), "honeypot always rejects");
        assert!(!rejects_with(HoneypotVerdict::Sellable, true), "sellable never rejects");
    }

    #[test]
    fn cache_returns_fresh_verdicts_within_ttl() {
        let mut cache = SellProbeCache::new(Duration::from_secs(60));
        let mint = Pubkey::new_unique();
        cache.insert(mint, SellProbeOutcome::Sellable, t(1_000));
        assert_eq!(cache.get(&mint, t(1_030)), Some(&SellProbeOutcome::Sellable));
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut cache = SellProbeCache::new(Duration::from_secs(60));
        let mint = Pubkey::new_unique();
        cache.insert(mint, SellProbeOutcome::Sellable, t(1_000));
        assert_eq!(cache.get(&mint, t(1_061)), None, "past TTL = miss");
        assert!(cache.is_empty(), "expired entry dropped");
    }

    #[test]
    fn zero_ttl_disables_caching() {
        let mut cache = SellProbeCache::new(Duration::ZERO);
        let mint = Pubkey::new_unique();
        cache.insert(mint, SellProbeOutcome::Sellable, t(1_000));
        assert_eq!(cache.get(&mint, t(1_000)), None);
    }

    #[test]
    fn prune_removes_only_expired() {
        let mut cache = SellProbeCache::new(Duration::from_secs(60));
        let fresh = Pubkey::new_unique();
        let stale = Pubkey::new_unique();
        cache.insert(fresh, SellProbeOutcome::Sellable, t(1_000));
        cache.insert(stale, SellProbeOutcome::Sellable, t(900));
        cache.prune(t(1_050)); // stale is 150s old, fresh is 50s
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&fresh, t(1_050)), Some(&SellProbeOutcome::Sellable));
        assert_eq!(cache.get(&stale, t(1_050)), None);
    }

    #[test]
    fn honeypot_reason_carries_the_detail() {
        let outcome = SellProbeOutcome::NotSellable("sells disabled by mint".into());
        let verdict = decide(&outcome);
        assert!(reason(verdict, Some(&outcome)).contains("sells disabled by mint"));
    }
}
