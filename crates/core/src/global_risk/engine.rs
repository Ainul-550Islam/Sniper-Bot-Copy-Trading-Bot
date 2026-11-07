//! The global risk engine (TASK 5 §1): ONE authoritative, deterministic
//! portfolio-level decision in front of every module's own checks.
//!
//! `RiskEngine::check_entry` calls [`GlobalRiskEngine::decide`] first; only
//! an `Accept` proceeds to the module-specific limits that TASK 1–4 already
//! enforce. The engine reads exposure, realized PnL and drawdown from the
//! global ledger's book — never from a module's private view — so no module
//! can carry a conflicting second source of truth into the decision.
//!
//! Check order (the first failing check is the reason; every figure the
//! verdict used is journaled in the decision snapshot):
//!
//! 1. process-wide kill switch
//! 2. venue kill switch
//! 3. strategy kill switch
//! 4. reference rate present for every quote asset a reference-denominated
//!    limit needs
//! 5. `max_open_positions`
//! 6. `max_order_notional_ref`
//! 7. portfolio / wallet / venue / strategy / asset exposure caps
//! 8. `max_daily_loss_ref`
//! 9. `max_drawdown_ref` / `max_drawdown_pct`
//!
//! Every limit defaults to `0` = off, so a suite without a `[global_risk]`
//! section behaves exactly as before TASK 5 (kill switches excepted, which
//! are empty by default).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::audit;
use super::decision::{
    decision_id, DecisionSnapshot, GlobalRejectReason, GlobalRiskDecision, GlobalRiskRequest,
    GlobalVerdict,
};
use super::kill_switch::{KillScope, KillSwitchEvent, KillSwitches, SwitchOutcome};
use super::metrics;
use super::store::{MemoryRiskStore, RiskEntryReservation, RiskStore};
use crate::accounting::{GlobalLedger, PortfolioInputs, PortfolioView, PositionKey};
use crate::config::GlobalRiskConfig;
use crate::events::EventBus;
use crate::models::Venue;

/// Process facts the engine cannot read itself.
#[derive(Debug, Clone, Default)]
pub struct DecisionContext {
    /// `AppState::kill_switch()` at decision time.
    pub global_kill: bool,
    /// Latest mark per base asset (from the modules' operational positions).
    pub marks: HashMap<String, f64>,
}

/// How many decisions the engine keeps in memory for the API.
const RECENT_DECISIONS: usize = 256;

/// Config can also be installed programmatically, bypassing file validation.
/// Keep the trading gate fail-closed for malformed limits and rates.
fn global_risk_config_is_valid(config: &GlobalRiskConfig) -> bool {
    let limits = [
        config.capital_base_ref,
        config.max_portfolio_exposure_ref,
        config.max_wallet_exposure_ref,
        config.max_venue_exposure_ref,
        config.max_strategy_exposure_ref,
        config.max_asset_exposure_ref,
        config.max_order_notional_ref,
        config.max_daily_loss_ref,
        config.max_drawdown_ref,
    ];
    !config.reference_asset.trim().is_empty()
        && limits
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
        && config.max_drawdown_pct.is_finite()
        && (0.0..=1.0).contains(&config.max_drawdown_pct)
        && config
            .reference_rates
            .iter()
            .all(|(asset, rate)| !asset.trim().is_empty() && rate.is_finite() && *rate > 0.0)
}

#[derive(Debug, Clone)]
struct ReservedEntry {
    module: crate::models::BotModule,
    organization: Option<crate::tenant::OrganizationId>,
    wallet: String,
    venue: String,
    strategy: String,
    asset: String,
    quote_asset: String,
    requested_quote: f64,
    requested_ref: f64,
    expires_at: chrono::DateTime<Utc>,
    distributed: bool,
}

#[derive(Default)]
struct ReservationBook {
    /// Serialize aggregate-cap checks through creation of their reservation.
    gate: Mutex<()>,
    entries: std::sync::Mutex<HashMap<String, ReservedEntry>>,
}

struct ReservationLease {
    id: String,
    book: std::sync::Weak<ReservationBook>,
    store: Option<Arc<dyn RiskStore>>,
    released: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for ReservationLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReservationLease")
            .field("id", &self.id)
            .finish()
    }
}

impl ReservationLease {
    async fn release_now(&self) {
        if self
            .released
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        if let Some(book) = self.book.upgrade() {
            book.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.id);
        }
        if let Some(store) = &self.store {
            store.release_entry_reservation(&self.id).await;
        }
    }
}

impl Drop for ReservationLease {
    fn drop(&mut self) {
        if self
            .released
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        if let Some(book) = self.book.upgrade() {
            book.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.id);
        }
        if let (Some(store), Ok(runtime)) =
            (self.store.clone(), tokio::runtime::Handle::try_current())
        {
            let id = self.id.clone();
            runtime.spawn(async move {
                store.release_entry_reservation(&id).await;
            });
        }
    }
}

