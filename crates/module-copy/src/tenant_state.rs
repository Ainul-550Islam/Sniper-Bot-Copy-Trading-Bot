//! Tenant-scoped copy state (PROMPT 4/10 file 13).
//!
//! All in-memory leader/event bookkeeping in `module-copy` is keyed by
//! [`CopyTenantContext`] — so the same external leader address can be
//! tracked by many tenants at once, and no per-tenant map ever leaks
//! into another tenant's view. The dedup structure is FIFO-bounded: a
//! tenant cannot grow memory without bound by streaming events.

use std::collections::VecDeque;
use std::sync::RwLock;

/// Maximum leader-side event ids retained per tenant for dedup. FIFO:
/// the oldest id is evicted when the window is full.
const MAX_SEEN_EVENTS: usize = 8192;

/// Bounded per-tenant event dedup window.
struct EventDedup {
    seen: VecDeque<String>,
}

impl EventDedup {
    fn new() -> Self {
        EventDedup {
            seen: VecDeque::with_capacity(64),
        }
    }

    /// Record an event id; returns true when the id is NEW (first time).
    fn record(&mut self, event_id: &str) -> bool {
        if self.seen.iter().any(|id| id == event_id) {
            return false;
        }
        if self.seen.len() == MAX_SEEN_EVENTS {
            self.seen.pop_front();
        }
        self.seen.push_back(event_id.to_string());
        true
    }

    fn len(&self) -> usize {
        self.seen.len()
    }
}

/// Counters one tenant's copy run produces (per tenant, not global).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TenantCopyCounters {
    /// Leader events considered.
    pub considered: u64,
    /// Events rejected (risk, duplicate, sizing, gating).
    pub rejected: u64,
    /// Mirrored submissions broadcast.
    pub submitted: u64,
    /// Submissions confirmed on chain.
    pub succeeded: u64,
    /// Submissions vetoed by the tenant broadcast guard.
    pub guard_denied: u64,
}

impl TenantCopyCounters {
    fn on_rejected(&mut self) {
        self.rejected += 1;
    }

    fn on_submitted(&mut self) {
        self.submitted += 1;
    }

    fn on_succeeded(&mut self) {
        self.succeeded += 1;
    }

    fn on_guard_denied(&mut self) {
        self.guard_denied += 1;
    }
}

/// The per-tenant copy bookkeeping: leader links, event dedup and
/// counters. Safe to share across tasks; every method is tenant-local
/// by construction (the state belongs to exactly one tenant).
pub struct TenantCopyState {
    dedup: RwLock<EventDedup>,
    counters: RwLock<TenantCopyCounters>,
}

impl TenantCopyState {
    /// Fresh state for one tenant.
    pub fn new() -> Self {
        TenantCopyState {
            dedup: RwLock::new(EventDedup::new()),
            counters: RwLock::new(TenantCopyCounters::default()),
        }
    }

    /// First sight of this event id for this tenant? (Also records it.)
    pub fn first_sight(&self, event_id: &str) -> bool {
        self.dedup
            .write()
            .expect("dedup lock poisoned")
            .record(event_id)
    }

    /// Current dedup window length (diagnostics).
    pub fn dedup_len(&self) -> usize {
        self.dedup.read().expect("dedup lock poisoned").len()
    }

    /// Record that an event was considered.
    pub fn on_considered(&self) {
        self.counters
            .write()
            .expect("counters lock poisoned")
            .considered += 1;
    }

    /// Record a rejection.
    pub fn on_rejected(&self) {
        self.counters
            .write()
            .expect("counters lock poisoned")
            .on_rejected();
    }

    /// Record a broadcast submission.
    pub fn on_submitted(&self) {
        self.counters
            .write()
            .expect("counters lock poisoned")
            .on_submitted();
    }

    /// Record a confirmed submission.
    pub fn on_succeeded(&self) {
        self.counters
            .write()
            .expect("counters lock poisoned")
            .on_succeeded();
    }

    /// Record a guard veto.
    pub fn on_guard_denied(&self) {
        self.counters
            .write()
            .expect("counters lock poisoned")
            .on_guard_denied();
    }

    /// A snapshot of the counters (per tenant).
    pub fn counters(&self) -> TenantCopyCounters {
        *self.counters.read().expect("counters lock poisoned")
    }
}

impl Default for TenantCopyState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_events_are_suppressed_within_a_tenant() {
        let state = TenantCopyState::new();
        assert!(state.first_sight("evt-1"));
        assert!(!state.first_sight("evt-1"));
        assert!(state.first_sight("evt-2"));
        assert_eq!(state.dedup_len(), 2);
    }

    #[test]
    fn the_dedup_window_is_fifo_bounded() {
        let state = TenantCopyState::new();
        for i in 0..(MAX_SEEN_EVENTS + 10) {
            assert!(state.first_sight(&format!("evt-{i}")));
        }
        assert_eq!(state.dedup_len(), MAX_SEEN_EVENTS);
        // The oldest ids were evicted: they count as new again (the
        // window trades memory for a bounded replay horizon, as
        // specified).
        assert!(state.first_sight("evt-0"));
    }

    #[test]
    fn two_tenants_never_share_dedup_or_counters() {
        let a = TenantCopyState::new();
        let b = TenantCopyState::new();
        assert!(a.first_sight("evt-1"));
        // Tenant B sees the same external event id: independent state.
        assert!(b.first_sight("evt-1"));
        a.on_considered();
        a.on_submitted();
        a.on_guard_denied();
        assert_eq!(b.counters(), TenantCopyCounters::default());
        let ca = a.counters();
        assert_eq!(ca.considered, 1);
        assert_eq!(ca.submitted, 1);
        assert_eq!(ca.guard_denied, 1);
    }

    #[test]
    fn counters_track_the_full_outcome_set() {
        let state = TenantCopyState::new();
        state.on_considered();
        state.on_considered();
        state.on_rejected();
        state.on_submitted();
        state.on_submitted();
        state.on_succeeded();
        let c = state.counters();
        assert_eq!(c.considered, 2);
        assert_eq!(c.rejected, 1);
        assert_eq!(c.submitted, 2);
        assert_eq!(c.succeeded, 1);
        assert_eq!(c.guard_denied, 0);
    }
}
