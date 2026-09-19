use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::config::{AppConfig, Config};
use crate::error::{BotError, BotResult};
use crate::events::{AppEvent, EventBus};
use crate::models::{
    BotModule, ExecutionMode, ModuleState, ModuleStatus, Position, PositionStatus, Trade,
    TradeSource,
};

/// A FIFO-bounded membership set: remembers the most recent `cap` keys and
/// silently evicts the oldest once full. Used to de-duplicate launch mints and
/// transaction signatures without letting memory grow without limit over a long
/// run. Evicting an old key is safe: a launch or signature that fell out of the
/// window is old enough that re-processing it is harmless (and it will not
/// re-appear on the feed anyway).
#[derive(Debug, Default)]
pub(crate) struct BoundedSet {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl BoundedSet {
    /// Membership check without inserting.
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.set.contains(key)
    }

    /// Insert `key`, evicting the oldest entries so the set stays `<= cap`.
    /// Returns `true` when the key was newly added, `false` if already present.
    pub(crate) fn insert(&mut self, key: &str, cap: usize) -> bool {
        let cap = cap.max(1);
        if self.set.contains(key) {
            return false;
        }
        self.set.insert(key.to_string());
        self.order.push_back(key.to_string());
        while self.order.len() > cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            } else {
                break;
            }
        }
        true
    }

    /// Remove `key` if present. Returns `true` when it was there.
    pub(crate) fn remove(&mut self, key: &str) -> bool {
        if !self.set.remove(key) {
            return false;
        }
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        true
    }

    pub(crate) fn len(&self) -> usize {
        self.set.len()
    }
}

/// Drop cooldown-expired entries (they can no longer gate anything) and enforce
/// a hard size cap by evicting the oldest timestamps. Called on insert into the
/// `last_exit_at` / `last_copy_at` maps so they never grow without bound.
fn prune_timestamps(
    map: &mut HashMap<String, DateTime<Utc>>,
    now: DateTime<Utc>,
    cooldown_secs: i64,
    cap: usize,
) {
    // An entry older than the cooldown can never block a re-entry / re-copy
    // again, so it is pure garbage. Floor the TTL so a zero/negative cooldown
    // still keeps a short recent window rather than thrashing.
    let ttl = if cooldown_secs > 0 {
        cooldown_secs
    } else {
        600
    }
    .max(60);
    let cutoff = now - Duration::seconds(ttl);
    map.retain(|_, at| *at >= cutoff);

    // Hard cap backstop: if a burst of distinct keys outpaced the TTL prune,
    // evict the oldest until we are within `cap`.
    let cap = cap.max(1);
    if map.len() > cap {
        let mut entries: Vec<(String, DateTime<Utc>)> = map.drain().collect();
        entries.sort_by_key(|a| a.1);
        let skip = entries.len() - cap;
        for (k, v) in entries.into_iter().skip(skip) {
            map.insert(k, v);
        }
    }
}

/// Everything the modules share. Cloning the `Arc` is cheap.
pub type Shared = Arc<AppState>;

pub struct AppState {
    /// Config as loaded at startup (immutable snapshot).
    pub initial_config: AppConfig,
    /// Live config: Telegram and the REST API mutate it, modules read it.
    pub config: RwLock<Config>,
    pub events: EventBus,

    started_at: DateTime<Utc>,
    modules: RwLock<HashMap<BotModule, ModuleState>>,
    positions: RwLock<HashMap<String, Position>>,
    position_order: RwLock<VecDeque<String>>,
    trades: RwLock<Vec<Trade>>,

    kill_switch: AtomicBool,
    halted: AtomicBool,

    realized_pnl: RwLock<HashMap<BotModule, f64>>,
    daily: RwLock<DailyStats>,

    seen_launches: RwLock<BoundedSet>,
    seen_signatures: RwLock<BoundedSet>,
    /// symbol -> when we last exited it (re-entry cooldown).
    last_exit_at: RwLock<HashMap<String, DateTime<Utc>>>,
    /// "wallet:mint" -> when we last copied it.
    last_copy_at: RwLock<HashMap<String, DateTime<Utc>>>,

    balances: RwLock<Balances>,
    /// Last published reconciliation backlog per claim kind
    /// (`reconciliation_state` rows not yet resolved). Refreshed by the
    /// startup gate and the periodic sampler; surfaced via [`Summary`],
    /// `/api/status` and Telegram `/status`. Never a source of truth —
    /// Postgres + external venues are.
    recon_unresolved: RwLock<Vec<(String, i64)>>,
    /// Symbols with actively unresolved reconciliation claims. ENTRY into a
    /// blocked symbol is refused (§H per-symbol gating); exits/sells are
    /// never blocked (reducing exposure is always safe). Recomputed from the
    /// claim backlog, so resolving a claim unblocks its symbol automatically.
    /// Mirrors durable state for fast reads — Postgres remains the truth.
    blocked_symbols: RwLock<std::collections::HashSet<String>>,
    /// Stable identity of THIS process instance (§C). `[ha].replica_id`
    /// when set, else generated `{host}-{pid}-{rand}` at startup. A restart
    /// is a NEW replica; the old life's execution claims expire by lease and
    /// are taken over with an incremented epoch (never reused silently).
    replica_id: String,
    /// Cross-replica runtime-flag propagation (kill switch, module enabled).
    /// Local mutations stamp their flag here; the server's sync task applies
    /// remote values only when they are NEWER than the last local decision
    /// (kill-ON is the exception: the safe direction applies immediately).
    flags_touched: RwLock<HashMap<String, DateTime<Utc>>>,
    flags_writer: std::sync::OnceLock<std::sync::Arc<dyn crate::ownership::RuntimeFlagsWriter>>,
    /// Cluster-wide risk view (shared-DB capacity + daily PnL), attached by
    /// the server when Postgres is enabled. `None` = local-only risk view.
    risk_oracle: std::sync::OnceLock<std::sync::Arc<dyn crate::risk::GlobalRiskOracle>>,
    counter: AtomicU64,