/// Holds an accepted aggregate-capacity reservation for the lifetime of the
/// corresponding module entry decision. Clones share one lease.
#[derive(Debug, Clone, Default)]
pub struct GlobalRiskReservation {
    _lease: Option<Arc<ReservationLease>>,
}

impl GlobalRiskReservation {
    /// Reservation identity, excluded when the caller adds other pending
    /// entries to its own module-level limits.
    pub fn id(&self) -> Option<&str> {
        self._lease.as_ref().map(|lease| lease.id.as_str())
    }

    /// Release immediately when the caller has an explicit completion point.
    /// Ordinary drops release asynchronously and rely on lease expiry on crash.
    pub async fn release(self) {
        if let Some(lease) = self._lease {
            lease.release_now().await;
        }
    }

    fn new(id: String, book: &Arc<ReservationBook>, store: Option<Arc<dyn RiskStore>>) -> Self {
        Self {
            _lease: Some(Arc::new(ReservationLease {
                id,
                book: Arc::downgrade(book),
                store,
                released: std::sync::atomic::AtomicBool::new(false),
            })),
        }
    }
}

/// The engine. Constructed once by `AppState`; reached through
/// `state.global_risk()`.
pub struct GlobalRiskEngine {
    config: RwLock<GlobalRiskConfig>,
    ledger: Arc<GlobalLedger>,
    switches: KillSwitches,
    store: RwLock<Arc<dyn RiskStore>>,
    /// False until durable accounting recovery has proved the deployment
    /// portfolio complete. In-memory/no-database operation starts ready.
    portfolio_ready: RwLock<bool>,
    bus: EventBus,
    replica_id: String,
    seq: AtomicU64,
    recent: RwLock<VecDeque<GlobalRiskDecision>>,
    reservations: Arc<ReservationBook>,
}

impl GlobalRiskEngine {
    /// Engine over `ledger` with `config` (kill lists applied immediately).
    pub fn new(
        config: GlobalRiskConfig,
        ledger: Arc<GlobalLedger>,
        bus: EventBus,
        replica_id: impl Into<String>,
    ) -> Self {
        let switches = KillSwitches::new();
        switches.apply_config(&config.killed_venue_list(), &config.killed_strategies);
        GlobalRiskEngine {
            config: RwLock::new(config),
            ledger,
            switches,
            store: RwLock::new(Arc::new(MemoryRiskStore::default())),
            portfolio_ready: RwLock::new(true),
            bus,
            replica_id: replica_id.into(),
            seq: AtomicU64::new(1),
            recent: RwLock::new(VecDeque::with_capacity(RECENT_DECISIONS)),
            reservations: Arc::new(ReservationBook::default()),
        }
    }

    /// Attach the durable journal (startup, before recovery).
    pub async fn attach_store(&self, store: Arc<dyn RiskStore>) {
        *self.store.write().await = store;
    }

    /// Mark whether the deployment-wide accounting view is trustworthy.
    /// Durable server startup clears this before recovery and restores it only
    /// when the journal replay and module-position reconciliation are clean.
    pub async fn set_portfolio_ready(&self, ready: bool) {
        *self.portfolio_ready.write().await = ready;
    }

    /// True when the deployment-wide accounting view may safely enforce
    /// aggregate portfolio limits.
    pub async fn portfolio_ready(&self) -> bool {
        *self.portfolio_ready.read().await
    }

    /// The journal in use.
    pub async fn store(&self) -> Arc<dyn RiskStore> {
        self.store.read().await.clone()
    }

    /// The ledger this engine reads.
    pub fn ledger(&self) -> &Arc<GlobalLedger> {
        &self.ledger
    }

    /// The kill-switch registry.
    pub fn switches(&self) -> &KillSwitches {
        &self.switches
    }

    /// Current configuration.
    pub async fn config(&self) -> GlobalRiskConfig {
        self.config.read().await.clone()
    }

    /// Replace the configuration (config reload). Re-applies the kill lists.
    pub async fn update_config(&self, config: GlobalRiskConfig) {
        self.switches
            .apply_config(&config.killed_venue_list(), &config.killed_strategies);
        *self.config.write().await = config;
        self.publish_switch_gauges();
    }

    /// Restore runtime-engaged kill switches from the journal (startup).
    pub async fn restore(&self) -> usize {
        let store = self.store().await;
        let n = match store.load_kill_switches().await {
            Some(states) => self.switches.restore(states),
            None => {
                metrics::count_journal_error("load_kill_switches");
                warn!("global risk: kill-switch journal unavailable — only configured switches are active");
                0
            }
        };
        self.publish_switch_gauges();
        if n > 0 {
            info!(restored = n, "global risk: runtime kill switches restored");
            audit::kill_switch(
                &self.bus,
                "restored",
                "kill_switches",
                &format!("restored={n}"),
            );
        }
        n
    }

