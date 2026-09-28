//! Durable contract of the global ledger (TASK 5 §7) and its in-memory
//! implementation.
//!
//! The server implements [`LedgerStore`] over PostgreSQL
//! (`crates/server/src/accounting.rs`, migration 0015); [`MemoryLedgerStore`]
//! serves tests and no-database runs with identical semantics. Writes answer
//! `bool` / `Option<bool>` (success); reads answer `Option` (`None` = the
//! store could not answer, which is NOT the same as "nothing there").
//!
//! The guarded insert of [`LedgerStore::record_event`] is the durable half
//! of the ONE idempotency mechanism: `Some(false)` means the event id was
//! already booked (by this process, a previous life, or another replica).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::book::BookPosition;
use super::event::AccountingEvent;
use super::posting::Entry;
use super::reconcile::AccountingFinding;

/// An event as it sits in the journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Deterministic id.
    pub event_id: String,
    /// The event.
    pub event: AccountingEvent,
    /// When the ledger recorded it.
    pub recorded_at: DateTime<Utc>,
    /// Replica that recorded it.
    pub replica_id: String,
}

/// Journal contract (see module docs).
#[async_trait]
pub trait LedgerStore: Send + Sync {
    /// Guarded insert of one event with its balanced postings.
    /// `Some(true)` new, `Some(false)` already booked, `None` unavailable.
    async fn record_event(&self, stored: &StoredEvent, entry: &Entry) -> Option<bool>;
    /// Every journaled event, oldest first (`None` = unavailable).
    async fn load_events(&self) -> Option<Vec<StoredEvent>>;
    /// Upsert one aggregated-position snapshot (derived, informational).
    async fn upsert_position(&self, position: &BookPosition) -> bool;
    /// Append one accounting reconciliation finding.
    async fn append_finding(&self, finding: &AccountingFinding) -> bool;
    /// Recent findings, newest first (`None` = unavailable).
    async fn recent_findings(&self, limit: usize) -> Option<Vec<AccountingFinding>>;
}

/// In-memory journal (tests, no-DB runs). Same semantics as the Postgres
/// implementation; process lifetime only.
#[derive(Default)]
pub struct MemoryLedgerStore {
    events: RwLock<Vec<StoredEvent>>,
    index: RwLock<HashMap<String, usize>>,
    entries: RwLock<HashMap<String, Entry>>,
    positions: RwLock<HashMap<String, BookPosition>>,
    findings: RwLock<Vec<AccountingFinding>>,
    /// Test hook: when set, every call answers "unavailable" / `false`.
    unavailable: std::sync::atomic::AtomicBool,
}

impl MemoryLedgerStore {
    /// Empty store.
    pub fn new() -> Self {
        MemoryLedgerStore::default()
    }

    /// Simulate an unavailable backend (tests only — the production store
    /// reports real errors).
    pub fn set_unavailable(&self, on: bool) {
        self.unavailable
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    fn is_unavailable(&self) -> bool {
        self.unavailable.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Number of journaled events.
    pub async fn len(&self) -> usize {
        self.events.read().await.len()
    }

    /// True when nothing is journaled.
    pub async fn is_empty(&self) -> bool {
        self.events.read().await.is_empty()
    }

    /// Postings of one event.
    pub async fn entry(&self, event_id: &str) -> Option<Entry> {
        self.entries.read().await.get(event_id).cloned()
    }

    /// All postings, in journal order.
    pub async fn postings(&self) -> Vec<super::posting::Posting> {
        let events = self.events.read().await;
        let entries = self.entries.read().await;
        let mut out = Vec::new();
        for e in events.iter() {
            if let Some(entry) = entries.get(&e.event_id) {
                out.extend(entry.postings.iter().cloned());
            }
        }
        out
    }

    /// Position snapshots written so far.
    pub async fn positions(&self) -> Vec<BookPosition> {
        self.positions.read().await.values().cloned().collect()
    }

    /// All findings, oldest first.
    pub async fn findings(&self) -> Vec<AccountingFinding> {
        self.findings.read().await.clone()
    }
}

#[async_trait]
impl LedgerStore for MemoryLedgerStore {
    async fn record_event(&self, stored: &StoredEvent, entry: &Entry) -> Option<bool> {
        if self.is_unavailable() {
            return None;
        }
        let mut index = self.index.write().await;
        if index.contains_key(&stored.event_id) {
            return Some(false);
        }
        let mut events = self.events.write().await;
        index.insert(stored.event_id.clone(), events.len());
        events.push(stored.clone());
        self.entries
            .write()
            .await
            .insert(stored.event_id.clone(), entry.clone());
        Some(true)
    }

    async fn load_events(&self) -> Option<Vec<StoredEvent>> {
        if self.is_unavailable() {
            return None;
        }
        let mut out = self.events.read().await.clone();
        out.sort_by(|a, b| {
            a.event
                .ts
                .cmp(&b.event.ts)
                .then_with(|| a.recorded_at.cmp(&b.recorded_at))
                .then_with(|| a.event_id.cmp(&b.event_id))
        });
        Some(out)
    }

    async fn upsert_position(&self, position: &BookPosition) -> bool {
        if self.is_unavailable() {
            return false;
        }
        self.positions
            .write()
            .await
            .insert(position.key.as_string(), position.clone());
        true
    }

    async fn append_finding(&self, finding: &AccountingFinding) -> bool {
        if self.is_unavailable() {
            return false;
        }
        self.findings.write().await.push(finding.clone());
        true
    }

    async fn recent_findings(&self, limit: usize) -> Option<Vec<AccountingFinding>> {
        if self.is_unavailable() {
            return None;
        }
        let f = self.findings.read().await;
        Some(f.iter().rev().take(limit).cloned().collect())
    }
}
