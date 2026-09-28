//! Global accounting / ledger (TASK 5 §2–§6, §8, §9).
//!
//! One append-only, double-entry, idempotent ledger over every trading
//! module, plus the aggregation, reconciliation and recovery that make it
//! authoritative:
//!
//! ```text
//! module fill / fee / settlement / operator deposit …
//!   -> AccountingEvent (typed, explicit: module, venue, wallet, strategy,
//!                       asset, qty, price, fee, ts, reference / correlation)
//!   -> GlobalLedger::submit            ONE idempotency identity (event_id)
//!        -> balanced postings          double entry, per quote asset
//!        -> PositionBook               aggregation per module/venue/wallet/strategy/asset
//!        -> durable journal            ledger_events + ledger_postings (migration 0015)
//!        -> audit + metrics
//!   -> PortfolioView                   exposure / PnL / fees / utilization slices
//!   -> reconcile()                     module truth vs ledger → typed findings, never repaired
//!   -> recover()                       rebuild from the journal, replay-safe, gaps reported
//! ```
//!
//! | file | concern |
//! |---|---|
//! | `event.rs` | [`AccountingEvent`], [`EventKind`], the deterministic [`AccountingEvent::event_id`] |
//! | `posting.rs` | double-entry [`Posting`]s / [`Entry`] expansion |
//! | `book.rs` | [`PositionBook`] aggregation (average cost, realized, fees, exposure) |
//! | `ledger.rs` | [`GlobalLedger`]: the single mutation door, idempotency, journal, pending |
//! | `view.rs` | [`PortfolioView`] in reference units |
//! | `store.rs` | [`LedgerStore`] contract + [`MemoryLedgerStore`] |
//! | `reconcile.rs` | [`reconcile`] → [`AccountingFinding`]s |
//! | `recovery.rs` | [`GlobalLedger::recover`] |
//! | `metrics.rs` / `audit.rs` | `global_ledger_*` / `global_portfolio_*` series, `global.*` audit actions |
//!
//! Modules never touch the book, the postings or the journal: they build an
//! event and call [`GlobalLedger::submit`] through `state.ledger()`.

pub mod audit;
pub mod book;
pub mod event;
pub mod ledger;
pub mod metrics;
pub mod posting;
pub mod reconcile;
pub mod recovery;
pub mod store;
pub mod view;

pub use audit::AUDIT_ACTOR;
pub use book::{BookEffect, BookPosition, PositionBook, PositionKey};
pub use event::{
    event_id_for, fill_event, fill_event_for_trade, AccountingEvent, EventKind, EventSide,
    EVENT_ID_PREFIX,
};
pub use ledger::{Applied, GlobalLedger, RealizedSeries, ReconRun};
pub use posting::{expand, Account, Entry, EntrySide, Posting, PostingError};
pub use reconcile::{reconcile, AccountingFinding, AccountingFindingKind, ReconInputs};
pub use recovery::{AccountingRecoveryAction, AccountingRecoveryReport};
pub use store::{LedgerStore, MemoryLedgerStore, StoredEvent};
pub use view::{ExposureSlice, NativeSlice, PortfolioInputs, PortfolioView};
