//! Position aggregation (TASK 5 §3): one book over every module's fills.
//!
//! The book is a DERIVED view — it is rebuilt by replaying ledger events and
//! never edited directly. Positions are keyed by
//! `(module, venue, wallet, strategy, asset, quote_asset, mode)` so exposure
//! can be sliced per venue / wallet / strategy / asset without a second copy
//! of any venue-specific truth (the modules' own `Position`s stay the
//! operational record; reconciliation compares the two).
//!
//! Economics follow the suite's average-cost convention
//! ([`crate::reconciliation::reconstruct_pnl`]): inventory is carried at the
//! fee-exclusive running average cost, `realized` is gross proceeds minus the
//! cost of the sold slice, fees accumulate separately and `net = realized +
//! unrealized − fees`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::event::{AccountingEvent, EventKind, EventSide};
use crate::models::{BotModule, ExecutionMode, Venue};

/// Identity of one aggregated position.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PositionKey {
    /// Source module.
    pub module: BotModule,
    /// Venue.
    pub venue: Venue,
    /// Our wallet / account.
    pub wallet: String,
    /// Strategy label.
    pub strategy: String,
    /// Base asset.
    pub asset: String,
    /// Quote asset.
    pub quote_asset: String,
    /// Execution mode.
    pub mode: ExecutionMode,
}

impl PositionKey {
    /// Key of the position an event belongs to.
    pub fn of(event: &AccountingEvent) -> PositionKey {
        PositionKey {
            module: event.module,
            venue: event.venue,
            wallet: event.wallet.clone(),
            strategy: event.strategy.clone(),
            asset: event.asset.clone(),
            quote_asset: event.quote_asset.clone(),
            mode: event.mode,
        }
    }

    /// Stable textual form (`module|venue|wallet|strategy|asset|quote|mode`).
    pub fn as_string(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.module,
            self.venue.as_str(),
            self.wallet,
            self.strategy,
            self.asset,
            self.quote_asset,
            self.mode.as_str()
        )
    }
}

/// One aggregated position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookPosition {
    /// Identity.
    pub key: PositionKey,
    /// Open base quantity (`0` once flat).
    pub qty: f64,
    /// Fee-exclusive cost of the open quantity.
    pub cost_basis: f64,
    /// Gross proceeds − cost of sold slices (fees NOT deducted).
    pub realized: f64,
    /// Fees paid over the life of the position.
    pub fees: f64,
    /// Total quote spent on buys (fee-inclusive).
    pub bought_quote: f64,
    /// Total quote received on sells (fee-exclusive net proceeds).
    pub sold_quote: f64,
    /// Base units bought / sold in total.
    pub bought_qty: f64,
    /// Base units sold in total.
    pub sold_qty: f64,
    /// Last fill price seen (fallback mark).
    pub last_price: f64,
    /// First event time.
    pub opened_at: DateTime<Utc>,
    /// Last event time.
    pub updated_at: DateTime<Utc>,
    /// Number of events applied.
    pub event_count: u64,
    /// Last applied event id.
    pub last_event_id: String,
    /// Module position ids seen on the events (join key for reconciliation).
    pub position_ids: Vec<String>,
}

impl BookPosition {
    fn new(key: PositionKey, ts: DateTime<Utc>) -> Self {
        BookPosition {
            key,
            qty: 0.0,
            cost_basis: 0.0,
            realized: 0.0,
            fees: 0.0,
            bought_quote: 0.0,
            sold_quote: 0.0,
            bought_qty: 0.0,
            sold_qty: 0.0,
            last_price: 0.0,
            opened_at: ts,
            updated_at: ts,
            event_count: 0,
            last_event_id: String::new(),
            position_ids: Vec::new(),
        }
    }

    /// Running average cost per unit (fee-exclusive), `0` when flat.
    pub fn avg_cost(&self) -> f64 {
        if self.qty > 0.0 && self.cost_basis > 0.0 {
            self.cost_basis / self.qty
        } else {
            0.0
        }
    }

    /// True while quantity remains.
    pub fn is_open(&self) -> bool {
        self.qty > 1e-12
    }

    /// Notional at `mark` (falls back to the last fill price).
    pub fn notional(&self, mark: Option<f64>) -> f64 {
        let m = mark
            .filter(|m| m.is_finite() && *m > 0.0)
            .unwrap_or(self.last_price);
        self.qty * m
    }

    /// Exposure in quote units — the larger of cost and marked notional (the
    /// same rule the module risk engine applies to its own positions).
    pub fn exposure(&self, mark: Option<f64>) -> f64 {
        if !self.is_open() {
            return 0.0;
        }
        self.cost_basis.max(self.notional(mark))
    }

