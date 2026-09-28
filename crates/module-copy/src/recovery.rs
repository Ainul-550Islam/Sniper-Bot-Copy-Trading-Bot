//! Durable copy journal and restart recovery (TASK 3 §09).
//!
//! # The store
//!
//! [`CopyStore`] is the engine's durable memory: leaders and their lifecycle
//! events, every processed leader-trade event with its outcome, and the
//! leader ↔ follower position links reconciliation works from. The server
//! injects a Postgres-backed implementation (migration `0013_copy_trading`,
//! `bot_core::db::copy::CopyRepo`); without one the engine runs on
//! [`MemoryCopyStore`], which has identical semantics for the lifetime of the
//! process. Writes are best effort — a failed journal write is logged and
//! metered (`copy_journal_errors_total`) but never fails a trade; reads
//! return `None` when the backend is unavailable so recovery can degrade
//! explicitly instead of acting on an empty answer.
//!
//! # Restart recovery
//!
//! A crash can hit the engine at any of these points:
//!
//! | crash point | evidence after restart | action |
//! |---|---|---|
//! | before dedup claim | nothing | the feed redelivers; processed normally |
//! | after claim, before broadcast | journal row `REJECTED`/none, no ledger record | nothing to do (claim was in-memory) — redelivery is re-decided; with a durable dedup facade the event stays decided |
//! | after broadcast, before fill bookkeeping | ledger record `submitted`/`pending`, position may be missing | `HoldAmbiguous`: never resubmit; the ledger's own restart resolution + reconciliation own the signature |
//! | after fill, before link | position with `copied_wallet`, no link | `RestoreLink`: rebuild the link from the position |
//! | position closed, link still open | closed/missing position, open link | `CloseLink` |
//! | entry provably never landed | position whose entry signature is `failed`/`expired` in the ledger | `CleanupFailedEntry`: close the position without selling |
//!
//! Plus, always: `SeedDedup` (re-mark the dedup keys of journaled events so
//! an in-memory deployment does not re-mirror a replayed backlog) and
//! `SeedCursor` (raise each leader's ordering cursor to the newest journaled
//! slot). [`plan_recovery`] is pure; [`crate::CopyBot::recover_after_restart`]
//! gathers the inputs and applies the plan.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use bot_core::db::copy::{CopyEventRecord, CopyLinkRecord, LeaderEventRecord, LeaderRecord};
use bot_core::execution::{ExecutionRecord, ExecutionState};
use bot_core::models::{Position, PositionStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event_dedup::key_from_parts;

/// Durable memory of the copy engine (see the module docs).
#[async_trait]
pub trait CopyStore: Send + Sync {
    /// Upsert a leader row. `true` on success.
    async fn upsert_leader(&self, rec: LeaderRecord) -> bool;
    /// Append a leader lifecycle event. `true` on success.
    async fn append_leader_event(&self, rec: LeaderEventRecord) -> bool;
    /// Every leader row; `None` when the backend is unavailable.
    async fn load_leaders(&self) -> Option<Vec<LeaderRecord>>;
    /// Upsert a processed event. `true` on success.
    async fn record_event(&self, rec: CopyEventRecord) -> bool;
    /// Events observed at/after `since`, oldest first; `None` when unavailable.
    async fn events_since(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Option<Vec<CopyEventRecord>>;
    /// Upsert a link. `true` on success.
    async fn upsert_link(&self, rec: CopyLinkRecord) -> bool;
    /// Open links; `None` when unavailable.
    async fn open_links(&self) -> Option<Vec<CopyLinkRecord>>;
    /// Transition an open link to `closed` / `orphaned` / `mismatch`.
    /// `true` when a row changed.
    async fn close_link(
        &self,
        position_id: &str,
        status: &str,
        exit_event_id: Option<&str>,
        note: Option<&str>,
    ) -> bool;
}

/// In-process [`CopyStore`]: the default when no durable backend is
/// injected, and the reference semantics for the Postgres implementation.
#[derive(Debug, Default)]
pub struct MemoryCopyStore {
    inner: Mutex<MemoryInner>,
}

#[derive(Debug, Default)]
struct MemoryInner {
    leaders: HashMap<String, LeaderRecord>,
    leader_events: Vec<LeaderEventRecord>,
    events: HashMap<String, CopyEventRecord>,
    links: HashMap<String, CopyLinkRecord>,
}

/// Bound on journaled events kept in memory (oldest evicted first).
pub const MEMORY_EVENT_CAP: usize = 10_000;

impl MemoryCopyStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Leader lifecycle events, oldest first.
    pub fn leader_events(&self) -> Vec<LeaderEventRecord> {
        self.inner.lock().unwrap().leader_events.clone()
    }

    /// One journaled event.
    pub fn event(&self, event_id: &str) -> Option<CopyEventRecord> {
        self.inner.lock().unwrap().events.get(event_id).cloned()
    }

    /// Every journaled event, oldest first.
    pub fn events(&self) -> Vec<CopyEventRecord> {
        let mut v: Vec<CopyEventRecord> = self
            .inner
            .lock()
            .unwrap()
            .events
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| {
            a.observed_at
                .cmp(&b.observed_at)
                .then(a.event_id.cmp(&b.event_id))
        });
        v
    }

    /// One link.
    pub fn link(&self, position_id: &str) -> Option<CopyLinkRecord> {
        self.inner.lock().unwrap().links.get(position_id).cloned()
    }

    /// Every link (any status), oldest first.
    pub fn links(&self) -> Vec<CopyLinkRecord> {
        let mut v: Vec<CopyLinkRecord> =
            self.inner.lock().unwrap().links.values().cloned().collect();
        v.sort_by(|a, b| {
            a.opened_at
                .cmp(&b.opened_at)
                .then(a.position_id.cmp(&b.position_id))
        });
        v
    }
}

