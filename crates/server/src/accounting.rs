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

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::Row;
use tracing::{debug, info, warn};

use bot_core::accounting::{
    AccountingFinding, AccountingRecoveryReport, BookPosition, Entry, LedgerStore, StoredEvent,
};
use bot_core::db::accounting::AccountingRepo;
use bot_core::db::Database;
use bot_core::global_risk::{
    GlobalRiskDecision, KillSwitchEvent, KillSwitchState, RiskEntryReservation, RiskStore,
};
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
        let repo = self.repo();
        match repo.load_events().await {
            Ok(events) => match repo.position_ledger_coverage_complete().await {
                Ok(true) => Some(events),
                Ok(false) => {
                    warn!("module positions include realized activity without matching ledger history");
                    None
                }
                Err(error) => {
                    warn!(error = %error, "module-position ledger coverage check failed");
                    None
                }
            },
            Err(error) => {
                warn!(error = %error, "ledger event journal read failed");
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

/// Durable admission lease held from the risk snapshot read through
/// reservation insertion. It does not pin a pool connection; process crashes
/// release it by expiry.
struct PgEntryAdmissionGuard {
    db: Arc<Database>,
    organization_id: uuid::Uuid,
    holder_id: uuid::Uuid,
}

impl Drop for PgEntryAdmissionGuard {
    fn drop(&mut self) {
        let db = self.db.clone();
        let organization_id = self.organization_id;
        let holder_id = self.holder_id;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let pool = db.system_pool().unwrap_or_else(|| db.pool());
                if let Err(error) = sqlx::query(
                    "UPDATE global_risk_entry_admission_locks
                     SET holder_id = NULL, lease_until = now(), updated_at = now()
                     WHERE organization_id = $1 AND holder_id = $2",
                )
                .bind(organization_id)
                .bind(holder_id)
                .execute(pool)
                .await
                {
                    warn!(%error, %holder_id, "global risk admission lease release failed; it will expire");
                }
            });
        }
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

    /// Background risk reservations span replicas and therefore use the
    /// system identity, always with an explicit deployment-org predicate.
    fn risk_pool(&self) -> &sqlx::PgPool {
        self.db.system_pool().unwrap_or_else(|| self.db.pool())
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

    fn has_distributed_entry_reservations(&self) -> bool {
        true
    }

    async fn acquire_entry_admission(&self) -> Option<Box<dyn Send>> {
        let organization_id = bot_core::db::deployment_org::deployment_org_uuid(&self.db)
            .await
            .ok()?;
        let holder_id = uuid::Uuid::new_v4();
        let until = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let row = sqlx::query(
                "INSERT INTO global_risk_entry_admission_locks
                    (organization_id, holder_id, lease_until, updated_at)
                 VALUES ($1, $2, now() + INTERVAL '120 seconds', now())
                 ON CONFLICT (organization_id) DO UPDATE
                    SET holder_id = EXCLUDED.holder_id,
                        lease_until = EXCLUDED.lease_until,
                        updated_at = now()
                  WHERE global_risk_entry_admission_locks.lease_until <= now()
                 RETURNING holder_id",
            )
            .bind(organization_id)
            .bind(holder_id)
            .fetch_optional(self.risk_pool())
            .await
            .ok()?;
            if row.is_some() {
                return Some(Box::new(PgEntryAdmissionGuard {
                    db: self.db.clone(),
                    organization_id,
                    holder_id,
                }));
            }
            if tokio::time::Instant::now() >= until {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    async fn active_entry_reservations(&self) -> Option<Vec<RiskEntryReservation>> {
        let organization_id = bot_core::db::deployment_org::deployment_org_uuid(&self.db)
            .await
            .ok()?;
        sqlx::query(
            "DELETE FROM global_risk_entry_reservations
             WHERE organization_id = $1 AND expires_at <= now()",
        )
        .bind(organization_id)
        .execute(self.risk_pool())
        .await
        .ok()?;
        let rows = sqlx::query(
            "SELECT reservation_id, tenant_id, module, venue, wallet, strategy,
                    asset, quote_asset, requested_quote, requested_ref, expires_at
             FROM global_risk_entry_reservations
             WHERE organization_id = $1 AND expires_at > now()
             ORDER BY created_at, reservation_id",
        )
        .bind(organization_id)
        .fetch_all(self.risk_pool())
        .await
        .ok()?;
        rows.iter()
            .map(|row| {
                let module = bot_core::models::BotModule::from_str(
                    &row.try_get::<String, _>("module").ok()?,
                )
                .ok()?;
                let venue =
                    bot_core::models::Venue::parse(&row.try_get::<String, _>("venue").ok()?)?;
                let tenant_id: Option<uuid::Uuid> = row.try_get("tenant_id").ok()?;
                Some(RiskEntryReservation {
                    reservation_id: row.try_get("reservation_id").ok()?,
                    module,
                    organization: tenant_id.map(bot_core::tenant::OrganizationId::from),
                    venue,
                    wallet: row.try_get("wallet").ok()?,
                    strategy: row.try_get("strategy").ok()?,
                    asset: row.try_get("asset").ok()?,
                    quote_asset: row.try_get("quote_asset").ok()?,
                    requested_quote: row.try_get("requested_quote").ok()?,
                    requested_ref: row.try_get("requested_ref").ok()?,
                    expires_at: row.try_get("expires_at").ok()?,
                })
            })
            .collect()
    }

    async fn create_entry_reservation(&self, reservation: &RiskEntryReservation) -> bool {
        let Ok(organization_id) = bot_core::db::deployment_org::deployment_org_uuid(&self.db).await
        else {
            return false;
        };
        sqlx::query(
            "INSERT INTO global_risk_entry_reservations
                (organization_id, reservation_id, tenant_id, module, venue, wallet,
                 strategy, asset, quote_asset, requested_quote, requested_ref, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(organization_id)
        .bind(&reservation.reservation_id)
        .bind(reservation.organization.map(|org| org.as_uuid()))
        .bind(reservation.module.as_str())
        .bind(reservation.venue.as_str())
        .bind(&reservation.wallet)
        .bind(&reservation.strategy)
        .bind(&reservation.asset)
        .bind(&reservation.quote_asset)
        .bind(reservation.requested_quote)
        .bind(reservation.requested_ref)
        .bind(reservation.expires_at)
        .execute(self.risk_pool())
        .await
        .is_ok()
    }

    async fn release_entry_reservation(&self, reservation_id: &str) {
        let Ok(organization_id) = bot_core::db::deployment_org::deployment_org_uuid(&self.db).await
        else {
            return;
        };
        if let Err(error) = sqlx::query(
            "DELETE FROM global_risk_entry_reservations
             WHERE organization_id = $1 AND reservation_id = $2",
        )
        .bind(organization_id)
        .bind(reservation_id)
        .execute(self.risk_pool())
        .await
        {
            warn!(reservation_id, %error, "global risk reservation release failed; lease will expire");
        }
    }
}

/// Install the durable stores when a database is attached. Must run before
/// [`recover`] and before any module starts.
pub async fn attach(state: &Shared, db: Option<&Arc<Database>>) {
    if let Some(db) = db {
        state.global_risk().set_portfolio_ready(false).await;
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
    let pending = state.ledger().pending_count().await;
    let portfolio_ready = report.journal_available
        && report.invalid_events == 0
        && report.gaps.is_empty()
        && pending == 0;
    state
        .global_risk()
        .set_portfolio_ready(portfolio_ready)
        .await;
    info!(
        portfolio_ready,
        pending_events = pending,
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
