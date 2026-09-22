//! Durable journal for the Polymarket engine (migration 0014).
//!
//! The trait is implemented by the server against Postgres
//! (`DbPolyStore`) and by [`MemoryPolyStore`] for tests / no-DB runs.
//! Writes answer `bool` (success); reads answer `Option` (`None` = the store
//! could not answer, which is NOT the same as "nothing there").
//!
//! The record types are the migration-0014 rows re-exported from
//! [`bot_core::db::polymarket`]; the engine's order-snapshot write path
//! ([`PolyBot::journal_order`]) lives next to the contract it writes to.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

pub use bot_core::db::polymarket::{
    PolyFillRecord, PolyOrderRecord, PolyReconFindingRecord, PolySignalRecord,
};

use crate::metrics;
use crate::orders::TrackedOrder;
use crate::PolyBot;

/// Journal contract (see module docs).
#[async_trait]
pub trait PolyStore: Send + Sync {
    /// Upsert the outcome of one signal.
    async fn record_signal(&self, rec: PolySignalRecord) -> bool;
    /// Upsert one venue order's lifecycle snapshot.
    async fn upsert_order(&self, rec: PolyOrderRecord) -> bool;
    /// Every non-terminal venue order (`None` = store unavailable).
    async fn open_orders(&self) -> Option<Vec<PolyOrderRecord>>;
    /// Guarded fill insert: `Some(true)` new, `Some(false)` already
    /// booked, `None` store unavailable.
    async fn record_fill(&self, rec: PolyFillRecord) -> Option<bool>;
    /// Append one reconciliation finding.
    async fn append_finding(&self, rec: PolyReconFindingRecord) -> bool;
}

/// In-memory journal (tests, single-process runs without a database).
#[derive(Default)]
pub struct MemoryPolyStore {
    signals: RwLock<HashMap<String, PolySignalRecord>>,
    orders: RwLock<HashMap<String, PolyOrderRecord>>,
    fills: RwLock<HashMap<String, PolyFillRecord>>,
    findings: RwLock<Vec<PolyReconFindingRecord>>,
}

impl MemoryPolyStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
    /// Snapshot of every journaled signal.
    pub async fn signals(&self) -> Vec<PolySignalRecord> {
        self.signals.read().await.values().cloned().collect()
    }
    /// Snapshot of every journaled venue order.
    pub async fn orders(&self) -> Vec<PolyOrderRecord> {
        self.orders.read().await.values().cloned().collect()
    }
    /// Snapshot of every booked fill.
    pub async fn fills(&self) -> Vec<PolyFillRecord> {
        self.fills.read().await.values().cloned().collect()
    }
    /// Snapshot of every reconciliation finding.
    pub async fn findings(&self) -> Vec<PolyReconFindingRecord> {
        self.findings.read().await.clone()
    }
    /// Seed an order row (crash-recovery tests).
    pub async fn seed_order(&self, rec: PolyOrderRecord) {
        self.orders
            .write()
            .await
            .insert(rec.venue_order_id.clone(), rec);
    }
}

#[async_trait]
impl PolyStore for MemoryPolyStore {
    async fn record_signal(&self, rec: PolySignalRecord) -> bool {
        let mut m = self.signals.write().await;
        match m.get_mut(&rec.signal_id) {
            Some(existing) => {
                let created = existing.created_at;
                let order_id = rec.order_id.clone().or(existing.order_id.clone());
                let venue = rec
                    .venue_order_id
                    .clone()
                    .or(existing.venue_order_id.clone());
                let position = rec.position_id.clone().or(existing.position_id.clone());
                *existing = rec;
                existing.created_at = created;
                existing.order_id = order_id;
                existing.venue_order_id = venue;
                existing.position_id = position;
            }
            None => {
                m.insert(rec.signal_id.clone(), rec);
            }
        }
        true
    }

    async fn upsert_order(&self, rec: PolyOrderRecord) -> bool {
        let mut m = self.orders.write().await;
        match m.get_mut(&rec.venue_order_id) {
            Some(existing) => {
                let submitted = existing.submitted_at;
                let matched = existing.size_matched.max(rec.size_matched);
                let position = rec.position_id.clone().or(existing.position_id.clone());
                let closed = rec.closed_at.or(existing.closed_at);
                *existing = rec;
                existing.submitted_at = submitted;
                existing.size_matched = matched;
                existing.position_id = position;
                existing.closed_at = closed;
            }
            None => {
                m.insert(rec.venue_order_id.clone(), rec);
            }
        }
        true
    }

    async fn open_orders(&self) -> Option<Vec<PolyOrderRecord>> {
        let mut out: Vec<PolyOrderRecord> = self
            .orders
            .read()
            .await
            .values()
            .filter(|o| o.closed_at.is_none())
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.submitted_at
                .cmp(&b.submitted_at)
                .then_with(|| a.venue_order_id.cmp(&b.venue_order_id))
        });
        Some(out)
    }

    async fn record_fill(&self, rec: PolyFillRecord) -> Option<bool> {
        let mut m = self.fills.write().await;
        if m.contains_key(&rec.fill_id) {
            return Some(false);
        }
        m.insert(rec.fill_id.clone(), rec);
        Some(true)
    }

    async fn append_finding(&self, rec: PolyReconFindingRecord) -> bool {
        self.findings.write().await.push(rec);
        true
    }
}

