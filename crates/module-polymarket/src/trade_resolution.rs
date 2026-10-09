//! Trade-id and settlement-hash resolution (PROMPT 4/10 §D).
//!
//! When the async commit pipeline hands back an acceptance whose
//! settlement is still open (matched without hashes, or delayed), the
//! missing facts come from the venue's own records:
//!
//! * `GET /data/trades` — per-trade status with the settlement
//!   transaction hash once it exists;
//! * `GET /data/order` — the order's current venue status.
//!
//! Official trade lifecycle (terminal in bold):
//! `MATCHED` (sent to the executor) → `MINED` (seen on-chain, no
//! finality threshold) → **`CONFIRMED`** (final, successful);
//! `RETRYING` (revert/reorg, operator resubmits); **`FAILED`**
//! (terminal, not retried). A trade may settle across **multiple
//! transactions** (`bucket_index`), each with its own hash.
//!
//! This module maps venue trades to [`TradeSettlement`] states and
//! implements the official client's wait semantics: poll until every
//! requested trade id reaches a terminal state, with a timeout that
//! reports "still settling" — a timeout is NOT a failure and never
//! undoes what actually executed.
//!
//! Nothing here fabricates: a trade the venue has not shown stays
//! [`TradeSettlement::Unseen`], a hash the venue has not published
//! stays `None`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::clob::{ClobClient, ClobOrder, ClobTrade};
use crate::error::{PolyError, PolyResult};

/// The settlement state of ONE venue trade id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeSettlement {
    /// The venue has not shown this trade id yet (it may genuinely not
    /// exist — or the list may be lagging; only a terminal state or a
    /// definitive order answer closes the question).
    Unseen,
    /// Matched but not yet on-chain (`MATCHED`).
    Matched,
    /// Seen on-chain, not yet final (`MINED`), or a settlement
    /// transaction failed and is being retried (`RETRYING`).
    Settling {
        /// The venue's status string, verbatim.
        status: String,
    },
    /// Terminal success (`CONFIRMED`).
    Confirmed {
        /// Settlement transaction hash (the venue's own value).
        transaction_hash: String,
        /// Bucket index when the trade settled across multiple
        /// transactions.
        bucket_index: Option<i64>,
    },
    /// Terminal failure (`FAILED`) — must never be booked as a fill.
    Failed {
        /// The venue's status string, verbatim.
        status: String,
    },
}

impl TradeSettlement {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &str {
        match self {
            TradeSettlement::Unseen => "unseen",
            TradeSettlement::Matched => "matched",
            TradeSettlement::Settling { .. } => "settling",
            TradeSettlement::Confirmed { .. } => "confirmed",
            TradeSettlement::Failed { .. } => "failed",
        }
    }

    /// Whether this state is terminal (no further polling will change
    /// it).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TradeSettlement::Confirmed { .. } | TradeSettlement::Failed { .. }
        )
    }

    /// Whether this state represents a successful on-chain settle.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, TradeSettlement::Confirmed { .. })
    }
}

/// Classify one venue trade record into a settlement state.
pub fn classify_trade(trade: &ClobTrade) -> TradeSettlement {
    let status = trade.status.trim().to_string();
    if status.eq_ignore_ascii_case("CONFIRMED") {
        return TradeSettlement::Confirmed {
            transaction_hash: trade.transaction_hash.trim().to_string(),
            bucket_index: trade.bucket_index,
        };
    }
    if status.eq_ignore_ascii_case("FAILED") {
        return TradeSettlement::Failed { status };
    }
    if status.eq_ignore_ascii_case("MATCHED") {
        return TradeSettlement::Matched;
    }
    // MINED, RETRYING and anything unknown-with-a-record are "still
    // settling": the venue has the trade, the outcome is not final.
    TradeSettlement::Settling { status }
}

/// One resolved trade: the venue record's settlement-relevant facts.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTrade {
    /// The venue trade id.
    pub trade_id: String,
    /// The order id the venue attributes this trade to.
    pub order_id: String,
    /// The settlement state.
    pub settlement: TradeSettlement,
    /// Fill size in outcome tokens (human units, from the venue's
    /// decimal string).
    pub size: f64,
    /// Fill price (human units).
    pub price: f64,
    /// The outcome asset id the venue reports.
    pub asset_id: String,
    /// `BUY` | `SELL` from our perspective, verbatim.
    pub side: String,
}

impl ResolvedTrade {
    /// Map one venue trade record.
    pub fn from_trade(trade: &ClobTrade) -> ResolvedTrade {
        ResolvedTrade {
            trade_id: trade.id.clone(),
            order_id: trade.order_id.clone(),
            settlement: classify_trade(trade),
            size: trade.size_f64(),
            price: trade.price_f64(),
            asset_id: trade.asset_id.clone(),
            side: trade.side.clone(),
        }
    }
}

