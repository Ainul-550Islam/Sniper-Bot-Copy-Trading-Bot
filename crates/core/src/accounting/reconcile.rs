//! Accounting reconciliation (TASK 5 §5): module truth vs the global ledger.
//!
//! [`reconcile`] is PURE: it takes snapshots — the OMS orders, the modules'
//! operational positions and trades (the authoritative venue-facing
//! records), the aggregated book, the journaled events and the
//! not-yet-durable event ids — and returns typed [`AccountingFinding`]s. It
//! never repairs anything: an unexplained discrepancy is reported (journal +
//! audit + metric) and stays reported until an operator books an explicit,
//! referenced `Correction` event or the underlying record catches up.
//!
//! The four record layers the engine compares (TASK 5 §5):
//!
//! ```text
//! OMS orders  ->  module trades (fills)  ->  ledger events  ->  positions
//!   (intent)        (what the venue           (money booked      (module book
//!                    reported filled)          exactly once)      + ledger book)
//! ```
//!
//! | kind | meaning |
//! |---|---|
//! | `missing_ledger_entry` | a module trade — or a filled OMS order — has no ledger fill event |
//! | `duplicate_ledger_entry` | one trade / signature is behind two different ledger events |
//! | `position_mismatch` | an open module position has no ledger position at all |
//! | `quantity_mismatch` | ledger and module quantities differ beyond tolerance for one asset |
//! | `fee_mismatch` | the fee on a trade and on its ledger event differ |
//! | `pnl_mismatch` | flat on both sides but realized PnL differs beyond tolerance |
//! | `orphan_accounting_event` | a ledger event names a trade / position / order no module knows |
//! | `unresolved_financial_event` | an applied event is not yet durably journaled, or a filled order's money never reached the ledger |

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::book::PositionBook;
use super::event::EventKind;
use super::store::StoredEvent;
use crate::models::{BotModule, ExecutionMode, Position, Trade, TradeSource, Venue};
use crate::oms::{Order, OrderStatus};
use crate::reconciliation::QuantityTolerance;

/// Finding vocabulary (closed set — bounded metric label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingFindingKind {
    /// A module trade has no ledger event.
    MissingLedgerEntry,
    /// One trade / signature sits behind two ledger events.
    DuplicateLedgerEntry,
    /// An open module position has no ledger position.
    PositionMismatch,
    /// Quantities differ beyond tolerance.
    QuantityMismatch,
    /// Fee differs between a trade and its event.
    FeeMismatch,
    /// Realized PnL differs on a flat asset.
    PnlMismatch,
    /// A ledger event no module truth explains.
    OrphanAccountingEvent,
    /// Applied but not durably journaled.
    UnresolvedFinancialEvent,
}

impl AccountingFindingKind {
    /// Every kind, stable order.
    pub const ALL: [AccountingFindingKind; 8] = [
        AccountingFindingKind::MissingLedgerEntry,
        AccountingFindingKind::DuplicateLedgerEntry,
        AccountingFindingKind::PositionMismatch,
        AccountingFindingKind::QuantityMismatch,
        AccountingFindingKind::FeeMismatch,
        AccountingFindingKind::PnlMismatch,
        AccountingFindingKind::OrphanAccountingEvent,
        AccountingFindingKind::UnresolvedFinancialEvent,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountingFindingKind::MissingLedgerEntry => "missing_ledger_entry",
            AccountingFindingKind::DuplicateLedgerEntry => "duplicate_ledger_entry",
            AccountingFindingKind::PositionMismatch => "position_mismatch",
            AccountingFindingKind::QuantityMismatch => "quantity_mismatch",
            AccountingFindingKind::FeeMismatch => "fee_mismatch",
            AccountingFindingKind::PnlMismatch => "pnl_mismatch",
            AccountingFindingKind::OrphanAccountingEvent => "orphan_accounting_event",
            AccountingFindingKind::UnresolvedFinancialEvent => "unresolved_financial_event",
        }
    }

    /// Inverse of [`AccountingFindingKind::as_str`].
    pub fn parse(s: &str) -> Option<AccountingFindingKind> {
        AccountingFindingKind::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for AccountingFindingKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One reconciliation finding. `finding_id` is a digest of the identity
/// fields (kind + what it is about), NOT of the numbers, so a persisting
/// discrepancy is journaled once and a changed one keeps its id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountingFinding {
    /// Deterministic id (`acf_` + digest).
    pub finding_id: String,
    /// Kind.
    pub kind: AccountingFindingKind,
    /// Module the subject belongs to, when known.
    pub module: Option<BotModule>,
    /// Venue, when known.
    pub venue: Option<Venue>,
    /// Asset, when known.
    pub asset: Option<String>,
    /// Module position id, when the finding is about one.
    pub position_id: Option<String>,
    /// Ledger event id, when the finding is about one.
    pub event_id: Option<String>,
    /// Module trade id, when the finding is about one.
    pub trade_id: Option<String>,
    /// OMS order id, when the finding is about one.
    pub order_id: Option<String>,
    /// Expected value (module truth side) where numeric.
    pub expected: f64,
    /// Actual value (ledger side) where numeric.
    pub actual: f64,
    /// Human detail.
    pub detail: String,
    /// What was done: always `reported` — this engine never repairs.
    pub action: String,
    /// Replica that produced it.
    pub replica_id: String,
    /// When it was produced.
    pub ts: DateTime<Utc>,
}

