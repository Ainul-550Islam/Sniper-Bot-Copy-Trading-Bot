//! The global ledger (TASK 5 §2, §4): append-only, double-entry, idempotent.
//!
//! [`GlobalLedger::submit`] is the single mutation door. Under the ledger
//! write lock it (1) refuses malformed or locally duplicated events,
//! (2) expands balanced postings, (3) performs the guarded durable insert
//! before mutating memory, and (4) applies the event to the book and in-memory
//! index. Thus a durable duplicate changes nothing in this process. If the
//! journal is unavailable, the event is applied in memory and parked as
//! *pending*; later submissions cannot overtake pending writes in the journal.
//! Pending writes are retried by [`GlobalLedger::flush_pending`] and reported
//! by reconciliation as `unresolved_financial_event`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::audit;
use super::book::{BookEffect, BookPosition, PositionBook};
use super::event::{AccountingEvent, EventKind};
use super::metrics;
use super::posting::{expand, Entry};
use super::reconcile::{self, AccountingFinding, ReconInputs};
use super::store::{LedgerStore, MemoryLedgerStore, StoredEvent};
use super::view::{PortfolioInputs, PortfolioView};
use crate::events::EventBus;
use crate::models::{Position, Trade};
use crate::oms::Order;
use crate::reconciliation::QuantityTolerance;

/// Outcome of one submission.
#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    /// First sighting: booked, journaled (or parked pending), audited.
    New(BookEffect),
    /// The same fact was already booked — nothing changed.
    Duplicate,
    /// Structurally invalid — nothing changed; the reason names the field.
    Rejected(String),
}

impl Applied {
    /// True for [`Applied::New`].
    pub fn is_new(&self) -> bool {
        matches!(self, Applied::New(_))
    }
}

/// Realized PnL per UTC day per quote asset, derived from the event stream
/// (the durable input of the daily-loss and drawdown checks).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RealizedSeries {
    /// `(day "YYYY-MM-DD", quote_asset) → net realized (realized − fees)`.
    pub by_day: HashMap<(String, String), f64>,
    /// Cumulative net realized per quote asset over the whole stream.
    pub total: HashMap<String, f64>,
    /// Peak of the running cumulative net realized per quote asset.
    pub peak: HashMap<String, f64>,
}

struct Inner {
    book: PositionBook,
    events: Vec<StoredEvent>,
    ids: HashSet<String>,
    /// Canonical payload for each id, so a replayed/corrupted row with the
    /// same id but different economics cannot be silently treated as a dup.
    canonical_events: HashMap<String, AccountingEvent>,
    entries: HashMap<String, Entry>,
    /// Event ids applied in memory whose durable write has not succeeded.
    pending: Vec<String>,
    series: RealizedSeries,
    /// Finding ids already journaled by this process life (a persisting
    /// discrepancy is journaled once, not on every run).
    seen_findings: HashSet<String>,
    /// Last journal timestamp used to preserve a strict local append order.
    last_recorded_at: Option<DateTime<Utc>>,
}

/// Result of one reconciliation run.
#[derive(Debug, Clone, Default)]
pub struct ReconRun {
    /// Every finding of this run (deterministic order).
    pub findings: Vec<AccountingFinding>,
    /// The subset first seen in this process life (journaled + audited).
    pub new: Vec<AccountingFinding>,
}

/// The process-wide ledger. Constructed once by `AppState`; modules reach it
/// through `state.ledger()`.
pub struct GlobalLedger {
    inner: RwLock<Inner>,
    store: RwLock<Arc<dyn LedgerStore>>,
    pub(crate) bus: EventBus,
    replica_id: String,
}

impl GlobalLedger {
    /// A ledger over the in-memory store (the server attaches the durable
    /// store before recovery with [`GlobalLedger::attach_store`]).
    pub fn new(bus: EventBus, replica_id: impl Into<String>) -> Self {
        GlobalLedger {
            inner: RwLock::new(Inner {
                book: PositionBook::new(),
                events: Vec::new(),
                ids: HashSet::new(),
                canonical_events: HashMap::new(),
                entries: HashMap::new(),
                pending: Vec::new(),
                series: RealizedSeries::default(),
                seen_findings: HashSet::new(),
                last_recorded_at: None,
            }),
            store: RwLock::new(Arc::new(MemoryLedgerStore::new())),
            bus,
            replica_id: replica_id.into(),
        }
    }