impl PolyBot {
    /// Upsert the journal snapshot of one tracked venue order. Called
    /// write-ahead (before the POST) and after every observation /
    /// local transition; a failed write is metered, never fatal.
    pub(crate) async fn journal_order(&self, t: &TrackedOrder) {
        let rec = PolyOrderRecord {
            venue_order_id: t.venue_order_id.clone(),
            order_id: t.order_id.clone(),
            signal_id: t.signal_id.clone(),
            condition_id: t.condition_id.clone(),
            token_id: t.token_id.clone(),
            outcome: t.outcome.clone(),
            side: if t.is_buy { "buy" } else { "sell" }.into(),
            order_type: t.order_type.clone(),
            limit_price: t.limit_price,
            size_tokens: t.size_tokens,
            size_matched: t.size_matched,
            mode: t.mode.as_str().to_string(),
            state: t.state.as_str().to_string(),
            venue_status: t.venue_status.clone(),
            expiration: t.expiration.min(i64::MAX as u64) as i64,
            position_id: t.position_id.clone(),
            replica_id: self.state.replica_id().to_string(),
            submitted_at: t.submitted_at,
            updated_at: t.updated_at,
            closed_at: if t.state.is_terminal() {
                Some(t.updated_at)
            } else {
                None
            },
        };
        if !self.store.upsert_order(rec).await {
            metrics::count_journal_error("upsert_order");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn memory_store_keeps_first_seen_and_dedups_fills() {
        let s = MemoryPolyStore::new();
        let t0 = Utc::now() - chrono::Duration::minutes(5);
        let sig = PolySignalRecord {
            signal_id: "psig_a".into(),
            condition_id: "0xc".into(),
            token_id: "1".into(),
            outcome: "Yes".into(),
            side: "buy".into(),
            strategy: "value".into(),
            limit_price: 0.4,
            size_tokens: 10.0,
            stake_usd: 4.0,
            mode: "paper".into(),
            stage: "RISK_APPROVED".into(),
            reject_reason: None,
            detail: String::new(),
            order_id: Some("ord_1".into()),
            venue_order_id: None,
            position_id: None,
            created_at: t0,
            updated_at: t0,
        };
        assert!(s.record_signal(sig.clone()).await);
        let mut later = sig.clone();
        later.stage = "FILLED".into();
        later.order_id = None;
        later.venue_order_id = Some("paper:x".into());
        later.created_at = Utc::now();
        assert!(s.record_signal(later).await);
        let got = s.signals().await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stage, "FILLED");
        assert_eq!(got[0].created_at, t0);
        assert_eq!(got[0].order_id.as_deref(), Some("ord_1"), "ids coalesce");
        assert_eq!(got[0].venue_order_id.as_deref(), Some("paper:x"));

        let order = PolyOrderRecord {
            venue_order_id: "0xo".into(),
            order_id: "ord_1".into(),
            signal_id: "psig_a".into(),
            condition_id: "0xc".into(),
            token_id: "1".into(),
            outcome: "Yes".into(),
            side: "buy".into(),
            order_type: "GTC".into(),
            limit_price: 0.4,
            size_tokens: 10.0,
            size_matched: 4.0,
            mode: "live".into(),
            state: "partially_filled".into(),
            venue_status: "live".into(),
            expiration: 0,
            position_id: None,
            replica_id: "r".into(),
            submitted_at: t0,
            updated_at: t0,
            closed_at: None,
        };
        assert!(s.upsert_order(order.clone()).await);
        let mut stale = order.clone();
        stale.size_matched = 1.0;
        stale.submitted_at = Utc::now();
        assert!(s.upsert_order(stale).await);
        let o = &s.orders().await[0];
        assert_eq!(o.size_matched, 4.0, "matched never regresses");
        assert_eq!(o.submitted_at, t0, "submitted_at keeps the first time");
        assert_eq!(s.open_orders().await.unwrap().len(), 1);
        let mut done = order.clone();
        done.state = "filled".into();
        done.closed_at = Some(Utc::now());
        assert!(s.upsert_order(done).await);
        assert!(s.open_orders().await.unwrap().is_empty());

        let fill = PolyFillRecord {
            fill_id: "trade:1".into(),
            venue_order_id: "0xo".into(),
            order_id: "ord_1".into(),
            token_id: "1".into(),
            side: "buy".into(),
            price: 0.4,
            size_tokens: 4.0,
            quote_usd: 1.6,
            source: "poll".into(),
            position_id: None,
            ts: Utc::now(),
        };
        assert_eq!(s.record_fill(fill.clone()).await, Some(true));
        assert_eq!(s.record_fill(fill).await, Some(false));
        assert_eq!(s.fills().await.len(), 1);
        assert!(
            s.append_finding(PolyReconFindingRecord {
                id: 0,
                kind: "stale_order".into(),
                venue_order_id: Some("0xo".into()),
                order_id: None,
                token_id: None,
                detail: String::new(),
                action: "reported".into(),
                replica_id: "r".into(),
                ts: Utc::now(),
            })
            .await
        );
        assert_eq!(s.findings().await.len(), 1);
    }
}