#[async_trait]
impl CopyStore for MemoryCopyStore {
    async fn upsert_leader(&self, rec: LeaderRecord) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.leaders.get_mut(&rec.address) {
            Some(cur) => {
                let followed_at = cur.followed_at.min(rec.followed_at);
                let events_seen = cur.events_seen.max(rec.events_seen);
                let mirrored = cur.mirrored.max(rec.mirrored);
                let rejected = cur.rejected.max(rec.rejected);
                let last_slot = match (cur.last_slot, rec.last_slot) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                let last_event_at = rec.last_event_at.or(cur.last_event_at);
                *cur = LeaderRecord {
                    followed_at,
                    events_seen,
                    mirrored,
                    rejected,
                    last_slot,
                    last_event_at,
                    ..rec
                };
            }
            None => {
                g.leaders.insert(rec.address.clone(), rec);
            }
        }
        true
    }

    async fn append_leader_event(&self, rec: LeaderEventRecord) -> bool {
        let mut g = self.inner.lock().unwrap();
        let id = g.leader_events.len() as i64 + 1;
        g.leader_events.push(LeaderEventRecord { id, ..rec });
        true
    }

    async fn load_leaders(&self) -> Option<Vec<LeaderRecord>> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<LeaderRecord> = g.leaders.values().cloned().collect();
        v.sort_by(|a, b| {
            a.followed_at
                .cmp(&b.followed_at)
                .then(a.address.cmp(&b.address))
        });
        Some(v)
    }

    async fn record_event(&self, rec: CopyEventRecord) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.events.get_mut(&rec.event_id) {
            Some(cur) => {
                let created_at = cur.created_at;
                let intent_id = rec.intent_id.clone().or_else(|| cur.intent_id.clone());
                let position_id = rec.position_id.clone().or_else(|| cur.position_id.clone());
                *cur = CopyEventRecord {
                    created_at,
                    intent_id,
                    position_id,
                    ..rec
                };
            }
            None => {
                if g.events.len() >= MEMORY_EVENT_CAP {
                    if let Some(oldest) = g
                        .events
                        .values()
                        .min_by(|a, b| a.observed_at.cmp(&b.observed_at))
                        .map(|e| e.event_id.clone())
                    {
                        g.events.remove(&oldest);
                    }
                }
                g.events.insert(rec.event_id.clone(), rec);
            }
        }
        true
    }

    async fn events_since(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Option<Vec<CopyEventRecord>> {
        let mut v: Vec<CopyEventRecord> = self
            .inner
            .lock()
            .unwrap()
            .events
            .values()
            .filter(|e| e.observed_at >= since)
            .cloned()
            .collect();
        v.sort_by(|a, b| {
            a.observed_at
                .cmp(&b.observed_at)
                .then(a.event_id.cmp(&b.event_id))
        });
        v.truncate(limit.max(1));
        Some(v)
    }

    async fn upsert_link(&self, rec: CopyLinkRecord) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.links.get_mut(&rec.position_id) {
            Some(cur) => {
                let opened_at = cur.opened_at;
                let intent_id = rec.intent_id.clone().or_else(|| cur.intent_id.clone());
                let exit_event_id = rec
                    .exit_event_id
                    .clone()
                    .or_else(|| cur.exit_event_id.clone());
                *cur = CopyLinkRecord {
                    opened_at,
                    intent_id,
                    exit_event_id,
                    ..rec
                };
            }
            None => {
                g.links.insert(rec.position_id.clone(), rec);
            }
        }
        true
    }

    async fn open_links(&self) -> Option<Vec<CopyLinkRecord>> {
        let mut v: Vec<CopyLinkRecord> = self
            .inner
            .lock()
            .unwrap()
            .links
            .values()
            .filter(|l| l.status == "open")
            .cloned()
            .collect();
        v.sort_by(|a, b| {
            a.opened_at
                .cmp(&b.opened_at)
                .then(a.position_id.cmp(&b.position_id))
        });
        Some(v)
    }

    async fn close_link(
        &self,
        position_id: &str,
        status: &str,
        exit_event_id: Option<&str>,
        note: Option<&str>,
    ) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.links.get_mut(position_id) {
            Some(l) if l.status == "open" => {
                l.status = status.to_string();
                if exit_event_id.is_some() {
                    l.exit_event_id = exit_event_id.map(|s| s.to_string());
                }
                if note.is_some() {
                    l.note = note.map(|s| s.to_string());
                }
                l.closed_at = Some(Utc::now());
                l.updated_at = Utc::now();
                true
            }
            _ => false,
        }
    }
}

