//! Async-acceptance backfill coordinator (PROMPT 4/10 §D).
//!
//! The async commit pipeline splits acceptance from settlement. This
//! module owns the gap: it remembers which acceptances still owe us
//! settlement facts (missing hashes, or a `delayed` match that has not
//! happened yet), and resolves them from the venue when asked —
//! turning [`PendingAcceptance`] records into
//! [`BackfillVerdict`]s using ONLY what `GET /data/trades` and
//! `GET /data/order` actually say.
//!
//! Invariants:
//!
//! * fills are only ever built from venue trade records — a
//!   `CONFIRMED` trade with a published hash; a `FAILED` trade is
//!   reported as failed and never becomes a fill;
//! * `delayed`/still-settling orders stay pending — a timeout or an
//!   empty poll round is "keep waiting", never a synthesized verdict
//!   (the official client's own wait helper documents exactly this:
//!   a timeout does not undo executed trades);
//! * the registry is bounded: a stuck pending order cannot grow the
//!   book indefinitely (oldest entries are evicted and surfaced via
//!   [`PendingAcceptance::stale_after`] for reconciliation).
//!
//! The registry is deliberately in-memory per engine: the durable
//! truth for orders is the existing OMS + journal (`store.rs`,
//! migration 0014); this is the working set the async pipeline needs
//! between acceptance and settlement.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::async_commit::{AsyncOrderAcceptance, CommitState};
use crate::clob::ClobClient;
use crate::error::PolyResult;
use crate::trade_resolution::{
    resolve_trade_ids, wait_for_settlement, ResolvedTrade, SettlementOutcome, TradeSettlement,
};

/// How many pending acceptances the registry retains before evicting
/// the oldest (each is tiny; this is a safety valve, not a policy).
pub const PENDING_REGISTRY_CAP: usize = 4_096;

/// A pending order the venue accepted but whose settlement facts are
/// still open.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAcceptance {
    /// The venue order id.
    pub order_id: String,
    /// The trade ids the acceptance reported (empty for `delayed`).
    pub trade_ids: Vec<String>,
    /// The hashes the acceptance reported (may be empty for a match).
    pub transactions_hashes: Vec<String>,
    /// The acceptance's commit state when recorded.
    pub commit_state: CommitState,
    /// When the order was submitted (for staleness accounting).
    pub submitted_at: DateTime<Utc>,
    /// How old a pending order may get before reconciliation must
    /// look at it specifically.
    pub stale_after: chrono::Duration,
}

impl PendingAcceptance {
    /// Whether this pending order has outlived its staleness budget.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        now - self.submitted_at >= self.stale_after
    }

    /// Build from a parsed acceptance. Only acceptances that
    /// `requires_backfill()` belong here — the caller enforces that
    /// via [`AsyncPendingRegistry::record`].
    pub fn from_acceptance(
        acceptance: &AsyncOrderAcceptance,
        submitted_at: DateTime<Utc>,
        stale_after: chrono::Duration,
    ) -> PolyResult<PendingAcceptance> {
        let order_id = acceptance.order_id.clone().ok_or_else(|| {
            crate::error::PolyError::invalid("pending acceptance has no venue order id")
        })?;
        Ok(PendingAcceptance {
            order_id,
            trade_ids: acceptance.trade_ids.clone(),
            transactions_hashes: acceptance.transactions_hashes.clone(),
            commit_state: acceptance.commit_state(),
            submitted_at,
            stale_after,
        })
    }
}

/// One venue-confirmed fill, as backfilled from a trade record.
#[derive(Debug, Clone, PartialEq)]
pub struct BackfilledFill {
    /// The venue order id.
    pub order_id: String,
    /// The venue trade id.
    pub trade_id: String,
    /// Fill size in outcome tokens (human units).
    pub size: f64,
    /// Fill price (human units).
    pub price: f64,
    /// `BUY` | `SELL`.
    pub side: String,
    /// The outcome asset id.
    pub asset_id: String,
    /// The settlement transaction hash (venue-published).
    pub transaction_hash: String,
    /// Bucket index for multi-transaction settlements.
    pub bucket_index: Option<i64>,
}

