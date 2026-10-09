//! Runtime fencing generation (STEP 3 file 04).
//!
//! A [`RuntimeGeneration`] is a monotonically increasing counter per
//! tenant: every runtime rotation (restart, failover, operator reassign)
//! increments it. A worker that holds generation N while the registry's
//! active runtime is at M > N is STALE — it lost the tenant, and every
//! money-moving action it still attempts must be denied (fence).
//!
//! This is the tenant-scoped sibling of the process-wide HA lease
//! generations in `crate::ha` (which use `i64`); conversions are explicit
//! and checked so the two worlds can never drift silently.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The first generation a tenant runtime can hold.
pub const FIRST_GENERATION: u64 = 1;

/// A fencing generation. Greater = newer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeGeneration(u64);

impl RuntimeGeneration {
    /// The first generation (used when a tenant registers its first
    /// runtime, or when the registry has no history).
    pub fn first() -> Self {
        RuntimeGeneration(FIRST_GENERATION)
    }

    /// Build from a raw counter (must be >= 1; the registry never issues 0).
    pub fn from_raw(raw: u64) -> Option<Self> {
        (raw >= FIRST_GENERATION).then_some(RuntimeGeneration(raw))
    }

    /// The raw counter.
    pub fn raw(self) -> u64 {
        self.0
    }

    /// Checked conversion from the HA layer's `i64` generations.
    pub fn from_i64(value: i64) -> Option<Self> {
        u64::try_from(value)
            .ok()
            .and_then(RuntimeGeneration::from_raw)
    }

    /// Checked conversion to the HA layer's `i64` generations.
    pub fn to_i64(self) -> Option<i64> {
        i64::try_from(self.0).ok()
    }

    /// The next generation after this one (saturating at the i64 boundary
    /// the durable registry uses).
    pub fn next(self) -> Option<Self> {
        self.0
            .checked_add(1)
            .and_then(RuntimeGeneration::from_raw)
            .filter(|g| g.to_i64().is_some())
    }

    /// Compare against another generation for fencing.
    pub fn compare_for_fence(self, other: RuntimeGeneration) -> FenceOrdering {
        match self.cmp(&other) {
            Ordering::Less => FenceOrdering::Stale,
            Ordering::Equal => FenceOrdering::Current,
            Ordering::Greater => FenceOrdering::Ahead,
        }
    }
}

/// The fence relationship between a claimed generation and the registry's
/// current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceOrdering {
    /// The claimed generation is behind the registry: the claimant is a
    /// stale worker and MUST be denied.
    Stale,
    /// Exact match with the current active runtime.
    Current,
    /// The claimed generation is ahead of the registry (clock skew or a
    /// registry write that has not landed yet). Fail closed: deny — a
    /// legitimate runtime never claims a generation the registry did not
    /// issue to it.
    Ahead,
}

impl FenceOrdering {
    /// Is this the only ordering that authorizes execution?
    pub fn authorizes(self) -> bool {
        matches!(self, FenceOrdering::Current)
    }

    /// Stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            FenceOrdering::Stale => "stale",
            FenceOrdering::Current => "current",
            FenceOrdering::Ahead => "ahead",
        }
    }
}

impl fmt::Display for RuntimeGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gen-{}", self.0)
    }
}

impl FromStr for RuntimeGeneration {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw: u64 = s
            .trim()
            .trim_start_matches("gen-")
            .parse()
            .map_err(|_| format!("invalid runtime generation: {s:?}"))?;
        RuntimeGeneration::from_raw(raw)
            .ok_or_else(|| format!("generation must be >= {FIRST_GENERATION}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_generation_is_one_and_raw_round_trips() {
        assert_eq!(RuntimeGeneration::first().raw(), 1);
        assert_eq!(RuntimeGeneration::from_raw(0), None);
        assert_eq!(RuntimeGeneration::from_raw(1).unwrap().raw(), 1);
    }

    #[test]
    fn next_is_monotonic_and_bounded() {
        let g1 = RuntimeGeneration::first();
        let g2 = g1.next().unwrap();
        assert_eq!(g2.raw(), 2);
        assert!(g2 > g1);
        let max = RuntimeGeneration::from_i64(i64::MAX).unwrap();
        assert_eq!(max.next(), None);
    }

    #[test]
    fn i64_conversions_are_checked() {
        assert_eq!(RuntimeGeneration::from_i64(1).unwrap().raw(), 1);
        assert_eq!(RuntimeGeneration::from_i64(0), None);
        assert_eq!(RuntimeGeneration::from_i64(-1), None);
        assert_eq!(RuntimeGeneration::from_i64(42).unwrap().to_i64(), Some(42));
    }

    #[test]
    fn fence_ordering_semantics() {
        let claimed = RuntimeGeneration::from_raw(3).unwrap();
        assert_eq!(
            claimed.compare_for_fence(RuntimeGeneration::from_raw(4).unwrap()),
            FenceOrdering::Stale
        );
        assert_eq!(
            claimed.compare_for_fence(RuntimeGeneration::from_raw(3).unwrap()),
            FenceOrdering::Current
        );
        assert_eq!(
            claimed.compare_for_fence(RuntimeGeneration::from_raw(2).unwrap()),
            FenceOrdering::Ahead
        );
        // Only "current" authorizes.
        assert!(!FenceOrdering::Stale.authorizes());
        assert!(FenceOrdering::Current.authorizes());
        assert!(!FenceOrdering::Ahead.authorizes());
    }

    #[test]
    fn display_and_parse() {
        let g = RuntimeGeneration::from_raw(7).unwrap();
        assert_eq!(g.to_string(), "gen-7");
        assert_eq!("gen-7".parse::<RuntimeGeneration>().unwrap(), g);
        assert_eq!("7".parse::<RuntimeGeneration>().unwrap(), g);
        assert!("gen-x".parse::<RuntimeGeneration>().is_err());
    }
}