/// Build the link row for a freshly mirrored position.
#[allow(clippy::too_many_arguments)]
pub fn link_for(
    position_id: &str,
    leader: &str,
    mint: &str,
    entry_event_id: &str,
    entry_signature: &str,
    intent_id: Option<&str>,
    leader_token_amount: f64,
    follower_qty: f64,
) -> CopyLinkRecord {
    let now = Utc::now();
    CopyLinkRecord {
        position_id: position_id.to_string(),
        leader: leader.to_string(),
        mint: mint.to_string(),
        entry_event_id: entry_event_id.to_string(),
        entry_signature: entry_signature.to_string(),
        intent_id: intent_id.map(|s| s.to_string()),
        leader_token_amount,
        follower_qty,
        status: "open".into(),
        opened_at: now,
        closed_at: None,
        exit_event_id: None,
        last_reconciled_at: None,
        note: None,
        updated_at: now,
    }
}

// ---------------------------------------------------------------------------
// Restart recovery plan
// ---------------------------------------------------------------------------

/// What the ledger says about a mirrored position's entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryFate {
    /// Landed (confirmed / reconciled) or no ledger record to consult.
    Held,
    /// Still ambiguous: submitted / pending / not yet settled.
    Unknown,
    /// Provably never landed: failed / expired.
    Failed,
}

/// Classify an entry's ledger state. Same rule as the sniper's sweeper.
pub fn entry_fate(state: Option<ExecutionState>) -> EntryFate {
    match state {
        None => EntryFate::Held,
        Some(ExecutionState::Confirmed) | Some(ExecutionState::Reconciled) => EntryFate::Held,
        Some(ExecutionState::Failed) | Some(ExecutionState::Expired) => EntryFate::Failed,
        Some(_) => EntryFate::Unknown,
    }
}