impl BackfilledFill {
    /// Build from a resolved trade; `None` unless the trade is
    /// CONFIRMED with a published hash (a confirmed trade without a
    /// hash stays pending — we do not invent one).
    fn from_resolved(trade: &ResolvedTrade) -> Option<BackfilledFill> {
        match &trade.settlement {
            TradeSettlement::Confirmed {
                transaction_hash,
                bucket_index,
            } if !transaction_hash.trim().is_empty() => Some(BackfilledFill {
                order_id: trade.order_id.clone(),
                trade_id: trade.trade_id.clone(),
                size: trade.size,
                price: trade.price,
                side: trade.side.clone(),
                asset_id: trade.asset_id.clone(),
                transaction_hash: transaction_hash.clone(),
                bucket_index: *bucket_index,
            }),
            _ => None,
        }
    }
}

/// The verdict of one backfill pass over a pending acceptance.
#[derive(Debug, Clone, PartialEq)]
pub enum BackfillVerdict {
    /// Every trade the venue attributes to the order is terminal.
    /// `fills` carries only CONFIRMED-with-hash trades; `failed`
    /// carries FAILED ones (reported, never booked as fills).
    Settled {
        /// Confirmed fills.
        fills: Vec<BackfilledFill>,
        /// Terminal failures with their venue status.
        failed: Vec<(String, String)>,
    },
    /// Settlement is still in flight (or the match is still delayed).
    /// The pending record must be KEPT.
    StillPending {
        /// Everything known this round.
        trades: Vec<ResolvedTrade>,
    },
    /// The venue does not know the order (404 / terminal-cancelled):
    /// there will never be trades; drop the pending record and let
    /// reconciliation handle the local order state.
    OrderGoneOnVenue,
}

/// Bounded in-memory registry of acceptances awaiting settlement
/// facts.
#[derive(Debug, Default)]
pub struct AsyncPendingRegistry {
    pending: RwLock<HashMap<String, PendingAcceptance>>,
}

impl AsyncPendingRegistry {
    /// New empty registry.
    pub fn new() -> AsyncPendingRegistry {
        AsyncPendingRegistry::default()
    }

    /// Record an acceptance IF it still owes settlement facts
    /// (matched-without-hashes or delayed). Complete acceptances
    /// (settled / resting / rejected) are not stored — `false` is
    /// returned for them, `true` when recorded.
    pub fn record(
        &self,
        acceptance: &AsyncOrderAcceptance,
        submitted_at: DateTime<Utc>,
        stale_after: chrono::Duration,
    ) -> PolyResult<bool> {
        if !acceptance.requires_backfill() {
            return Ok(false);
        }
        let entry = PendingAcceptance::from_acceptance(acceptance, submitted_at, stale_after)?;
        let mut pending = self
            .pending
            .write()
            .map_err(|_| crate::error::PolyError::invalid("pending registry lock poisoned"))?;
        if pending.len() >= PENDING_REGISTRY_CAP && !pending.contains_key(&entry.order_id) {
            // Evict the OLDEST entry (safety valve; the durable order
            // state lives in the OMS/journal, so nothing is lost —
            // reconciliation still sees the order).
            if let Some(oldest) = pending
                .values()
                .min_by_key(|p| p.submitted_at)
                .map(|p| p.order_id.clone())
            {
                pending.remove(&oldest);
            }
        }
        pending.insert(entry.order_id.clone(), entry);
        Ok(true)
    }

    /// Every pending acceptance, oldest first.
    pub fn pending(&self) -> Vec<PendingAcceptance> {
        let Ok(pending) = self.pending.read() else {
            return Vec::new();
        };
        let mut out: Vec<PendingAcceptance> = pending.values().cloned().collect();
        out.sort_by_key(|p| p.submitted_at);
        out
    }

    /// Pending entries that have outlived their staleness budget.
    pub fn stale(&self, now: DateTime<Utc>) -> Vec<PendingAcceptance> {
        self.pending()
            .into_iter()
            .filter(|p| p.is_stale(now))
            .collect()
    }

    /// Drop one order from the registry (after a terminal verdict).
    pub fn remove(&self, order_id: &str) {
        if let Ok(mut pending) = self.pending.write() {
            pending.remove(order_id);
        }
    }