impl AccountingFinding {
    #[allow(clippy::too_many_arguments)]
    fn new(
        kind: AccountingFindingKind,
        module: Option<BotModule>,
        venue: Option<Venue>,
        asset: Option<String>,
        position_id: Option<String>,
        event_id: Option<String>,
        trade_id: Option<String>,
        order_id: Option<String>,
        expected: f64,
        actual: f64,
        detail: String,
        replica_id: &str,
        ts: DateTime<Utc>,
    ) -> Self {
        let mut h = Sha256::new();
        h.update(b"accounting-finding-v1|");
        h.update(kind.as_str().as_bytes());
        for part in [
            module.map(|m| m.as_str().to_string()),
            venue.map(|v| v.as_str().to_string()),
            asset.clone(),
            position_id.clone(),
            event_id.clone(),
            trade_id.clone(),
            order_id.clone(),
        ] {
            h.update(b"|");
            h.update(part.unwrap_or_default().as_bytes());
        }
        let finding_id = format!("acf_{}", &hex::encode(h.finalize())[..32]);
        AccountingFinding {
            finding_id,
            kind,
            module,
            venue,
            asset,
            position_id,
            event_id,
            trade_id,
            order_id,
            expected,
            actual,
            detail,
            action: "reported".into(),
            replica_id: replica_id.to_string(),
            ts,
        }
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "kind={} module={} venue={} asset={} position={} event={} trade={} order={} expected={:.8} actual={:.8} action={} detail={}",
            self.kind,
            self.module.map(|m| m.as_str()).unwrap_or("-"),
            self.venue.map(|v| v.as_str()).unwrap_or("-"),
            self.asset.as_deref().unwrap_or("-"),
            self.position_id.as_deref().unwrap_or("-"),
            self.event_id.as_deref().unwrap_or("-"),
            self.trade_id.as_deref().unwrap_or("-"),
            self.order_id.as_deref().unwrap_or("-"),
            self.expected,
            self.actual,
            self.action,
            self.detail
        )
    }
}

/// Inputs of one reconciliation run — all snapshots, no handles.
pub struct ReconInputs<'a> {
    /// OMS orders in the comparison window — the intent layer. A terminal
    /// `Filled` order whose money never reached the ledger is the earliest
    /// detectable accounting gap (a fill was observed but nothing was
    /// booked), so orders are compared before trades.
    pub orders: &'a [Order],
    /// Module positions (open and closed) as the modules see them.
    pub positions: &'a [Position],
    /// Module trades in the comparison window (this process life).
    pub trades: &'a [Trade],
    /// The aggregated book.
    pub book: &'a PositionBook,
    /// Journaled + applied events (the ledger's in-memory index).
    pub events: &'a [StoredEvent],
    /// Event ids applied but not yet durably journaled.
    pub pending: &'a [String],
    /// Only events recorded at or after this instant are checked for
    /// orphans (older events may legitimately predate the in-memory
    /// module records of this process life).
    pub since: DateTime<Utc>,
    /// Quantity tolerance (same policy as position reconciliation).
    pub tolerance: QuantityTolerance,
    /// This replica.
    pub replica_id: &'a str,
    /// Timestamp stamped on findings.
    pub now: DateTime<Utc>,
}

fn module_of(source: TradeSource) -> BotModule {
    match source {
        TradeSource::Sniper | TradeSource::Manual | TradeSource::Risk => BotModule::Sniper,
        TradeSource::Copy => BotModule::Copy,
        TradeSource::Polymarket => BotModule::Polymarket,
    }
}

