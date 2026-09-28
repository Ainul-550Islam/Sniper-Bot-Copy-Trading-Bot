//! Server wiring of the TASK 5 global risk / accounting layer.
//!
//! * [`DbLedgerStore`] / [`DbRiskStore`] — the PostgreSQL implementations
//!   of the core store contracts (migration 0015, [`AccountingRepo`]).
//! * [`attach`] — installs the durable stores BEFORE recovery and before any
//!   module runs (with no database the in-memory stores stay in place —
//!   identical semantics, process lifetime only).
//! * [`recover`] — restores runtime kill switches, rebuilds the ledger from
//!   the journal against the positions the persistence layer just
//!   restored, runs one accounting reconciliation and publishes the
//!   portfolio gauges.
//! * [`maintenance_tick`] — the periodic maintenance pass (flush pending
//!   journal writes, reconcile module truth against the ledger, refresh
//!   gauges). It is driven by the TASK 6 leased worker, so exactly one
//!   worker in the cluster runs it and every tick is fenced.
//!
//! Nothing here decides or books anything on its own; every mutation goes
//! through the core ledger / engine.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use bot_core::accounting::{
    AccountingFinding, AccountingRecoveryReport, BookPosition, Entry, LedgerStore, StoredEvent,
};
use bot_core::db::accounting::AccountingRepo;
use bot_core::db::Database;
use bot_core::global_risk::{GlobalRiskDecision, KillSwitchEvent, KillSwitchState, RiskStore};
use bot_core::reconciliation::QuantityTolerance;
use bot_core::state::Shared;

/// PostgreSQL journal of the global ledger.
pub struct DbLedgerStore {
    db: Arc<Database>,
}

impl DbLedgerStore {
    /// Store over `db`.
    pub fn new(db: Arc<Database>) -> Self {
        DbLedgerStore { db }
    }

    fn repo(&self) -> AccountingRepo {
        AccountingRepo::new(self.db.clone())
    }
}

#[async_trait]
impl LedgerStore for DbLedgerStore {
    async fn record_event(&self, stored: &StoredEvent, entry: &Entry) -> Option<bool> {
        match self.repo().record_event(stored, entry).await {
            Ok(new) => Some(new),
            Err(e) => {
                warn!(error = %e, event = %stored.event_id, "ledger event journal write failed");
                None
            }
        }
    }

    async fn load_events(&self) -> Option<Vec<StoredEvent>> {
        match self.repo().load_events().await {
            Ok(v) => Some(v),
            Err(e) => {
                warn!(error = %e, "ledger event journal read failed");
                None
            }
        }
    }

    async fn upsert_position(&self, position: &BookPosition) -> bool {
        match self.repo().upsert_position(position).await {
            Ok(()) => true,
            Err(e) => {
                debug!(error = %e, key = %position.key.as_string(), "global position snapshot failed");
                false
            }
        }
    }

    async fn append_finding(&self, finding: &AccountingFinding) -> bool {
        match self.repo().append_finding(finding).await {
            Ok(()) => true,
            Err(e) => {
                warn!(error = %e, finding = %finding.finding_id, "accounting finding journal failed");
                false
            }
        }
    }

    async fn recent_findings(&self, limit: usize) -> Option<Vec<AccountingFinding>> {
        self.repo().recent_findings(limit as i64).await.ok()
    }
}

/// PostgreSQL journal of the global risk engine.
pub struct DbRiskStore {
    db: Arc<Database>,
}

impl DbRiskStore {
    /// Store over `db`.
    pub fn new(db: Arc<Database>) -> Self {
        DbRiskStore { db }
    }

    fn repo(&self) -> AccountingRepo {
        AccountingRepo::new(self.db.clone())
    }
}

#[async_trait]
impl RiskStore for DbRiskStore {
    async fn record_decision(&self, decision: &GlobalRiskDecision) -> bool {
        match self.repo().record_decision(decision).await {
            Ok(()) => true,
            Err(e) => {
                debug!(error = %e, decision = %decision.decision_id, "global risk decision journal failed");
                false
            }
        }
    }

    async fn recent_decisions(&self, limit: usize) -> Option<Vec<GlobalRiskDecision>> {
        self.repo().recent_decisions(limit as i64).await.ok()
    }