    /// Portfolio inputs from the configuration + marks.
    pub async fn portfolio_inputs(&self, marks: HashMap<String, f64>) -> PortfolioInputs {
        let cfg = self.config.read().await;
        PortfolioInputs {
            marks,
            rates: cfg.reference_rates.clone(),
            capital_base_ref: cfg.capital_base_ref,
            day: Utc::now().format("%Y-%m-%d").to_string(),
        }
    }

    /// The portfolio view under the current configuration.
    pub async fn portfolio(&self, marks: HashMap<String, f64>) -> PortfolioView {
        let inputs = self.portfolio_inputs(marks).await;
        let mut view = self.ledger.portfolio(&inputs).await;
        view.reference_asset = self.config.read().await.reference_asset.clone();
        view
    }

    /// Engage a venue / strategy switch at runtime (durable + audited).
    pub async fn engage(&self, scope: KillScope, reason: &str, actor: &str) -> SwitchOutcome {
        let outcome = self.switches.engage(scope.clone(), reason, actor);
        if outcome == SwitchOutcome::Changed {
            self.persist_switch(&scope, "engage", reason, actor).await;
        }
        outcome
    }

    /// Release a runtime-engaged switch (durable + audited).
    pub async fn release(&self, scope: KillScope, reason: &str, actor: &str) -> SwitchOutcome {
        let outcome = self.switches.release(&scope, reason, actor);
        if outcome == SwitchOutcome::Changed {
            self.persist_switch(&scope, "release", reason, actor).await;
        }
        outcome
    }

    async fn persist_switch(&self, scope: &KillScope, action: &str, reason: &str, actor: &str) {
        let store = self.store().await;
        let state =
            self.switches
                .get(scope)
                .unwrap_or_else(|| super::kill_switch::KillSwitchState {
                    scope: scope.clone(),
                    configured: false,
                    engaged: false,
                    reason: reason.to_string(),
                    actor: actor.to_string(),
                    updated_at: Utc::now(),
                });
        if !store.upsert_kill_switch(&state).await {
            metrics::count_journal_error("upsert_kill_switch");
        }
        let event = KillSwitchEvent {
            scope: scope.clone(),
            action: action.to_string(),
            reason: reason.to_string(),
            actor: actor.to_string(),
            replica_id: self.replica_id.clone(),
            ts: Utc::now(),
        };
        if !store.append_kill_switch_event(&event).await {
            metrics::count_journal_error("append_kill_switch_event");
        }
        metrics::count_kill_switch_change(scope.kind(), action);
        self.publish_switch_gauges();
        audit::kill_switch(
            &self.bus,
            action,
            &scope.as_string(),
            &format!("reason={reason} actor={actor}"),
        );
        info!(scope = %scope, action, reason, actor, "global risk: kill switch changed");
    }

    fn publish_switch_gauges(&self) {
        metrics::set_kill_switch_count(&self.switches.active());
    }

