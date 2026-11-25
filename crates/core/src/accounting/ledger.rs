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
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::audit;
use super::book::{BookEffect, BookPosition, PositionBook, PositionKey};
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

impl Inner {
    /// Empty book state (used by the journal repair to rebuild from scratch).
    fn empty() -> Self {
        Self {
            book: PositionBook::new(),
            events: Vec::new(),
            ids: HashSet::new(),
            canonical_events: HashMap::new(),
            entries: HashMap::new(),
            pending: Vec::new(),
            journal_conflict: None,
            contested_events: HashMap::new(),
            conflict_halt: None,
            series: RealizedSeries::default(),
            seen_findings: HashSet::new(),
            last_recorded_at: None,
        }
    }
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
    /// A proven conflict means this process's already-applied state disagrees
    /// with the journal. Do not compound it with more local mutations.
    journal_conflict: Option<String>,
    /// The event THIS process tried to book under a contested id, kept so the
    /// operator report can show both sides even though nothing was applied.
    /// Cleared by nothing: a conflict is human-resolved, and the repair path
    /// rebuilds the book from the journal.
    contested_events: HashMap<String, AccountingEvent>,
    /// Cluster-wide halt hook: called once per process when a conflict is
    /// first proven, so every replica stops taking new entries instead of
    /// only the one that noticed. Never called again while latched.
    conflict_halt: Option<Arc<dyn ConflictHalt>>,
    series: RealizedSeries,
    /// Finding ids already journaled by this process life (a persisting
    /// discrepancy is journaled once, not on every run).
    seen_findings: HashSet<String>,
    /// Last journal timestamp used to preserve a strict local append order.
    last_recorded_at: Option<DateTime<Utc>>,
}

/// Outcome of repairing this replica's book from the durable journal.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct JournalRepair {
    /// True when the in-memory book now equals a replay of the journal.
    pub repaired: bool,
    /// The conflicting event id that was latched before the repair.
    pub claimed_conflict: Option<String>,
    /// True when a latched conflict was cleared by this repair.
    pub conflict_cleared: bool,
    /// Durable rows replayed into the fresh book.
    pub replayed_events: usize,
    /// Local rows dropped because the journal does not back them.
    pub unbacked_events: usize,
    /// Alias of `unbacked_events` (symmetric naming for operators).
    pub dropped_events: usize,
    /// Pending (unjournaled) local rows the journal contradicted and that the
    /// repair replaced with the journal's version.
    pub discarded_pending_events: usize,
    /// Process-life findings retained (do not re-journal known discrepancies).
    pub seen_findings: usize,
    /// Why the repair was refused, when it was.
    pub reason: String,
}

impl JournalRepair {
    /// One-line operator/audit summary.
    pub fn summary(&self) -> String {
        if self.repaired {
            format!(
                "repaired from journal: replayed={} dropped={} discarded_pending={} conflict={}",
                self.replayed_events,
                self.dropped_events,
                self.discarded_pending_events,
                self.claimed_conflict.as_deref().unwrap_or("-")
            )
        } else {
            format!("repair refused: {}", self.reason)
        }
    }
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
/// Deployment-wide reaction to a PROVEN accounting conflict. The ledger is
/// process-local, so a divergent replica must be able to stop the whole
/// cluster from taking new entries; the server attaches an implementation
/// that engages the deployment kill switch.
#[async_trait::async_trait]
pub trait ConflictHalt: Send + Sync {
    /// Called once per process, on the transition into a proven conflict.
    /// Returning `false` means "already halted" (the hook is idempotent).
    async fn halt(&self, conflict: &str) -> bool;
}

impl Inner {
    /// Latch a proven conflict and take the cluster-halt hook exactly once.
    /// Returns the hook when this call is the transition into the conflict.
    fn latch_conflict(&mut self, event_id: &str) -> Option<Arc<dyn ConflictHalt>> {
        if self.journal_conflict.is_some() {
            return None;
        }
        self.journal_conflict = Some(event_id.to_string());
        self.conflict_halt.clone()
    }
}

pub struct GlobalLedger {
    inner: RwLock<Inner>,
    store: RwLock<Arc<dyn LedgerStore>>,
    pub(crate) bus: EventBus,
    replica_id: String,
}

// Identity excludes observation metadata (timestamp, detail, trade id), which
// can differ when the same venue fact is observed again. Financial and
// attribution fields must agree before a retry is accepted as identical.
fn same_event_economics(old: &AccountingEvent, event: &AccountingEvent) -> bool {
    old.kind == event.kind
        && old.module == event.module
        && old.venue == event.venue
        && old.wallet == event.wallet
        && old.reference_id == event.reference_id
        && old.strategy == event.strategy
        && old.asset == event.asset
        && old.quote_asset == event.quote_asset
        && old.side == event.side
        && old.quantity == event.quantity
        && old.price == event.price
        && old.quote_amount == event.quote_amount
        && old.fee == event.fee
        && old.mode == event.mode
        && old.correlation_id == event.correlation_id
        && old.position_id == event.position_id
        && old.counterparty_wallet == event.counterparty_wallet
}

/// Which DIFFERENCE a durable row shows against the event this process holds
/// under the same id — named, so the operator report and the halt reason say
/// what diverged instead of just "payload".
fn describe_mismatch(local: &AccountingEvent, durable: &AccountingEvent) -> String {
    let mut fields: Vec<&str> = Vec::new();
    if local.kind != durable.kind {
        fields.push("kind");
    }
    if local.wallet != durable.wallet {
        fields.push("wallet");
    }
    if local.module != durable.module {
        fields.push("module");
    }
    if local.asset != durable.asset {
        fields.push("asset");
    }
    if local.quote_asset != durable.quote_asset {
        fields.push("quote_asset");
    }
    if local.side != durable.side {
        fields.push("side");
    }
    if local.quantity != durable.quantity {
        fields.push("quantity");
    }
    if local.price != durable.price {
        fields.push("price");
    }
    if local.quote_amount != durable.quote_amount {
        fields.push("quote_amount");
    }
    if local.fee != durable.fee {
        fields.push("fee");
    }
    if local.mode != durable.mode {
        fields.push("mode");
    }
    if local.correlation_id != durable.correlation_id {
        fields.push("correlation_id");
    }
    if local.position_id != durable.position_id {
        fields.push("position_id");
    }
    if local.counterparty_wallet != durable.counterparty_wallet {
        fields.push("counterparty_wallet");
    }
    if fields.is_empty() {
        "the payload differs in a field this report does not name (timestamps or metadata)"
            .to_string()
    } else {
        format!("diverging fields: {}", fields.join(", "))
    }
}

/// The durable journal's account of ONE contested event id: the event this
/// process holds locally, the row (or rows) it is stored under, and which
/// fields differ. Built for the operator surface; never mutates anything.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PayloadMismatch {
    pub event_id: String,
    /// The local canonical event, summarised (`None` when this replica never
    /// applied one — the divergence is then journal-side only).
    pub local: Option<String>,
    /// Every row the journal holds under that id, summarised.
    pub durable: Vec<String>,
    /// What diverged, in plain words.
    pub differences: Vec<String>,
}

