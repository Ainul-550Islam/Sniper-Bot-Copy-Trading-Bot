//! Double-entry postings (TASK 5 §2).
//!
//! Every applied [`AccountingEvent`] expands into a balanced set of
//! [`Posting`]s — per event and per quote asset the debits equal the
//! credits. Postings are append-only rows behind the event; the position
//! book ([`crate::accounting::book`]) is a derived view over the same events.
//!
//! Accounts (all denominated in the event's quote asset):
//!
//! | account | nature | meaning |
//! |---|---|---|
//! | `cash` | asset | quote balance of one wallet |
//! | `inventory` | asset | open positions at cost (fee-exclusive) |
//! | `fees` | expense | fees paid |
//! | `realized_pnl` | income | gains (credit) / losses (debit) on closed quantity |
//! | `equity` | equity | deposits (credit) / withdrawals (debit) |
//! | `funding` | income/expense | venue funding, rebates, interest |
//! | `adjustments` | equity | explicit reconciliation corrections |
//!
//! Expansion rules (`q` = quote_amount, `f` = fee, `c` = cost of the sold
//! slice at the running average cost):
//!
//! * buy fill / settlement-in: `Dr inventory (q − f)`, `Dr fees f`, `Cr cash q`
//! * sell fill / settlement-out: `Dr cash q`, `Dr fees f`, `Cr inventory c`,
//!   `Cr realized_pnl (q + f − c)` (a debit when negative)
//! * fee: `Dr fees f`, `Cr cash f`
//! * deposit: `Dr cash q`, `Cr equity q` — withdrawal: the reverse
//! * transfer: `Cr cash(wallet) q`, `Dr cash(counterparty) q`
//! * funding adjustment: `Dr cash q`, `Cr funding q` (positive) — negative
//!   adjustments are reported with `side = Sell`: `Dr funding q`, `Cr cash q`
//! * correction: like a buy / sell against `adjustments` instead of `cash`

use serde::{Deserialize, Serialize};

use super::event::{AccountingEvent, EventKind, EventSide};

/// Ledger account families (closed set — a bounded label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Account {
    /// Quote-asset balance of a wallet.
    Cash,
    /// Open positions at (fee-exclusive) cost.
    Inventory,
    /// Fees paid.
    Fees,
    /// Realized trading result.
    RealizedPnl,
    /// External capital (deposits / withdrawals).
    Equity,
    /// Venue funding / rebates / interest.
    Funding,
    /// Explicit reconciliation corrections.
    Adjustments,
}

impl Account {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Account::Cash => "cash",
            Account::Inventory => "inventory",
            Account::Fees => "fees",
            Account::RealizedPnl => "realized_pnl",
            Account::Equity => "equity",
            Account::Funding => "funding",
            Account::Adjustments => "adjustments",
        }
    }

    /// Inverse of [`Account::as_str`].
    pub fn parse(s: &str) -> Option<Account> {
        Some(match s {
            "cash" => Account::Cash,
            "inventory" => Account::Inventory,
            "fees" => Account::Fees,
            "realized_pnl" => Account::RealizedPnl,
            "equity" => Account::Equity,
            "funding" => Account::Funding,
            "adjustments" => Account::Adjustments,
            _ => return None,
        })
    }
}

/// Debit or credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntrySide {
    /// Debit.
    Debit,
    /// Credit.
    Credit,
}

impl EntrySide {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntrySide::Debit => "debit",
            EntrySide::Credit => "credit",
        }
    }

    /// Inverse of [`EntrySide::as_str`].
    pub fn parse(s: &str) -> Option<EntrySide> {
        match s {
            "debit" => Some(EntrySide::Debit),
            "credit" => Some(EntrySide::Credit),
            _ => None,
        }
    }
}

/// One line of a balanced entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Posting {
    /// Event this posting belongs to.
    pub event_id: String,
    /// Position of the line inside the event's entry (0-based).
    pub seq: u32,
    /// Account family.
    pub account: Account,
    /// Wallet the account belongs to.
    pub wallet: String,
    /// Asset the `amount` is denominated in (the event's quote asset).
    pub asset: String,
    /// Debit / credit.
    pub side: EntrySide,
    /// Amount in `asset`, always `>= 0`.
    pub amount: f64,
    /// Base units moved by this line (inventory lines only).
    pub quantity: f64,
    /// Base asset the `quantity` refers to (inventory lines only).
    pub base_asset: Option<String>,
}

/// Errors from expanding an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostingError {
    /// The event failed structural validation.
    Invalid(String),
}

impl std::fmt::Display for PostingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PostingError::Invalid(m) => write!(f, "invalid accounting event: {m}"),
        }
    }
}

impl std::error::Error for PostingError {}

/// A balanced set of postings for one event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The lines.
    pub postings: Vec<Posting>,
}