/// Resolve a set of trade ids against the venue's trade list (one
/// polling round — no waiting; callers that want to wait use
/// [`wait_for_settlement`]).
///
/// Trades are matched by id; a trade whose `taker_order_id` matches
/// the given order id but whose id was not requested is ALSO included
/// (the venue can split one acceptance across more trades than the
/// acceptance response listed — `bucket_index` exists for exactly
/// that). Unseen ids stay [`TradeSettlement::Unseen`].
pub async fn resolve_trade_ids(
    client: &ClobClient,
    order_id: &str,
    requested_trade_ids: &[String],
) -> PolyResult<Vec<ResolvedTrade>> {
    let trades = client.trades(None, None).await?;
    Ok(project_trades(&trades, order_id, requested_trade_ids))
}

/// Pure projection of venue trades onto a request — separated from
/// the I/O so tests can drive it without a server.
pub fn project_trades(
    trades: &[ClobTrade],
    order_id: &str,
    requested_trade_ids: &[String],
) -> Vec<ResolvedTrade> {
    let mut requested: HashMap<&str, bool> = HashMap::new();
    for id in requested_trade_ids {
        requested.insert(id.as_str(), false);
    }
    let mut out: Vec<ResolvedTrade> = Vec::new();
    for trade in trades {
        let is_requested = requested.contains_key(trade.id.as_str());
        let belongs_to_order = trade.order_id.trim().eq_ignore_ascii_case(order_id.trim());
        if is_requested || (belongs_to_order && !trade.id.trim().is_empty()) {
            if is_requested {
                requested.insert(trade.id.as_str(), true);
            }
            out.push(ResolvedTrade::from_trade(trade));
        }
    }
    // Requested-but-unseen ids are reported as unseen — the caller can
    // distinguish "venue says nothing yet" from "trade does not exist"
    // by polling the order status alongside.
    for (id, seen) in requested {
        if !seen {
            out.push(ResolvedTrade {
                trade_id: id.to_string(),
                order_id: order_id.to_string(),
                settlement: TradeSettlement::Unseen,
                size: 0.0,
                price: 0.0,
                asset_id: String::new(),
                side: String::new(),
            });
        }
    }
    out
}

/// The outcome of a settlement wait.
#[derive(Debug, Clone, PartialEq)]
pub enum SettlementOutcome {
    /// Every requested trade id reached a terminal state.
    AllTerminal {
        /// The resolved trades (terminal states only).
        trades: Vec<ResolvedTrade>,
    },
    /// The deadline passed with some trades still settling — the
    /// executed trades are unaffected; this is "keep waiting
    /// later", never a failure verdict (official client
    /// semantics: TimeoutError does not undo fills).
    StillSettling {
        /// Everything known at the deadline.
        trades: Vec<ResolvedTrade>,
    },
    /// The venue definitively does not know the order (404/null):
    /// there is nothing to wait for — the acceptance's ids cannot
    /// belong to a live order. Surfaced so callers stop polling.
    OrderUnknownOnVenue,
}