    /// Replace the journal (durable store attached at startup, BEFORE
    /// recovery and before any module runs).
    pub async fn attach_store(&self, store: Arc<dyn LedgerStore>) {
        *self.store.write().await = store;
    }

    /// The journal in use.
    pub async fn store(&self) -> Arc<dyn LedgerStore> {
        self.store.read().await.clone()
    }

    /// This replica's id (stamped on every journal row).
    pub fn replica_id(&self) -> &str {
        &self.replica_id
    }

    /// Submit one financial event (see module docs).
    pub async fn submit(&self, event: AccountingEvent) -> Applied {
        if let Err(reason) = event.validate() {
            metrics::count_ledger_event(event.kind.as_str(), "rejected");
            warn!(reason = %reason, event = %event.summary(), "ledger rejected a malformed event");
            return Applied::Rejected(reason);
        }
        let event_id = event.event_id();
        let store = self.store().await;
        let mut inner = self.inner.write().await;
        if inner.ids.contains(&event_id) {
            metrics::count_ledger_event(event.kind.as_str(), "duplicate");
            metrics::count_duplicate_prevented("global_ledger");
            debug!(event = %event_id, "ledger duplicate — not booked twice");
            return Applied::Duplicate;
        }
        let cost_of_slice = inner.book.cost_of_slice(&event);
        let entry = match expand(&event, cost_of_slice) {
            Ok(e) => e,
            Err(e) => {
                metrics::count_ledger_event(event.kind.as_str(), "rejected");
                return Applied::Rejected(e.to_string());
            }
        };
        if let Err(reason) = inner.book.validate_apply(&event, &event_id) {
            metrics::count_ledger_event(event.kind.as_str(), "rejected");
            return Applied::Rejected(reason);
        }
        if let Err(reason) = Self::validate_series_update(&inner.series, &event, cost_of_slice) {
            metrics::count_ledger_event(event.kind.as_str(), "rejected");
            return Applied::Rejected(reason);
        }
        let now = Utc::now();
        let recorded_at = inner
            .last_recorded_at
            .as_ref()
            .and_then(|last| last.checked_add_signed(chrono::Duration::microseconds(1)))
            .map(|floor| now.max(floor))
            .unwrap_or(now);
        let stored = StoredEvent {
            event_id: event_id.clone(),
            event: event.clone(),
            recorded_at,
            replica_id: self.replica_id.clone(),
        };

        // Preserve journal order. If an earlier event is still pending, flush
        // it first; while the journal remains unavailable, later events also
        // stay pending rather than being durably written ahead of it.
        if !inner.pending.is_empty() {
            Self::flush_pending_locked(&mut inner, &store).await;
        }
        let pending_blocked = !inner.pending.is_empty();
        let journaled = if pending_blocked {
            None
        } else {
            store.record_event(&stored, &entry).await
        };
        if journaled == Some(false) {
            // A previous process life / replica already journaled this fact.
            // Do not mutate the in-memory book or realized series.
            metrics::count_ledger_event(event.kind.as_str(), "duplicate");
            metrics::count_duplicate_prevented("global_ledger_journal");
            warn!(event = %event_id, "event already in the durable journal — treated as a duplicate");
            return Applied::Duplicate;
        }

        let effect = inner.book.apply(&event, &event_id);
        Self::fold_series(&mut inner.series, &event, &effect);
        inner.ids.insert(event_id.clone());
        inner
            .canonical_events
            .insert(event_id.clone(), event.clone());
        inner.last_recorded_at = Some(stored.recorded_at);
        inner.events.push(stored);
        inner.entries.insert(event_id.clone(), entry);
        if journaled.is_none() {
            if !pending_blocked {
                metrics::count_journal_error("record_event");
            }
            inner.pending.push(event_id.clone());
            warn!(event = %event_id, "ledger journal unavailable — event parked as pending");
        }
        drop(inner);
        metrics::count_ledger_event(event.kind.as_str(), "new");
        if effect.transition != "cash" {
            if let Some(p) = self.inner.read().await.book.get(&effect.key) {
                if !store.upsert_position(p).await {
                    metrics::count_journal_error("upsert_position");
                }
            }
        }
        audit::ledger_mutation(&self.bus, &event, &event_id, &effect);
        Applied::New(effect)
    }

