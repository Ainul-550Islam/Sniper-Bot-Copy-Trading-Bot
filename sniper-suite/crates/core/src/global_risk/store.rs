//! Durable contract of the global risk engine (TASK 5 §7): the decision
//! journal and the kill-switch state / event log, plus the in-memory
//! implementation used by tests and no-database runs.

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::decision::GlobalRiskDecision;
use super::kill_switch::{KillScope, KillSwitchEvent, KillSwitchState};

/// Journal contract. Writes answer `bool` (success); reads answer `Option`
/// (`None` = unavailable).
#[async_trait]
pub trait RiskStore: Send + Sync {
    /// Append one decision (accept or reject).
    async fn record_decision(&self, decision: &GlobalRiskDecision) -> bool;
    /// Recent decisions, newest first.
    async fn recent_decisions(&self, limit: usize) -> Option<Vec<GlobalRiskDecision>>;
    /// Upsert the current state of one switch.
    async fn upsert_kill_switch(&self, state: &KillSwitchState) -> bool;
    /// Append one switch change.
    async fn append_kill_switch_event(&self, event: &KillSwitchEvent) -> bool;
    /// Every stored switch state (restart restore).
    async fn load_kill_switches(&self) -> Option<Vec<KillSwitchState>>;
}

/// In-memory journal. Bounded decision ring (newest kept).
pub struct MemoryRiskStore {
    decisions: RwLock<Vec<GlobalRiskDecision>>,
    switches: RwLock<Vec<KillSwitchState>>,
    events: RwLock<Vec<KillSwitchEvent>>,
    cap: usize,
    unavailable: std::sync::atomic::AtomicBool,
}

impl Default for MemoryRiskStore {
    fn default() -> Self {
        MemoryRiskStore::new(2_000)
    }
}

impl MemoryRiskStore {
    /// Store keeping the newest `cap` decisions.
    pub fn new(cap: usize) -> Self {
        MemoryRiskStore {
            decisions: RwLock::new(Vec::new()),
            switches: RwLock::new(Vec::new()),
            events: RwLock::new(Vec::new()),
            cap: cap.max(1),
            unavailable: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Simulate an unavailable backend (tests only).
    pub fn set_unavailable(&self, on: bool) {
        self.unavailable
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    fn is_unavailable(&self) -> bool {
        self.unavailable.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Switch events recorded so far.
    pub async fn kill_switch_events(&self) -> Vec<KillSwitchEvent> {
        self.events.read().await.clone()
    }

    /// Number of decisions held.
    pub async fn decision_count(&self) -> usize {
        self.decisions.read().await.len()
    }
}

#[async_trait]
impl RiskStore for MemoryRiskStore {
    async fn record_decision(&self, decision: &GlobalRiskDecision) -> bool {
        if self.is_unavailable() {
            return false;
        }
        let mut d = self.decisions.write().await;
        d.push(decision.clone());
        if d.len() > self.cap {
            let overflow = d.len() - self.cap;
            d.drain(0..overflow);
        }
        true
    }

    async fn recent_decisions(&self, limit: usize) -> Option<Vec<GlobalRiskDecision>> {
        if self.is_unavailable() {
            return None;
        }
        let d = self.decisions.read().await;
        Some(d.iter().rev().take(limit).cloned().collect())
    }

    async fn upsert_kill_switch(&self, state: &KillSwitchState) -> bool {
        if self.is_unavailable() {
            return false;
        }
        let mut s = self.switches.write().await;
        if let Some(existing) = s.iter_mut().find(|x| x.scope == state.scope) {
            *existing = state.clone();
        } else {
            s.push(state.clone());
        }
        true
    }

    async fn append_kill_switch_event(&self, event: &KillSwitchEvent) -> bool {
        if self.is_unavailable() {
            return false;
        }
        self.events.write().await.push(event.clone());
        true
    }

    async fn load_kill_switches(&self) -> Option<Vec<KillSwitchState>> {
        if self.is_unavailable() {
            return None;
        }
        Some(self.switches.read().await.clone())
    }
}

/// Helper for stores: the textual scope key.
pub fn scope_key(scope: &KillScope) -> String {
    scope.as_string()
}