/// Aggregation key used to compare quantities: the book aggregates by
/// wallet and strategy as well, so both sides are summed over those.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct AssetKey {
    module: BotModule,
    venue: Venue,
    asset: String,
    mode: ExecutionMode,
}

fn within(tol: QuantityTolerance, expected: f64, actual: f64) -> bool {
    let diff = (expected - actual).abs();
    if diff <= tol.dust {
        return true;
    }
    let scale = expected.abs().max(actual.abs());
    scale > 0.0 && diff / scale <= tol.relative
}

/// The `oms=<id>` token a Polymarket trade note carries (the OMS order the
/// fill belongs to). Solana module trades carry no OMS token; their orders
/// are matched by signature instead.
fn oms_order_id_from_note(note: &str) -> Option<&str> {
    note.split_whitespace()
        .find_map(|tok| tok.strip_prefix("oms="))
}

/// The `fill=<id>` token a Polymarket trade note carries.
fn fill_id_from_note(note: Option<&str>) -> Option<&str> {
    note?
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("fill="))
}

/// Run one reconciliation over snapshots. Deterministic: same inputs, same
/// findings in the same order.
pub fn reconcile(input: ReconInputs<'_>) -> Vec<AccountingFinding> {
    let mut out: Vec<AccountingFinding> = Vec::new();
    let tol = input.tolerance;

    // Indexes over the ledger events.
    let inventory_events: Vec<&StoredEvent> = input
        .events
        .iter()
        .filter(|e| matches!(e.event.kind, EventKind::Fill | EventKind::Settlement))
        .collect();
    let mut by_trade: HashMap<&str, Vec<&StoredEvent>> = HashMap::new();
    let mut by_reference: HashMap<&str, Vec<&StoredEvent>> = HashMap::new();
    for e in &inventory_events {
        if let Some(t) = e.event.trade_id.as_deref() {
            by_trade.entry(t).or_default().push(e);
        }
        by_reference
            .entry(e.event.reference_id.as_str())
            .or_default()
            .push(e);
    }
    let event_ids: HashSet<&str> = input.events.iter().map(|e| e.event_id.as_str()).collect();
    let position_ids: HashSet<&str> = input.positions.iter().map(|p| p.id.as_str()).collect();
    let trade_ids: HashSet<&str> = input.trades.iter().map(|t| t.id.as_str()).collect();
    let order_ids: HashSet<&str> = input.orders.iter().map(|o| o.id.as_str()).collect();

    // 1. Every module trade must have exactly one ledger event; fees must
    //    agree on the matched pair.
    for t in input.trades {
        let module = module_of(t.source);
        let mut matched: Vec<&StoredEvent> =
            by_trade.get(t.id.as_str()).cloned().unwrap_or_default();
        if matched.is_empty() {
            if let Some(sig) = t.signature.as_deref() {
                if let Some(v) = by_reference.get(sig) {
                    matched.extend(v.iter().copied());
                }
            }
            if let Some(fid) = fill_id_from_note(t.note.as_deref()) {
                if let Some(v) = by_reference.get(fid) {
                    matched.extend(v.iter().copied());
                }
            }
            matched.sort_by(|a, b| a.event_id.cmp(&b.event_id));
            matched.dedup_by(|a, b| a.event_id == b.event_id);
        }
        match matched.len() {
            0 => out.push(AccountingFinding::new(
                AccountingFindingKind::MissingLedgerEntry,
                Some(module),
                Some(t.venue),
                Some(t.symbol.clone()),
                t.position_id.clone(),
                None,
                Some(t.id.clone()),
                t.note
                    .as_deref()
                    .and_then(oms_order_id_from_note)
                    .map(str::to_string),
                if t.is_buy() {
                    t.amount_out
                } else {
                    t.amount_in
                },
                0.0,
                format!(
                    "trade {} ({} {} {}) has no ledger fill event",
                    t.id,
                    t.side.as_str(),
                    t.symbol_display,
                    t.venue.as_str()
                ),
                input.replica_id,
                input.now,
            )),
            1 => {
                let e = matched[0];
                if (e.event.fee - t.fee).abs() > 1e-9 {
                    out.push(AccountingFinding::new(
                        AccountingFindingKind::FeeMismatch,
                        Some(module),
                        Some(t.venue),
                        Some(t.symbol.clone()),
                        t.position_id.clone(),
                        Some(e.event_id.clone()),
                        Some(t.id.clone()),
                        e.event.correlation_id.clone(),
                        t.fee,
                        e.event.fee,
                        format!(
                            "trade {} fee {:.8} vs ledger event {} fee {:.8}",
                            t.id, t.fee, e.event_id, e.event.fee
                        ),
                        input.replica_id,
                        input.now,
                    ));
                }
            }
            n => {
                let ids: Vec<&str> = matched.iter().map(|e| e.event_id.as_str()).collect();
                out.push(AccountingFinding::new(
                    AccountingFindingKind::DuplicateLedgerEntry,
                    Some(module),
                    Some(t.venue),
                    Some(t.symbol.clone()),
                    t.position_id.clone(),
                    Some(ids.join(",")),
                    Some(t.id.clone()),
                    None,
                    1.0,
                    n as f64,
                    format!(
                        "trade {} is behind {} ledger events: {}",
                        t.id,
                        n,
                        ids.join(", ")
                    ),
                    input.replica_id,
                    input.now,
                ));
            }
        }
    }

    // 1b. Order layer (TASK 5 §5): every OMS order that reports money moved
    //     — `Filled` or `PartiallyFilled` — must be behind at least one
    //     ledger event. The join is the correlation id the modules stamp on
    //     their fill events (OMS order id / intent id), the order's venue
    //     signature, or its external id. Orders that never moved money
    //     (created / validated / queued / submitted / failed / cancelled /
    //     expired / unknown / reconciled) are intent only and are skipped:
    //     the ledger books fills, never intents.
    let mut by_correlation: HashMap<&str, Vec<&StoredEvent>> = HashMap::new();
    for e in &inventory_events {
        if let Some(c) = e.event.correlation_id.as_deref() {
            by_correlation.entry(c).or_default().push(e);
        }
    }
    let mut trade_notes_by_order: HashMap<&str, Vec<&Trade>> = HashMap::new();
    for t in input.trades {
        if let Some(oms) = t.note.as_deref().and_then(oms_order_id_from_note) {
            trade_notes_by_order.entry(oms).or_default().push(t);
        }
    }
    for o in input.orders {
        if !matches!(o.status, OrderStatus::Filled | OrderStatus::PartiallyFilled) {
            continue;
        }
        let booked = by_correlation.contains_key(o.id.as_str())
            || o.signature
                .as_deref()
                .map(|sig| by_reference.contains_key(sig))
                .unwrap_or(false)
            || o.external_id
                .as_deref()
                .map(|ext| by_reference.contains_key(ext))
                .unwrap_or(false)
            || trade_notes_by_order
                .get(o.id.as_str())
                .map(|trades| {
                    trades.iter().any(|t| {
                        by_trade.contains_key(t.id.as_str())
                            || t.signature
                                .as_deref()
                                .map(|sig| by_reference.contains_key(sig))
                                .unwrap_or(false)
                            || fill_id_from_note(t.note.as_deref())
                                .map(|fid| by_reference.contains_key(fid))
                                .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
        if booked {
            continue;
        }
        // A filled order with a module trade that itself has no ledger event
        // is already reported per trade (missing_ledger_entry); reporting the
        // order too would duplicate the same fact, so the order-level finding
        // is raised only when NO module trade names this order either.
        let has_trade = trade_notes_by_order.contains_key(o.id.as_str())
            || input
                .trades
                .iter()
                .any(|t| o.signature.is_some() && t.signature == o.signature);
        let kind = if has_trade {
            AccountingFindingKind::MissingLedgerEntry
        } else {
            AccountingFindingKind::UnresolvedFinancialEvent
        };
        out.push(AccountingFinding::new(
            kind,
            Some(o.module),
            Venue::parse(&o.venue),
            Some(o.symbol.clone()),
            None,
            None,
            None,
            Some(o.id.clone()),
            o.qty,
            0.0,
            format!(
                "OMS order {} ({} {} {} qty {:.8}, status {}) reports a fill but no ledger event references it",
                o.id,
                o.module,
                o.side,
                o.symbol,
                o.qty,
                o.status.as_str()
            ),
            input.replica_id,
            input.now,
        ));
    }

    // 2. Open module positions must exist in the book at all.
    for p in input.positions {
        if p.status.is_terminal() || p.qty <= tol.dust {
            continue;
        }
        if input.book.by_position_id(&p.id).next().is_none() {
            out.push(AccountingFinding::new(
                AccountingFindingKind::PositionMismatch,
                Some(module_of(p.source)),
                Some(p.venue),
                Some(p.symbol.clone()),
                Some(p.id.clone()),
                None,
                None,
                None,
                p.qty,
                0.0,
                format!(
                    "open module position {} ({} qty {:.8}) has no ledger position",
                    p.id, p.symbol_display, p.qty
                ),
                input.replica_id,
                input.now,
            ));
        }
    }

    // 3. Quantities per (module, venue, asset, mode); realized PnL when flat
    //    on both sides.
    let mut module_qty: BTreeMap<AssetKey, (f64, f64, bool)> = BTreeMap::new();
    for p in input.positions {
        let k = AssetKey {
            module: module_of(p.source),
            venue: p.venue,
            asset: p.symbol.clone(),
            mode: p.mode,
        };
        let e = module_qty.entry(k).or_insert((0.0, 0.0, true));
        if !p.status.is_terminal() {
            e.0 += p.qty.max(0.0);
        }
        e.1 += p.realised();
        if !p.status.is_terminal() && p.qty > tol.dust {
            e.2 = false;
        }
    }
    let mut book_qty: BTreeMap<AssetKey, (f64, f64, f64)> = BTreeMap::new();
    for b in input.book.positions() {
        let k = AssetKey {
            module: b.key.module,
            venue: b.key.venue,
            asset: b.key.asset.clone(),
            mode: b.key.mode,
        };
        let e = book_qty.entry(k).or_insert((0.0, 0.0, 0.0));
        e.0 += b.qty;
        e.1 += b.net_realized();
        e.2 += b.fees;
    }
    for (k, (mq, mreal, mflat)) in &module_qty {
        let Some((bq, breal, _bfees)) = book_qty.get(k) else {
            // Reported per position above (position_mismatch) when open;
            // a closed asset with no ledger history predates the ledger.
            continue;
        };
        if !within(tol, *mq, *bq) {
            out.push(AccountingFinding::new(
                AccountingFindingKind::QuantityMismatch,
                Some(k.module),
                Some(k.venue),
                Some(k.asset.clone()),
                None,
                None,
                None,
                None,
                *mq,
                *bq,
                format!(
                    "{} {} {}: module qty {:.8} vs ledger qty {:.8}",
                    k.module,
                    k.venue.as_str(),
                    k.asset,
                    mq,
                    bq
                ),
                input.replica_id,
                input.now,
            ));
            continue;
        }
        if *mflat && *bq <= tol.dust && !within(tol, *mreal, *breal) {
            out.push(AccountingFinding::new(
                AccountingFindingKind::PnlMismatch,
                Some(k.module),
                Some(k.venue),
                Some(k.asset.clone()),
                None,
                None,
                None,
                None,
                *mreal,
                *breal,
                format!(
                    "{} {} {}: module realized {:.8} vs ledger net realized {:.8}",
                    k.module,
                    k.venue.as_str(),
                    k.asset,
                    mreal,
                    breal
                ),
                input.replica_id,
                input.now,
            ));
        }
    }

    // 4. Orphans: recent inventory events nobody in the module truth knows.
    for e in &inventory_events {
        if e.recorded_at < input.since {
            continue;
        }
        let trade_known = e
            .event
            .trade_id
            .as_deref()
            .map(|t| trade_ids.contains(t))
            .unwrap_or(false);
        let position_known = e
            .event
            .position_id
            .as_deref()
            .map(|p| position_ids.contains(p))
            .unwrap_or(false);
        let order_known = e
            .event
            .correlation_id
            .as_deref()
            .map(|c| order_ids.contains(c))
            .unwrap_or(false);
        if !trade_known && !position_known && !order_known {
            out.push(AccountingFinding::new(
                AccountingFindingKind::OrphanAccountingEvent,
                Some(e.event.module),
                Some(e.event.venue),
                Some(e.event.asset.clone()),
                e.event.position_id.clone(),
                Some(e.event_id.clone()),
                e.event.trade_id.clone(),
                e.event.correlation_id.clone(),
                0.0,
                e.event.quantity,
                format!(
                    "ledger event {} ({}) names trade {} / position {} / order {} unknown to the module truth",
                    e.event_id,
                    e.event.kind,
                    e.event.trade_id.as_deref().unwrap_or("-"),
                    e.event.position_id.as_deref().unwrap_or("-"),
                    e.event.correlation_id.as_deref().unwrap_or("-")
                ),
                input.replica_id,
                input.now,
            ));
        }
    }

    // 5. Applied but not durably journaled.
    for id in input.pending {
        let ev = input.events.iter().find(|e| &e.event_id == id);
        out.push(AccountingFinding::new(
            AccountingFindingKind::UnresolvedFinancialEvent,
            ev.map(|e| e.event.module),
            ev.map(|e| e.event.venue),
            ev.map(|e| e.event.asset.clone()),
            ev.and_then(|e| e.event.position_id.clone()),
            Some(id.clone()),
            ev.and_then(|e| e.event.trade_id.clone()),
            ev.and_then(|e| e.event.correlation_id.clone()),
            1.0,
            0.0,
            format!("ledger event {id} is applied in memory but not durably journaled"),
            input.replica_id,
            input.now,
        ));
    }
    let _ = event_ids;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::event::{fill_event, AccountingEvent, EventSide};
    use crate::models::{PositionSide, PositionStatus};

    fn trade(id: &str, sig: Option<&str>, buy: bool, qty: f64, quote: f64, fee: f64) -> Trade {
        Trade {
            id: id.into(),
            ts: Utc::now(),
            source: TradeSource::Sniper,
            venue: Venue::PumpFun,
            mode: ExecutionMode::Paper,
            side: if buy {
                PositionSide::Long
            } else {
                PositionSide::Short
            },
            symbol: "MINT".into(),
            symbol_display: "MINT".into(),
            amount_in: if buy { quote } else { qty },
            amount_out: if buy { qty } else { quote },
            quote_symbol: "SOL".into(),
            price: quote / qty,
            fee,
            slippage_bps: 0,
            signature: sig.map(|s| s.to_string()),
            position_id: Some("p-1".into()),
            note: None,
            latency_ms: None,
        }
    }

    fn event(
        trade_id: &str,
        reference: &str,
        side: EventSide,
        qty: f64,
        quote: f64,
        fee: f64,
    ) -> AccountingEvent {
        let mut e = fill_event(
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
            fee,
            ExecutionMode::Paper,
            reference,
            None,
            Some("p-1".into()),
            Utc::now(),
            "",
        );
        e.trade_id = Some(trade_id.into());
        e
    }

    fn stored(e: &AccountingEvent) -> StoredEvent {
        StoredEvent {
            event_id: e.event_id(),
            event: e.clone(),
            recorded_at: Utc::now(),
            replica_id: "r".into(),
        }
    }

    fn position(id: &str, qty: f64, cost: f64, realized_quote: f64, open: bool) -> Position {
        let mut p = Position::new(
            id.into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            ExecutionMode::Paper,
            "MINT".into(),
            "MINT".into(),
            "SOL".into(),
        );
        p.qty = qty;
        p.cost_basis = cost;
        p.realized_quote = realized_quote;
        if !open {
            p.status = PositionStatus::Closed;
        }
        p
    }

    fn inputs<'a>(
        positions: &'a [Position],
        trades: &'a [Trade],
        book: &'a PositionBook,
        events: &'a [StoredEvent],
        pending: &'a [String],
    ) -> ReconInputs<'a> {
        inputs_with(&[], positions, trades, book, events, pending)
    }

    fn inputs_with<'a>(
        orders: &'a [Order],
        positions: &'a [Position],
        trades: &'a [Trade],
        book: &'a PositionBook,
        events: &'a [StoredEvent],
        pending: &'a [String],
    ) -> ReconInputs<'a> {
        ReconInputs {
            orders,
            positions,
            trades,
            book,
            events,
            pending,
            since: Utc::now() - chrono::Duration::hours(1),
            tolerance: QuantityTolerance::default(),
            replica_id: "r",
            now: Utc::now(),
        }
    }

    #[test]
    fn in_sync_book_produces_no_findings() {
        let t = trade("t-1", Some("sig-1"), true, 100.0, 1.0, 0.0);
        let e = event("t-1", "sig-1", EventSide::Buy, 100.0, 1.0, 0.0);
        let mut book = PositionBook::new();
        book.apply(&e, &e.event_id());
        let events = vec![stored(&e)];
        let positions = vec![position("p-1", 100.0, 1.0, 0.0, true)];
        let f = reconcile(inputs(&positions, &[t], &book, &events, &[]));
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn missing_entry_and_fee_mismatch_are_reported() {
        let t1 = trade("t-1", Some("sig-1"), true, 100.0, 1.0, 0.0);
        let t2 = trade("t-2", Some("sig-2"), true, 50.0, 0.5, 0.01);
        let e2 = event("t-2", "sig-2", EventSide::Buy, 50.0, 0.5, 0.0);
        let mut book = PositionBook::new();
        book.apply(&e2, &e2.event_id());
        let events = vec![stored(&e2)];
        let positions = vec![position("p-1", 150.0, 1.5, 0.0, true)];
        let f = reconcile(inputs(&positions, &[t1, t2], &book, &events, &[]));
        let kinds: Vec<_> = f.iter().map(|x| x.kind).collect();
        assert!(kinds.contains(&AccountingFindingKind::MissingLedgerEntry));
        assert!(kinds.contains(&AccountingFindingKind::FeeMismatch));
        assert!(
            kinds.contains(&AccountingFindingKind::QuantityMismatch),
            "{kinds:?}"
        );
        assert!(f.iter().all(|x| x.action == "reported"));
    }

    #[test]
    fn duplicate_entries_and_orphans_are_reported() {
        let t = trade("t-1", Some("sig-1"), true, 100.0, 1.0, 0.0);
        let e_a = event("t-1", "sig-1", EventSide::Buy, 100.0, 1.0, 0.0);
        let mut e_b = e_a.clone();
        e_b.wallet = "other-wallet".into(); // a second identity for the same fact
        let mut orphan = event("t-ghost", "sig-ghost", EventSide::Buy, 1.0, 0.1, 0.0);
        orphan.position_id = Some("p-ghost".into());
        let mut book = PositionBook::new();
        for e in [&e_a, &e_b, &orphan] {
            book.apply(e, &e.event_id());
        }
        let events = vec![stored(&e_a), stored(&e_b), stored(&orphan)];
        let positions = vec![position("p-1", 100.0, 1.0, 0.0, true)];
        let f = reconcile(inputs(&positions, &[t], &book, &events, &[]));
        let kinds: Vec<_> = f.iter().map(|x| x.kind).collect();
        assert!(
            kinds.contains(&AccountingFindingKind::DuplicateLedgerEntry),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&AccountingFindingKind::OrphanAccountingEvent),
            "{kinds:?}"
        );
    }

    #[test]
    fn position_without_ledger_history_and_pending_events_are_reported() {
        let book = PositionBook::new();
        let positions = vec![position("p-old", 10.0, 1.0, 0.0, true)];
        let pending = vec!["led_pending".to_string()];
        let f = reconcile(inputs(&positions, &[], &book, &[], &pending));
        let kinds: Vec<_> = f.iter().map(|x| x.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AccountingFindingKind::PositionMismatch,
                AccountingFindingKind::UnresolvedFinancialEvent
            ]
        );
    }

    #[test]
    fn pnl_mismatch_only_when_flat_on_both_sides() {
        let buy = event("t-1", "sig-1", EventSide::Buy, 100.0, 1.0, 0.0);
        let sell = event("t-2", "sig-2", EventSide::Sell, 100.0, 2.0, 0.0);
        let mut book = PositionBook::new();
        book.apply(&buy, &buy.event_id());
        book.apply(&sell, &sell.event_id());
        let events = vec![stored(&buy), stored(&sell)];
        let trades = vec![
            trade("t-1", Some("sig-1"), true, 100.0, 1.0, 0.0),
            trade("t-2", Some("sig-2"), false, 100.0, 2.0, 0.0),
        ];
        // Module says it realized 0.5 (wrong) on a closed position.
        let positions = vec![position("p-1", 0.0, 1.0, 1.5, false)];
        let f = reconcile(inputs(&positions, &trades, &book, &events, &[]));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].kind, AccountingFindingKind::PnlMismatch);
        assert!((f[0].expected - 0.5).abs() < 1e-9);
        assert!((f[0].actual - 1.0).abs() < 1e-9);
        // Same module figures → in sync.
        let positions = vec![position("p-1", 0.0, 1.0, 2.0, false)];
        let f = reconcile(inputs(&positions, &trades, &book, &events, &[]));
        assert!(f.is_empty(), "{f:?}");
    }

    fn oms_order(id: &str, status: OrderStatus, sig: Option<&str>) -> Order {
        Order {
            id: id.into(),
            idempotency_key: format!("intent-{id}"),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "MINT".into(),
            venue: Venue::PumpFun.as_str().into(),
            mode: ExecutionMode::Paper,
            status,
            qty: 100.0,
            price: Some(0.01),
            external_id: None,
            signature: sig.map(|s| s.to_string()),
            error: None,
            meta: serde_json::Value::Null,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            submitted_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn filled_orders_without_a_ledger_event_are_reported() {
        let book = PositionBook::new();
        // A filled order nothing references: unresolved financial event.
        let orders = vec![oms_order("o-1", OrderStatus::Filled, Some("sig-1"))];
        let f = reconcile(inputs_with(&orders, &[], &[], &book, &[], &[]));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].kind, AccountingFindingKind::UnresolvedFinancialEvent);
        assert_eq!(f[0].order_id.as_deref(), Some("o-1"));

        // Intent-only states are never reported (no money moved).
        for st in [
            OrderStatus::Created,
            OrderStatus::Validated,
            OrderStatus::Queued,
            OrderStatus::Submitted,
            OrderStatus::Accepted,
            OrderStatus::Failed,
            OrderStatus::Cancelled,
            OrderStatus::Expired,
            OrderStatus::Unknown,
            OrderStatus::Reconciled,
        ] {
            let orders = vec![oms_order("o-x", st, Some("sig-x"))];
            let f = reconcile(inputs_with(&orders, &[], &[], &book, &[], &[]));
            assert!(
                f.is_empty(),
                "{st:?} must not be an accounting finding: {f:?}"
            );
        }
    }

    #[test]
    fn a_filled_order_matched_by_correlation_signature_or_trade_is_in_sync() {
        let mut book = PositionBook::new();
        // Matched by correlation id (the OMS order id the module stamps).
        let mut e = event("t-1", "sig-1", EventSide::Buy, 100.0, 1.0, 0.0);
        e.correlation_id = Some("o-1".into());
        book.apply(&e, &e.event_id());
        let events = vec![stored(&e)];
        let orders = vec![oms_order("o-1", OrderStatus::Filled, None)];
        let trades = vec![trade("t-1", Some("sig-1"), true, 100.0, 1.0, 0.0)];
        let positions = vec![position("p-1", 100.0, 1.0, 0.0, true)];
        let f = reconcile(inputs_with(
            &orders,
            &positions,
            &trades,
            &book,
            &events,
            &[],
        ));
        assert!(f.is_empty(), "{f:?}");

        // Matched by the order's venue signature alone (no correlation).
        let mut e2 = event("t-2", "sig-2", EventSide::Buy, 100.0, 1.0, 0.0);
        e2.correlation_id = None;
        e2.trade_id = None;
        e2.position_id = Some("p-1".into());
        let mut book2 = PositionBook::new();
        book2.apply(&e2, &e2.event_id());
        let orders2 = vec![oms_order(
            "o-2",
            OrderStatus::PartiallyFilled,
            Some("sig-2"),
        )];
        let f = reconcile(inputs_with(
            &orders2,
            &positions,
            &[],
            &book2,
            &[stored(&e2)],
            &[],
        ));
        assert!(
            f.iter().all(|x| x.order_id.as_deref() != Some("o-2")),
            "{f:?}"
        );
    }

    #[test]
    fn a_ledger_event_explained_only_by_its_order_is_not_an_orphan() {
        let mut e = event("t-ghost", "sig-ghost", EventSide::Buy, 1.0, 0.1, 0.0);
        e.correlation_id = Some("o-9".into());
        e.trade_id = None;
        e.position_id = None;
        let mut book = PositionBook::new();
        book.apply(&e, &e.event_id());
        let events = vec![stored(&e)];
        // Without the order: orphan.
        let f = reconcile(inputs_with(&[], &[], &[], &book, &events, &[]));
        assert!(f
            .iter()
            .any(|x| x.kind == AccountingFindingKind::OrphanAccountingEvent));
        // With the order known: explained.
        let orders = vec![oms_order("o-9", OrderStatus::Filled, None)];
        let f = reconcile(inputs_with(&orders, &[], &[], &book, &events, &[]));
        assert!(
            !f.iter()
                .any(|x| x.kind == AccountingFindingKind::OrphanAccountingEvent),
            "{f:?}"
        );
    }

    #[test]
    fn finding_ids_are_stable_across_runs_and_ignore_numbers() {
        let book = PositionBook::new();
        let positions = vec![position("p-old", 10.0, 1.0, 0.0, true)];
        let a = reconcile(inputs(&positions, &[], &book, &[], &[]));
        let positions = vec![position("p-old", 12.0, 1.0, 0.0, true)];
        let b = reconcile(inputs(&positions, &[], &book, &[], &[]));
        assert_eq!(a[0].finding_id, b[0].finding_id);
        assert!(a[0].finding_id.starts_with("acf_"));
        for k in AccountingFindingKind::ALL {
            assert_eq!(AccountingFindingKind::parse(k.as_str()), Some(k));
        }
    }
}