    fn fold_series(series: &mut RealizedSeries, event: &AccountingEvent, effect: &BookEffect) {
        let fee = if event.kind == EventKind::Fee && event.fee <= 0.0 {
            event.quote_amount
        } else {
            event.fee
        };
        let net = effect.realized_delta - fee;
        if net == 0.0 {
            return;
        }
        let day = event.ts.format("%Y-%m-%d").to_string();
        *series
            .by_day
            .entry((day, event.quote_asset.clone()))
            .or_insert(0.0) += net;
        let total = series.total.entry(event.quote_asset.clone()).or_insert(0.0);
        *total += net;
        let peak = series.peak.entry(event.quote_asset.clone()).or_insert(0.0);
        if *total > *peak {
            *peak = *total;
        }
    }

    fn validate_series_update(
        series: &RealizedSeries,
        event: &AccountingEvent,
        cost_of_slice: f64,
    ) -> Result<(), String> {
        let realized_delta =
            if event.kind.moves_inventory() && event.side == Some(super::event::EventSide::Sell) {
                event.gross_quote() - cost_of_slice
            } else {
                0.0
            };
        let fee = if event.kind == EventKind::Fee && event.fee <= 0.0 {
            event.quote_amount
        } else {
            event.fee
        };
        let net = realized_delta - fee;
        if !realized_delta.is_finite() || !net.is_finite() {
            return Err("event arithmetic would make realized PnL non-finite".into());
        }
        if net == 0.0 {
            return Ok(());
        }
        let day = event.ts.format("%Y-%m-%d").to_string();
        let day_key = (day, event.quote_asset.clone());
        let day_total = series.by_day.get(&day_key).copied().unwrap_or(0.0) + net;
        let total = series.total.get(&event.quote_asset).copied().unwrap_or(0.0) + net;
        let peak = series
            .peak
            .get(&event.quote_asset)
            .copied()
            .unwrap_or(0.0)
            .max(total);
        if day_total.is_finite() && total.is_finite() && peak.is_finite() {
            Ok(())
        } else {
            Err("event arithmetic would make realized series non-finite".into())
        }
    }

    /// Retry the durable write of every pending event. Returns how many
    /// were flushed. The ledger write lock keeps later submissions from
    /// overtaking these events in the durable journal.
    pub async fn flush_pending(&self) -> usize {
        let store = self.store().await;
        let mut inner = self.inner.write().await;
        Self::flush_pending_locked(&mut inner, &store).await
    }

    async fn flush_pending_locked(inner: &mut Inner, store: &Arc<dyn LedgerStore>) -> usize {
        let pending: Vec<(StoredEvent, Entry)> = inner
            .pending
            .iter()
            .filter_map(|id| {
                let event = inner.events.iter().find(|event| &event.event_id == id)?;
                let entry = inner.entries.get(id)?;
                Some((event.clone(), entry.clone()))
            })
            .collect();
        let mut flushed = Vec::new();
        for (event, entry) in pending {
            match store.record_event(&event, &entry).await {
                Some(_) => flushed.push(event.event_id),
                None => {
                    metrics::count_journal_error("record_event");
                    break;
                }
            }
        }
        if !flushed.is_empty() {
            inner.pending.retain(|id| !flushed.contains(id));
        }
        flushed.len()
    }