    /// Unrealized PnL at `mark` in quote units.
    pub fn unrealized(&self, mark: Option<f64>) -> f64 {
        if !self.is_open() {
            return 0.0;
        }
        self.notional(mark) - self.cost_basis
    }

    /// Realized minus fees.
    pub fn net_realized(&self) -> f64 {
        self.realized - self.fees
    }

    /// Fee-exclusive cost of `qty` units at the running average cost
    /// (clamped to the open quantity — over-sells cannot invent cost).
    pub fn cost_of(&self, qty: f64) -> f64 {
        let q = qty.min(self.qty).max(0.0);
        self.avg_cost() * q
    }
}

/// What applying one event did to the book (for audit / metrics).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BookEffect {
    /// Position touched.
    pub key: PositionKey,
    /// Quantity before / after.
    pub qty_before: f64,
    /// Quantity after.
    pub qty_after: f64,
    /// Realized PnL booked by this event (gross of fees).
    pub realized_delta: f64,
    /// Fee booked by this event.
    pub fee_delta: f64,
    /// Fee-exclusive cost of the slice that left (sells).
    pub cost_of_slice: f64,
    /// `opened` / `increased` / `reduced` / `closed` / `cash`.
    pub transition: &'static str,
}

/// The aggregated book.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PositionBook {
    positions: BTreeMap<PositionKey, BookPosition>,
    /// Cash movements that are not attached to a position, per
    /// `(wallet, quote_asset)`: deposits (+), withdrawals (−), transfers,
    /// funding, stand-alone fees. Informational (capital tracking).
    cash: BTreeMap<(String, String), f64>,
    /// Cumulative fees from stand-alone fee events, per `(wallet, quote)`.
    standalone_fees: BTreeMap<(String, String), f64>,
}

impl PositionBook {
    /// Empty book.
    pub fn new() -> Self {
        PositionBook::default()
    }

    /// Fee-exclusive cost of the slice an event would remove (sells,
    /// settlement-out, correction-out); `0` for everything else. Computed
    /// BEFORE the event is applied so the postings can be expanded first.
    pub fn cost_of_slice(&self, event: &AccountingEvent) -> f64 {
        if !event.kind.moves_inventory() || event.side != Some(EventSide::Sell) {
            return 0.0;
        }
        self.positions
            .get(&PositionKey::of(event))
            .map(|p| p.cost_of(event.quantity))
            .unwrap_or(0.0)
    }

    /// Apply one (already validated, already deduplicated) event.
    pub fn apply(&mut self, event: &AccountingEvent, event_id: &str) -> BookEffect {
        let key = PositionKey::of(event);
        if !event.kind.moves_inventory() {
            let cash_key = (event.wallet.clone(), event.quote_asset.clone());
            let delta = match event.kind {
                EventKind::Deposit => event.quote_amount,
                EventKind::Withdrawal => -(event.quote_amount + event.fee),
                EventKind::Transfer => {
                    if let Some(to) = &event.counterparty_wallet {
                        *self
                            .cash
                            .entry((to.clone(), event.quote_asset.clone()))
                            .or_insert(0.0) += event.quote_amount;
                    }
                    -(event.quote_amount + event.fee)
                }
                EventKind::FundingAdjustment => match event.side {
                    Some(EventSide::Sell) => -event.quote_amount,
                    _ => event.quote_amount,
                },
                EventKind::Fee => {
                    let f = if event.fee > 0.0 {
                        event.fee
                    } else {
                        event.quote_amount
                    };
                    *self.standalone_fees.entry(cash_key.clone()).or_insert(0.0) += f;
                    -f
                }
                EventKind::Fill | EventKind::Settlement | EventKind::Correction => 0.0,
            };
            *self.cash.entry(cash_key).or_insert(0.0) += delta;
            return BookEffect {
                key,
                qty_before: 0.0,
                qty_after: 0.0,
                realized_delta: 0.0,
                fee_delta: event.fee,
                cost_of_slice: 0.0,
                transition: "cash",
            };
        }

        let pos = self
            .positions
            .entry(key.clone())
            .or_insert_with(|| BookPosition::new(key.clone(), event.ts));
        let qty_before = pos.qty;
        let mut realized_delta = 0.0;
        let mut cost_of_slice = 0.0;
        match event.side.unwrap_or(EventSide::Buy) {
            EventSide::Buy => {
                pos.qty += event.quantity;
                pos.cost_basis += (event.quote_amount - event.fee).max(0.0);
                pos.bought_quote += event.quote_amount;
                pos.bought_qty += event.quantity;
            }
            EventSide::Sell => {
                let q = event.quantity.min(pos.qty).max(0.0);
                cost_of_slice = pos.avg_cost() * q;
                let gross = event.quote_amount + event.fee;
                realized_delta = gross - cost_of_slice;
                pos.realized += realized_delta;
                pos.cost_basis = (pos.cost_basis - cost_of_slice).max(0.0);
                pos.qty = (pos.qty - q).max(0.0);
                if pos.qty <= 1e-12 {
                    pos.qty = 0.0;
                    pos.cost_basis = 0.0;
                }
                pos.sold_quote += event.quote_amount;
                pos.sold_qty += q;
            }
        }
        pos.fees += event.fee;
        if let Some(p) = event.price.filter(|p| p.is_finite() && *p > 0.0) {
            pos.last_price = p;
        } else if event.quantity > 0.0 && event.quote_amount > 0.0 {
            pos.last_price = event.quote_amount / event.quantity;
        }
        pos.updated_at = event.ts.max(pos.updated_at);
        pos.event_count += 1;
        pos.last_event_id = event_id.to_string();
        if let Some(id) = &event.position_id {
            if !pos.position_ids.iter().any(|x| x == id) {
                pos.position_ids.push(id.clone());
            }
        }
        let qty_after = pos.qty;
        let transition = match (qty_before > 1e-12, qty_after > 1e-12) {
            (false, true) => "opened",
            (true, true) if qty_after > qty_before => "increased",
            (true, true) => "reduced",
            (true, false) => "closed",
            (false, false) => "closed",
        };
        BookEffect {
            key,
            qty_before,
            qty_after,
            realized_delta,
            fee_delta: event.fee,
            cost_of_slice,
            transition,
        }
    }

