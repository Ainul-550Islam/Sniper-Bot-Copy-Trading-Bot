//! Tenant-scoped sniper runtime state (PROMPT 4/10 file 10).
//!
//! [`TenantSniperState`] is the per-tenant, in-process runtime state the
//! tenant sniper executor keeps beside the (global) detection and risk
//! engines: which external launch events this tenant has already acted
//! on, an outcome counter set for observability, and the tenant's module
//! mode snapshot. It contains NO secrets — public ids, counters and
//! booleans only.
//!
//! The seen-set is bounded: a tenant that sees a flood of launches keeps
//! at most [`MAX_SEEN_EVENTS`] entries (FIFO eviction), so memory is
//! predictable no matter how hot the feed is. Dedup is best-effort at the
//! tenant layer; correctness is enforced downstream by the deterministic
//! intent ids and the execution ledger (the same double-submission guard
//! the operator path uses).

use std::collections::VecDeque;

use bot_core::models::ExecutionMode;

/// How many external event ids the tenant seen-set retains.
pub const MAX_SEEN_EVENTS: usize = 8_192;

/// Per-tenant outcome counters (observability; public data).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TenantSniperCounters {
    /// Events considered for this tenant.
    pub considered: u64,
    /// Events rejected before money (gates/risk/dedup).
    pub rejected: u64,
    /// Executions that reached the (guarded) executor.
    pub submitted: u64,
    /// Executions that succeeded (paper fill or confirmed).
    pub succeeded: u64,
    /// Broadcasts refused by the tenant guard (should stay zero in
    /// healthy operation — a non-zero value means a wiring bug).
    pub guard_denied: u64,
}

/// Tenant-scoped sniper runtime state.
#[derive(Debug)]
pub struct TenantSniperState {
    organization_label: String,
    runtime_label: String,
    mode: ExecutionMode,
    seen: VecDeque<String>,
    counters: TenantSniperCounters,
}

impl TenantSniperState {
    /// New state for one tenant runtime.
    pub fn new(
        organization_label: impl Into<String>,
        runtime_label: impl Into<String>,
        mode: ExecutionMode,
    ) -> Self {
        TenantSniperState {
            organization_label: organization_label.into(),
            runtime_label: runtime_label.into(),
            mode,
            seen: VecDeque::with_capacity(64),
            counters: TenantSniperCounters::default(),
        }
    }

    /// The tenant label (public, for logs and metrics).
    pub fn organization_label(&self) -> &str {
        &self.organization_label
    }

    /// The runtime label (public).
    pub fn runtime_label(&self) -> &str {
        &self.runtime_label
    }

    /// The mode this tenant's sniper runs under.
    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }

    /// The outcome counters.
    pub fn counters(&self) -> TenantSniperCounters {
        self.counters
    }

    /// Record a considered event.
    pub fn note_considered(&mut self) {
        self.counters.considered += 1;
    }

    /// Record a pre-money rejection.
    pub fn note_rejected(&mut self) {
        self.counters.rejected += 1;
    }

    /// Record a submission to the guarded executor.
    pub fn note_submitted(&mut self) {
        self.counters.submitted += 1;
    }

    /// Record a successful execution.
    pub fn note_succeeded(&mut self) {
        self.counters.succeeded += 1;
    }

    /// Record a guard denial (a wiring bug by construction: the tenant
    /// executor must never build a request the guard would refuse).
    pub fn note_guard_denied(&mut self) {
        self.counters.guard_denied += 1;
    }

    /// Have we already acted on this dedup key? When not, remember it.
    ///
    /// Returns `true` when the key is NEW (first sight), `false` when it
    /// was already seen (a duplicate for this tenant).
    pub fn mark_seen(&mut self, dedup_key: &str) -> bool {
        if self.seen.iter().any(|k| k == dedup_key) {
            return false;
        }
        if self.seen.len() == MAX_SEEN_EVENTS {
            self.seen.pop_front();
        }
        self.seen.push_back(dedup_key.to_string());
        true
    }

    /// Whether a dedup key has been seen (without recording it).
    pub fn has_seen(&self, dedup_key: &str) -> bool {
        self.seen.iter().any(|k| k == dedup_key)
    }

    /// Number of retained seen keys.
    pub fn seen_len(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_keys_are_first_sight_and_repeats_are_duplicates() {
        let mut state = TenantSniperState::new("org-a", "runtime-1", ExecutionMode::Paper);
        assert!(state.mark_seen("sniper:o1:r1:launch-1"));
        assert!(!state.mark_seen("sniper:o1:r1:launch-1"));
        assert!(state.has_seen("sniper:o1:r1:launch-1"));
        assert!(!state.has_seen("sniper:o1:r1:launch-2"));
        assert_eq!(state.seen_len(), 1);
    }

    #[test]
    fn two_tenants_never_share_seen_state() {
        let mut a = TenantSniperState::new("org-a", "runtime-1", ExecutionMode::Paper);
        let mut b = TenantSniperState::new("org-b", "runtime-1", ExecutionMode::Paper);
        assert!(a.mark_seen("sniper:o1:r1:launch-1"));
        // The SAME external event is new for tenant B — one tenant's
        // seen-set must never suppress another tenant's trade.
        assert!(b.mark_seen("sniper:o1:r1:launch-1"));
    }

    #[test]
    fn seen_set_is_bounded() {
        let mut state = TenantSniperState::new("org-a", "runtime-1", ExecutionMode::Paper);
        for i in 0..=(MAX_SEEN_EVENTS + 8) {
            assert!(state.mark_seen(&format!("sniper:o1:r1:launch-{i}")));
        }
        assert_eq!(state.seen_len(), MAX_SEEN_EVENTS);
        // The oldest entries were evicted FIFO.
        assert!(!state.has_seen("sniper:o1:r1:launch-0"));
        assert!(state.has_seen(&format!("sniper:o1:r1:launch-{}", MAX_SEEN_EVENTS + 8)));
    }

    #[test]
    fn counters_count() {
        let mut state = TenantSniperState::new("org-a", "runtime-1", ExecutionMode::Paper);
        state.note_considered();
        state.note_considered();
        state.note_rejected();
        state.note_submitted();
        state.note_succeeded();
        state.note_guard_denied();
        let c = state.counters();
        assert_eq!(c.considered, 2);
        assert_eq!(c.rejected, 1);
        assert_eq!(c.submitted, 1);
        assert_eq!(c.succeeded, 1);
        assert_eq!(c.guard_denied, 1);
    }

    #[test]
    fn state_carries_no_secret_material() {
        let state = TenantSniperState::new("org-a", "runtime-1", ExecutionMode::Paper);
        let rendered = format!("{state:?}");
        assert!(!rendered.to_lowercase().contains("secret"));
        assert!(!rendered.to_lowercase().contains("keypair"));
        assert!(!rendered.to_lowercase().contains("seed"));
        assert_eq!(state.mode(), ExecutionMode::Paper);
    }
}