    /// Load `events` (from the durable journal) into the book without
    /// re-journaling them. Returns `(applied, duplicates_skipped)`.
    pub(crate) async fn hydrate(&self, events: Vec<StoredEvent>) -> (usize, usize, usize) {
        let mut inner = self.inner.write().await;
        let mut applied = 0usize;
        let mut dup = 0usize;
        let mut invalid = 0usize;
        for stored in events {
            inner.last_recorded_at = Some(
                inner
                    .last_recorded_at
                    .map(|last| last.max(stored.recorded_at))
                    .unwrap_or(stored.recorded_at),
            );
            if inner.ids.contains(&stored.event_id) {
                if inner.canonical_events.get(&stored.event_id) == Some(&stored.event) {
                    dup += 1;
                } else {
                    invalid += 1;
                }
                continue;
            }
            if stored.event_id != stored.event.event_id() || stored.event.validate().is_err() {
                invalid += 1;
                continue;
            }
            let cost = inner.book.cost_of_slice(&stored.event);
            let entry = match expand(&stored.event, cost) {
                Ok(e) => e,
                Err(_) => {
                    invalid += 1;
                    continue;
                }
            };
            if inner
                .book
                .validate_apply(&stored.event, &stored.event_id)
                .is_err()
                || Self::validate_series_update(&inner.series, &stored.event, cost).is_err()
            {
                invalid += 1;
                continue;
            }
            let effect = inner.book.apply(&stored.event, &stored.event_id);
            Self::fold_series(&mut inner.series, &stored.event, &effect);
            inner.ids.insert(stored.event_id.clone());
            inner
                .canonical_events
                .insert(stored.event_id.clone(), stored.event.clone());
            inner.entries.insert(stored.event_id.clone(), entry);
            inner.events.push(stored);
            applied += 1;
        }
        (applied, dup, invalid)
    }

    /// Number of applied events that are still missing from the durable journal.
    pub async fn pending_count(&self) -> usize {
        self.inner.read().await.pending.len()
    }

    /// Merge all events currently visible in the shared journal into this
    /// replica's book. Returns false if the journal cannot answer, local
    /// writes are pending, or any durable row fails replay validation.
    pub async fn refresh_from_store(&self) -> bool {
        if self.pending_count().await > 0 {
            return false;
        }
        let store = self.store().await;
        let Some(events) = store.load_events().await else {
            return false;
        };
        let (_, _, invalid) = self.hydrate(events).await;
        invalid == 0
    }

    /// Snapshot of the aggregated book.
    pub async fn book(&self) -> PositionBook {
        self.inner.read().await.book.clone()
    }

    /// Open aggregated positions.
    pub async fn open_positions(&self) -> Vec<BookPosition> {
        self.inner
            .read()
            .await
            .book
            .open_positions()
            .cloned()
            .collect()
    }

    /// Every event applied so far, in application order.
    pub async fn events(&self) -> Vec<StoredEvent> {
        self.inner.read().await.events.clone()
    }

    /// Postings of one event.
    pub async fn entry(&self, event_id: &str) -> Option<Entry> {
        self.inner.read().await.entries.get(event_id).cloned()
    }

    /// Event ids applied but not durably journaled.
    pub async fn pending(&self) -> Vec<String> {
        self.inner.read().await.pending.clone()
    }

    /// True when the id is known to this ledger.
    pub async fn contains(&self, event_id: &str) -> bool {
        self.inner.read().await.ids.contains(event_id)
    }

    /// Number of applied events.
    pub async fn len(&self) -> usize {
        self.inner.read().await.events.len()
    }