    /// Every position (open and flat), stable order.
    pub fn positions(&self) -> impl Iterator<Item = &BookPosition> {
        self.positions.values()
    }

    /// Open positions only.
    pub fn open_positions(&self) -> impl Iterator<Item = &BookPosition> {
        self.positions.values().filter(|p| p.is_open())
    }

    /// Position by key.
    pub fn get(&self, key: &PositionKey) -> Option<&BookPosition> {
        self.positions.get(key)
    }

    /// Positions that carry `position_id` (a module position id).
    pub fn by_position_id<'a>(
        &'a self,
        position_id: &'a str,
    ) -> impl Iterator<Item = &'a BookPosition> + 'a {
        self.positions
            .values()
            .filter(move |p| p.position_ids.iter().any(|x| x == position_id))
    }

    /// Number of open positions.
    pub fn open_count(&self) -> usize {
        self.open_positions().count()
    }

    /// Unattached cash movements per `(wallet, quote_asset)`.
    pub fn cash(&self) -> &BTreeMap<(String, String), f64> {
        &self.cash
    }

    /// Stand-alone fees per `(wallet, quote_asset)`.
    pub fn standalone_fees(&self) -> &BTreeMap<(String, String), f64> {
        &self.standalone_fees
    }

    /// Total number of positions tracked (open + flat).
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// True when nothing has been booked.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty() && self.cash.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::event::fill_event;

    fn fill(
        side: EventSide,
        qty: f64,
        price: f64,
        amount: f64,
        fee: f64,
        r: &str,
    ) -> AccountingEvent {
        fill_event(
            BotModule::Sniper,
            Venue::PumpFun,
            "w",
            "sniper",
            "MINT",
            "SOL",
            side,
            qty,
            price,
            amount,
            fee,
            ExecutionMode::Paper,
            r,
            None,
            Some("p-1".into()),
            Utc::now(),
            "",
        )
    }

    #[test]
    fn average_cost_and_realized_follow_the_shared_convention() {
        let mut book = PositionBook::new();
        let e1 = fill(EventSide::Buy, 100.0, 0.01, 1.0, 0.0, "a");
        let fx = book.apply(&e1, "id-a");
        assert_eq!(fx.transition, "opened");
        let e2 = fill(EventSide::Buy, 100.0, 0.03, 3.0, 0.0, "b");
        book.apply(&e2, "id-b");
        let key = PositionKey::of(&e1);
        let p = book.get(&key).unwrap();
        assert!((p.qty - 200.0).abs() < 1e-9);
        assert!((p.avg_cost() - 0.02).abs() < 1e-12);
        // Sell half at 0.05 → proceeds 5.0, cost of slice 2.0, realized 3.0.
        let e3 = fill(EventSide::Sell, 100.0, 0.05, 5.0, 0.0, "c");
        assert!((book.cost_of_slice(&e3) - 2.0).abs() < 1e-12);
        let fx = book.apply(&e3, "id-c");
        assert_eq!(fx.transition, "reduced");
        assert!((fx.realized_delta - 3.0).abs() < 1e-12);
        let p = book.get(&key).unwrap();
        assert!((p.qty - 100.0).abs() < 1e-9);
        assert!((p.cost_basis - 2.0).abs() < 1e-12);
        assert!((p.realized - 3.0).abs() < 1e-12);
        // Matches reconstruct_pnl on the same fills.
        let recon = crate::reconciliation::reconstruct_pnl(&[
            crate::reconciliation::FillRecord {
                side: "buy".into(),
                qty: 100.0,
                quote: 1.0,
            },
            crate::reconciliation::FillRecord {
                side: "buy".into(),
                qty: 100.0,
                quote: 3.0,
            },
            crate::reconciliation::FillRecord {
                side: "sell".into(),
                qty: 100.0,
                quote: 5.0,
            },
        ]);
        assert!((recon.realized - p.realized).abs() < 1e-12);
        assert!((recon.open_cost_basis - p.cost_basis).abs() < 1e-12);
        // Close the rest at a loss.
        let e4 = fill(EventSide::Sell, 100.0, 0.01, 1.0, 0.0, "d");
        let fx = book.apply(&e4, "id-d");
        assert_eq!(fx.transition, "closed");
        assert!((fx.realized_delta + 1.0).abs() < 1e-12);
        let p = book.get(&key).unwrap();
        assert!(!p.is_open());
        assert_eq!(p.cost_basis, 0.0);
        assert!((p.realized - 2.0).abs() < 1e-12);
        assert_eq!(book.open_count(), 0);
    }

    #[test]
    fn fees_are_tracked_separately_and_net_out() {
        let mut book = PositionBook::new();
        book.apply(&fill(EventSide::Buy, 10.0, 0.1, 1.1, 0.1, "a"), "a");
        book.apply(&fill(EventSide::Sell, 10.0, 0.2, 1.9, 0.1, "b"), "b");
        let p = book.positions().next().unwrap();
        assert!((p.cost_basis).abs() < 1e-12);
        assert!((p.fees - 0.2).abs() < 1e-12);
        // gross proceeds 2.0 − ex-fee cost 1.0 = 1.0 realized; net 0.8.
        assert!((p.realized - 1.0).abs() < 1e-12);
        assert!((p.net_realized() - 0.8).abs() < 1e-12);
        // Equals the module convention: net proceeds 1.9 − fee-inclusive cost 1.1.
        assert!((p.net_realized() - (1.9 - 1.1)).abs() < 1e-12);
    }

    #[test]
    fn over_sell_is_clamped_and_never_creates_negative_inventory() {
        let mut book = PositionBook::new();
        book.apply(&fill(EventSide::Buy, 10.0, 0.1, 1.0, 0.0, "a"), "a");
        let fx = book.apply(&fill(EventSide::Sell, 25.0, 0.1, 2.5, 0.0, "b"), "b");
        assert_eq!(fx.qty_after, 0.0);
        let p = book.positions().next().unwrap();
        assert_eq!(p.qty, 0.0);
        assert!((p.sold_qty - 10.0).abs() < 1e-12);
        // Proceeds beyond the held quantity still count as cash received.
        assert!((p.realized - 1.5).abs() < 1e-12);
    }

    #[test]
    fn exposure_and_unrealized_use_the_mark() {
        let mut book = PositionBook::new();
        book.apply(&fill(EventSide::Buy, 10.0, 0.1, 1.0, 0.0, "a"), "a");
        let p = book.positions().next().unwrap();
        assert!(
            (p.exposure(Some(0.05)) - 1.0).abs() < 1e-12,
            "cost wins when marked lower"
        );
        assert!(
            (p.exposure(Some(0.3)) - 3.0).abs() < 1e-12,
            "notional wins when marked higher"
        );
        assert!((p.unrealized(Some(0.3)) - 2.0).abs() < 1e-12);
        assert!(
            (p.exposure(None) - 1.0).abs() < 1e-12,
            "falls back to the last price"
        );
    }

    #[test]
    fn cash_kinds_land_in_the_cash_view() {
        let mut book = PositionBook::new();
        let mut d = fill(EventSide::Buy, 0.0, 0.0, 50.0, 0.0, "dep");
        d.kind = EventKind::Deposit;
        d.side = None;
        book.apply(&d, "dep");
        let mut t = d.clone();
        t.kind = EventKind::Transfer;
        t.quote_amount = 20.0;
        t.counterparty_wallet = Some("w2".into());
        book.apply(&t, "tr");
        assert!((book.cash()[&("w".to_string(), "SOL".to_string())] - 30.0).abs() < 1e-12);
        assert!((book.cash()[&("w2".to_string(), "SOL".to_string())] - 20.0).abs() < 1e-12);
        assert_eq!(book.open_count(), 0);
    }
}