    /// Recent decisions (newest first).
    pub async fn recent_decisions(&self, limit: usize) -> Vec<GlobalRiskDecision> {
        self.recent
            .read()
            .await
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// The one deployment-wide decision for `req` (see module docs).
    pub async fn decide(
        &self,
        req: &GlobalRiskRequest,
        ctx: &DecisionContext,
    ) -> GlobalRiskDecision {
        let decision = self.decide_inner(req, ctx, true).await;
        self.record(&decision).await;
        decision
    }

    /// Entry-path variant of [`Self::decide`] that atomically reserves the
    /// accepted request against concurrent aggregate-capacity checks. The
    /// caller must retain the returned lease until that entry has either
    /// completed or failed; dropping it releases the reservation.
    pub async fn decide_with_reservation(
        &self,
        req: &GlobalRiskRequest,
        ctx: &DecisionContext,
    ) -> (GlobalRiskDecision, Option<GlobalRiskReservation>) {
        let _gate = self.reservations.gate.lock().await;
        let store = self.store().await;
        let _distributed_guard = if store.has_distributed_entry_reservations() {
            match store.acquire_entry_admission().await {
                Some(guard) => Some(guard),
                None => {
                    let decision = self
                        .reservation_failure(
                            req,
                            ctx,
                            true,
                            "cluster entry-admission lock unavailable",
                        )
                        .await;
                    return (decision, None);
                }
            }
        } else {
            None
        };
        if store.has_distributed_entry_reservations() {
            let Some(active) = store.active_entry_reservations().await else {
                let decision = self
                    .reservation_failure(req, ctx, true, "cluster entry reservations unavailable")
                    .await;
                return (decision, None);
            };
            self.install_distributed_reservations(active);
        }
        let mut decision = self.decide_inner(req, ctx, true).await;
        let expires_at = Utc::now() + chrono::Duration::seconds(300);
        let reservation = if decision.accepted() {
            let record = Self::reservation_record(&decision, req, None, expires_at);
            if store.has_distributed_entry_reservations()
                && !store.create_entry_reservation(&record).await
            {
                decision.verdict = GlobalVerdict::Reject;
                decision.reason = Some(GlobalRejectReason::InvalidPortfolio);
                decision.detail = "cluster entry reservation could not be persisted".into();
                None
            } else {
                self.reserve_decision(&decision, req, None, expires_at, store.clone())
            }
        } else {
            None
        };
        self.record(&decision).await;
        (decision, reservation)
    }

    /// Tenant-runtime gate: apply process, venue and strategy kill switches
    /// without reading or returning the deployment-wide portfolio. Per-order
    /// notional remains enforceable; configured aggregate portfolio caps fail
    /// closed until a tenant-scoped aggregate source is attached.
    pub async fn decide_tenant(
        &self,
        req: &GlobalRiskRequest,
        ctx: &DecisionContext,
    ) -> GlobalRiskDecision {
        let decision = self.decide_inner(req, ctx, false).await;
        self.record(&decision).await;
        decision
    }

    /// Tenant entry-path decision with a tenant-isolated in-flight reservation.
    pub async fn decide_tenant_with_reservation(
        &self,
        req: &GlobalRiskRequest,
        ctx: &DecisionContext,
        organization: crate::tenant::OrganizationId,
    ) -> (GlobalRiskDecision, Option<GlobalRiskReservation>) {
        let _gate = self.reservations.gate.lock().await;
        let store = self.store().await;
        let _distributed_guard = if store.has_distributed_entry_reservations() {
            match store.acquire_entry_admission().await {
                Some(guard) => Some(guard),
                None => {
                    let decision = self
                        .reservation_failure(
                            req,
                            ctx,
                            false,
                            "cluster entry-admission lock unavailable",
                        )
                        .await;
                    return (decision, None);
                }
            }
        } else {
            None
        };
        if store.has_distributed_entry_reservations() {
            let Some(active) = store.active_entry_reservations().await else {
                let decision = self
                    .reservation_failure(req, ctx, false, "cluster entry reservations unavailable")
                    .await;
                return (decision, None);
            };
            self.install_distributed_reservations(active);
        }
        let mut decision = self.decide_inner(req, ctx, false).await;
        let expires_at = Utc::now() + chrono::Duration::seconds(300);
        let reservation = if decision.accepted() {
            let record = Self::reservation_record(&decision, req, Some(organization), expires_at);
            if store.has_distributed_entry_reservations()
                && !store.create_entry_reservation(&record).await
            {
                decision.verdict = GlobalVerdict::Reject;
                decision.reason = Some(GlobalRejectReason::InvalidPortfolio);
                decision.detail = "cluster entry reservation could not be persisted".into();
                None
            } else {
                self.reserve_decision(
                    &decision,
                    req,
                    Some(organization),
                    expires_at,
                    store.clone(),
                )
            }
        } else {
            None
        };
        self.record(&decision).await;
        (decision, reservation)
    }

    fn reservation_record(
        decision: &GlobalRiskDecision,
        req: &GlobalRiskRequest,
        organization: Option<crate::tenant::OrganizationId>,
        expires_at: chrono::DateTime<Utc>,
    ) -> RiskEntryReservation {
        RiskEntryReservation {
            reservation_id: decision.decision_id.clone(),
            module: req.module,
            organization,
            venue: req.venue,
            wallet: req.wallet.clone(),
            strategy: req.strategy.clone(),
            asset: req.asset.clone(),
            quote_asset: req.quote_asset.clone(),
            requested_quote: req.requested_quote,
            requested_ref: decision.snapshot.requested_ref,
            expires_at,
        }
    }

    fn reserve_decision(
        &self,
        decision: &GlobalRiskDecision,
        req: &GlobalRiskRequest,
        organization: Option<crate::tenant::OrganizationId>,
        expires_at: chrono::DateTime<Utc>,
        store: Arc<dyn RiskStore>,
    ) -> Option<GlobalRiskReservation> {
        if !decision.accepted() {
            return None;
        }
        let id = decision.decision_id.clone();
        let row = ReservedEntry {
            module: req.module,
            organization,
            wallet: req.wallet.clone(),
            venue: req.venue.as_str().to_string(),
            strategy: req.strategy.clone(),
            asset: req.asset.clone(),
            quote_asset: req.quote_asset.clone(),
            requested_quote: req.requested_quote,
            requested_ref: decision.snapshot.requested_ref,
            expires_at,
            distributed: store.has_distributed_entry_reservations(),
        };
        let mut entries = self
            .reservations
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(id.clone(), row);
        Some(GlobalRiskReservation::new(
            id,
            &self.reservations,
            store.has_distributed_entry_reservations().then_some(store),
        ))
    }

    fn install_distributed_reservations(&self, active: Vec<RiskEntryReservation>) {
        let mut entries = self
            .reservations
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| !entry.distributed);
        for reservation in active {
            if reservation.expires_at <= Utc::now() {
                continue;
            }
            entries.insert(
                reservation.reservation_id,
                ReservedEntry {
                    module: reservation.module,
                    organization: reservation.organization,
                    wallet: reservation.wallet,
                    venue: reservation.venue.as_str().to_string(),
                    strategy: reservation.strategy,
                    asset: reservation.asset,
                    quote_asset: reservation.quote_asset,
                    requested_quote: reservation.requested_quote,
                    requested_ref: reservation.requested_ref,
                    expires_at: reservation.expires_at,
                    distributed: true,
                },
            );
        }
    }