    /// Restart-safe dedup facade; attached once at startup when a durable
    /// backend (redis/postgres) is configured. `None` = legacy in-memory
    /// BoundedSet behaviour (identical semantics, process lifetime only).
    dedup: std::sync::OnceLock<std::sync::Arc<crate::dedup::DedupStore>>,
    /// Order management system ledger; attached once at startup.
    orders: std::sync::OnceLock<std::sync::Arc<crate::oms::OrderManager>>,
    /// Process shutdown coordinator; attached once at startup. Module loops
    /// watch it so SIGTERM drains instead of killing in-flight work.
    shutdown: std::sync::OnceLock<std::sync::Arc<crate::lifecycle::Shutdown>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DailyStats {
    pub day: String,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub buys: u64,
    pub sells: u64,
    pub wins: u64,
    pub losses: u64,
    pub loss_limit_tripped: bool,
    pub loss_limit_tripped_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Balances {
    pub sol: f64,
    pub usdc_polygon: f64,
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub started_at: DateTime<Utc>,
    pub kill_switch: bool,
    pub execution_mode: ExecutionMode,
    pub live_allowed: bool,
    pub cluster: String,
    pub modules: Vec<ModuleStatus>,
    pub open_positions: usize,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub daily: DailyStats,
    pub balances: Balances,
    /// Unresolved reconciliation claims per kind (`[]` = fully reconciled or
    /// no durable backend). While non-zero, affected modules are expected to
    /// be disabled by the startup gate (§H).
    pub recon_unresolved: Vec<(String, i64)>,
    /// Symbols currently refused ENTRY because of unresolved reconciliation
    /// claims (exits are never blocked). Empty = nothing gated.
    pub blocked_symbols: Vec<String>,
    /// Identity of the replica that produced this summary (§C).
    pub replica_id: String,
    pub event_subscribers: usize,
}

/// What one [`AppState::apply_flag_sync`] round changed locally.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FlagSyncReport {
    pub kill_changed: bool,
    pub modules_changed: usize,
    pub skipped_stale: usize,
}

/// `{hostname}-{pid}-{random}` — unique among concurrent replicas, safe to
/// expose operationally (no secret material), observable in logs/metrics and
/// persisted in claim rows. Restart semantics: a restarted process is a NEW
/// replica; its previous life's claims expire by lease and are taken over
/// with an incremented epoch (fencing rejects the zombie if it wakes).
fn generate_replica_id() -> String {
    let host: String = std::env::var("HOSTNAME")
        .unwrap_or_else(|_| "host".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    let host = if host.is_empty() { "host" } else { &host };
    let rand: u32 = rand::random();
    format!("{host}-{}-{:08x}", std::process::id(), rand)
}

impl AppState {
    pub fn new(config: AppConfig) -> Arc<Self> {
        let raw = config.raw.clone();
        let kill = raw.risk.kill_switch;

        let mut modules = HashMap::new();
        for m in BotModule::ALL {
            let mut state = ModuleState::new(m);
            state.enabled = match m {
                BotModule::Sniper => raw.sniper.enabled,
                BotModule::Copy => raw.copy.enabled,
                BotModule::Polymarket => raw.polymarket.enabled,
                BotModule::Contract => raw.contract.program_id.is_some(),
                BotModule::Telegram => raw.telegram.enabled,
            };
            modules.insert(m, state);
        }

        let replica_id = {
            let configured = raw.ha.replica_id.trim();
            if configured.is_empty() {
                generate_replica_id()
            } else {
                configured.to_string()
            }
        };

        Arc::new(AppState {
            initial_config: config,
            config: RwLock::new(raw),
            events: EventBus::new(2_000),
            started_at: Utc::now(),
            modules: RwLock::new(modules),
            positions: RwLock::new(HashMap::new()),
            position_order: RwLock::new(VecDeque::new()),
            trades: RwLock::new(Vec::new()),
            kill_switch: AtomicBool::new(kill),
            halted: AtomicBool::new(false),
            realized_pnl: RwLock::new(HashMap::new()),
            daily: RwLock::new(DailyStats {
                day: today(),
                ..Default::default()
            }),
            seen_launches: RwLock::new(BoundedSet::default()),
            seen_signatures: RwLock::new(BoundedSet::default()),
            last_exit_at: RwLock::new(HashMap::new()),
            last_copy_at: RwLock::new(HashMap::new()),
            balances: RwLock::new(Balances::default()),
            recon_unresolved: RwLock::new(Vec::new()),
            blocked_symbols: RwLock::new(std::collections::HashSet::new()),
            replica_id,
            flags_touched: RwLock::new(HashMap::new()),
            flags_writer: std::sync::OnceLock::new(),
            risk_oracle: std::sync::OnceLock::new(),
            counter: AtomicU64::new(1),
            dedup: std::sync::OnceLock::new(),
            orders: std::sync::OnceLock::new(),
            shutdown: std::sync::OnceLock::new(),
        })
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    /// Publish the reconciliation backlog snapshot (kinds are a fixed,
    /// low-cardinality set: order/position/transaction/polymarket_order/
    /// balance).
    pub async fn set_recon_unresolved(&self, counts: Vec<(String, i64)>) {
        *self.recon_unresolved.write().await = counts;
    }

    /// Current reconciliation backlog snapshot.
    pub async fn recon_unresolved(&self) -> Vec<(String, i64)> {
        self.recon_unresolved.read().await.clone()
    }

    /// Block one symbol for ENTRY while its reconciliation claim is unresolved.
    /// Returns true if the symbol was not already blocked.
    pub async fn block_symbol(&self, symbol: &str) -> bool {
        self.blocked_symbols
            .write()
            .await
            .insert(symbol.to_string())
    }

    /// Unblock a symbol (claim resolved). Returns true if it was blocked.
    pub async fn unblock_symbol(&self, symbol: &str) -> bool {
        self.blocked_symbols.write().await.remove(symbol)
    }

    /// Replace the blocked set wholesale (recomputed from unresolved claims).
    pub async fn set_blocked_symbols(&self, symbols: Vec<String>) {
        *self.blocked_symbols.write().await = symbols.into_iter().collect();
    }

    pub async fn is_symbol_blocked(&self, symbol: &str) -> bool {
        self.blocked_symbols.read().await.contains(symbol)
    }

    /// Sorted snapshot for `/api/status`, Telegram and tests.
    pub async fn blocked_symbols(&self) -> Vec<String> {
        let mut v: Vec<String> = self.blocked_symbols.read().await.iter().cloned().collect();
        v.sort();
        v
    }

    /// Total unresolved reconciliation claims across all kinds.
    pub async fn recon_unresolved_total(&self) -> i64 {
        self.recon_unresolved
            .read()
            .await
            .iter()
            .map(|(_, n)| *n)
            .sum()
    }

    /// `p-1`, `p-2`, … — short, unique, sortable.
    pub fn next_id(&self, prefix: &str) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{n}")
    }

    // ---------------------------------------------------------- kill switch --

    pub fn kill_switch(&self) -> bool {
        self.kill_switch.load(Ordering::SeqCst) || self.halted.load(Ordering::SeqCst)
    }

    pub async fn set_kill_switch(&self, on: bool, reason: &str) {
        self.kill_switch.store(on, Ordering::SeqCst);
        self.config.write().await.risk.kill_switch = on;
        self.events.publish(AppEvent::Lifecycle {
            ts: Utc::now(),
            message: format!(
                "kill switch {} — {reason}",
                if on { "ENGAGED" } else { "released" }
            ),
        });
        // §Q: the kill switch is GLOBAL state — propagate to every replica.
        self.publish_flag("kill_switch", on, reason).await;
    }

    /// Emergency stop: engage the kill switch and disable every trading module.
    pub async fn emergency_stop(&self, reason: &str) {
        self.kill_switch.store(true, Ordering::SeqCst);
        self.halted.store(true, Ordering::SeqCst);
        {
            let mut config = self.config.write().await;
            config.risk.kill_switch = true;
            config.sniper.enabled = false;
            config.copy.enabled = false;
            config.polymarket.enabled = false;
        }
        {
            let mut modules = self.modules.write().await;
            for m in BotModule::TRADING {
                if let Some(state) = modules.get_mut(&m) {
                    state.enabled = false;
                    state.running = false;
                    state.connected = false;
                    state.detail = Some(format!("emergency stop: {reason}"));
                }
            }
        }
        self.events.publish(AppEvent::Error {
            ts: Utc::now(),
            module: None,
            message: format!("EMERGENCY STOP: {reason}"),
            fatal: true,
        });
        // §Q: an emergency stop must reach EVERY replica, not just this one.
        self.publish_flag("kill_switch", true, reason).await;
        self.publish_flag("halted", true, reason).await;
        for m in BotModule::TRADING {
            self.publish_flag(&format!("module:{}", m.as_str()), false, reason)
                .await;
        }
    }

    /// Release an emergency stop. Each module still has to be re-enabled by
    /// hand. The release propagates to every replica (§Q) — without it, a
    /// /resume served by replica B would leave replica A halted forever.
    pub async fn clear_halt(&self) {
        self.halted.store(false, Ordering::SeqCst);
        self.events.publish(AppEvent::Lifecycle {
            ts: Utc::now(),
            message: "emergency halt released — modules are still disabled".into(),
        });
        self.publish_flag("halted", false, "resume").await;
    }

    // -------------------------------------------------------------- modules --

    pub async fn module_state(&self, module: BotModule) -> ModuleState {
        self.modules
            .read()
            .await
            .get(&module)
            .cloned()
            .unwrap_or_else(|| ModuleState::new(module))
    }

    pub async fn module_status(&self, module: BotModule) -> ModuleStatus {
        self.module_state(module).await.to_status()
    }

    pub async fn all_module_status(&self) -> Vec<ModuleStatus> {
        let modules = self.modules.read().await;
        BotModule::ALL
            .iter()
            .map(|m| {
                modules
                    .get(m)
                    .cloned()
                    .unwrap_or_else(|| ModuleState::new(*m))
                    .to_status()
            })
            .collect()
    }

    pub async fn is_enabled(&self, module: BotModule) -> bool {
        self.modules
            .read()
            .await
            .get(&module)
            .map(|s| s.enabled)
            .unwrap_or(false)
    }

    /// Enable/disable a module and mirror the change into the live config.
    /// Returns the previous value. The change is published to the shared
    /// runtime-flag store so every replica converges (§Q); use
    /// [`AppState::apply_remote_enabled`] for values observed FROM the store
    /// (they must not be echoed back).
    pub async fn set_enabled(&self, module: BotModule, enabled: bool) -> bool {
        let previous = self.set_enabled_local(module, enabled).await;
        if previous != enabled {
            self.publish_flag(
                &format!("module:{}", module.as_str()),
                enabled,
                "operator/gate",
            )
            .await;
        }
        previous
    }

    /// Apply an enabled-flag value observed from the shared store (sync
    /// task). Same local semantics as [`AppState::set_enabled`], but never
    /// writes back. Returns whether anything changed.
    pub async fn apply_remote_enabled(&self, module: BotModule, enabled: bool) -> bool {
        let previous = self.set_enabled_local(module, enabled).await;
        previous != enabled
    }

    /// Apply a kill-switch value observed from the shared store (sync task).
    /// Never writes back. Returns whether anything changed.
    pub async fn apply_remote_kill(&self, on: bool, reason: &str) -> bool {
        if self.kill_switch.load(Ordering::SeqCst) == on {
            return false;
        }
        self.kill_switch.store(on, Ordering::SeqCst);
        self.config.write().await.risk.kill_switch = on;
        self.events.publish(AppEvent::Lifecycle {
            ts: Utc::now(),
            message: format!(
                "kill switch {} (replica sync) — {reason}",
                if on { "ENGAGED" } else { "released" }
            ),
        });
        true
    }

    /// Apply a halt-flag value observed from the shared store (sync task).
    /// Never writes back. Returns whether anything changed. `halted` is the
    /// daily-loss / emergency latch OR-ed into [`AppState::kill_switch`].
    pub async fn apply_remote_halted(&self, on: bool, reason: &str) -> bool {
        if self.halted.load(Ordering::SeqCst) == on {
            return false;
        }
        self.halted.store(on, Ordering::SeqCst);
        self.events.publish(AppEvent::Lifecycle {
            ts: Utc::now(),
            message: format!(
                "emergency halt {} (replica sync) — {reason}",
                if on { "ENGAGED" } else { "released" }
            ),
        });
        true
    }

    // ---- runtime-flag propagation internals (§Q) ----

    /// Attach the shared-store writer (server-side; Postgres/Redis backed).
    pub fn attach_flags_writer(&self, w: std::sync::Arc<dyn crate::ownership::RuntimeFlagsWriter>) {
        let _ = self.flags_writer.set(w);
    }

    /// Attach the cluster-wide risk oracle (server-side, Postgres-backed).
    pub fn attach_risk_oracle(&self, o: std::sync::Arc<dyn crate::risk::GlobalRiskOracle>) {
        let _ = self.risk_oracle.set(o);
    }

    /// The attached cluster-wide risk oracle, if any.
    pub fn risk_oracle(&self) -> Option<std::sync::Arc<dyn crate::risk::GlobalRiskOracle>> {
        self.risk_oracle.get().cloned()
    }

    /// When this replica last mutated `flag` locally (staleness comparison).
    pub async fn flag_touched_at(&self, flag: &str) -> Option<DateTime<Utc>> {
        self.flags_touched.read().await.get(flag).copied()
    }

    /// Merge shared position-book rows (from the database) into local state
    /// (§Q book sync — risk capacity converges across replicas).
    ///
    /// Rules: a row this replica has never seen is INSERTED; a local row is
    /// OVERWRITTEN only when the shared row is newer AND the local row is
    /// not terminal (a locally closed position is never resurrected by a
    /// stale/lagging DB read). Returns `(inserted, refreshed)`.
    pub async fn merge_positions(&self, rows: Vec<Position>) -> (usize, usize) {
        let (mut inserted, mut refreshed) = (0usize, 0usize);
        for p in rows {
            match self.position(&p.id).await {
                None => {
                    self.upsert_position(p).await;
                    inserted += 1;
                }
                Some(local) => {
                    if p.updated_at > local.updated_at
                        && matches!(local.status, PositionStatus::Open | PositionStatus::Closing)
                    {
                        self.upsert_position(p).await;
                        refreshed += 1;
                    }
                }
            }
        }
        (inserted, refreshed)
    }

    async fn publish_flag(&self, flag: &str, enabled: bool, reason: &str) {
        self.flags_touched
            .write()
            .await
            .insert(flag.to_string(), Utc::now());
        if let Some(w) = self.flags_writer.get() {
            w.write(flag, enabled, reason, &self.replica_id).await;
        }
    }

    /// Stable identity of this replica (§C): logs, metrics, claim rows.
    pub fn replica_id(&self) -> &str {
        &self.replica_id
    }

    /// Apply one round of flags observed from the shared runtime-flag store
    /// (the server's sync task calls this every `[ha].flag_sync_secs`).
    ///
    /// Staleness rules (§Q):
    /// * kill ON applies IMMEDIATELY — the safe direction wins over any
    ///   local recency claim;
    /// * kill OFF and module flags apply only when the shared row is newer
    ///   than this replica's last local decision for that flag (or this
    ///   replica never touched it — boot convergence).
    ///
    /// Remote values are never echoed back to the store. Assumes roughly
    /// NTP-synced clocks across replicas (documented in docs/DISTRIBUTED.md).
    pub async fn apply_flag_sync(&self, flags: &[crate::ownership::RuntimeFlag]) -> FlagSyncReport {
        let mut report = FlagSyncReport::default();
        for f in flags {
            if f.flag == "kill_switch" || f.flag == "halted" {
                // Safe direction (ON) applies immediately; OFF is
                // recency-gated against the last local decision.
                let changed = if f.enabled {
                    if f.flag == "kill_switch" {
                        self.apply_remote_kill(true, &f.reason).await
                    } else {
                        self.apply_remote_halted(true, &f.reason).await
                    }
                } else if self.remote_flag_is_current(&f.flag, f.updated_at).await {
                    if f.flag == "kill_switch" {
                        self.apply_remote_kill(false, &f.reason).await
                    } else {
                        self.apply_remote_halted(false, &f.reason).await
                    }
                } else {
                    report.skipped_stale += 1;
                    false
                };
                if changed {
                    report.kill_changed = true;
                }
            } else if let Some(name) = f.flag.strip_prefix("module:") {
                let Ok(module) = name.parse::<BotModule>() else {
                    continue;
                };
                if self.remote_flag_is_current(&f.flag, f.updated_at).await
                    && self.apply_remote_enabled(module, f.enabled).await
                {
                    report.modules_changed += 1;
                } else {
                    report.skipped_stale += 1;
                }
            }
        }
        if report.kill_changed || report.modules_changed > 0 {
            crate::obs::metrics::global()
                .counter(
                    "bot_distributed_flag_sync_applied_total",
                    "Remote runtime-flag changes applied by the sync task.",
                    &[],
                )
                .inc_by((report.modules_changed + report.kill_changed as usize) as u64);
            tracing::info!(
                kill_changed = report.kill_changed,
                modules_changed = report.modules_changed,
                skipped_stale = report.skipped_stale,
                replica = %self.replica_id,
                "runtime flags converged from shared store"
            );
        }
        report
    }

    /// True when the shared row's timestamp beats this replica's last local
    /// decision for `flag` (or the flag was never touched locally).
    async fn remote_flag_is_current(&self, flag: &str, remote_updated: DateTime<Utc>) -> bool {
        match self.flag_touched_at(flag).await {
            Some(touched) => remote_updated > touched,
            None => true,
        }
    }

    async fn set_enabled_local(&self, module: BotModule, enabled: bool) -> bool {
        let previous;
        {
            let mut modules = self.modules.write().await;
            let state = modules
                .entry(module)
                .or_insert_with(|| ModuleState::new(module));
            previous = state.enabled;
            state.enabled = enabled;
            if enabled {
                state.started_at = Some(Utc::now());
                state.stopped_at = None;
                state.consecutive_errors = 0;
            } else {
                state.stopped_at = Some(Utc::now());
                state.running = false;
                state.connected = false;
                state.degraded = false;
            }
        }
        {
            let mut config = self.config.write().await;
            match module {
                BotModule::Sniper => config.sniper.enabled = enabled,
                BotModule::Copy => config.copy.enabled = enabled,
                BotModule::Polymarket => config.polymarket.enabled = enabled,
                BotModule::Telegram => config.telegram.enabled = enabled,
                BotModule::Contract => {
                    // Module 4 has no "enabled" flag; force dry-run when the
                    // program id is unknown so it can never sign by accident.
                    if config.contract.program_id.is_none() {
                        config.contract.dry_run = true;
                    }
                }
            }
        }
        if previous != enabled {
            self.events.publish(AppEvent::ModuleStatus {
                ts: Utc::now(),
                module,
                enabled,
                running: false,
                connected: false,
                detail: Some(format!(
                    "{} by operator",
                    if enabled { "enabled" } else { "disabled" }
                )),
            });
        }
        previous
    }

    pub async fn set_running(&self, module: BotModule, running: bool, connected: bool) {
        let mut modules = self.modules.write().await;
        let state = modules
            .entry(module)
            .or_insert_with(|| ModuleState::new(module));
        state.running = running;
        state.connected = connected;
        state.degraded = running && !connected;
        state.last_heartbeat = Some(Utc::now());
    }

    pub async fn heartbeat(&self, module: BotModule) {
        let mut modules = self.modules.write().await;
        let state = modules
            .entry(module)
            .or_insert_with(|| ModuleState::new(module));
        state.last_heartbeat = Some(Utc::now());
    }

    pub async fn record_error(&self, module: BotModule, err: &str) {
        let consecutive;
        {
            let mut modules = self.modules.write().await;
            let state = modules
                .entry(module)
                .or_insert_with(|| ModuleState::new(module));
            state.consecutive_errors = state.consecutive_errors.saturating_add(1);
            state.last_error = Some(err.to_string());
            state.last_error_at = Some(Utc::now());
            consecutive = state.consecutive_errors;
        }
        self.events.publish(AppEvent::Error {
            ts: Utc::now(),
            module: Some(module),
            message: err.to_string(),
            fatal: false,
        });

        let limit = self.config.read().await.risk.max_consecutive_failures;
        if limit > 0 && consecutive >= limit {
            self.events.publish(AppEvent::Error {
                ts: Utc::now(),
                module: Some(module),
                message: format!(
                    "{consecutive} consecutive failures (limit {limit}) — module auto-disabled"
                ),
                fatal: true,
            });
            self.set_enabled(module, false).await;
        }
    }

    pub async fn clear_error(&self, module: BotModule) {
        let mut modules = self.modules.write().await;
        if let Some(state) = modules.get_mut(&module) {
            state.consecutive_errors = 0;
        }
    }

    /// Apply `f` to a module's state and stamp `last_event`.
    pub async fn bump<F>(&self, module: BotModule, f: F)
    where
        F: FnOnce(&mut ModuleState),
    {
        let mut modules = self.modules.write().await;
        let state = modules
            .entry(module)
            .or_insert_with(|| ModuleState::new(module));
        f(state);
        state.last_event = Some(Utc::now());
    }

    pub async fn inc_events(&self, module: BotModule, n: u64) {
        self.bump(module, |s| s.events_seen = s.events_seen.saturating_add(n))
            .await;
    }

    pub async fn inc_signals(&self, module: BotModule) {
        self.bump(module, |s| {
            s.signals_generated = s.signals_generated.saturating_add(1)
        })
        .await;
    }

    pub async fn inc_orders_sent(&self, module: BotModule) {
        self.bump(module, |s| s.orders_sent = s.orders_sent.saturating_add(1))
            .await;
    }

    pub async fn inc_orders_failed(&self, module: BotModule) {
        self.bump(module, |s| {
            s.orders_failed = s.orders_failed.saturating_add(1)
        })
        .await;
    }

    pub async fn inc_risk_rejected(&self, module: BotModule) {
        self.bump(module, |s| {
            s.orders_rejected_by_risk = s.orders_rejected_by_risk.saturating_add(1)
        })
        .await;
    }

    pub async fn set_detail(&self, module: BotModule, detail: impl Into<String>) {
        let mut modules = self.modules.write().await;
        let state = modules
            .entry(module)
            .or_insert_with(|| ModuleState::new(module));
        state.detail = Some(detail.into());
    }

    // ------------------------------------------------------------ positions --

    pub async fn upsert_position(&self, position: Position) {
        let id = position.id.clone();
        let is_new = {
            let positions = self.positions.read().await;
            !positions.contains_key(&id)
        };
        {
            let mut positions = self.positions.write().await;
            positions.insert(id.clone(), position.clone());
        }
        if is_new {
            self.position_order.write().await.push_back(id);
        }
        self.events.publish(AppEvent::PositionUpdate {
            ts: Utc::now(),
            position: Box::new(position),
        });
    }

    pub async fn position(&self, id: &str) -> Option<Position> {
        self.positions.read().await.get(id).cloned()
    }

    /// Bulk-load positions during restart recovery. Publishes NO events —
    /// the durable store already knows these rows; re-emitting would loop
    /// them back through the persistence pump and confuse live dashboards.
    /// Returns how many positions were newly inserted.
    pub async fn restore_positions(&self, positions: Vec<Position>) -> usize {
        let mut inserted = 0;
        {
            let mut map = self.positions.write().await;
            let mut order = self.position_order.write().await;
            for p in positions {
                if !map.contains_key(&p.id) {
                    order.push_back(p.id.clone());
                    inserted += 1;
                }
                map.insert(p.id.clone(), p);
            }
        }
        inserted
    }

    /// Mutate a position in place and return the new value.
    pub async fn with_position<F>(&self, id: &str, f: F) -> Option<Position>
    where
        F: FnOnce(&mut Position),
    {
        let mut positions = self.positions.write().await;
        let position = positions.get_mut(id)?;
        f(position);
        position.updated_at = Utc::now();
        Some(position.clone())
    }

    pub async fn all_positions(&self) -> Vec<Position> {
        let positions = self.positions.read().await;
        let order = self.position_order.read().await;
        let mut seen: HashSet<String> = HashSet::with_capacity(order.len());
        let mut out: Vec<Position> = Vec::with_capacity(positions.len());
        for id in order.iter().rev() {
            if let Some(p) = positions.get(id) {
                seen.insert(id.clone());
                out.push(p.clone());
            }
        }
        for (id, p) in positions.iter() {
            if !seen.contains(id) {
                out.push(p.clone());
            }
        }
        out
    }

    pub async fn open_positions(&self) -> Vec<Position> {
        self.all_positions()
            .await
            .into_iter()
            .filter(|p| !p.status.is_terminal())
            .collect()
    }

    fn source_of(module: BotModule) -> Option<TradeSource> {
        match module {
            BotModule::Sniper => Some(TradeSource::Sniper),
            BotModule::Copy => Some(TradeSource::Copy),
            BotModule::Polymarket => Some(TradeSource::Polymarket),
            BotModule::Contract | BotModule::Telegram => None,
        }
    }

    pub async fn open_positions_for(&self, module: BotModule) -> Vec<Position> {
        let Some(source) = Self::source_of(module) else {
            return Vec::new();
        };
        self.open_positions()
            .await
            .into_iter()
            .filter(|p| p.source == source)
            .collect()
    }

    /// Open position on `symbol` for `module`, if any.
    pub async fn find_open(&self, module: BotModule, symbol: &str) -> Option<Position> {
        let source = Self::source_of(module)?;
        let positions = self.positions.read().await;
        positions
            .values()
            .find(|p| !p.status.is_terminal() && p.source == source && p.symbol == symbol)
            .cloned()
    }

    pub async fn close_position(
        &self,
        id: &str,
        status: PositionStatus,
        reason: &str,
    ) -> Option<Position> {
        let (cloned, pnl, pnl_pct) = {
            let mut positions = self.positions.write().await;
            let position = positions.get_mut(id)?;
            let pnl = position.realised();
            let pnl_pct = position.total_pnl_pct();
            position.close(status, reason.to_string());
            (position.clone(), pnl, pnl_pct)
        };
        // Taken *after* releasing the positions lock: never nest these two.
        let (cooldown, cap) = {
            let cfg = self.config.read().await;
            (
                cfg.risk.reentry_cooldown_secs,
                cfg.storage.max_dedup_entries,
            )
        };
        let now = Utc::now();
        {
            let mut m = self.last_exit_at.write().await;
            m.insert(cloned.symbol.clone(), now);
            prune_timestamps(&mut m, now, cooldown, cap);
        }

        self.events.publish(AppEvent::PositionClosed {
            ts: Utc::now(),
            position: Box::new(cloned.clone()),
            pnl,
            pnl_pct,
            reason: reason.to_string(),
        });
        Some(cloned)
    }

    // --------------------------------------------------------------- trades --

    pub async fn record_trade(&self, trade: Trade) {
        let module = match trade.source {
            TradeSource::Sniper | TradeSource::Manual | TradeSource::Risk => BotModule::Sniper,
            TradeSource::Copy => BotModule::Copy,
            TradeSource::Polymarket => BotModule::Polymarket,
        };
        {
            let cap = self
                .config
                .read()
                .await
                .storage
                .max_trades_in_memory
                .max(64);
            let mut trades = self.trades.write().await;
            trades.push(trade.clone());
            if trades.len() > cap {
                let overflow = trades.len() - cap;
                trades.drain(0..overflow);
            }
        }
        {
            let mut daily = self.daily.write().await;
            self.roll_day(&mut daily).await;
            if trade.is_buy() {
                daily.buys += 1;
            } else {
                daily.sells += 1;
            }
        }
        self.bump(module, |s| {
            s.orders_filled = s.orders_filled.saturating_add(1)
        })
        .await;
        self.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade),
        });
    }

    pub async fn trades(&self, limit: usize) -> Vec<Trade> {
        let trades = self.trades.read().await;
        let start = trades.len().saturating_sub(limit.max(1));
        let mut out: Vec<Trade> = trades[start..].to_vec();
        out.reverse();
        out
    }

    // ------------------------------------------------------------------ pnl --

    async fn roll_day(&self, daily: &mut DailyStats) {
        let t = today();
        if daily.day != t {
            *daily = DailyStats {
                day: t,
                ..Default::default()
            };
        }
    }

    pub async fn add_realized(&self, module: BotModule, amount: f64) {
        {
            let mut pnl = self.realized_pnl.write().await;
            *pnl.entry(module).or_insert(0.0) += amount;
        }
        {
            let mut daily = self.daily.write().await;
            self.roll_day(&mut daily).await;
            daily.realized_pnl += amount;
            if amount > 0.0 {
                daily.wins += 1;
            } else if amount < 0.0 {
                daily.losses += 1;
            }
        }
        self.bump(module, |s| s.realized_pnl += amount).await;
    }

    pub async fn realized_pnl(&self, module: BotModule) -> f64 {
        *self.realized_pnl.read().await.get(&module).unwrap_or(&0.0)
    }

    pub async fn total_realized(&self) -> f64 {
        self.realized_pnl.read().await.values().sum()
    }

    /// Trip the daily loss limit so `preflight` starts refusing new entries.
    pub async fn mark_loss_limit_tripped(&self) -> bool {
        let mut d = self.daily.write().await;
        let t = today();
        if d.day != t {
            *d = DailyStats {
                day: t,
                ..Default::default()
            };
        }
        if d.loss_limit_tripped {
            return false;
        }
        d.loss_limit_tripped = true;
        d.loss_limit_tripped_at = Some(Utc::now());
        true
    }

    /// Clear the daily loss limit (operator action, e.g. Telegram `/risk reset`).
    pub async fn clear_loss_limit(&self) {
        let mut d = self.daily.write().await;
        d.loss_limit_tripped = false;
        d.loss_limit_tripped_at = None;
    }

    /// Realised + unrealised for the current UTC day.
    pub async fn daily_stats(&self) -> DailyStats {
        let mut d = {
            let guard = self.daily.read().await;
            guard.clone()
        };
        let t = today();
        if d.day != t {
            d = DailyStats {
                day: t,
                ..Default::default()
            };
        }
        let unrealized: f64 = self
            .open_positions()
            .await
            .iter()
            .map(|p| p.unrealised())
            .sum();
        d.unrealized_pnl = unrealized;
        d
    }

    pub async fn set_unrealized(&self, module: BotModule, amount: f64) {
        self.bump(module, |s| s.unrealized_pnl = amount).await;
    }

    /// Quote exposure of all open positions for a module.
    pub async fn open_exposure(&self, module: BotModule) -> f64 {
        self.open_positions_for(module)
            .await
            .iter()
            .map(|p| p.cost_basis.max(p.notional()))
            .sum()
    }

    // --------------------------------------------------------------- guards --

    /// Returns `false` when this mint was already seen (duplicate launch event).
    /// Routes through the durable dedup facade when one is attached.
    pub async fn mark_launch_seen(&self, mint: &str) -> bool {
        if let Some(d) = self.dedup.get() {
            return d.mark("launch", mint).await;
        }
        let cap = self.config.read().await.storage.max_dedup_entries;
        self.seen_launches.write().await.insert(mint, cap)
    }

    /// Returns `false` when this signature was already processed.
    /// Routes through the durable dedup facade when one is attached.
    pub async fn mark_signature_seen(&self, sig: &str) -> bool {
        if let Some(d) = self.dedup.get() {
            return d.mark("sig", sig).await;
        }
        let cap = self.config.read().await.storage.max_dedup_entries;
        self.seen_signatures.write().await.insert(sig, cap)
    }

    pub async fn forget_signature(&self, sig: &str) {
        if let Some(d) = self.dedup.get() {
            d.forget("sig", sig).await;
            return;
        }
        self.seen_signatures.write().await.remove(sig);
    }

    pub async fn seen_launch_count(&self) -> usize {
        if let Some(d) = self.dedup.get() {
            return d.len("launch").await;
        }
        self.seen_launches.read().await.len()
    }

    /// Attach the restart-safe dedup facade (startup only; later calls are
    /// ignored so the hot path never races).
    pub fn attach_dedup(&self, store: std::sync::Arc<crate::dedup::DedupStore>) {
        if self.dedup.set(store).is_err() {
            tracing::warn!("dedup store already attached — ignoring");
        }
    }

    pub fn dedup(&self) -> Option<&std::sync::Arc<crate::dedup::DedupStore>> {
        self.dedup.get()
    }

    /// Attach the OMS ledger (startup only).
    pub fn attach_orders(&self, mgr: std::sync::Arc<crate::oms::OrderManager>) {
        if self.orders.set(mgr).is_err() {
            tracing::warn!("order manager already attached — ignoring");
        }
    }

    pub fn orders(&self) -> Option<&std::sync::Arc<crate::oms::OrderManager>> {
        self.orders.get()
    }

    /// Attach the process shutdown coordinator (startup only).
    pub fn attach_shutdown(&self, s: std::sync::Arc<crate::lifecycle::Shutdown>) {
        if self.shutdown.set(s).is_err() {
            tracing::warn!("shutdown coordinator already attached — ignoring");
        }
    }

    pub fn shutdown(&self) -> Option<&std::sync::Arc<crate::lifecycle::Shutdown>> {
        self.shutdown.get()
    }

    /// Cheap synchronous check for module loops.
    pub fn shutdown_signalled(&self) -> bool {
        self.shutdown
            .get()
            .map(|s| s.is_signalled())
            .unwrap_or(false)
    }

    /// Resolves when shutdown is requested; pends forever when no
    /// coordinator is attached (tests, embedded use).
    pub async fn wait_shutdown(&self) {
        match self.shutdown.get() {
            Some(s) => s.wait().await,
            None => std::future::pending::<()>().await,
        }
    }

    pub async fn last_exit(&self, symbol: &str) -> Option<DateTime<Utc>> {
        self.last_exit_at.read().await.get(symbol).copied()
    }

    /// `true` when the re-entry cooldown for `symbol` has elapsed.
    pub async fn reentry_allowed(&self, symbol: &str) -> bool {
        let cooldown = self.config.read().await.risk.reentry_cooldown_secs;
        if cooldown <= 0 {
            return true;
        }
        match self.last_exit_at.read().await.get(symbol) {
            None => true,
            Some(at) => Utc::now().signed_duration_since(*at).num_seconds() >= cooldown,
        }
    }

    /// `true` when we may copy `wallet` on `mint` again.
    pub async fn copy_allowed(&self, wallet: &str, mint: &str) -> bool {
        let cooldown = self.config.read().await.risk.copy_cooldown_secs;
        if cooldown <= 0 {
            return true;
        }
        let key = format!("{wallet}:{mint}");
        match self.last_copy_at.read().await.get(&key) {
            None => true,
            Some(at) => Utc::now().signed_duration_since(*at).num_seconds() >= cooldown,
        }
    }

    pub async fn mark_copied(&self, wallet: &str, mint: &str) {
        let (cooldown, cap) = {
            let cfg = self.config.read().await;
            (cfg.risk.copy_cooldown_secs, cfg.storage.max_dedup_entries)
        };
        let now = Utc::now();
        let mut m = self.last_copy_at.write().await;
        m.insert(format!("{wallet}:{mint}"), now);
        prune_timestamps(&mut m, now, cooldown, cap);
    }

    // ------------------------------------------------------------- balances --

    pub async fn set_balances(&self, sol: Option<f64>, usdc: Option<f64>) {
        let mut b = self.balances.write().await;
        if let Some(v) = sol {
            b.sol = v;
        }
        if let Some(v) = usdc {
            b.usdc_polygon = v;
        }
        b.checked_at = Some(Utc::now());
    }

    pub async fn balances(&self) -> Balances {
        self.balances.read().await.clone()
    }

    // --------------------------------------------------------------- config --

    pub async fn config_snapshot(&self) -> Config {
        self.config.read().await.clone()
    }

    pub async fn execution_mode(&self) -> ExecutionMode {
        let c = self.config.read().await;
        // The kill switch downgrades live to simulate: we keep building orders
        // (useful for debugging) but never broadcast.
        if self.kill_switch() && c.execution.mode.is_live() {
            return ExecutionMode::Simulate;
        }
        c.execution.mode
    }

    /// The one place that decides whether a broadcast may happen.
    pub async fn may_broadcast(&self) -> BotResult<()> {
        if self.kill_switch() {
            return Err(BotError::KillSwitch(
                "kill switch is engaged — refusing to broadcast".into(),
            ));
        }
        let c = self.config.read().await;
        if !c.execution.live_allowed() {
            return Err(BotError::ModuleDisabled(format!(
                "execution.mode={} allow_live_trading={} — broadcasting is disabled",
                c.execution.mode, c.execution.allow_live_trading
            )));
        }
        Ok(())
    }

    pub async fn update_config<F>(&self, f: F) -> Config
    where
        F: FnOnce(&mut Config),
    {
        let mut c = self.config.write().await;
        f(&mut c);
        c.clone()
    }

    pub async fn summary(&self) -> Summary {
        let modules = self.all_module_status().await;
        let positions = self.open_positions().await;
        let daily = self.daily_stats().await;
        let balances = self.balances().await;
        let unrealized: f64 = positions.iter().map(|p| p.unrealised()).sum();
        let (live_allowed, cluster) = {
            let c = self.config.read().await;
            (c.execution.live_allowed(), c.network.cluster.clone())
        };
        Summary {
            started_at: self.started_at,
            kill_switch: self.kill_switch(),
            execution_mode: self.execution_mode().await,
            live_allowed,
            cluster,
            modules,
            open_positions: positions.len(),
            unrealized_pnl: unrealized,
            realized_pnl: daily.realized_pnl,
            daily,
            balances,
            recon_unresolved: self.recon_unresolved().await,
            blocked_symbols: self.blocked_symbols().await,
            replica_id: self.replica_id.clone(),
            event_subscribers: self.events.receiver_count(),
        }
    }
}

