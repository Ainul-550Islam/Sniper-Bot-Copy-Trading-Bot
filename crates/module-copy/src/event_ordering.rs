//! Per-leader event ordering (TASK 3 §04).
//!
//! Feeds do not promise order: a websocket backlog replays old slots after
//! new ones, the poller pages newest-first, two feeds race. The tracker keeps
//! one cursor per leader — the highest slot processed and the signature that
//! set it — and classifies every incoming event:
//!
//! | verdict | meaning | default action | `strict_ordering` |
//! |---|---|---|---|
//! | `InOrder` | slot above the cursor | advance, process | same |
//! | `SameSlot` | same slot, other signature (intra-slot order unknown) | process | same |
//! | `UnknownSlot` | slot `0` (source had none) | process, no advance | same |
//! | `OutOfOrder` | slot below the cursor | process (late) | **reject `OUT_OF_ORDER`** |
//!
//! Sequence gaps are reported separately: when a source numbers its
//! deliveries and the number jumps, the gap size is surfaced (metric +
//! audit) but never blocks — a gap means "we may have missed a trade", and
//! the right response is reconciliation, not dropping what did arrive.
//!
//! The tracker is pure data; it is seeded from the durable leader rows at
//! startup so a restart does not treat a replayed backlog as new.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event::{EventSource, LeaderTradeEvent};

/// Ordering classification of one event relative to its leader's cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingVerdict {
    /// Slot above the cursor.
    InOrder,
    /// Same slot as the cursor, different signature.
    SameSlot,
    /// The source carried no slot.
    UnknownSlot,
    /// Slot below the cursor.
    OutOfOrder {
        /// Cursor slot at the time.
        newest_slot: u64,
        /// `newest_slot - event.slot`.
        behind_by: u64,
    },
}

impl OrderingVerdict {
    /// Metric label.
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderingVerdict::InOrder => "in_order",
            OrderingVerdict::SameSlot => "same_slot",
            OrderingVerdict::UnknownSlot => "unknown_slot",
            OrderingVerdict::OutOfOrder { .. } => "out_of_order",
        }
    }

    /// Whether the event is behind the cursor.
    pub fn is_out_of_order(&self) -> bool {
        matches!(self, OrderingVerdict::OutOfOrder { .. })
    }
}

/// What the tracker observed for one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingReport {
    /// Ordering relative to the leader cursor.
    pub verdict: OrderingVerdict,
    /// Missing deliveries between the previous and this sequence number
    /// from the same source (`0` = contiguous or unnumbered).
    pub sequence_gap: u64,
    /// Whether the event should be refused (`strict` and out of order).
    pub reject: bool,
}

/// Per-leader high-water mark.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderCursor {
    /// Highest slot processed (`0` = none yet).
    pub last_slot: u64,
    /// Signature that set `last_slot`.
    pub last_signature: String,
    /// Observation time of the newest event.
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Last delivery number seen per source.
    pub last_sequence: HashMap<EventSource, u64>,
    /// Events observed for this leader.
    pub observed: u64,
    /// Events that were behind the cursor.
    pub out_of_order: u64,
    /// Sum of sequence gaps.
    pub gaps: u64,
}

/// Ordering tracker for every leader.
#[derive(Debug, Default)]
pub struct OrderingTracker {
    cursors: HashMap<String, LeaderCursor>,
    strict: bool,
}

impl OrderingTracker {
    /// `strict` = refuse out-of-order events (`copy.strict_ordering`).
    pub fn new(strict: bool) -> Self {
        OrderingTracker {
            cursors: HashMap::new(),
            strict,
        }
    }

    /// Whether out-of-order events are refused.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Flip strictness (config hot reload).
    pub fn set_strict(&mut self, strict: bool) {
        self.strict = strict;
    }

    /// Cursor of a leader, if any event was observed.
    pub fn cursor(&self, leader: &str) -> Option<&LeaderCursor> {
        self.cursors.get(leader.trim())
    }