// Journal rows are append-only. If the store cannot read back exactly one
// matching identity, do not classify an unverified conflict as a duplicate.
async fn matching_durable_event(
    store: &Arc<dyn LedgerStore>,
    event_id: &str,
    event: &AccountingEvent,
) -> Option<bool> {
    let row = store.load_event(event_id).await??;
    if row.event_id != event_id || row.event.event_id() != event_id {
        return Some(false);
    }
    Some(same_event_economics(&row.event, event))
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
                journal_conflict: None,
                contested_events: HashMap::new(),
                conflict_halt: None,
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
        if let Some(conflict) = &inner.journal_conflict {
            return Applied::Rejected(format!(
                "ledger blocked by unresolved pending journal conflict: {conflict}"
            ));
        }
        if inner.ids.contains(&event_id) {
            let same_economics = inner
                .canonical_events
                .get(&event_id)
                .is_some_and(|old| same_event_economics(old, &event));
            if !same_economics {
                metrics::count_ledger_event(event.kind.as_str(), "rejected");
                warn!(event = %event_id, "ledger event identity reused with a different payload");
                return Applied::Rejected(
                    "ledger event identity conflicts with existing payload".into(),
                );
            }
            metrics::count_ledger_event(event.kind.as_str(), "duplicate");
            metrics::count_duplicate_prevented("global_ledger");
            debug!(event = %event_id, "ledger duplicate — not booked twice");
            return Applied::Duplicate;
        }
        // Price the slice from the COMMITTED journal, not just this replica's
        // local book: a replica that has not seen the opening buy would
        // otherwise post the sell at zero cost (see `committed_slice_cost`).
        let event_recorded_at = inner
            .last_recorded_at
            .as_ref()
            .and_then(|last| last.checked_add_signed(chrono::Duration::microseconds(1)))
            .map(|floor| Utc::now().max(floor))
            .unwrap_or_else(Utc::now);
        let target = StoredEvent {
            event_id: event_id.clone(),
            event: event.clone(),
            recorded_at: event_recorded_at,
            replica_id: self.replica_id.clone(),
        };
        let cost_of_slice = Self::committed_slice_cost(
            &store,
            &inner.book,
            &inner.events,
            &inner.canonical_events,
            &target,
        )
        .await;
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
        if let Some(conflict) = &inner.journal_conflict {
            return Applied::Rejected(format!(
                "ledger blocked by unresolved pending journal conflict: {conflict}"
            ));
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
            match matching_durable_event(&store, &event_id, &event).await {
                Some(true) => {
                    metrics::count_ledger_event(event.kind.as_str(), "duplicate");
                    metrics::count_duplicate_prevented("global_ledger_journal");
                    return Applied::Duplicate;
                }
                Some(false) => {
                    // Keep the contested local event so the operator report can
                    // show BOTH sides even though nothing was booked.
                    inner
                        .contested_events
                        .insert(event_id.clone(), event.clone());
                    let durable = Self::durable_rows_for(&store, &event_id).await;
                    let reason = match durable.first() {
                        Some(row) => format!(
                            "ledger event identity conflicts with durable payload ({})",
                            describe_mismatch(&event, row)
                        ),
                        None => "ledger event identity conflicts with durable payload".to_string(),
                    };
                    warn!(event = %event_id, reason = %reason, "durable ledger payload mismatch");
                    if let Some(halt) = inner.latch_conflict(&event_id) {
                        let conflict = event_id.clone();
                        tokio::spawn(async move {
                            warn!(
                                conflict = %conflict,
                                "proven ledger conflict; halting every replica"
                            );
                            halt.halt(&conflict).await;
                        });
                    }
                    return Applied::Rejected(reason);
                }
                None => {
                    return Applied::Rejected("unable to verify durable ledger duplicate".into())
                }
            }
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

    /// Every durable row the journal holds under `event_id` (bounded read;
    /// empty when the journal cannot be read — the caller then says less, not
    /// more).
    async fn durable_rows_for(
        store: &Arc<dyn LedgerStore>,
        event_id: &str,
    ) -> Vec<AccountingEvent> {
        match store.load_events().await {
            Some(rows) => rows
                .into_iter()
                .filter(|row| row.event_id == event_id)
                .map(|row| row.event)
                .collect(),
            None => Vec::new(),
        }
    }

    /// The conflict an operator needs to adjudicate, in full: the contested
    /// event ids and, for each, the local event, every durable row and the
    /// diverging fields. Read-only; the latch is left exactly as it is.
    pub async fn conflict_payload_mismatches(&self) -> Vec<PayloadMismatch> {
        let store = self.store().await;
        let inner = self.inner.read().await;
        let ids: Vec<String> = inner.journal_conflict.iter().cloned().collect();
        drop(inner);
        let mut out = Vec::new();
        for event_id in ids {
            let local = {
                let inner = self.inner.read().await;
                inner
                    .canonical_events
                    .get(&event_id)
                    .or_else(|| inner.contested_events.get(&event_id))
                    .cloned()
            };
            let durable = Self::durable_rows_for(&store, &event_id).await;
            let differences: Vec<String> = match &local {
                Some(local) => durable
                    .iter()
                    .map(|row| describe_mismatch(local, row))
                    .collect(),
                None => durable
                    .iter()
                    .map(|row| {
                        format!(
                            "this replica never applied an event under this id; the journal \
                             holds {}",
                            row.summary()
                        )
                    })
                    .collect(),
            };
            out.push(PayloadMismatch {
                event_id: event_id.clone(),
                local: local.as_ref().map(|e| e.summary()),
                durable: durable.iter().map(|e| e.summary()).collect(),
                differences,
            });
        }
        out
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
                Some(true) => flushed.push(event.event_id),
                Some(false) => {
                    match matching_durable_event(store, &event.event_id, &event.event).await {
                        Some(true) => flushed.push(event.event_id),
                        Some(false) => {
                            warn!(event = %event.event_id, "pending ledger event conflicts with durable journal; blocking further local booking");
                            if let Some(halt) = inner.latch_conflict(&event.event_id) {
                                let conflict = event.event_id.clone();
                                tokio::spawn(async move {
                                    halt.halt(&conflict).await;
                                });
                            }
                            break;
                        }
                        None => {
                            warn!(event = %event.event_id, "unable to verify pending ledger duplicate; keeping pending");
                            break;
                        }
                    }
                }
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

    /// The cost of the slice this event sells, computed from the COMMITTED
    /// journal rather than from this replica's in-memory book.
    ///
    /// A replica that has not yet seen the opening buy has an empty local book,
    /// so `book.cost_of_slice` returns 0 and the sell would be posted at zero
    /// cost: nothing credited to inventory, the whole proceeds booked as realized
    /// PnL. The journal is the shared truth.
    ///
    /// Cost-free when the local book already holds the position's history: the
    /// replay reproduces it and the local answer stands. Fail-closed: when the
    /// journal cannot be read (or a committed posting cannot be decoded) the
    /// local cost is used and a warning says so — a missing cost must never
    /// silently become zero without a trace.
    async fn committed_slice_cost(
        store: &Arc<dyn LedgerStore>,
        book: &PositionBook,
        events: &[StoredEvent],
        canonical: &HashMap<String, AccountingEvent>,
        target: &StoredEvent,
    ) -> f64 {
        let key = PositionKey::of(&target.event);
        let local = book.cost_of_slice(&target.event);

        // Width of this replica's knowledge: the first journaled event of this
        // position it has actually applied. If the history starts there, the
        // local book already reflects everything the journal knows before this
        // event and there is nothing to repair.
        let applied_first = events
            .iter()
            .filter(|row| PositionKey::of(&row.event) == key)
            .filter(|row| canonical.contains_key(&row.event_id))
            .map(|row| (row.recorded_at, row.event_id.clone()))
            .min();
        let Some((first_at, first_id)) = applied_first else {
            // This replica has applied nothing for the position: only the journal
            // can price the slice.
            return Self::journal_slice_cost(store, target, local).await;
        };
        let journal_prefix = match store.load_events().await {
            Some(rows) => rows,
            None => {
                if local == 0.0 {
                    warn!(
                        event = %target.event_id,
                        "journal unavailable while pricing a sell this replica has no cost for"
                    );
                }
                return local;
            }
        };
        let older_than_known = journal_prefix
            .iter()
            .filter(|row| PositionKey::of(&row.event) == key)
            .any(|row| {
                (row.recorded_at, row.event_id.clone()) < (first_at, first_id.clone())
                    && row.event_id != target.event_id
            });
        if !older_than_known {
            return local;
        }
        Self::journal_slice_cost(store, target, local).await
    }

    /// Price `target` from the durable postings alone: replay the position's
    /// committed history, applying what the journal recorded instead of
    /// recomputing it.
    async fn journal_slice_cost(
        store: &Arc<dyn LedgerStore>,
        target: &StoredEvent,
        local: f64,
    ) -> f64 {
        let key = PositionKey::of(&target.event);
        let Some(rows) = store.load_events().await else {
            return local;
        };
        let mut history: Vec<StoredEvent> = rows
            .into_iter()
            .filter(|row| PositionKey::of(&row.event) == key)
            .filter(|row| {
                (row.recorded_at, &row.event_id) <= (target.recorded_at, &target.event_id)
            })
            .collect();
        history.sort_by(|a, b| {
            a.recorded_at
                .cmp(&b.recorded_at)
                .then_with(|| a.event_id.cmp(&b.event_id))
        });
        let mut replay = PositionBook::new();
        for row in history {
            if row.event_id == target.event_id {
                continue;
            }
            if row.event.validate().is_err() {
                return local;
            }
            match store.load_entry(&row.event_id).await {
                Some(Some(entry)) => {
                    replay.apply_committed(&row.event, &row.event_id, &entry);
                }
                Some(None) => return local,
                None => return local,
            }
        }
        let from_journal = replay.cost_of_slice(&target.event);
        if (from_journal - local).abs() <= f64::EPSILON {
            local
        } else if local == 0.0 {
            warn!(
                event = %target.event_id,
                from_journal,
                "pricing a sell from the committed journal: this replica had no local cost"
            );
            from_journal
        } else {
            warn!(
                event = %target.event_id,
                local, from_journal,
                "local cost disagrees with the committed journal"
            );
            local
        }
    }

    /// Load `events` (from the durable journal) into the book without
    /// re-journaling them. Returns `(applied, duplicates_skipped)`.
    pub(crate) async fn hydrate(&self, events: Vec<StoredEvent>) -> (usize, usize, usize) {
        let store = self.store().await;
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
                    // A durable row under an already-applied identity with
                    // different economics is a proven divergence, not a
                    // transient replay failure. Record WHICH fields differ so
                    // the operator surface and the halt reason are actionable.
                    if let Some(local) = inner.canonical_events.get(&stored.event_id).cloned() {
                        warn!(
                            event = %stored.event_id,
                            divergence = %describe_mismatch(&local, &stored.event),
                            "durable journal row disagrees with the local book"
                        );
                        inner
                            .contested_events
                            .insert(stored.event_id.clone(), local);
                    }
                    inner
                        .journal_conflict
                        .get_or_insert_with(|| stored.event_id.clone());
                }
                continue;
            }
            if stored.event_id != stored.event.event_id() || stored.event.validate().is_err() {
                invalid += 1;
                continue;
            }
            let cost = Self::committed_slice_cost(
                &store,
                &inner.book,
                &inner.events,
                &inner.canonical_events,
                &stored,
            )
            .await;
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

    /// Replace this process's in-memory book with a replay of the durable
    /// journal — the ONLY way out of a proven journal conflict. A conflict
    /// means the local book disagrees with the journal, so the divergent local
    /// rows are dropped instead of reversed: the journal is the source of
    /// truth for what was actually recorded, and inventing reversal events
    /// would book money that never moved.
    ///
    /// Returns `repaired = false` when the journal cannot be read, when local
    /// writes are still pending (their fate is unknown), or when a durable row
    /// fails replay validation. The latch then stays set and trading remains
    /// fail-closed. Repairs are idempotent: running it twice is a no-op.
    pub async fn reconcile_from_journal(&self) -> JournalRepair {
        let store = self.store().await;
        let Some(durable) = store.load_events().await else {
            return JournalRepair {
                repaired: false,
                reason: "durable journal unavailable".into(),
                ..JournalRepair::default()
            };
        };
        let mut inner = self.inner.write().await;

        // Pending (unjournaled) local bookings: their fate decides whether the
        // journal may replace the book. A pending row the journal knows with
        // the SAME economics is simply backed (it is a lost `record_event`
        // acknowledgement). A pending row the journal knows with DIFFERENT
        // economics is the conflict itself — the journal wins. A pending row
        // the journal does not know at all cannot be adjudicated here, so the
        // repair is refused and the fence stays (never discard an event whose
        // journal write might still land).
        let by_id: HashMap<&str, &StoredEvent> =
            durable.iter().map(|e| (e.event_id.as_str(), e)).collect();
        let mut discarded_pending = 0usize;
        for id in &inner.pending {
            let Some(local) = inner.events.iter().find(|e| &e.event_id == id) else {
                continue;
            };
            match by_id.get(id.as_str()) {
                Some(durable_row) if same_event_economics(&durable_row.event, &local.event) => {}
                Some(_) => discarded_pending += 1,
                None => {
                    return JournalRepair {
                        repaired: false,
                        reason: format!(
                            "pending booking {id} is absent from the durable journal; operator review required"
                        ),
                        ..JournalRepair::default()
                    };
                }
            }
        }
        let had_conflict = inner.journal_conflict.is_some();
        let mut fresh = Inner::empty();
        let mut applied = 0usize;
        let mut invalid = 0usize;
        for stored in &durable {
            let ok = stored.event_id == stored.event.event_id()
                && stored.event.validate().is_ok()
                && fresh
                    .book
                    .validate_apply(&stored.event, &stored.event_id)
                    .is_ok();
            let entry = if ok {
                expand(&stored.event, fresh.book.cost_of_slice(&stored.event)).ok()
            } else {
                None
            };
            let Some(entry) = entry else {
                invalid += 1;
                continue;
            };
            if Self::validate_series_update(
                &fresh.series,
                &stored.event,
                fresh.book.cost_of_slice(&stored.event),
            )
            .is_err()
            {
                invalid += 1;
                continue;
            }
            let effect = fresh.book.apply(&stored.event, &stored.event_id);
            Self::fold_series(&mut fresh.series, &stored.event, &effect);
            fresh.last_recorded_at = Some(
                fresh
                    .last_recorded_at
                    .map(|last| last.max(stored.recorded_at))
                    .unwrap_or(stored.recorded_at),
            );
            fresh.ids.insert(stored.event_id.clone());
            fresh
                .canonical_events
                .insert(stored.event_id.clone(), stored.event.clone());
            fresh.entries.insert(stored.event_id.clone(), entry);
            fresh.events.push(stored.clone());
            applied += 1;
        }
        if invalid > 0 || fresh.events.len() != durable.len() {
            inner.journal_conflict = Some(
                inner
                    .journal_conflict
                    .clone()
                    .unwrap_or_else(|| "journal replay rejected a durable row".to_string()),
            );
            metrics::count_journal_repair("rejected");
            let report = JournalRepair {
                repaired: false,
                reason: format!("{invalid} durable row(s) failed replay validation"),
                ..JournalRepair::default()
            };
            warn!(
                invalid,
                "ledger journal repair aborted; book left untouched"
            );
            return report;
        }

        // The local rows the journal does not back: they are what a conflict
        // makes untrustworthy. Counted, then dropped with the rest.
        let dropped = inner
            .events
            .iter()
            .filter(|e| !fresh.ids.contains(&e.event_id) && !inner.pending.contains(&e.event_id))
            .count();
        let mut report = JournalRepair {
            repaired: true,
            claimed_conflict: inner.journal_conflict.clone(),
            conflict_cleared: had_conflict,
            replayed_events: applied,
            unbacked_events: dropped,
            dropped_events: dropped,
            discarded_pending_events: discarded_pending,
            seen_findings: inner.seen_findings.len(),
            reason: String::new(),
        };
        inner.book = fresh.book;
        inner.events = fresh.events;
        inner.ids = fresh.ids;
        inner.canonical_events = fresh.canonical_events;
        inner.entries = fresh.entries;
        inner.series = fresh.series;
        inner.last_recorded_at = fresh.last_recorded_at;
        inner.pending.clear();
        inner.journal_conflict = None;
        report.seen_findings = inner.seen_findings.len();
        metrics::count_journal_repair("repaired");
        info!(
            replayed = report.replayed_events,
            dropped = report.dropped_events,
            conflict = report.claimed_conflict.as_deref().unwrap_or("-"),
            "ledger book rebuilt from the durable journal"
        );
        report
    }

    /// Number of applied events that are still missing from the durable journal.
    pub async fn pending_count(&self) -> usize {
        self.inner.read().await.pending.len()
    }

    /// Attach the deployment-wide halt hook (startup only). It is invoked at
    /// most once per process, on the transition into a proven conflict.
    pub async fn attach_conflict_halt(&self, halt: Arc<dyn ConflictHalt>) {
        self.inner.write().await.conflict_halt = Some(halt);
    }

    /// The id of a proven pending-vs-durable accounting conflict, if one has
    /// latched. Callers must fail closed until reconciliation replaces this
    /// process's divergent in-memory book.
    pub async fn journal_conflict(&self) -> Option<String> {
        self.inner.read().await.journal_conflict.clone()
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

    /// A journal that ALREADY holds `rows` under the same ids: every
    /// `record_event` reports "already booked", which is exactly the shape of
    /// a row written by another replica or a previous process life.
    struct PreloadedStore {
        rows: Vec<StoredEvent>,
    }

    #[async_trait::async_trait]
    impl LedgerStore for PreloadedStore {
        async fn record_event(&self, _stored: &StoredEvent, _entry: &Entry) -> Option<bool> {
            Some(false)
        }
        async fn load_events(&self) -> Option<Vec<StoredEvent>> {
            Some(self.rows.clone())
        }
        async fn upsert_position(&self, _position: &BookPosition) -> bool {
            true
        }
        async fn append_finding(&self, _finding: &AccountingFinding) -> bool {
            true
        }
        async fn recent_findings(&self, _limit: usize) -> Option<Vec<AccountingFinding>> {
            Some(Vec::new())
        }
    }

    fn stored(event: &AccountingEvent) -> StoredEvent {
        StoredEvent {
            event_id: event.event_id(),
            event: event.clone(),
            recorded_at: Utc::now(),
            replica_id: "other-replica".into(),
        }
    }

    #[tokio::test]
    async fn a_durable_row_with_different_economics_names_the_diverging_fields() {
        let local = fill("sig-mm", EventSide::Buy, 100.0, 1.0);
        let mut theirs = local.clone();
        theirs.quantity = 250.0;
        theirs.fee = 0.75;

        let l = ledger();
        l.attach_store(Arc::new(PreloadedStore {
            rows: vec![stored(&theirs)],
        }))
        .await;
        let rejected = l.submit(local).await;
        let Applied::Rejected(reason) = rejected else {
            panic!("a divergent durable row must be rejected, got {rejected:?}");
        };
        assert!(
            reason.contains("quantity") && reason.contains("fee"),
            "the refusal must name the diverging fields: {reason}"
        );
        assert_eq!(l.len().await, 0, "nothing is booked from a contested id");
    }

    #[tokio::test]
    async fn the_conflict_report_carries_both_sides_and_the_latch_stays_set() {
        let local = fill("sig-mm2", EventSide::Buy, 100.0, 1.0);
        let expected_id = local.event_id();
        let mut theirs = local.clone();
        theirs.quote_amount = 42.0;

        let l = ledger();
        l.attach_store(Arc::new(PreloadedStore {
            rows: vec![stored(&theirs)],
        }))
        .await;
        // A row under an id this replica never applied does not latch on its
        // own; the contested local event does.
        assert!(matches!(l.submit(local).await, Applied::Rejected(_)));
        assert_eq!(
            l.journal_conflict().await.as_deref(),
            Some(expected_id.as_str())
        );

        let report = l.conflict_payload_mismatches().await;
        assert_eq!(report.len(), 1, "{report:?}");
        assert_eq!(report[0].event_id, expected_id);
        assert!(report[0].local.is_some(), "the local side is reported");
        assert_eq!(report[0].durable.len(), 1, "the durable side is reported");
        assert!(
            report[0].differences[0].contains("quote_amount"),
            "{:?}",
            report[0].differences
        );

        // The report is read-only: the latch is still set and new entries are
        // still refused.
        assert!(matches!(
            l.submit(fill("sig-mm3", EventSide::Buy, 1.0, 1.0)).await,
            Applied::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn an_identical_durable_row_is_a_duplicate_not_a_conflict() {
        let local = fill("sig-mm4", EventSide::Buy, 100.0, 1.0);
        let l = ledger();
        l.attach_store(Arc::new(PreloadedStore {
            rows: vec![stored(&local)],
        }))
        .await;
        assert_eq!(l.submit(local).await, Applied::Duplicate);
        assert!(l.journal_conflict().await.is_none());
        assert!(l.conflict_payload_mismatches().await.is_empty());
    }

    #[tokio::test]
    async fn a_durable_row_this_replica_never_applied_is_described_not_applied() {
        // The journal holds an event under an id this process does not know;
        // hydrating it is the journal's job, but the mismatch report must say
        // what the journal holds rather than inventing a local side.
        let theirs = fill("sig-mm5", EventSide::Sell, 5.0, 2.0);
        let l = ledger();
        l.attach_store(Arc::new(PreloadedStore {
            rows: vec![stored(&theirs)],
        }))
        .await;
        let (applied, dup, invalid) = l.hydrate(vec![stored(&theirs)]).await;
        assert_eq!((applied, dup, invalid), (1, 0, 0));
        assert!(l.conflict_payload_mismatches().await.is_empty());
    }

    #[tokio::test]
    async fn the_same_fact_is_booked_exactly_once() {
        let l = ledger();
        let e = fill("sig-1", EventSide::Buy, 100.0, 1.0);
        assert!(l.submit(e.clone()).await.is_new());
        assert_eq!(l.submit(e.clone()).await, Applied::Duplicate);
        let mut replay = e.clone();
        replay.quantity = 999.0; // same identity with different economics is a conflict
        assert!(matches!(l.submit(replay).await, Applied::Rejected(_)));
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
    async fn local_retry_rejects_changed_economics_without_double_booking() {
        let ledger = ledger();
        let event = fill("local-conflicting-retry", EventSide::Buy, 10.0, 1.0);
        assert!(ledger.submit(event.clone()).await.is_new());
        assert_eq!(ledger.submit(event.clone()).await, Applied::Duplicate);
        let mut changed = event;
        changed.quote_amount = 2.0;
        assert!(matches!(ledger.submit(changed).await, Applied::Rejected(_)));
        assert_eq!(ledger.len().await, 1);
        assert!((ledger.book().await.positions().next().unwrap().cost_basis - 1.0).abs() < 1e-9);
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
    async fn pending_conflict_with_other_replica_stays_unresolved() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        let second = ledger();
        first.attach_store(store.clone()).await;
        second.attach_store(store.clone()).await;
        let event = fill("replica-conflict", EventSide::Buy, 10.0, 1.0);
        store.set_unavailable(true);
        assert!(first.submit(event.clone()).await.is_new());
        assert_eq!(first.pending().await, vec![event.event_id()]);
        store.set_unavailable(false);
        let mut conflicting = event.clone();
        conflicting.quote_amount = 2.0;
        assert!(second.submit(conflicting).await.is_new());
        assert_eq!(first.flush_pending().await, 0);
        assert_eq!(first.pending().await, vec![event.event_id()]);
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn detected_pending_conflict_blocks_further_local_booking() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        let second = ledger();
        first.attach_store(store.clone()).await;
        second.attach_store(store.clone()).await;
        let original = fill("pending-conflict-fence", EventSide::Buy, 10.0, 1.0);
        store.set_unavailable(true);
        assert!(first.submit(original.clone()).await.is_new());
        store.set_unavailable(false);
        let mut other = original.clone();
        other.quote_amount = 2.0;
        assert!(second.submit(other).await.is_new());
        assert_eq!(first.flush_pending().await, 0);
        let prior = first.events().await.len();
        let later = fill("after-known-conflict", EventSide::Buy, 5.0, 1.0);
        assert!(matches!(first.submit(later).await, Applied::Rejected(_)));
        assert!(matches!(first.submit(original).await, Applied::Rejected(_)));
        assert_eq!(first.events().await.len(), prior);
        assert_eq!(store.len().await, 1);
    }

    #[tokio::test]
    async fn conflict_discovered_during_submission_refuses_that_submission() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        let second = ledger();
        first.attach_store(store.clone()).await;
        second.attach_store(store.clone()).await;
        let original = fill("pending-conflict-on-submit", EventSide::Buy, 10.0, 1.0);
        store.set_unavailable(true);
        assert!(first.submit(original.clone()).await.is_new());
        store.set_unavailable(false);
        let mut other = original;
        other.quote_amount = 2.0;
        assert!(second.submit(other).await.is_new());
        let prior = first.events().await.len();
        let later = fill("detect-conflict-on-submit", EventSide::Buy, 5.0, 1.0);
        assert!(matches!(first.submit(later).await, Applied::Rejected(_)));
        assert_eq!(first.events().await.len(), prior);
        assert_eq!(store.len().await, 1);
    }

    /// A pending booking the journal has never seen cannot be adjudicated, so
    /// the repair must refuse and keep the fail-closed fence.
    #[tokio::test]
    async fn repair_refuses_when_a_pending_booking_is_absent_from_the_journal() {
        let store = Arc::new(MemoryLedgerStore::new());
        let l = ledger();
        l.attach_store(store.clone()).await;
        store.set_unavailable(true);
        assert!(l
            .submit(fill("never-journaled", EventSide::Buy, 3.0, 1.0))
            .await
            .is_new());
        assert_eq!(l.pending_count().await, 1);
        // The journal is READABLE again — it simply never saw this booking.
        store.set_unavailable(false);
        let report = l.reconcile_from_journal().await;
        assert!(!report.repaired, "{report:?}");
        assert!(
            report.reason.contains("absent from the durable journal"),
            "unexpected refusal reason: {report:?}"
        );
        assert_eq!(
            l.pending_count().await,
            1,
            "the booking must not be discarded"
        );
        assert_eq!(l.events().await.len(), 1);
    }

    /// RED→GREEN: a proven journal conflict must be RECOVERABLE. Before the
    /// reconcile-from-journal path existed, the latch was permanent for the
    /// life of the process: the divergent in-memory row stayed booked, the
    /// journal's version was ignored, and every later event was refused with
    /// no way back short of a restart.
    /// Counts halt calls so "exactly once" is observable.
    struct CountingHalt(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl ConflictHalt for CountingHalt {
        async fn halt(&self, _conflict: &str) -> bool {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
        }
    }

    #[tokio::test]
    async fn a_proven_conflict_halts_the_cluster_exactly_once() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        let second = ledger();
        first.attach_store(store.clone()).await;
        second.attach_store(store.clone()).await;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        first
            .attach_conflict_halt(Arc::new(CountingHalt(calls.clone())))
            .await;

        // Divergent local booking whose journal write was lost.
        let original = fill("cluster-halt-1", EventSide::Buy, 10.0, 1.0);
        store.set_unavailable(true);
        assert!(first.submit(original.clone()).await.is_new());
        store.set_unavailable(false);
        // Another replica journals the same identity with other economics.
        let mut other = original.clone();
        other.quote_amount = 2.0;
        assert!(second.submit(other).await.is_new());

        // First divergence: rejected AND the cluster hook fires.
        assert!(matches!(
            first
                .submit(fill("cluster-halt-2", EventSide::Buy, 5.0, 1.0))
                .await,
            Applied::Rejected(_)
        ));
        assert!(first.journal_conflict().await.is_some());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a proven conflict must halt the cluster once"
        );

        // Every later rejection keeps the latch but does NOT halt again.
        for n in 3..6 {
            assert!(matches!(
                first
                    .submit(fill(&format!("cluster-halt-{n}"), EventSide::Buy, 1.0, 1.0))
                    .await,
                Applied::Rejected(_)
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the halt must be idempotent, not re-fired per rejection"
        );
    }

    #[tokio::test]
    async fn an_ordinary_rejection_never_halts_the_cluster() {
        let store = Arc::new(MemoryLedgerStore::new());
        let l = ledger();
        l.attach_store(store.clone()).await;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        l.attach_conflict_halt(Arc::new(CountingHalt(calls.clone())))
            .await;
        // A booking that fails local validation is not a conflict.
        let mut bad = fill("cluster-halt-bad", EventSide::Buy, 1.0, 1.0);
        bad.reference_id = String::new();
        assert!(matches!(l.submit(bad).await, Applied::Rejected(_)));
        assert_eq!(l.journal_conflict().await, None);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn journal_conflict_is_repaired_from_the_durable_journal() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        let second = ledger();
        first.attach_store(store.clone()).await;
        second.attach_store(store.clone()).await;

        // The divergent replica applies a local booking its journal write lost.
        let original = fill("reconcile-conflict", EventSide::Buy, 10.0, 1.0);
        store.set_unavailable(true);
        assert!(first.submit(original.clone()).await.is_new());
        store.set_unavailable(false);

        // Another replica journals the same identity with other economics.
        let mut other = original.clone();
        other.quote_amount = 2.0;
        other.price = Some(0.2);
        assert!(second.submit(other).await.is_new());
        assert_eq!(store.len().await, 1);

        // The divergent replica cannot flush, detects the conflict and fences.
        let prior_events = first.events().await.len();
        let later = fill("reconcile-later", EventSide::Buy, 5.0, 1.0);
        assert!(matches!(
            first.submit(later.clone()).await,
            Applied::Rejected(_)
        ));
        let conflict = first.journal_conflict().await.expect("conflict must latch");
        assert_eq!(conflict, original.event_id());
        assert_eq!(first.events().await.len(), prior_events);

        // RED assertion: there was no recovery path at all.
        let report = first.reconcile_from_journal().await;
        assert!(
            report.repaired,
            "the durable journal must be able to repair the divergent book: {report:?}"
        );
        assert!(report.conflict_cleared);
        assert_eq!(
            report.discarded_pending_events, 1,
            "the pending local booking the journal contradicts must be discarded: {report:?}"
        );
        assert_eq!(first.journal_conflict().await, None);
        assert_eq!(first.pending_count().await, 0);

        // The replacement row carries the JOURNAL's economics, not the local guess.
        let replayed = first
            .events()
            .await
            .into_iter()
            .find(|e| e.event_id == original.event_id())
            .expect("the journaled row must be in the repaired book");
        assert_eq!(replayed.event.quote_amount, 2.0);
        assert_eq!(replayed.event.price, Some(0.2));

        // The book now matches the durable journal exactly.
        let durable = store.load_events().await.expect("journal readable");
        assert_eq!(first.events().await.len(), durable.len());
        let mut want: Vec<String> = durable.iter().map(|e| e.event_id.clone()).collect();
        want.sort();
        let mut got: Vec<String> = first
            .events()
            .await
            .iter()
            .map(|e| e.event_id.clone())
            .collect();
        got.sort();
        assert_eq!(got, want);

        // And the replica can book again.
        assert!(first.submit(later).await.is_new());
    }

    #[tokio::test]
    async fn direct_durable_payload_conflict_latches_the_local_fence() {
        let store = Arc::new(MemoryLedgerStore::new());
        let first = ledger();
        let second = ledger();
        first.attach_store(store.clone()).await;
        second.attach_store(store.clone()).await;
        let original = fill("direct-durable-conflict", EventSide::Buy, 10.0, 1.0);
        assert!(first.submit(original.clone()).await.is_new());
        let mut conflicting = original;
        conflicting.quote_amount = 2.0;
        conflicting.price = Some(0.2);
        assert!(matches!(
            second.submit(conflicting).await,
            Applied::Rejected(_)
        ));
        assert!(second.journal_conflict().await.is_some());
        assert!(matches!(
            second
                .submit(fill(
                    "later-after-direct-conflict",
                    EventSide::Buy,
                    5.0,
                    1.0
                ))
                .await,
            Applied::Rejected(_)
        ));
        assert_eq!(second.len().await, 0);
        assert_eq!(store.len().await, 1);
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
        assert!(matches!(
            second.submit(altered_replay).await,
            Applied::Rejected(_)
        ));
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