fn today() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> Arc<AppState> {
        AppState::new(crate::config::AppConfig {
            raw: crate::config::Config::default(),
            source_path: None,
            warnings: Vec::new(),
        })
    }

    fn flag(name: &str, enabled: bool, updated_at: DateTime<Utc>) -> crate::ownership::RuntimeFlag {
        crate::ownership::RuntimeFlag {
            flag: name.to_string(),
            enabled,
            reason: "test".to_string(),
            updated_by: "replica-X".to_string(),
            updated_at,
        }
    }

    /// §Q: kill ON propagates immediately (safe direction); kill OFF only
    /// when the shared row is newer than the local decision.
    #[tokio::test]
    async fn flag_sync_kill_rules() {
        let state = test_state();
        assert!(!state.kill_switch());

        // Stale remote kill-ON still applies immediately.
        let r = state
            .apply_flag_sync(&[flag("kill_switch", true, Utc::now() - Duration::hours(1))])
            .await;
        assert!(r.kill_changed && state.kill_switch());

        // Local release touches the flag "now"; an older remote OFF row is
        // stale — skipped (nothing to change anyway, but no flapping).
        state.set_kill_switch(false, "local release").await;
        let r = state
            .apply_flag_sync(&[flag("kill_switch", false, Utc::now() - Duration::hours(1))])
            .await;
        assert!(!r.kill_changed);
        assert_eq!(r.skipped_stale, 1);

        // A NEWER remote ON re-engages the kill everywhere.
        let r = state
            .apply_flag_sync(&[flag("kill_switch", true, Utc::now() + Duration::seconds(1))])
            .await;
        assert!(r.kill_changed && state.kill_switch());

        // The `halted` latch rides the same channel (§Q): a remote
        // emergency stop halts this replica, and a newer remote resume
        // clears it — without this, /resume on replica B would leave
        // replica A halted forever.
        state.set_kill_switch(false, "test").await;
        let r = state
            .apply_flag_sync(&[flag("halted", true, Utc::now() - Duration::hours(1))])
            .await;
        assert!(
            r.kill_changed && state.kill_switch(),
            "halted OR-s into the kill gate"
        );
        let r = state
            .apply_flag_sync(&[flag("halted", false, Utc::now() + Duration::seconds(1))])
            .await;
        assert!(r.kill_changed && !state.kill_switch());
    }

    /// §Q: module flags converge only when the shared row is newer than
    /// the local decision (boot convergence: untouched flags always apply).
    #[tokio::test]
    async fn flag_sync_module_rules() {
        let state = test_state();
        // Default config: sniper disabled.
        assert!(!state.is_enabled(BotModule::Sniper).await);

        // Untouched locally → remote value applies (boot convergence).
        let r = state
            .apply_flag_sync(&[flag("module:sniper", true, Utc::now() - Duration::hours(1))])
            .await;
        assert_eq!(r.modules_changed, 1);
        assert!(state.is_enabled(BotModule::Sniper).await);

        // Local disable touches the flag; the older remote ON row must NOT
        // clobber the newer local decision.
        state.set_enabled(BotModule::Sniper, false).await;
        let r = state
            .apply_flag_sync(&[flag("module:sniper", true, Utc::now() - Duration::hours(1))])
            .await;
        assert_eq!(r.modules_changed, 0);
        assert_eq!(r.skipped_stale, 1);
        assert!(!state.is_enabled(BotModule::Sniper).await);

        // A newer remote row wins.
        let r = state
            .apply_flag_sync(&[flag(
                "module:sniper",
                true,
                Utc::now() + Duration::seconds(1),
            )])
            .await;
        assert_eq!(r.modules_changed, 1);
        assert!(state.is_enabled(BotModule::Sniper).await);
    }

    /// §Q book sync merge rules: insert unknown, refresh older non-terminal,
    /// never resurrect locally terminal rows, never overwrite newer locals.
    #[tokio::test]
    async fn merge_positions_rules() {
        let state = test_state();
        let mk = |id: &str, qty: f64, updated: DateTime<Utc>, status: PositionStatus| {
            let mut p = Position::new(
                id.to_string(),
                TradeSource::Sniper,
                crate::models::Venue::PumpFun,
                ExecutionMode::Paper,
                id.to_string(),
                id.to_string(),
                "SOL".to_string(),
            );
            p.qty = qty;
            p.avg_entry = 1.0;
            p.last_mark = 1.0;
            p.updated_at = updated;
            p.status = status;
            p
        };
        let t0 = Utc::now();

        // Unknown row → inserted.
        let (ins, ref_) = state
            .merge_positions(vec![mk("p1", 10.0, t0, PositionStatus::Open)])
            .await;
        assert_eq!((ins, ref_), (1, 0));

        // Newer shared row overwrites the open local row.
        let (ins, ref_) = state
            .merge_positions(vec![mk(
                "p1",
                7.0,
                t0 + Duration::seconds(5),
                PositionStatus::Open,
            )])
            .await;
        assert_eq!((ins, ref_), (0, 1));
        assert_eq!(state.position("p1").await.unwrap().qty, 7.0);

        // Older shared row does NOT overwrite.
        let (ins, ref_) = state
            .merge_positions(vec![mk(
                "p1",
                3.0,
                t0 + Duration::seconds(1),
                PositionStatus::Open,
            )])
            .await;
        assert_eq!((ins, ref_), (0, 0));
        assert_eq!(state.position("p1").await.unwrap().qty, 7.0);

        // Locally terminal rows are never resurrected/overwritten.
        state
            .close_position("p1", PositionStatus::Closed, "test")
            .await;
        let (ins, ref_) = state
            .merge_positions(vec![mk(
                "p1",
                99.0,
                t0 + Duration::seconds(60),
                PositionStatus::Open,
            )])
            .await;
        assert_eq!((ins, ref_), (0, 0));
        assert_eq!(state.position("p1").await.unwrap().qty, 7.0);
    }

    /// §C: configured replica id wins; otherwise a unique id is generated
    /// and exposed via the summary.
    #[tokio::test]
    async fn replica_id_configured_or_generated() {
        let mut cfg = crate::config::Config::default();
        cfg.ha.replica_id = "replica-eu-1".to_string();
        let state = AppState::new(crate::config::AppConfig {
            raw: cfg,
            source_path: None,
            warnings: Vec::new(),
        });
        assert_eq!(state.replica_id(), "replica-eu-1");
        assert_eq!(state.summary().await.replica_id, "replica-eu-1");

        let auto = test_state();
        let id = auto.replica_id().to_string();
        assert!(id.contains(&std::process::id().to_string()), "got {id}");
        assert_eq!(id, auto.replica_id(), "stable within the process");
    }

    #[tokio::test]
    async fn symbol_gate_blocks_entries_and_recomputes() {
        // §H per-symbol gating semantics: block/unblock/replace + Summary.
        let state = AppState::new(crate::config::AppConfig {
            raw: crate::config::Config::default(),
            source_path: None,
            warnings: Vec::new(),
        });
        assert!(state.block_symbol("SOL-A").await, "first block is new");
        assert!(!state.block_symbol("SOL-A").await, "second block is not");
        assert!(state.is_symbol_blocked("SOL-A").await);
        assert!(!state.is_symbol_blocked("SOL-B").await);
        assert_eq!(state.blocked_symbols().await, vec!["SOL-A".to_string()]);

        // Wholesale recompute (what the periodic sampler does): symbols whose
        // claims resolved disappear automatically; output stays sorted.
        state
            .set_blocked_symbols(vec!["X".into(), "B".into(), "A".into()])
            .await;
        assert_eq!(
            state.blocked_symbols().await,
            vec!["A".to_string(), "B".into(), "X".into()]
        );
        assert!(
            !state.is_symbol_blocked("SOL-A").await,
            "replaced wholesale"
        );
        assert!(state.unblock_symbol("B").await);
        assert!(!state.is_symbol_blocked("B").await);
        assert!(!state.unblock_symbol("B").await, "already unblocked");

        let summary = state.summary().await;
        assert!(summary.blocked_symbols.contains(&"A".to_string()));
        assert!(!summary.blocked_symbols.contains(&"B".to_string()));
    }

    #[test]
    fn bounded_set_dedups_and_reports_novelty() {
        let mut s = BoundedSet::default();
        assert!(s.insert("a", 10));
        assert!(!s.insert("a", 10), "second insert of same key is not new");
        assert!(s.insert("b", 10));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn bounded_set_evicts_oldest_past_cap() {
        let mut s = BoundedSet::default();
        for k in ["a", "b", "c"] {
            s.insert(k, 3);
        }
        assert_eq!(s.len(), 3);
        // 4th distinct key evicts the oldest ("a").
        assert!(s.insert("d", 3));
        assert_eq!(s.len(), 3, "size stays at the cap");
        assert!(!s.set.contains("a"), "oldest evicted");
        assert!(s.set.contains("b") && s.set.contains("c") && s.set.contains("d"));
        // "a" was evicted, so it reads as new again and evicts the next oldest.
        assert!(s.insert("a", 3));
        assert!(!s.set.contains("b"), "next oldest evicted");
        assert!(s.set.contains("a") && s.set.contains("c") && s.set.contains("d"));
    }

    #[test]
    fn bounded_set_cap_of_one_keeps_only_latest() {
        let mut s = BoundedSet::default();
        s.insert("a", 1);
        s.insert("b", 1);
        assert_eq!(s.len(), 1);
        assert!(s.set.contains("b") && !s.set.contains("a"));
    }

    #[test]
    fn bounded_set_remove_drops_membership() {
        let mut s = BoundedSet::default();
        s.insert("a", 10);
        s.insert("b", 10);
        assert!(s.remove("a"));
        assert!(!s.remove("a"), "removing an absent key is a no-op");
        assert_eq!(s.len(), 1);
        assert!(s.insert("a", 10), "removed key reads as new again");
    }

    #[test]
    fn prune_drops_expired_then_caps_size() {
        let now = Utc::now();
        let mut m: HashMap<String, DateTime<Utc>> = HashMap::new();
        m.insert("fresh".into(), now);
        m.insert("stale".into(), now - Duration::seconds(600));
        prune_timestamps(&mut m, now, 60, 100);
        assert!(m.contains_key("fresh"));
        assert!(!m.contains_key("stale"), "cooldown-expired entry pruned");

        // Cap enforcement: 5 fresh entries, cap 2 -> keep the newest 2.
        let mut m2: HashMap<String, DateTime<Utc>> = HashMap::new();
        for i in 0..5 {
            m2.insert(format!("k{i}"), now - Duration::seconds(i));
        }
        prune_timestamps(&mut m2, now, 3600, 2);
        assert_eq!(m2.len(), 2);
        assert!(
            m2.contains_key("k0") && m2.contains_key("k1"),
            "newest kept"
        );
        assert!(!m2.contains_key("k4"), "oldest evicted");
    }

    #[test]
    fn prune_zero_cooldown_uses_floor_ttl() {
        let now = Utc::now();
        let mut m: HashMap<String, DateTime<Utc>> = HashMap::new();
        m.insert("recent".into(), now - Duration::seconds(30));
        m.insert("old".into(), now - Duration::seconds(1200));
        // cooldown 0 -> floor TTL (600s): 30s kept, 1200s pruned.
        prune_timestamps(&mut m, now, 0, 100);
        assert!(m.contains_key("recent"));
        assert!(!m.contains_key("old"));
    }
}