/// One recovery action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Re-mark journaled dedup keys.
    SeedDedup {
        /// Keys to mark.
        keys: Vec<String>,
    },
    /// Raise a leader's ordering cursor.
    SeedCursor {
        /// Leader address.
        leader: String,
        /// Newest journaled slot.
        slot: u64,
        /// Signature at that slot.
        signature: String,
    },
    /// A mirrored entry is still live in the ledger: keep the position, do
    /// not resubmit, let the ledger / reconciliation resolve it.
    HoldAmbiguous {
        /// Ledger intent id.
        intent_id: String,
        /// Position booked for it, when one exists.
        position_id: Option<String>,
        /// Mint.
        mint: String,
    },
    /// A position whose entry provably never landed: close it without
    /// selling.
    CleanupFailedEntry {
        /// Position id.
        position_id: String,
        /// Entry signature the ledger reports failed / expired.
        signature: String,
    },
    /// A copy position without a link: rebuild it from the position.
    RestoreLink {
        /// Link to upsert.
        link: Box<CopyLinkRecord>,
    },
    /// An open link whose position is closed or gone.
    CloseLink {
        /// Position id (link key).
        position_id: String,
        /// `closed` when the position closed normally, else `orphaned`.
        status: String,
        /// Why.
        note: String,
    },
}

impl RecoveryAction {
    /// Metric / audit label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RecoveryAction::SeedDedup { .. } => "seed_dedup",
            RecoveryAction::SeedCursor { .. } => "seed_cursor",
            RecoveryAction::HoldAmbiguous { .. } => "hold_ambiguous",
            RecoveryAction::CleanupFailedEntry { .. } => "cleanup_failed_entry",
            RecoveryAction::RestoreLink { .. } => "restore_link",
            RecoveryAction::CloseLink { .. } => "close_link",
        }
    }
}

/// Inputs to [`plan_recovery`].
#[derive(Debug, Clone, Default)]
pub struct RecoveryInputs {
    /// Journaled events inside the lookback window.
    pub journal: Vec<CopyEventRecord>,
    /// Open links from the store.
    pub links: Vec<CopyLinkRecord>,
    /// Copy positions (open and closed) from the position book.
    pub positions: Vec<Position>,
    /// Execution-ledger records for module `copy`.
    pub ledger: Vec<ExecutionRecord>,
}

/// The plan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecoveryPlan {
    /// Actions in application order.
    pub actions: Vec<RecoveryAction>,
}

impl RecoveryPlan {
    /// Count of actions of one kind.
    pub fn count(&self, label: &str) -> usize {
        self.actions.iter().filter(|a| a.as_str() == label).count()
    }