impl Entry {
    /// Sum of debits and credits (must be equal for a balanced entry).
    pub fn totals(&self) -> (f64, f64) {
        self.postings
            .iter()
            .fold((0.0, 0.0), |(d, c), p| match p.side {
                EntrySide::Debit => (d + p.amount, c),
                EntrySide::Credit => (d, c + p.amount),
            })
    }

    /// True when debits equal credits within `1e-9` relative tolerance.
    pub fn is_balanced(&self) -> bool {
        let (d, c) = self.totals();
        let scale = d.abs().max(c.abs()).max(1.0);
        (d - c).abs() <= 1e-9 * scale
    }

    /// Signed movement of one account (debits positive, credits negative).
    pub fn net(&self, account: Account) -> f64 {
        self.postings
            .iter()
            .filter(|p| p.account == account)
            .map(|p| match p.side {
                EntrySide::Debit => p.amount,
                EntrySide::Credit => -p.amount,
            })
            .sum()
    }
}

/// Expand an event into its balanced entry. `cost_of_slice` is the
/// fee-exclusive inventory value of the quantity leaving on a sell /
/// settlement-out / correction-out (computed by the position book at the
/// running average cost); ignored for other kinds.
pub fn expand(event: &AccountingEvent, cost_of_slice: f64) -> Result<Entry, PostingError> {
    event.validate().map_err(PostingError::Invalid)?;
    let id = event.event_id();
    let mut lines: Vec<Posting> = Vec::with_capacity(4);
    let mut push = |account: Account,
                    wallet: &str,
                    side: EntrySide,
                    amount: f64,
                    quantity: f64,
                    base: Option<&str>| {
        if amount.abs() <= 0.0 && quantity.abs() <= 0.0 {
            return;
        }
        // Negative amounts flip the side so every stored amount is >= 0.
        let (side, amount) = if amount < 0.0 {
            (
                match side {
                    EntrySide::Debit => EntrySide::Credit,
                    EntrySide::Credit => EntrySide::Debit,
                },
                -amount,
            )
        } else {
            (side, amount)
        };
        lines.push(Posting {
            event_id: id.clone(),
            seq: lines.len() as u32,
            account,
            wallet: wallet.to_string(),
            asset: event.quote_asset.clone(),
            side,
            amount,
            quantity,
            base_asset: base.map(|s| s.to_string()),
        });
    };

    let q = event.quote_amount;
    let f = event.fee;
    let w = event.wallet.as_str();
    match event.kind {
        EventKind::Fill | EventKind::Settlement | EventKind::Correction => {
            let cash_account = if event.kind == EventKind::Correction {
                Account::Adjustments
            } else {
                Account::Cash
            };
            match event.side.unwrap_or(EventSide::Buy) {
                EventSide::Buy => {
                    push(
                        Account::Inventory,
                        w,
                        EntrySide::Debit,
                        (q - f).max(0.0),
                        event.quantity,
                        Some(&event.asset),
                    );
                    push(Account::Fees, w, EntrySide::Debit, f, 0.0, None);
                    push(cash_account, w, EntrySide::Credit, q, 0.0, None);
                }
                EventSide::Sell => {
                    let c = cost_of_slice.max(0.0);
                    push(cash_account, w, EntrySide::Debit, q, 0.0, None);
                    push(Account::Fees, w, EntrySide::Debit, f, 0.0, None);
                    push(
                        Account::Inventory,
                        w,
                        EntrySide::Credit,
                        c,
                        event.quantity,
                        Some(&event.asset),
                    );
                    push(
                        Account::RealizedPnl,
                        w,
                        EntrySide::Credit,
                        q + f - c,
                        0.0,
                        None,
                    );
                }
            }
        }
        EventKind::Fee => {
            let amount = if f > 0.0 { f } else { q };
            push(Account::Fees, w, EntrySide::Debit, amount, 0.0, None);
            push(Account::Cash, w, EntrySide::Credit, amount, 0.0, None);
        }
        EventKind::Deposit => {
            push(Account::Cash, w, EntrySide::Debit, q, 0.0, None);
            push(Account::Equity, w, EntrySide::Credit, q, 0.0, None);
        }
        EventKind::Withdrawal => {
            push(Account::Equity, w, EntrySide::Debit, q, 0.0, None);
            push(Account::Fees, w, EntrySide::Debit, f, 0.0, None);
            push(Account::Cash, w, EntrySide::Credit, q + f, 0.0, None);
        }
        EventKind::Transfer => {
            let to = event.counterparty_wallet.as_deref().unwrap_or("");
            push(Account::Cash, to, EntrySide::Debit, q, 0.0, None);
            push(Account::Fees, w, EntrySide::Debit, f, 0.0, None);
            push(Account::Cash, w, EntrySide::Credit, q + f, 0.0, None);
        }
        EventKind::FundingAdjustment => match event.side {
            Some(EventSide::Sell) => {
                push(Account::Funding, w, EntrySide::Debit, q, 0.0, None);
                push(Account::Cash, w, EntrySide::Credit, q, 0.0, None);
            }
            _ => {
                push(Account::Cash, w, EntrySide::Debit, q, 0.0, None);
                push(Account::Funding, w, EntrySide::Credit, q, 0.0, None);
            }
        },
    }
    let entry = Entry { postings: lines };
    debug_assert!(
        entry.is_balanced(),
        "unbalanced entry for {}",
        event.summary()
    );
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::event::fill_event;
    use crate::models::{BotModule, ExecutionMode, Venue};
    use chrono::Utc;

    fn fill(side: EventSide, qty: f64, price: f64, amount: f64, fee: f64) -> AccountingEvent {
        fill_event(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "w",
            "copy:leader",
            "MINT",
            "SOL",
            side,
            qty,
            price,
            amount,
            fee,
            ExecutionMode::Live,
            "sig",
            None,
            None,
            Utc::now(),
            "",
        )
    }

    #[test]
    fn buy_is_balanced_and_splits_the_fee() {
        let e = expand(&fill(EventSide::Buy, 10.0, 0.1, 1.05, 0.05), 0.0).unwrap();
        assert!(e.is_balanced());
        assert!((e.net(Account::Inventory) - 1.0).abs() < 1e-12);
        assert!((e.net(Account::Fees) - 0.05).abs() < 1e-12);
        assert!((e.net(Account::Cash) + 1.05).abs() < 1e-12);
        assert_eq!(e.postings[0].quantity, 10.0);
        assert_eq!(e.postings[0].base_asset.as_deref(), Some("MINT"));
    }

    #[test]
    fn sell_books_gain_or_loss_against_realized_pnl() {
        let gain = expand(&fill(EventSide::Sell, 10.0, 0.2, 2.0, 0.0), 1.0).unwrap();
        assert!(gain.is_balanced());
        assert!(
            (gain.net(Account::RealizedPnl) + 1.0).abs() < 1e-12,
            "credit = gain"
        );
        assert!((gain.net(Account::Inventory) + 1.0).abs() < 1e-12);
        let loss = expand(&fill(EventSide::Sell, 10.0, 0.05, 0.5, 0.0), 1.0).unwrap();
        assert!(loss.is_balanced());
        assert!(
            (loss.net(Account::RealizedPnl) - 0.5).abs() < 1e-12,
            "debit = loss"
        );
        let with_fee = expand(&fill(EventSide::Sell, 10.0, 0.2, 1.9, 0.1), 1.0).unwrap();
        assert!(with_fee.is_balanced());
        assert!((with_fee.net(Account::Fees) - 0.1).abs() < 1e-12);
        assert!((with_fee.net(Account::RealizedPnl) + 1.0).abs() < 1e-12);
    }

    #[test]
    fn cash_kinds_balance() {
        let mut d = fill(EventSide::Buy, 0.0, 0.0, 100.0, 0.0);
        d.kind = EventKind::Deposit;
        d.side = None;
        d.asset = "USDC".into();
        d.quote_asset = "USDC".into();
        let e = expand(&d, 0.0).unwrap();
        assert!(e.is_balanced());
        assert!((e.net(Account::Cash) - 100.0).abs() < 1e-12);
        let mut wd = d.clone();
        wd.kind = EventKind::Withdrawal;
        wd.fee = 1.0;
        let e = expand(&wd, 0.0).unwrap();
        assert!(e.is_balanced());
        assert!((e.net(Account::Cash) + 101.0).abs() < 1e-12);
        let mut t = d.clone();
        t.kind = EventKind::Transfer;
        t.counterparty_wallet = Some("w2".into());
        let e = expand(&t, 0.0).unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.postings[0].wallet, "w2");
        let mut fee = d.clone();
        fee.kind = EventKind::Fee;
        fee.quote_amount = 0.0;
        fee.fee = 0.25;
        let e = expand(&fee, 0.0).unwrap();
        assert!(e.is_balanced());
        assert!((e.net(Account::Fees) - 0.25).abs() < 1e-12);
        let mut fund = d.clone();
        fund.kind = EventKind::FundingAdjustment;
        fund.side = Some(EventSide::Sell);
        fund.quote_amount = 3.0;
        let e = expand(&fund, 0.0).unwrap();
        assert!(e.is_balanced());
        assert!((e.net(Account::Cash) + 3.0).abs() < 1e-12);
    }

    #[test]
    fn invalid_events_do_not_expand() {
        let mut bad = fill(EventSide::Buy, -1.0, 0.1, 1.0, 0.0);
        bad.quantity = -1.0;
        assert!(matches!(expand(&bad, 0.0), Err(PostingError::Invalid(_))));
    }

    #[test]
    fn account_labels_round_trip() {
        for a in [
            Account::Cash,
            Account::Inventory,
            Account::Fees,
            Account::RealizedPnl,
            Account::Equity,
            Account::Funding,
            Account::Adjustments,
        ] {
            assert_eq!(Account::parse(a.as_str()), Some(a));
        }
        assert_eq!(EntrySide::parse("debit"), Some(EntrySide::Debit));
    }
}