    /// Number of leaders with a cursor.
    pub fn len(&self) -> usize {
        self.cursors.len()
    }

    /// Whether no cursor exists.
    pub fn is_empty(&self) -> bool {
        self.cursors.is_empty()
    }

    /// Seed a cursor from durable state (restart). Only raises the mark.
    pub fn seed(&mut self, leader: &str, slot: u64, signature: &str) {
        let c = self.cursors.entry(leader.trim().to_string()).or_default();
        if slot > c.last_slot {
            c.last_slot = slot;
            c.last_signature = signature.to_string();
        }
    }

    /// Drop a leader's cursor (unfollowed).
    pub fn forget(&mut self, leader: &str) {
        self.cursors.remove(leader.trim());
    }

    /// Classify `event` and advance the leader's cursor. In strict mode an
    /// out-of-order event does not advance anything and is flagged for
    /// rejection; otherwise it is counted and processed late.
    pub fn observe(&mut self, event: &LeaderTradeEvent) -> OrderingReport {
        let c = self
            .cursors
            .entry(event.leader.trim().to_string())
            .or_default();
        c.observed = c.observed.saturating_add(1);
        if c.last_seen_at
            .map(|t| event.observed_at > t)
            .unwrap_or(true)
        {
            c.last_seen_at = Some(event.observed_at);
        }

        let sequence_gap = if event.source_sequence > 0 {
            let gap = match c.last_sequence.get(&event.source) {
                Some(prev) if event.source_sequence > prev + 1 => event.source_sequence - prev - 1,
                _ => 0,
            };
            let entry = c.last_sequence.entry(event.source).or_insert(0);
            if event.source_sequence > *entry {
                *entry = event.source_sequence;
            }
            gap
        } else {
            0
        };
        c.gaps = c.gaps.saturating_add(sequence_gap);

        let verdict = if event.slot == 0 {
            OrderingVerdict::UnknownSlot
        } else if c.last_slot == 0 || event.slot > c.last_slot {
            c.last_slot = event.slot;
            c.last_signature = event.signature.clone();
            OrderingVerdict::InOrder
        } else if event.slot == c.last_slot {
            OrderingVerdict::SameSlot
        } else {
            c.out_of_order = c.out_of_order.saturating_add(1);
            OrderingVerdict::OutOfOrder {
                newest_slot: c.last_slot,
                behind_by: c.last_slot - event.slot,
            }
        };
        OrderingReport {
            verdict,
            sequence_gap,
            reject: self.strict && verdict.is_out_of_order(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::{PositionSide, Venue, WalletTrade};

    fn ev(leader: &str, sig: &str, slot: u64, source: EventSource, seq: u64) -> LeaderTradeEvent {
        LeaderTradeEvent::from_wallet_trade(
            &WalletTrade {
                wallet: leader.into(),
                signature: sig.into(),
                slot,
                block_time: None,
                side: PositionSide::Long,
                mint: "mint".into(),
                symbol: None,
                token_amount: 1.0,
                sol_amount: 0.1,
                venue: Venue::PumpFun,
                fee_sol: 0.0,
                discriminator: None,
                observed_at: Utc::now(),
            },
            source,
            seq,
        )
    }

    #[test]
    fn advisory_mode_processes_late_events_but_counts_them() {
        let mut t = OrderingTracker::new(false);
        let r = t.observe(&ev("A", "s1", 100, EventSource::PumpPortal, 0));
        assert_eq!(r.verdict, OrderingVerdict::InOrder);
        assert!(!r.reject);
        let r = t.observe(&ev("A", "s2", 100, EventSource::PumpPortal, 0));
        assert_eq!(r.verdict, OrderingVerdict::SameSlot);
        let r = t.observe(&ev("A", "s0", 90, EventSource::PumpPortal, 0));
        assert_eq!(
            r.verdict,
            OrderingVerdict::OutOfOrder {
                newest_slot: 100,
                behind_by: 10
            }
        );
        assert!(!r.reject);
        let c = t.cursor("A").unwrap();
        assert_eq!(c.last_slot, 100);
        assert_eq!(c.last_signature, "s1");
        assert_eq!(c.out_of_order, 1);
        assert_eq!(c.observed, 3);
        let r = t.observe(&ev("A", "s3", 0, EventSource::PumpPortal, 0));
        assert_eq!(r.verdict, OrderingVerdict::UnknownSlot);
        assert_eq!(
            t.cursor("A").unwrap().last_slot,
            100,
            "unknown slot never moves the cursor"
        );
    }

    #[test]
    fn strict_mode_rejects_out_of_order_only() {
        let mut t = OrderingTracker::new(true);
        assert!(t.is_strict());
        t.observe(&ev("A", "s1", 100, EventSource::LogsPoll, 0));
        assert!(
            t.observe(&ev("A", "s0", 99, EventSource::LogsPoll, 0))
                .reject
        );
        assert!(
            !t.observe(&ev("A", "s2", 100, EventSource::LogsPoll, 0))
                .reject
        );
        assert!(
            !t.observe(&ev("A", "s3", 101, EventSource::LogsPoll, 0))
                .reject
        );
        assert!(
            !t.observe(&ev("A", "s4", 0, EventSource::LogsPoll, 0))
                .reject
        );
        t.set_strict(false);
        assert!(
            !t.observe(&ev("A", "s5", 50, EventSource::LogsPoll, 0))
                .reject
        );
    }

    #[test]
    fn leaders_have_independent_cursors() {
        let mut t = OrderingTracker::new(true);
        t.observe(&ev("A", "a1", 500, EventSource::PumpPortal, 0));
        let r = t.observe(&ev("B", "b1", 10, EventSource::PumpPortal, 0));
        assert_eq!(
            r.verdict,
            OrderingVerdict::InOrder,
            "B's first event is in order"
        );
        assert_eq!(t.len(), 2);
        t.forget("A");
        assert!(t.cursor("A").is_none());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn sequence_gaps_are_per_source_and_never_reject() {
        let mut t = OrderingTracker::new(true);
        assert_eq!(
            t.observe(&ev("A", "s1", 1, EventSource::PumpPortal, 1))
                .sequence_gap,
            0
        );
        assert_eq!(
            t.observe(&ev("A", "s2", 2, EventSource::PumpPortal, 2))
                .sequence_gap,
            0
        );
        let r = t.observe(&ev("A", "s5", 3, EventSource::PumpPortal, 5));
        assert_eq!(r.sequence_gap, 2);
        assert!(!r.reject);
        // Another source has its own numbering.
        assert_eq!(
            t.observe(&ev("A", "s6", 4, EventSource::LogsPoll, 40))
                .sequence_gap,
            0
        );
        assert_eq!(
            t.observe(&ev("A", "s7", 5, EventSource::LogsPoll, 41))
                .sequence_gap,
            0
        );
        // A late redelivery with a lower number is not a gap.
        assert_eq!(
            t.observe(&ev("A", "s8", 6, EventSource::PumpPortal, 4))
                .sequence_gap,
            0
        );
        assert_eq!(t.cursor("A").unwrap().gaps, 2);
        assert_eq!(
            t.cursor("A").unwrap().last_sequence[&EventSource::PumpPortal],
            5
        );
    }

    #[test]
    fn seed_only_raises_the_mark() {
        let mut t = OrderingTracker::new(true);
        t.seed("A", 100, "sA");
        t.seed("A", 50, "old");
        assert_eq!(t.cursor("A").unwrap().last_slot, 100);
        assert_eq!(t.cursor("A").unwrap().last_signature, "sA");
        assert!(
            t.observe(&ev("A", "s", 99, EventSource::PumpPortal, 0))
                .reject
        );
        assert!(
            !t.observe(&ev("A", "s", 101, EventSource::PumpPortal, 0))
                .reject
        );
    }
}