    async fn reservation_failure(
        &self,
        req: &GlobalRiskRequest,
        ctx: &DecisionContext,
        include_deployment_portfolio: bool,
        detail: &str,
    ) -> GlobalRiskDecision {
        let mut decision = self
            .decide_inner(req, ctx, include_deployment_portfolio)
            .await;
        decision.verdict = GlobalVerdict::Reject;
        decision.reason = Some(GlobalRejectReason::InvalidPortfolio);
        decision.detail = detail.to_string();
        self.record(&decision).await;
        decision
    }

    /// Other in-flight entries in this module/tenant scope. The current
    /// decision's own reservation is omitted by id.
    pub fn reserved_entry_usage(
        &self,
        module: crate::models::BotModule,
        organization: Option<crate::tenant::OrganizationId>,
        quote_asset: &str,
        asset: &str,
        exclude_id: Option<&str>,
    ) -> (usize, f64, bool) {
        let entries = self
            .reservations
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut count = 0_usize;
        let mut exposure = 0.0_f64;
        let mut duplicate_asset = false;
        for (id, entry) in entries.iter() {
            if exclude_id == Some(id.as_str())
                || entry.expires_at <= Utc::now()
                || entry.module != module
                || entry.organization != organization
            {
                continue;
            }
            count = count.saturating_add(1);
            if entry.quote_asset == quote_asset {
                exposure += entry.requested_quote;
            }
            if entry.asset == asset {
                duplicate_asset = true;
            }
        }
        (count, exposure, duplicate_asset)
    }