    /// True when nothing has been applied.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.events.is_empty()
    }

    /// Events by kind (for API / diagnostics).
    pub async fn count_by_kind(&self) -> HashMap<EventKind, usize> {
        let inner = self.inner.read().await;
        let mut m = HashMap::new();
        for e in &inner.events {
            *m.entry(e.event.kind).or_insert(0) += 1;
        }
        m
    }

    /// Realized series (daily / cumulative / peak per quote asset).
    pub async fn realized_series(&self) -> RealizedSeries {
        self.inner.read().await.series.clone()
    }

    /// The aggregated portfolio view (exposure, PnL, utilization).
    pub async fn portfolio(&self, inputs: &PortfolioInputs) -> PortfolioView {
        let inner = self.inner.read().await;
        PortfolioView::compute(&inner.book, &inner.series, inputs)
    }

    /// Run accounting reconciliation across the four record layers — OMS
    /// orders, module trades (fills), the ledger and the positions (see
    /// [`super::reconcile`]). New findings are journaled, audited and
    /// metered; nothing is repaired.
    pub async fn reconcile(
        &self,
        orders: &[Order],
        positions: &[Position],
        trades: &[Trade],
        since: DateTime<Utc>,
        tolerance: QuantityTolerance,
    ) -> ReconRun {
        let findings = {
            let inner = self.inner.read().await;
            reconcile::reconcile(ReconInputs {
                orders,
                positions,
                trades,
                book: &inner.book,
                events: &inner.events,
                pending: &inner.pending,
                since,
                tolerance,
                replica_id: &self.replica_id,
                now: Utc::now(),
            })
        };
        metrics::set_pending(self.inner.read().await.pending.len());
        let new: Vec<AccountingFinding> = {
            let mut inner = self.inner.write().await;
            findings
                .iter()
                .filter(|f| inner.seen_findings.insert(f.finding_id.clone()))
                .cloned()
                .collect()
        };
        if !new.is_empty() {
            let store = self.store().await;
            for f in &new {
                metrics::count_finding(f.kind.as_str());
                audit::finding(&self.bus, f);
                if !store.append_finding(f).await {
                    metrics::count_journal_error("append_finding");
                }
            }
        }
        ReconRun { findings, new }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::event::{fill_event, EventSide};
    use crate::models::{BotModule, ExecutionMode, Venue};

    fn ledger() -> Arc<GlobalLedger> {
        Arc::new(GlobalLedger::new(EventBus::new(64), "r1"))
    }

    fn fill(reference: &str, side: EventSide, qty: f64, quote: f64) -> AccountingEvent {
        fill_event(
            BotModule::Sniper,
            Venue::PumpFun,
            "w",
            "sniper",
            "MINT",
            "SOL",
            side,
            qty,
            quote / qty,
            quote,
            0.0,
            ExecutionMode::Paper,
            reference,
            Some("order-1".into()),
            Some("p-1".into()),
            Utc::now(),
            "",
        )
    }

    #[tokio::test]
    async fn the_same_fact_is_booked_exactly_once() {
        let l = ledger();
        let e = fill("sig-1", EventSide::Buy, 100.0, 1.0);
        assert!(l.submit(e.clone()).await.is_new());
        assert_eq!(l.submit(e.clone()).await, Applied::Duplicate);
        let mut replay = e.clone();
        replay.quantity = 999.0; // a replay with a different amount is STILL the same fact
        assert_eq!(l.submit(replay).await, Applied::Duplicate);
        assert_eq!(l.len().await, 1);
        let book = l.book().await;
        assert_eq!(book.open_count(), 1);
        assert!((book.positions().next().unwrap().qty - 100.0).abs() < 1e-9);
        let entry = l.entry(&e.event_id()).await.unwrap();
        assert!(entry.is_balanced());
    }

    #[tokio::test]
    async fn concurrent_identical_events_yield_one_mutation() {
        let l = ledger();
        let e = fill("sig-c", EventSide::Buy, 10.0, 1.0);
        let mut handles = Vec::new();
        for _ in 0..16 {
            let l = Arc::clone(&l);
            let e = e.clone();
            handles.push(tokio::spawn(async move { l.submit(e).await }));
        }
        let mut new = 0;
        let mut dup = 0;
        for h in handles {
            match h.await.unwrap() {
                Applied::New(_) => new += 1,
                Applied::Duplicate => dup += 1,
                Applied::Rejected(r) => panic!("rejected: {r}"),
            }
        }
        assert_eq!(new, 1);
        assert_eq!(dup, 15);
        assert_eq!(l.len().await, 1);
        assert!((l.book().await.positions().next().unwrap().qty - 10.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn arithmetic_overflow_is_rejected_before_it_corrupts_the_book() {
        let l = ledger();
        let first = fill("huge-buy-a", EventSide::Buy, f64::MAX, 1.0);
        assert!(l.submit(first).await.is_new());
        let second = fill("huge-buy-b", EventSide::Buy, f64::MAX, 1.0);
        assert!(matches!(l.submit(second).await, Applied::Rejected(_)));
        assert_eq!(l.len().await, 1);
        let book = l.book().await;
        let position = book.positions().next().unwrap();
        assert_eq!(position.qty, f64::MAX);
        assert!(position.qty.is_finite());
    }

    #[tokio::test]
    async fn malformed_events_change_nothing() {
        let l = ledger();
        let mut e = fill("sig-bad", EventSide::Buy, 10.0, 1.0);
        e.quantity = -5.0;
        assert!(matches!(l.submit(e).await, Applied::Rejected(_)));
        assert!(l.is_empty().await);
    }

    #[tokio::test]
    async fn unavailable_journal_parks_the_event_and_flushes_later() {
        let l = ledger();
        let store = Arc::new(MemoryLedgerStore::new());
        l.attach_store(store.clone()).await;
        store.set_unavailable(true);
        let e = fill("sig-p", EventSide::Buy, 10.0, 1.0);
        assert!(l.submit(e.clone()).await.is_new());
        assert_eq!(l.pending().await, vec![e.event_id()]);
        assert_eq!(store.len().await, 0);
        assert_eq!(l.flush_pending().await, 0, "still unavailable");
        store.set_unavailable(false);
        assert_eq!(l.flush_pending().await, 1);
        assert!(l.pending().await.is_empty());
        assert_eq!(store.len().await, 1);
        // A second flush is a no-op and the journal still holds one row.
        assert_eq!(l.flush_pending().await, 0);
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn hydrate_reports_rows_whose_durable_id_does_not_match_the_event() {
        let replay = ledger();
        let event = fill("durable-id-check", EventSide::Buy, 10.0, 1.0);
        let stored = StoredEvent {
            event_id: "forged-id".into(),
            event,
            recorded_at: Utc::now(),
            replica_id: "r-corrupt".into(),
        };
        assert_eq!(replay.hydrate(vec![stored]).await, (0, 0, 1));
        assert!(replay.is_empty().await);
    }

    #[tokio::test]
    async fn hydrate_rejects_same_event_id_with_changed_economics() {
        let replay = ledger();
        let event = fill("same-id-mutated", EventSide::Buy, 10.0, 1.0);
        let event_id = event.event_id();
        assert!(replay.submit(event.clone()).await.is_new());
        let mut changed = event;
        changed.quote_amount = 2.0;
        let stored = StoredEvent {
            event_id,
            event: changed,
            recorded_at: Utc::now(),
            replica_id: "r-mutated".into(),
        };
        assert_eq!(replay.hydrate(vec![stored]).await, (0, 0, 1));
        assert!((replay.book().await.positions().next().unwrap().cost_basis - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn recovery_replays_append_order_not_out_of_order_fact_timestamps() {
        let l = ledger();
        let store = Arc::new(MemoryLedgerStore::new());
        l.attach_store(store.clone()).await;
        let t0 = Utc::now();
        let mut buy = fill("arrival-buy", EventSide::Buy, 10.0, 1.0);
        buy.ts = t0 + chrono::Duration::seconds(2);
        let mut sell = fill("arrival-sell", EventSide::Sell, 5.0, 1.0);
        sell.ts = t0;
        assert!(l.submit(buy).await.is_new());
        assert!(l.submit(sell).await.is_new());

        let journal = store.load_events().await.unwrap();
        assert_eq!(journal[0].event.reference_id, "arrival-buy");
        assert_eq!(journal[1].event.reference_id, "arrival-sell");
        let replay = ledger();
        assert_eq!(replay.hydrate(journal).await, (2, 0, 0));
        let live = l.book().await;
        let restored = replay.book().await;
        let live_position = live.positions().next().unwrap();
        let restored_position = restored.positions().next().unwrap();
        assert!((live_position.qty - 5.0).abs() < 1e-9);
        assert!((restored_position.qty - live_position.qty).abs() < 1e-9);
        assert!((restored_position.cost_basis - live_position.cost_basis).abs() < 1e-9);
        assert!((restored_position.realized - live_position.realized).abs() < 1e-9);
    }

    #[tokio::test]
    async fn pending_journal_events_cannot_be_overtaken_by_new_submissions() {
        let l = ledger();
        let store = Arc::new(MemoryLedgerStore::new());
        l.attach_store(store.clone()).await;
        store.set_unavailable(true);
        let t0 = Utc::now();
        let mut buy = fill("pending-buy", EventSide::Buy, 10.0, 1.0);
        buy.ts = t0;
        let mut sell = fill("pending-sell", EventSide::Sell, 5.0, 1.0);
        sell.ts = t0 + chrono::Duration::seconds(1);
        assert!(l.submit(buy.clone()).await.is_new());
        assert!(l.submit(sell.clone()).await.is_new());
        assert_eq!(l.pending().await, vec![buy.event_id(), sell.event_id()]);

        store.set_unavailable(false);
        let mut later = fill("after-pending", EventSide::Buy, 5.0, 1.0);
        later.ts = t0 + chrono::Duration::seconds(2);
        assert!(l.submit(later.clone()).await.is_new());
        assert!(l.pending().await.is_empty());
        let journal = store.load_events().await.unwrap();
        assert_eq!(
            journal
                .iter()
                .map(|stored| stored.event.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec!["pending-buy", "pending-sell", "after-pending"]
        );

        let replay = ledger();
        assert_eq!(replay.hydrate(journal).await, (3, 0, 0));
        let live_position = l.book().await.positions().next().unwrap().clone();
        let replay_position = replay.book().await.positions().next().unwrap().clone();
        assert!((live_position.qty - replay_position.qty).abs() < 1e-9);
        assert!((live_position.cost_basis - replay_position.cost_basis).abs() < 1e-9);
    }

    #[tokio::test]
    async fn journal_that_already_holds_the_fact_makes_it_a_duplicate() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        first.attach_store(store.clone()).await;
        let e = fill("sig-j", EventSide::Buy, 10.0, 1.0);
        assert!(first.submit(e.clone()).await.is_new());
        // A fresh process life that skipped recovery submits the same fact.
        let second = ledger();
        second.attach_store(store.clone()).await;
        assert_eq!(second.submit(e.clone()).await, Applied::Duplicate);
        let mut altered_replay = e;
        altered_replay.quantity = 20.0;
        altered_replay.quote_amount = 2.0;
        assert_eq!(second.submit(altered_replay).await, Applied::Duplicate);
        assert_eq!(
            second.len().await,
            0,
            "durable duplicates are not applied in memory"
        );
        assert_eq!(second.book().await.open_count(), 0);
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn realized_series_overflow_is_rejected_before_journaling() {
        let l = ledger();
        for (wallet, reference) in [("w1", "fee-a"), ("w2", "fee-b")] {
            let mut fee = fill(reference, EventSide::Buy, 1.0, 1.0);
            fee.kind = EventKind::Fee;
            fee.side = None;
            fee.quantity = 0.0;
            fee.wallet = wallet.into();
            fee.quote_amount = 0.0;
            fee.fee = f64::MAX / 2.0;
            assert!(l.submit(fee).await.is_new());
        }
        let mut overflow = fill("fee-c", EventSide::Buy, 1.0, 1.0);
        overflow.kind = EventKind::Fee;
        overflow.side = None;
        overflow.quantity = 0.0;
        overflow.wallet = "w3".into();
        overflow.quote_amount = 0.0;
        overflow.fee = f64::MAX / 2.0;
        assert!(matches!(l.submit(overflow).await, Applied::Rejected(_)));
        assert_eq!(l.len().await, 2);
    }

    #[tokio::test]
    async fn standalone_fee_quote_amount_counts_as_realized_loss() {
        let l = ledger();
        let mut fee = fill("fee-fallback", EventSide::Buy, 1.0, 1.0);
        fee.kind = EventKind::Fee;
        fee.side = None;
        fee.quantity = 0.0;
        fee.quote_amount = 0.25;
        fee.fee = 0.0;
        assert!(l.submit(fee).await.is_new());
        let series = l.realized_series().await;
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert!((series.by_day[&(today, "SOL".to_string())] + 0.25).abs() < 1e-9);
        assert!((series.total["SOL"] + 0.25).abs() < 1e-9);
    }

    #[tokio::test]
    async fn realized_series_tracks_day_totals_and_peak() {
        let l = ledger();
        l.submit(fill("b1", EventSide::Buy, 100.0, 1.0)).await;
        l.submit(fill("s1", EventSide::Sell, 50.0, 1.5)).await; // +1.0
        l.submit(fill("s2", EventSide::Sell, 50.0, 0.2)).await; // −0.3
        let s = l.realized_series().await;
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert!((s.by_day[&(today, "SOL".to_string())] - 0.7).abs() < 1e-9);
        assert!((s.total["SOL"] - 0.7).abs() < 1e-9);
        assert!((s.peak["SOL"] - 1.0).abs() < 1e-9);
    }
}