    /// Whether nothing needs doing.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// Pure planner. Deterministic for the same inputs.
pub fn plan_recovery(inputs: &RecoveryInputs) -> RecoveryPlan {
    let mut actions = Vec::new();

    // 1. Dedup keys of everything journaled.
    let keys: Vec<String> = inputs
        .journal
        .iter()
        .map(|e| key_from_parts(&e.signature, &e.leader, &e.mint, &e.side))
        .collect();
    if !keys.is_empty() {
        actions.push(RecoveryAction::SeedDedup { keys });
    }

    // 2. Ordering cursors: newest journaled slot per leader.
    let mut newest: HashMap<&str, (u64, &str)> = HashMap::new();
    for e in &inputs.journal {
        if e.slot == 0 {
            continue;
        }
        let entry = newest.entry(e.leader.as_str()).or_insert((0, ""));
        if e.slot > entry.0 {
            *entry = (e.slot, e.signature.as_str());
        }
    }
    let mut leaders: Vec<&str> = newest.keys().copied().collect();
    leaders.sort_unstable();
    for leader in leaders {
        let (slot, signature) = newest[leader];
        actions.push(RecoveryAction::SeedCursor {
            leader: leader.to_string(),
            slot,
            signature: signature.to_string(),
        });
    }

    // 3. Ledger: live copy entries → hold; failed entries → cleanup.
    let by_signature: HashMap<&str, &ExecutionRecord> = inputs
        .ledger
        .iter()
        .filter_map(|r| r.signature.as_deref().map(|s| (s, r)))
        .collect();
    let open_positions: Vec<&Position> = inputs
        .positions
        .iter()
        .filter(|p| p.status == PositionStatus::Open || p.status == PositionStatus::Closing)
        .collect();
    for rec in inputs
        .ledger
        .iter()
        .filter(|r| r.module == "copy" && crate::intent::is_entry_label(&r.label))
        .filter(|r| r.state.is_live())
    {
        let position_id = open_positions
            .iter()
            .find(|p| {
                rec.signature.is_some() && p.entry_signature.as_deref() == rec.signature.as_deref()
            })
            .map(|p| p.id.clone());
        actions.push(RecoveryAction::HoldAmbiguous {
            intent_id: rec.intent_id.clone(),
            position_id,
            mint: rec.symbol.clone(),
        });
    }
    for p in &open_positions {
        let Some(sig) = p.entry_signature.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let fate = entry_fate(by_signature.get(sig).map(|r| r.state));
        if fate == EntryFate::Failed {
            actions.push(RecoveryAction::CleanupFailedEntry {
                position_id: p.id.clone(),
                signature: sig.to_string(),
            });
        }
    }

    // 4. Links vs positions.
    let linked: std::collections::HashSet<&str> = inputs
        .links
        .iter()
        .map(|l| l.position_id.as_str())
        .collect();
    for p in &open_positions {
        if linked.contains(p.id.as_str()) {
            continue;
        }
        let Some(leader) = p.copied_wallet.as_deref().filter(|w| !w.is_empty()) else {
            continue;
        };
        let entry_signature = p.entry_signature.clone().unwrap_or_default();
        let entry_event_id = inputs
            .journal
            .iter()
            .find(|e| e.position_id.as_deref() == Some(p.id.as_str()))
            .map(|e| e.event_id.clone())
            .unwrap_or_else(|| format!("recovered:{}", p.id));
        let mut link = link_for(
            &p.id,
            leader,
            &p.symbol,
            &entry_event_id,
            &entry_signature,
            None,
            0.0,
            p.qty,
        );
        link.opened_at = p.opened_at;
        link.note = Some("restored from position book after restart".into());
        actions.push(RecoveryAction::RestoreLink {
            link: Box::new(link),
        });
    }
    for l in &inputs.links {
        if l.status != "open" {
            continue;
        }
        match inputs.positions.iter().find(|p| p.id == l.position_id) {
            Some(p) if p.status == PositionStatus::Open || p.status == PositionStatus::Closing => {}
            Some(p) => actions.push(RecoveryAction::CloseLink {
                position_id: l.position_id.clone(),
                status: "closed".into(),
                note: format!("position {} is {:?}", p.id, p.status),
            }),
            None => actions.push(RecoveryAction::CloseLink {
                position_id: l.position_id.clone(),
                status: "orphaned".into(),
                note: "position not found in the book".into(),
            }),
        }
    }

    RecoveryPlan { actions }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::{ExecutionMode, TradeSource, Venue};

    fn position(id: &str, mint: &str, sig: Option<&str>, leader: Option<&str>) -> Position {
        let mut p = Position::new(
            id.to_string(),
            TradeSource::Copy,
            Venue::PumpFun,
            ExecutionMode::Paper,
            mint.to_string(),
            mint.to_string(),
            "SOL".into(),
        );
        p.apply_buy(100.0, 0.001, 0.1);
        p.entry_signature = sig.map(|s| s.to_string());
        p.copied_wallet = leader.map(|s| s.to_string());
        p
    }

    fn journal(
        id: &str,
        leader: &str,
        sig: &str,
        mint: &str,
        slot: u64,
        pos: Option<&str>,
    ) -> CopyEventRecord {
        let now = Utc::now();
        CopyEventRecord {
            event_id: id.into(),
            leader: leader.into(),
            signature: sig.into(),
            slot,
            mint: mint.into(),
            side: "buy".into(),
            venue: "pump.fun".into(),
            token_amount: 1.0,
            sol_amount: 0.1,
            source: "pumpportal".into(),
            source_sequence: 0,
            event_at: None,
            observed_at: now,
            stage: "FILLED".into(),
            reject_reason: None,
            detail: None,
            intent_id: None,
            position_id: pos.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
        }
    }

    fn ledger(
        intent: &str,
        label: &str,
        sig: &str,
        mint: &str,
        state: ExecutionState,
    ) -> ExecutionRecord {
        let json = serde_json::json!({
            "intent_id": intent,
            "module": "copy",
            "label": label,
            "wallet": "w",
            "symbol": mint,
            "state": state.as_str(),
            "attempts": 1,
            "signature": sig,
            "blockhash": null,
            "last_valid_block_height": null,
            "priority_fee_micro_lamports": 0,
            "failure": null,
            "error": null,
            "created_at": Utc::now(),
            "updated_at": Utc::now(),
        });
        serde_json::from_value(json).expect("execution record json")
    }

    #[test]
    fn empty_inputs_make_an_empty_plan() {
        assert!(plan_recovery(&RecoveryInputs::default()).is_empty());
    }

    #[test]
    fn journal_seeds_dedup_and_cursors() {
        let inputs = RecoveryInputs {
            journal: vec![
                journal("e1", "A", "s1", "m1", 10, None),
                journal("e2", "A", "s2", "m2", 12, None),
                journal("e3", "B", "s3", "m3", 0, None),
            ],
            ..Default::default()
        };
        let plan = plan_recovery(&inputs);
        assert_eq!(plan.count("seed_dedup"), 1);
        match &plan.actions[0] {
            RecoveryAction::SeedDedup { keys } => {
                assert_eq!(keys.len(), 3);
                assert_eq!(keys[0], "copy:s1:A:m1:buy");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(plan.count("seed_cursor"), 1, "slot 0 never seeds a cursor");
        assert!(plan.actions.contains(&RecoveryAction::SeedCursor {
            leader: "A".into(),
            slot: 12,
            signature: "s2".into()
        }));
    }

    #[test]
    fn live_ledger_entries_are_held_and_failed_ones_cleaned() {
        let inputs = RecoveryInputs {
            positions: vec![
                position("p-live", "m1", Some("sig-live"), Some("A")),
                position("p-failed", "m2", Some("sig-failed"), Some("A")),
                position("p-ok", "m3", Some("sig-ok"), Some("A")),
            ],
            ledger: vec![
                ledger("i1", "copy-m1", "sig-live", "m1", ExecutionState::Submitted),
                ledger("i2", "copy-m2", "sig-failed", "m2", ExecutionState::Failed),
                ledger(
                    "i3",
                    "copy-jup-m3",
                    "sig-ok",
                    "m3",
                    ExecutionState::Confirmed,
                ),
                ledger(
                    "i4",
                    "copy-exit-p",
                    "sig-exit",
                    "m3",
                    ExecutionState::Pending,
                ),
                ledger("i5", "snipe-x", "sig-snipe", "m9", ExecutionState::Pending),
            ],
            ..Default::default()
        };
        let plan = plan_recovery(&inputs);
        assert_eq!(
            plan.count("hold_ambiguous"),
            1,
            "exits and sniper intents are not copy entries"
        );
        assert!(plan.actions.contains(&RecoveryAction::HoldAmbiguous {
            intent_id: "i1".into(),
            position_id: Some("p-live".into()),
            mint: "m1".into()
        }));
        assert_eq!(plan.count("cleanup_failed_entry"), 1);
        assert!(plan.actions.contains(&RecoveryAction::CleanupFailedEntry {
            position_id: "p-failed".into(),
            signature: "sig-failed".into()
        }));
        // All three positions lack links → restored (including the held one).
        assert_eq!(plan.count("restore_link"), 3);
    }

    #[test]
    fn links_are_restored_and_closed_against_the_book() {
        let mut closed = position("p-closed", "m2", Some("s2"), Some("A"));
        closed.status = PositionStatus::Closed;
        let inputs = RecoveryInputs {
            journal: vec![journal("e1", "A", "s1", "m1", 5, Some("p-open"))],
            links: vec![
                link_for("p-closed", "A", "m2", "e2", "s2", None, 0.0, 1.0),
                link_for("p-gone", "A", "m3", "e3", "s3", None, 0.0, 1.0),
                link_for("p-linked", "A", "m4", "e4", "s4", None, 0.0, 1.0),
            ],
            positions: vec![
                position("p-open", "m1", Some("s1"), Some("A")),
                closed,
                position("p-linked", "m4", Some("s4"), Some("A")),
                position("p-manual", "m5", Some("s5"), None),
            ],
            ..Default::default()
        };
        let plan = plan_recovery(&inputs);
        let restored: Vec<&CopyLinkRecord> = plan
            .actions
            .iter()
            .filter_map(|a| match a {
                RecoveryAction::RestoreLink { link } => Some(link.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(
            restored.len(),
            1,
            "linked and non-copied positions are skipped"
        );
        assert_eq!(restored[0].position_id, "p-open");
        assert_eq!(
            restored[0].entry_event_id, "e1",
            "journal supplies the event id"
        );
        assert_eq!(restored[0].leader, "A");
        assert_eq!(plan.count("close_link"), 2);
        assert!(plan.actions.iter().any(|a| matches!(a, RecoveryAction::CloseLink { position_id, status, .. } if position_id == "p-closed" && status == "closed")));
        assert!(plan.actions.iter().any(|a| matches!(a, RecoveryAction::CloseLink { position_id, status, .. } if position_id == "p-gone" && status == "orphaned")));
    }

    #[tokio::test]
    async fn memory_store_semantics_match_the_repo_contract() {
        let store = MemoryCopyStore::new();
        let now = Utc::now();
        let mut rec = LeaderRecord {
            address: "A".into(),
            label: "a".into(),
            status: "active".into(),
            source: "config".into(),
            followed_at: now,
            status_since: now,
            events_seen: 5,
            mirrored: 1,
            rejected: 0,
            last_event_at: Some(now),
            last_slot: Some(10),
            updated_at: now,
        };
        assert!(store.upsert_leader(rec.clone()).await);
        rec.events_seen = 2;
        rec.last_slot = Some(8);
        rec.status = "paused".into();
        rec.followed_at = now - chrono::Duration::days(1);
        assert!(store.upsert_leader(rec).await);
        let loaded = store.load_leaders().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].events_seen, 5, "counters only grow");
        assert_eq!(loaded[0].last_slot, Some(10));
        assert_eq!(loaded[0].status, "paused", "status follows the record");
        assert_eq!(loaded[0].followed_at, now - chrono::Duration::days(1));

        let mut e = journal("e1", "A", "s1", "m1", 1, None);
        e.stage = "REJECTED".into();
        assert!(store.record_event(e.clone()).await);
        e.stage = "FILLED".into();
        e.position_id = Some("p1".into());
        assert!(store.record_event(e.clone()).await);
        let got = store.event("e1").unwrap();
        assert_eq!(got.stage, "FILLED");
        assert_eq!(got.position_id.as_deref(), Some("p1"));
        assert_eq!(
            store
                .events_since(now - chrono::Duration::hours(1), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .events_since(now + chrono::Duration::hours(1), 10)
            .await
            .unwrap()
            .is_empty());

        let link = link_for("p1", "A", "m1", "e1", "s1", Some("int_1"), 100.0, 5.0);
        assert!(store.upsert_link(link).await);
        assert_eq!(store.open_links().await.unwrap().len(), 1);
        assert!(
            store
                .close_link("p1", "closed", Some("e9"), Some("leader exited"))
                .await
        );
        assert!(
            !store.close_link("p1", "closed", None, None).await,
            "only open links transition"
        );
        assert!(store.open_links().await.unwrap().is_empty());
        let l = store.link("p1").unwrap();
        assert_eq!(l.status, "closed");
        assert_eq!(l.exit_event_id.as_deref(), Some("e9"));
        assert!(l.closed_at.is_some());

        let t = LeaderEventRecord {
            id: 0,
            address: "A".into(),
            event: "paused".into(),
            reason: None,
            replica_id: "r".into(),
            ts: now,
        };
        assert!(store.append_leader_event(t).await);
        assert_eq!(store.leader_events()[0].id, 1);
    }

    #[test]
    fn entry_fate_classification() {
        assert_eq!(entry_fate(None), EntryFate::Held);
        assert_eq!(entry_fate(Some(ExecutionState::Confirmed)), EntryFate::Held);
        assert_eq!(
            entry_fate(Some(ExecutionState::Reconciled)),
            EntryFate::Held
        );
        assert_eq!(entry_fate(Some(ExecutionState::Failed)), EntryFate::Failed);
        assert_eq!(entry_fate(Some(ExecutionState::Expired)), EntryFate::Failed);
        assert_eq!(
            entry_fate(Some(ExecutionState::Submitted)),
            EntryFate::Unknown
        );
        assert_eq!(
            entry_fate(Some(ExecutionState::Created)),
            EntryFate::Unknown
        );
    }
}