    async fn decide_inner(
        &self,
        req: &GlobalRiskRequest,
        ctx: &DecisionContext,
        include_deployment_portfolio: bool,
    ) -> GlobalRiskDecision {
        let cfg = self.config.read().await.clone();
        let ts = Utc::now();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let id = decision_id(req, ts, seq);
        let mut snapshot = DecisionSnapshot {
            capital_base_ref: if include_deployment_portfolio {
                cfg.capital_base_ref
            } else {
                0.0
            },
            ..Default::default()
        };

        let outcome: Result<(), (GlobalRejectReason, String)> = 'check: {
            // 0. Request sanity.
            if !req.requested_quote.is_finite() || req.requested_quote <= 0.0 {
                break 'check Err((
                    GlobalRejectReason::InvalidRequest,
                    format!(
                        "requested size {} is not a positive number",
                        req.requested_quote
                    ),
                ));
            }
            if [
                req.wallet.as_str(),
                req.strategy.as_str(),
                req.asset.as_str(),
                req.quote_asset.as_str(),
            ]
            .iter()
            .any(|value| value.trim().is_empty() || value.trim() != *value)
            {
                break 'check Err((
                    GlobalRejectReason::InvalidRequest,
                    "wallet, strategy, asset, and quote asset must be non-empty and trimmed".into(),
                ));
            }
            // 1–3. Switches.
            if ctx.global_kill {
                break 'check Err((
                    GlobalRejectReason::GlobalKillSwitch,
                    "process-wide kill switch engaged".into(),
                ));
            }
            if let Some(s) = self.switches.venue_killed(req.venue) {
                break 'check Err((
                    GlobalRejectReason::VenueKillSwitch,
                    format!(
                        "venue {} killed ({}{}: {})",
                        req.venue.as_str(),
                        if s.configured { "config" } else { "operator" },
                        if s.engaged && s.configured {
                            "+operator"
                        } else {
                            ""
                        },
                        s.reason
                    ),
                ));
            }
            if let Some(s) = self.switches.strategy_killed(&req.strategy) {
                break 'check Err((
                    GlobalRejectReason::StrategyKillSwitch,
                    format!(
                        "strategy {} killed ({}: {})",
                        req.strategy,
                        if s.configured { "config" } else { "operator" },
                        s.reason
                    ),
                ));
            }
            if !global_risk_config_is_valid(&cfg) {
                break 'check Err((
                    GlobalRejectReason::InvalidPortfolio,
                    "global risk configuration contains an invalid limit or reference rate".into(),
                ));
            }
            if !include_deployment_portfolio {
                // A tenant must never inherit the deployment-wide portfolio
                // snapshot. Until tenant-aggregated wallet/venue/asset,
                // total exposure, position-count, loss and drawdown inputs
                // are available here, fail closed if any such limit is on.
                // The per-order notional cap is independent of portfolio
                // state, so it can still be enforced using its quote rate.
                if cfg.max_order_notional_ref > 0.0 {
                    let Some(rate) = cfg.reference_rates.get(&req.quote_asset).copied() else {
                        break 'check Err((
                            GlobalRejectReason::ReferenceRateMissing,
                            format!(
                                "no [global_risk.reference_rates] entry for {} — order notional cannot be evaluated",
                                req.quote_asset
                            ),
                        ));
                    };
                    let requested_ref = req.requested_quote * rate;
                    snapshot.rate = rate;
                    snapshot.requested_ref = requested_ref;
                    if !requested_ref.is_finite() {
                        break 'check Err((
                            GlobalRejectReason::InvalidRequest,
                            "requested size overflows when converted to the reference asset".into(),
                        ));
                    }
                    if requested_ref > cfg.max_order_notional_ref {
                        break 'check Err((
                            GlobalRejectReason::OrderNotional,
                            format!(
                                "order notional {:.4} {} exceeds max {:.4}",
                                requested_ref, cfg.reference_asset, cfg.max_order_notional_ref
                            ),
                        ));
                    }
                }
                let tenant_portfolio_caps_enabled = [
                    cfg.max_portfolio_exposure_ref,
                    cfg.max_wallet_exposure_ref,
                    cfg.max_venue_exposure_ref,
                    cfg.max_strategy_exposure_ref,
                    cfg.max_asset_exposure_ref,
                    cfg.max_daily_loss_ref,
                    cfg.max_drawdown_ref,
                    cfg.max_drawdown_pct,
                ]
                .iter()
                .any(|limit| *limit > 0.0)
                    || cfg.max_open_positions > 0;
                if tenant_portfolio_caps_enabled {
                    break 'check Err((
                        GlobalRejectReason::InvalidPortfolio,
                        "tenant-scoped aggregate portfolio data is unavailable; refusing to bypass configured global limits".into(),
                    ));
                }
                break 'check Ok(());
            }

            let aggregate_limits_enabled = [
                cfg.max_portfolio_exposure_ref,
                cfg.max_wallet_exposure_ref,
                cfg.max_venue_exposure_ref,
                cfg.max_strategy_exposure_ref,
                cfg.max_asset_exposure_ref,
                cfg.max_daily_loss_ref,
                cfg.max_drawdown_ref,
                cfg.max_drawdown_pct,
            ]
            .iter()
            .any(|limit| *limit > 0.0)
                || cfg.max_open_positions > 0;
            if aggregate_limits_enabled {
                let refreshed = self.ledger.refresh_from_store().await;
                if !refreshed {
                    *self.portfolio_ready.write().await = false;
                }
                let ready = *self.portfolio_ready.read().await;
                let pending = self.ledger.pending_count().await;
                if !ready || !refreshed || pending > 0 {
                    break 'check Err((
                        GlobalRejectReason::InvalidPortfolio,
                        format!(
                            "deployment accounting is incomplete (ready={ready}, pending_events={pending}); refusing to evaluate aggregate limits"
                        ),
                    ));
                }
            }

            // 4. Reference rates — only when a reference-denominated limit is on.
            let inputs = PortfolioInputs {
                marks: ctx.marks.clone(),
                rates: cfg.reference_rates.clone(),
                capital_base_ref: cfg.capital_base_ref,
                day: ts.format("%Y-%m-%d").to_string(),
            };
            let view = self.ledger.portfolio(&inputs).await;
            let rate = inputs.rate(&req.quote_asset);
            snapshot.rate = rate.unwrap_or(0.0);
            snapshot.requested_ref = req.requested_quote * rate.unwrap_or(0.0);
            snapshot.portfolio_ref = view.total_exposure_ref;
            snapshot.wallet_ref = view
                .by_wallet
                .get(&req.wallet)
                .map(|s| s.exposure_ref)
                .unwrap_or(0.0);
            snapshot.venue_ref = view
                .by_venue
                .get(req.venue.as_str())
                .map(|s| s.exposure_ref)
                .unwrap_or(0.0);
            snapshot.strategy_ref = view
                .by_strategy
                .get(&req.strategy)
                .map(|s| s.exposure_ref)
                .unwrap_or(0.0);
            snapshot.asset_ref = view
                .by_asset
                .get(&req.asset)
                .map(|s| s.exposure_ref)
                .unwrap_or(0.0);
            snapshot.open_positions = view.open_positions;
            snapshot.realized_today_ref = view.realized_today_ref;
            snapshot.drawdown_ref = view.drawdown_ref();

            // Include accepted entries whose module-level operation is still
            // in flight. Their RAII leases are released on success, failure,
            // or cancellation; holding the admission gate through this
            // snapshot and the next reservation closes the local TOCTOU gap.
            {
                let pending = self
                    .reservations
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for entry in pending.values() {
                    if entry.expires_at <= ts || entry.organization.is_some() {
                        continue;
                    }
                    snapshot.portfolio_ref += entry.requested_ref;
                    if entry.wallet == req.wallet {
                        snapshot.wallet_ref += entry.requested_ref;
                    }
                    if entry.venue == req.venue.as_str() {
                        snapshot.venue_ref += entry.requested_ref;
                    }
                    if entry.strategy == req.strategy {
                        snapshot.strategy_ref += entry.requested_ref;
                    }
                    if entry.asset == req.asset {
                        snapshot.asset_ref += entry.requested_ref;
                    }
                    snapshot.open_positions = snapshot.open_positions.saturating_add(1);
                }
            }

            if rate.is_some() && !snapshot.requested_ref.is_finite() {
                break 'check Err((
                    GlobalRejectReason::InvalidRequest,
                    "requested size overflows when converted to the reference asset".into(),
                ));
            }
            if [
                snapshot.rate,
                snapshot.portfolio_ref,
                snapshot.wallet_ref,
                snapshot.venue_ref,
                snapshot.strategy_ref,
                snapshot.asset_ref,
                snapshot.realized_today_ref,
                snapshot.drawdown_ref,
                snapshot.capital_base_ref,
            ]
            .iter()
            .any(|value| !value.is_finite())
            {
                break 'check Err((
                    GlobalRejectReason::InvalidPortfolio,
                    "accounting snapshot contains a non-finite value; refusing entry".into(),
                ));
            }

            if cfg.needs_reference_rates() {
                if rate.is_none() {
                    break 'check Err((
                        GlobalRejectReason::ReferenceRateMissing,
                        format!(
                            "no [global_risk.reference_rates] entry for {} — reference-denominated limits cannot be evaluated",
                            req.quote_asset
                        ),
                    ));
                }
                if !view.missing_rates.is_empty() {
                    break 'check Err((
                        GlobalRejectReason::ReferenceRateMissing,
                        format!(
                            "open exposure on {} has no reference rate — portfolio figures would be understated",
                            view.missing_rates.join(", ")
                        ),
                    ));
                }
            }
            if cfg.max_daily_loss_ref > 0.0 && !view.missing_daily_loss_rates.is_empty() {
                break 'check Err((
                    GlobalRejectReason::ReferenceRateMissing,
                    format!(
                        "today's realized PnL on {} has no reference rate — daily-loss figures would be understated",
                        view.missing_daily_loss_rates.join(", ")
                    ),
                ));
            }
            if cfg.effective_drawdown_limit().is_some() && !view.missing_drawdown_rates.is_empty() {
                break 'check Err((
                    GlobalRejectReason::ReferenceRateMissing,
                    format!(
                        "cumulative realized PnL on {} has no reference rate — drawdown figures would be understated",
                        view.missing_drawdown_rates.join(", ")
                    ),
                ));
            }

            // 5. Open positions across every module.
            if cfg.max_open_positions > 0 {
                let key = PositionKey {
                    module: req.module,
                    venue: req.venue,
                    wallet: req.wallet.clone(),
                    strategy: req.strategy.clone(),
                    asset: req.asset.clone(),
                    quote_asset: req.quote_asset.clone(),
                    mode: req.mode,
                };
                let already_open = self
                    .ledger
                    .book()
                    .await
                    .get(&key)
                    .map(|p| p.is_open())
                    .unwrap_or(false);
                if !already_open && view.open_positions >= cfg.max_open_positions {
                    break 'check Err((
                        GlobalRejectReason::MaxOpenPositions,
                        format!(
                            "{} open positions across every module (max {})",
                            view.open_positions, cfg.max_open_positions
                        ),
                    ));
                }
            }

            // 6. Per-order notional.
            let requested_ref = snapshot.requested_ref;
            if cfg.max_order_notional_ref > 0.0 && requested_ref > cfg.max_order_notional_ref {
                break 'check Err((
                    GlobalRejectReason::OrderNotional,
                    format!(
                        "order notional {:.4} {} exceeds max {:.4}",
                        requested_ref, cfg.reference_asset, cfg.max_order_notional_ref
                    ),
                ));
            }

            // 7. Exposure caps.
            for (limit, current, reason, label) in [
                (
                    cfg.max_portfolio_exposure_ref,
                    snapshot.portfolio_ref,
                    GlobalRejectReason::PortfolioExposure,
                    "portfolio",
                ),
                (
                    cfg.max_wallet_exposure_ref,
                    snapshot.wallet_ref,
                    GlobalRejectReason::WalletExposure,
                    "wallet",
                ),
                (
                    cfg.max_venue_exposure_ref,
                    snapshot.venue_ref,
                    GlobalRejectReason::VenueExposure,
                    "venue",
                ),
                (
                    cfg.max_strategy_exposure_ref,
                    snapshot.strategy_ref,
                    GlobalRejectReason::StrategyExposure,
                    "strategy",
                ),
                (
                    cfg.max_asset_exposure_ref,
                    snapshot.asset_ref,
                    GlobalRejectReason::AssetExposure,
                    "asset",
                ),
            ] {
                if limit > 0.0 && current + requested_ref > limit {
                    break 'check Err((
                        reason,
                        format!(
                            "{label} exposure {:.4} + {:.4} would exceed cap {:.4} {}",
                            current, requested_ref, limit, cfg.reference_asset
                        ),
                    ));
                }
            }

            // 8. Daily loss.
            if cfg.max_daily_loss_ref > 0.0
                && snapshot.realized_today_ref <= -cfg.max_daily_loss_ref
            {
                break 'check Err((
                    GlobalRejectReason::DailyLoss,
                    format!(
                        "net realized today {:.4} {} reached the daily loss limit {:.4}",
                        snapshot.realized_today_ref, cfg.reference_asset, cfg.max_daily_loss_ref
                    ),
                ));
            }

            // 9. Drawdown.
            if let Some(limit) = cfg.effective_drawdown_limit() {
                if snapshot.drawdown_ref >= limit {
                    break 'check Err((
                        GlobalRejectReason::Drawdown,
                        format!(
                            "drawdown {:.4} {} from the realized peak reached the limit {:.4}",
                            snapshot.drawdown_ref, cfg.reference_asset, limit
                        ),
                    ));
                }
            }
            Ok(())
        };

        let decision = match outcome {
            Ok(()) => GlobalRiskDecision {
                decision_id: id,
                ts,
                request: req.clone(),
                verdict: GlobalVerdict::Accept,
                reason: None,
                detail: "ok".into(),
                snapshot,
                replica_id: self.replica_id.clone(),
            },
            Err((reason, detail)) => GlobalRiskDecision {
                decision_id: id,
                ts,
                request: req.clone(),
                verdict: GlobalVerdict::Reject,
                reason: Some(reason),
                detail,
                snapshot,
                replica_id: self.replica_id.clone(),
            },
        };
        decision
    }

    async fn record(&self, decision: &GlobalRiskDecision) {
        {
            let mut recent = self.recent.write().await;
            if recent.len() >= RECENT_DECISIONS {
                recent.pop_front();
            }
            recent.push_back(decision.clone());
        }
        match decision.verdict {
            GlobalVerdict::Accept => {
                metrics::count_decision("accept", "-");
                debug!(decision = %decision.decision_id, "global risk: accept");
            }
            GlobalVerdict::Reject => {
                let reason = decision.reason.map(|r| r.as_str()).unwrap_or("-");
                metrics::count_decision("reject", reason);
                metrics::count_rejection(reason);
                audit::decision(&self.bus, decision);
                info!(
                    decision = %decision.decision_id,
                    module = %decision.request.module,
                    reason,
                    detail = %decision.detail,
                    "global risk: reject"
                );
            }
        }
        let store = self.store().await;
        if !store.record_decision(decision).await {
            metrics::count_journal_error("record_decision");
        }
    }
}

/// Quote asset a venue settles in (`SOL` on Solana venues, `USDC` on the
/// Polymarket CLOB, `SOL` for the paper venue — its fills are SOL-denominated
/// launches unless a module says otherwise).
pub fn quote_asset_for(venue: Venue) -> &'static str {
    match venue {
        Venue::PolymarketClob => "USDC",
        _ => "SOL",
    }
}

/// Strategy label conventions shared by the modules.
pub fn strategy_label(module: crate::models::BotModule, detail: Option<&str>) -> String {
    match (module, detail) {
        (crate::models::BotModule::Copy, Some(leader)) if !leader.is_empty() => {
            format!("copy:{leader}")
        }
        (crate::models::BotModule::Polymarket, Some(s)) if !s.is_empty() => s.to_string(),
        (m, _) => m.as_str().to_string(),
    }
}
