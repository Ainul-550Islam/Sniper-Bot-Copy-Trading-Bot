//! Restart recovery of the global ledger (TASK 5 §6).
//!
//! On startup — before any module trades — the server calls
//! [`GlobalLedger::recover`]. It rebuilds the book, the realized-PnL series
//! (daily loss / drawdown inputs) and the duplicate index from the durable
//! journal, replaying every event through the same idempotent path a live
//! submission takes, so nothing is booked twice and nothing is booked
//! blindly: a module position that has no ledger history is REPORTED as a
//! gap (an operator books an explicit opening `Correction` if it is real),
//! never synthesised.
//!
//! | action | meaning |
//! |---|---|
//! | `rebuilt` | one journaled event re-applied to the book |
//! | `duplicate_skipped` | a journaled event id was already present |
//! | `pending_flushed` | an event parked before the restart reached the journal |
//! | `gap_reported` | an open module position has no ledger history |
//! | `journal_unavailable` | the journal could not be read — the ledger starts empty and reconciliation will flag every position |

use serde::Serialize;

use super::audit;
use super::ledger::GlobalLedger;
use super::metrics;
use crate::models::Position;

/// Recovery action vocabulary (closed set — bounded metric label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingRecoveryAction {
    /// Journaled event re-applied.
    Rebuilt,
    /// Journaled event skipped (already present).
    DuplicateSkipped,
    /// Pending event flushed to the journal.
    PendingFlushed,
    /// Module position without ledger history reported.
    GapReported,
    /// Journal unreadable.
    JournalUnavailable,
}

impl AccountingRecoveryAction {
    /// Every action, stable order.
    pub const ALL: [AccountingRecoveryAction; 5] = [
        AccountingRecoveryAction::Rebuilt,
        AccountingRecoveryAction::DuplicateSkipped,
        AccountingRecoveryAction::PendingFlushed,
        AccountingRecoveryAction::GapReported,
        AccountingRecoveryAction::JournalUnavailable,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountingRecoveryAction::Rebuilt => "rebuilt",
            AccountingRecoveryAction::DuplicateSkipped => "duplicate_skipped",
            AccountingRecoveryAction::PendingFlushed => "pending_flushed",
            AccountingRecoveryAction::GapReported => "gap_reported",
            AccountingRecoveryAction::JournalUnavailable => "journal_unavailable",
        }
    }
}

/// What recovery did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AccountingRecoveryReport {
    /// Whether the journal answered.
    pub journal_available: bool,
    /// Events re-applied.
    pub rebuilt: usize,
    /// Journaled ids that were already present.
    pub duplicates_skipped: usize,
    /// Pending events flushed.
    pub pending_flushed: usize,
    /// Open aggregated positions after the rebuild.
    pub open_positions: usize,
    /// Module position ids with no ledger history.
    pub gaps: Vec<String>,
}

impl AccountingRecoveryReport {
    /// Single-line summary for logs / audit.
    pub fn summary(&self) -> String {
        format!(
            "journal_available={} rebuilt={} duplicates_skipped={} pending_flushed={} open_positions={} gaps={}",
            self.journal_available,
            self.rebuilt,
            self.duplicates_skipped,
            self.pending_flushed,
            self.open_positions,
            self.gaps.len()
        )
    }
}