/// Poll the venue until every requested trade id is terminal, the
/// order turns out unknown, or the deadline passes.
///
/// `interval` is the pause between polling rounds (the first round
/// happens immediately). Uses only venue facts: never fabricates a
/// hash, a status, or a trade.
pub async fn wait_for_settlement(
    client: &ClobClient,
    order_id: &str,
    requested_trade_ids: &[String],
    timeout: Duration,
    interval: Duration,
) -> PolyResult<SettlementOutcome> {
    let deadline = Instant::now() + timeout;
    loop {
        // An order the venue does not know settles the question
        // negatively — no amount of further polling will produce its
        // trades.
        match client.order(order_id).await? {
            None => return Ok(SettlementOutcome::OrderUnknownOnVenue),
            Some(order) => {
                if let Some(verdict) = order_unknown_verdict(&order) {
                    return Ok(verdict);
                }
            }
        }
        let trades = resolve_trade_ids(client, order_id, requested_trade_ids).await?;
        let all_terminal = !trades.is_empty() && trades.iter().all(|t| t.settlement.is_terminal());
        if all_terminal {
            return Ok(SettlementOutcome::AllTerminal { trades });
        }
        if Instant::now() >= deadline {
            return Ok(SettlementOutcome::StillSettling { trades });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(remaining.min(interval)).await;
        if Instant::now() >= deadline {
            // One final poll after the last sleep before declaring
            // still-settling keeps the loop from spinning.
            let trades = resolve_trade_ids(client, order_id, requested_trade_ids).await?;
            let all_terminal =
                !trades.is_empty() && trades.iter().all(|t| t.settlement.is_terminal());
            return Ok(if all_terminal {
                SettlementOutcome::AllTerminal { trades }
            } else {
                SettlementOutcome::StillSettling { trades }
            });
        }
    }
}

/// A venue order in a terminal rejected state means its trades cannot
/// exist — resolve the wait immediately.
fn order_unknown_verdict(order: &ClobOrder) -> Option<SettlementOutcome> {
    let s = order.status.trim();
    if s.eq_ignore_ascii_case("unmatched")
        || s.eq_ignore_ascii_case("canceled")
        || s.eq_ignore_ascii_case("cancelled")
        || s.eq_ignore_ascii_case("expired")
    {
        return Some(SettlementOutcome::OrderUnknownOnVenue);
    }
    None
}

/// Validate the parameters of a settlement wait (both durations
/// non-zero; the caller decides policy, this enforces sanity).
pub fn validate_wait(timeout: Duration, interval: Duration) -> PolyResult<()> {
    if timeout.is_zero() {
        return Err(PolyError::invalid("settlement wait timeout must be > 0"));
    }
    if interval.is_zero() {
        return Err(PolyError::invalid("settlement poll interval must be > 0"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(id: &str, order: &str, status: &str, hash: &str) -> ClobTrade {
        ClobTrade {
            id: id.into(),
            order_id: order.into(),
            market: "0xcond".into(),
            asset_id: "123".into(),
            side: "BUY".into(),
            size: "10".into(),
            price: "0.5".into(),
            status: status.into(),
            match_time: "1713398400".into(),
            transaction_hash: hash.into(),
            bucket_index: Some(0),
            last_update: "1713398401".into(),
        }
    }

    #[test]
    fn venue_trade_lifecycle_classifies() {
        assert_eq!(
            classify_trade(&trade("t", "0xo", "MATCHED", "")),
            TradeSettlement::Matched
        );
        assert!(matches!(
            classify_trade(&trade("t", "0xo", "MINED", "")),
            TradeSettlement::Settling { .. }
        ));
        assert!(matches!(
            classify_trade(&trade("t", "0xo", "RETRYING", "")),
            TradeSettlement::Settling { .. }
        ));
        let confirmed = classify_trade(&trade("t", "0xo", "CONFIRMED", "0xhash"));
        assert_eq!(
            confirmed,
            TradeSettlement::Confirmed {
                transaction_hash: "0xhash".into(),
                bucket_index: Some(0)
            }
        );
        assert!(confirmed.is_terminal() && confirmed.is_confirmed());
        let failed = classify_trade(&trade("t", "0xo", "FAILED", ""));
        assert_eq!(
            failed,
            TradeSettlement::Failed {
                status: "FAILED".into()
            }
        );
        assert!(failed.is_terminal() && !failed.is_confirmed());
    }

    #[test]
    fn projection_matches_requested_and_order_scoped_trades() {
        let trades = vec![
            trade("t1", "0xo", "CONFIRMED", "0xh1"),
            trade("t2", "0xo", "MINED", ""),
            trade("other", "0xdifferent", "CONFIRMED", "0xh9"),
            trade("t3-split", "0xo", "CONFIRMED", "0xh3"),
        ];
        // Only t1 requested; t2/t3-split belong to the order and are
        // included (venue-side splits); "other" is excluded.
        let out = project_trades(&trades, "0xo", &["t1".into()]);
        let ids: Vec<&str> = out.iter().map(|t| t.trade_id.as_str()).collect();
        assert!(ids.contains(&"t1"));
        assert!(ids.contains(&"t2"));
        assert!(ids.contains(&"t3-split"));
        assert!(!ids.contains(&"other"));
    }

    #[test]
    fn unseen_requested_ids_are_reported_as_unseen() {
        let out = project_trades(&[], "0xo", &["missing".into()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].settlement, TradeSettlement::Unseen);
        assert!(!out[0].settlement.is_terminal());
    }

    #[test]
    fn empty_projection_is_reported_not_fabricated() {
        // No requested ids and no matching trades -> empty (caller
        // decides what that means; we never invent a trade).
        assert!(project_trades(&[], "0xo", &[]).is_empty());
    }

    #[test]
    fn wait_parameters_are_validated() {
        assert!(validate_wait(Duration::from_secs(30), Duration::from_secs(2)).is_ok());
        assert!(validate_wait(Duration::ZERO, Duration::from_secs(2)).is_err());
        assert!(validate_wait(Duration::from_secs(30), Duration::ZERO).is_err());
    }
}