    async fn upsert_kill_switch(&self, state: &KillSwitchState) -> bool {
        match self.repo().upsert_kill_switch(state).await {
            Ok(()) => true,
            Err(e) => {
                warn!(error = %e, scope = %state.scope, "kill switch state write failed");
                false
            }
        }
    }

    async fn append_kill_switch_event(&self, event: &KillSwitchEvent) -> bool {
        match self.repo().append_kill_switch_event(event).await {
            Ok(()) => true,
            Err(e) => {
                warn!(error = %e, scope = %event.scope, "kill switch event write failed");
                false
            }
        }
    }

    async fn load_kill_switches(&self) -> Option<Vec<KillSwitchState>> {
        match self.repo().load_kill_switches().await {
            Ok(v) => Some(v),
            Err(e) => {
                warn!(error = %e, "kill switch state read failed");
                None
            }
        }
    }
}

/// Install the durable stores when a database is attached. Must run before
/// [`recover`] and before any module starts.
pub async fn attach(state: &Shared, db: Option<&Arc<Database>>) {
    if let Some(db) = db {
        state
            .ledger()
            .attach_store(Arc::new(DbLedgerStore::new(db.clone())))
            .await;
        state
            .global_risk()
            .attach_store(Arc::new(DbRiskStore::new(db.clone())))
            .await;
        info!("global ledger + risk journals attached (postgres)");
    } else {
        info!("global ledger + risk journals in memory (no database configured)");
    }
}

/// Startup recovery of the global layer (after the persistence layer
/// restored the module positions): kill switches, then the ledger, then one
/// reconciliation pass and the gauges.
pub async fn recover(state: &Shared) -> AccountingRecoveryReport {
    let restored = state.global_risk().restore().await;
    let positions = state.all_positions().await;
    let report = state.ledger().recover(&positions).await;
    info!(
        kill_switches_restored = restored,
        summary = %report.summary(),
        "global ledger recovered"
    );
    if !report.gaps.is_empty() {
        warn!(
            gaps = report.gaps.len(),
            "open module positions without ledger history — book an opening `correction` event per position (docs/ACCOUNTING-LEDGER.md §8) or accept the position_mismatch findings"
        );
    }
    maintenance_tick(state).await;
    report
}

/// How many recent OMS orders one reconciliation pass compares against the
/// ledger. The OMS keeps a bounded in-memory mirror; older orders are
/// terminal and already reconciled (their positions remain covered by the
/// position checks).
const RECON_ORDER_WINDOW: usize = 5_000;

/// How many recent module trades one reconciliation pass compares.
const RECON_TRADE_WINDOW: usize = 5_000;

/// One maintenance pass: flush pending journal writes, reconcile, refresh
/// the portfolio gauges.
pub async fn maintenance_tick(state: &Shared) {
    let ledger = state.ledger();
    let flushed = ledger.flush_pending().await;
    if flushed > 0 {
        info!(flushed, "pending ledger events reached the journal");
    }
    // The four record layers the accounting reconciliation compares
    // (TASK 5 §5): OMS orders (intent), module trades (fills), the ledger
    // and the positions. The OMS mirror is bounded, so the order window
    // matches the trade window.
    let orders = match state.orders() {
        Some(mgr) => mgr.list(RECON_ORDER_WINDOW).await,
        None => Vec::new(),
    };
    let positions = state.all_positions().await;
    let trades = state.trades(RECON_TRADE_WINDOW).await;
    let run = ledger
        .reconcile(
            &orders,
            &positions,
            &trades,
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    if !run.new.is_empty() {
        warn!(
            new = run.new.len(),
            total = run.findings.len(),
            "accounting reconciliation reported new findings"
        );
    }
    let view = state.global_risk().portfolio(state.marks().await).await;
    bot_core::accounting::metrics::publish_portfolio(&view);
}

// The TASK 5 maintenance pass is driven by the TASK 6 leased worker in
// `crates/server/src/ha.rs` (`LeaseRole::AccountingMaintenance`), so exactly
// one worker in the cluster runs it and every tick is fenced;
// `maintenance_tick` above is that job's body.