impl GlobalLedger {
    /// Rebuild the ledger from the durable journal and check the restored
    /// module positions for ledger gaps. Deterministic: the same journal and
    /// the same positions produce the same report.
    pub async fn recover(&self, module_positions: &[Position]) -> AccountingRecoveryReport {
        let mut report = AccountingRecoveryReport::default();
        let store = self.store().await;
        match store.load_events().await {
            Some(events) => {
                report.journal_available = true;
                let (applied, dup) = self.hydrate(events).await;
                report.rebuilt = applied;
                report.duplicates_skipped = dup;
                metrics::count_recovery_action(AccountingRecoveryAction::Rebuilt.as_str(), applied);
                metrics::count_recovery_action(
                    AccountingRecoveryAction::DuplicateSkipped.as_str(),
                    dup,
                );
            }
            None => {
                report.journal_available = false;
                metrics::count_recovery_action(
                    AccountingRecoveryAction::JournalUnavailable.as_str(),
                    1,
                );
                audit::recovery(
                    &self.bus,
                    AccountingRecoveryAction::JournalUnavailable.as_str(),
                    "ledger journal could not be read — starting with an empty book",
                );
            }
        }
        report.pending_flushed = self.flush_pending().await;
        metrics::count_recovery_action(
            AccountingRecoveryAction::PendingFlushed.as_str(),
            report.pending_flushed,
        );
        let book = self.book().await;
        report.open_positions = book.open_count();
        for p in module_positions {
            if p.status.is_terminal() || p.qty <= 1e-9 {
                continue;
            }
            if book.by_position_id(&p.id).next().is_none() {
                report.gaps.push(p.id.clone());
                audit::recovery(
                    &self.bus,
                    AccountingRecoveryAction::GapReported.as_str(),
                    &format!(
                        "position={} symbol={} qty={:.8} has no ledger history — not synthesised",
                        p.id, p.symbol, p.qty
                    ),
                );
            }
        }
        metrics::count_recovery_action(
            AccountingRecoveryAction::GapReported.as_str(),
            report.gaps.len(),
        );
        audit::recovery(&self.bus, "completed", &report.summary());
        report
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::accounting::event::{fill_event, EventSide};
    use crate::accounting::store::MemoryLedgerStore;
    use crate::events::EventBus;
    use crate::models::{BotModule, ExecutionMode, TradeSource, Venue};
    use chrono::Utc;

    fn fill(
        reference: &str,
        side: EventSide,
        qty: f64,
        quote: f64,
    ) -> crate::accounting::AccountingEvent {
        fill_event(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "w",
            "copy:leader",
            "MINT",
            "SOL",
            side,
            qty,
            quote / qty,
            quote,
            0.0,
            ExecutionMode::Live,
            reference,
            None,
            Some("p-1".into()),
            Utc::now(),
            "",
        )
    }

    #[tokio::test]
    async fn restart_rebuilds_the_book_once_and_refuses_replays() {
        let store = Arc::new(MemoryLedgerStore::new());
        let life1 = GlobalLedger::new(EventBus::new(64), "r1");
        life1.attach_store(store.clone()).await;
        life1.submit(fill("s1", EventSide::Buy, 100.0, 1.0)).await;
        life1.submit(fill("s2", EventSide::Sell, 40.0, 0.8)).await;
        assert_eq!(store.len().await, 2);

        let life2 = GlobalLedger::new(EventBus::new(64), "r2");
        life2.attach_store(store.clone()).await;
        let report = life2.recover(&[]).await;
        assert!(report.journal_available);
        assert_eq!(report.rebuilt, 2);
        assert_eq!(report.duplicates_skipped, 0);
        assert_eq!(report.open_positions, 1);
        let p = life2.book().await;
        let pos = p.positions().next().unwrap();
        assert!((pos.qty - 60.0).abs() < 1e-9);
        assert!((pos.realized - 0.4).abs() < 1e-9);
        // A replayed fill after the restart is a duplicate, not a second booking.
        assert_eq!(
            life2.submit(fill("s2", EventSide::Sell, 40.0, 0.8)).await,
            crate::accounting::Applied::Duplicate
        );
        assert_eq!(store.len().await, 2);
        // Recovering twice is idempotent.
        let again = life2.recover(&[]).await;
        assert_eq!(again.rebuilt, 0);
        assert_eq!(again.duplicates_skipped, 2);
    }

    #[tokio::test]
    async fn gaps_are_reported_never_synthesised() {
        let ledger = GlobalLedger::new(EventBus::new(64), "r");
        let mut p = Position::new(
            "p-old".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            ExecutionMode::Live,
            "MINT".into(),
            "MINT".into(),
            "SOL".into(),
        );
        p.qty = 10.0;
        p.cost_basis = 1.0;
        let report = ledger.recover(std::slice::from_ref(&p)).await;
        assert_eq!(report.gaps, vec!["p-old".to_string()]);
        assert!(ledger.is_empty().await, "nothing was booked");
    }

    #[tokio::test]
    async fn unavailable_journal_is_reported() {
        let store = Arc::new(MemoryLedgerStore::new());
        store.set_unavailable(true);
        let ledger = GlobalLedger::new(EventBus::new(64), "r");
        ledger.attach_store(store).await;
        let report = ledger.recover(&[]).await;
        assert!(!report.journal_available);
        assert_eq!(report.rebuilt, 0);
    }
}