    /// Current registry size.
    pub fn len(&self) -> usize {
        self.pending.read().map(|p| p.len()).unwrap_or(0)
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Backfill ONE pending acceptance: wait (bounded) for its trades to
/// reach terminal states and classify the result.
///
/// For a `delayed` acceptance (no trade ids yet) this first re-reads
/// the venue order: a delayed order that has since matched will show
/// trades for our order id; one still delayed stays pending; one the
/// venue no longer knows is `OrderGoneOnVenue`.
pub async fn backfill_order(
    client: &ClobClient,
    pending: &PendingAcceptance,
    timeout: Duration,
    interval: Duration,
) -> PolyResult<BackfillVerdict> {
    let outcome = wait_for_settlement(
        client,
        &pending.order_id,
        &pending.trade_ids,
        timeout,
        interval,
    )
    .await?;
    match outcome {
        SettlementOutcome::OrderUnknownOnVenue => Ok(BackfillVerdict::OrderGoneOnVenue),
        SettlementOutcome::StillSettling { trades } => Ok(BackfillVerdict::StillPending { trades }),
        SettlementOutcome::AllTerminal { trades } => {
            let mut fills = Vec::new();
            let mut failed = Vec::new();
            for trade in &trades {
                match &trade.settlement {
                    TradeSettlement::Confirmed { .. } => {
                        if let Some(fill) = BackfilledFill::from_resolved(trade) {
                            fills.push(fill);
                        } else {
                            // A CONFIRMED trade without a published
                            // hash cannot be a booked fill yet — but
                            // it IS terminal, so it must not stay
                            // pending either: report it as a
                            // (trade_id, status) failure-of-information
                            // so the caller can journal it rather than
                            // silently dropping it.
                            failed.push((
                                trade.trade_id.clone(),
                                "confirmed_without_hash".to_string(),
                            ));
                        }
                    }
                    TradeSettlement::Failed { status } => {
                        failed.push((trade.trade_id.clone(), status.clone()));
                    }
                    // AllTerminal guarantees terminal states only;
                    // the remaining arms are unreachable-by-contract.
                    _ => {}
                }
            }
            Ok(BackfillVerdict::Settled { fills, failed })
        }
    }
}

/// One polling round WITHOUT waiting (used by periodic
/// reconciliation): resolve the current venue state for a pending
/// acceptance and classify it.
pub async fn poll_once(
    client: &ClobClient,
    pending: &PendingAcceptance,
) -> PolyResult<BackfillVerdict> {
    let order = client.order(&pending.order_id).await?;
    let Some(order) = order else {
        return Ok(BackfillVerdict::OrderGoneOnVenue);
    };
    let s = order.status.trim();
    if s.eq_ignore_ascii_case("unmatched")
        || s.eq_ignore_ascii_case("canceled")
        || s.eq_ignore_ascii_case("cancelled")
        || s.eq_ignore_ascii_case("expired")
    {
        return Ok(BackfillVerdict::OrderGoneOnVenue);
    }
    let trades = resolve_trade_ids(client, &pending.order_id, &pending.trade_ids).await?;
    let all_terminal = !trades.is_empty() && trades.iter().all(|t| t.settlement.is_terminal());
    if all_terminal {
        let mut fills = Vec::new();
        let mut failed = Vec::new();
        for trade in &trades {
            match &trade.settlement {
                TradeSettlement::Confirmed { .. } => {
                    if let Some(fill) = BackfilledFill::from_resolved(trade) {
                        fills.push(fill);
                    } else {
                        failed.push((trade.trade_id.clone(), "confirmed_without_hash".to_string()));
                    }
                }
                TradeSettlement::Failed { status } => {
                    failed.push((trade.trade_id.clone(), status.clone()));
                }
                _ => {}
            }
        }
        Ok(BackfillVerdict::Settled { fills, failed })
    } else {
        Ok(BackfillVerdict::StillPending { trades })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_commit::{AcceptanceStatus, AsyncOrderAcceptance};

    fn acceptance(
        order_id: &str,
        status: AcceptanceStatus,
        trade_ids: Vec<String>,
        hashes: Vec<String>,
    ) -> AsyncOrderAcceptance {
        AsyncOrderAcceptance {
            order_id: Some(order_id.to_string()),
            status,
            success: Some(true),
            error_msg: None,
            making_amount: Some(0),
            taking_amount: Some(0),
            trade_ids,
            transactions_hashes: hashes,
        }
    }

    #[test]
    fn only_incomplete_acceptances_are_recorded() {
        let reg = AsyncPendingRegistry::new();
        let now = Utc::now();
        let stale = chrono::Duration::seconds(600);

        // settled: nothing to backfill.
        let settled = acceptance(
            "0xa",
            AcceptanceStatus::Matched,
            vec!["t1".into()],
            vec!["0xh".into()],
        );
        assert!(!reg.record(&settled, now, stale).unwrap());

        // resting: nothing to backfill.
        let resting = acceptance("0xb", AcceptanceStatus::Live, vec![], vec![]);
        assert!(!reg.record(&resting, now, stale).unwrap());

        // matched-without-hashes and delayed: both recorded.
        let awaiting = acceptance("0xc", AcceptanceStatus::Matched, vec!["t2".into()], vec![]);
        assert!(reg.record(&awaiting, now, stale).unwrap());
        let delayed = acceptance("0xd", AcceptanceStatus::Delayed, vec![], vec![]);
        assert!(reg.record(&delayed, now, stale).unwrap());

        assert_eq!(reg.len(), 2);
        let pending_now = reg.pending();
        let ids: Vec<&str> = pending_now.iter().map(|p| p.order_id.as_str()).collect();
        assert!(ids.contains(&"0xc") && ids.contains(&"0xd"));
    }

    #[test]
    fn staleness_is_measured_from_submission() {
        let reg = AsyncPendingRegistry::new();
        let submitted = Utc::now() - chrono::Duration::seconds(700);
        let delayed = acceptance("0xd", AcceptanceStatus::Delayed, vec![], vec![]);
        reg.record(&delayed, submitted, chrono::Duration::seconds(600))
            .unwrap();
        assert_eq!(reg.stale(Utc::now()).len(), 1);
        assert!(reg.pending()[0].is_stale(Utc::now()));
    }

    #[test]
    fn removal_drops_the_entry() {
        let reg = AsyncPendingRegistry::new();
        let delayed = acceptance("0xd", AcceptanceStatus::Delayed, vec![], vec![]);
        reg.record(&delayed, Utc::now(), chrono::Duration::seconds(600))
            .unwrap();
        assert_eq!(reg.len(), 1);
        reg.remove("0xd");
        assert!(reg.is_empty());
    }

    #[test]
    fn an_acceptance_without_an_order_id_cannot_be_pending() {
        let mut a = acceptance("0xd", AcceptanceStatus::Delayed, vec![], vec![]);
        a.order_id = None;
        let err =
            PendingAcceptance::from_acceptance(&a, Utc::now(), chrono::Duration::seconds(600));
        assert!(err.is_err());
    }

    #[test]
    fn registry_cap_evicts_the_oldest() {
        let reg = AsyncPendingRegistry::new();
        let stale = chrono::Duration::seconds(600);
        let base = Utc::now() - chrono::Duration::seconds(10_000);
        for i in 0..PENDING_REGISTRY_CAP {
            let a = acceptance(&format!("0x{i}"), AcceptanceStatus::Delayed, vec![], vec![]);
            reg.record(&a, base + chrono::Duration::seconds(i as i64), stale)
                .unwrap();
        }
        assert_eq!(reg.len(), PENDING_REGISTRY_CAP);
        // one more — the oldest (0x0) is evicted.
        let a = acceptance("0xnew", AcceptanceStatus::Delayed, vec![], vec![]);
        reg.record(&a, Utc::now(), stale).unwrap();
        assert_eq!(reg.len(), PENDING_REGISTRY_CAP);
        let pending_now = reg.pending();
        let ids: Vec<String> = pending_now.into_iter().map(|p| p.order_id).collect();
        assert!(!ids.contains(&"0x0".to_string()));
        assert!(ids.contains(&"0xnew".to_string()));
    }
}
