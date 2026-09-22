//! Module 3 — Polymarket trading engine.
//!
//! Pipeline: **discover** markets (Gamma) → **price** them (CLOB REST +
//! websocket) → **decide** (strategy, explicit verdicts) → **gate** (staged
//! order pipeline: validation, market, quote, exposure, sizing, the ONE
//! shared risk decision, collateral, OMS idempotency, ownership) → **sign**
//! (EIP-712 V2) → **submit** (CLOB) → **track** (status polling + the
//! authenticated user websocket channel) → **reconcile** (local order state
//! vs the venue) → **recover** (restart re-adoption). Paper mode is the
//! default: the full pipeline runs against live market data, but orders are
//! only broadcast when `execution.mode = "live"` *and*
//! `execution.allow_live_trading = true`. Without a `POLYMARKET_PRIVATE_KEY`
//! the bot runs read-only and records paper fills, so it is safe to demo with
//! no funds at risk.
//!
//! ## One order, one record, one key (TASK 4)
//! Every decision becomes an [`orders::OrderSignal`] with a deterministic
//! identity. That identity is the OMS `idempotency_key`
//! ([`bot_core::oms::OrderManager::create`]) — the single authoritative
//! duplicate gate across retries, restarts and replicas. Venue-level detail
//! (venue order id, matched size, fills, reconciliation findings) lives in
//! the durable journal ([`store::PolyStore`], migration 0014) keyed by the
//! OMS order id. There is no second risk engine: the pipeline asks the
//! shared [`RiskEngine`] exactly once per signal
//! ([`RiskEngine::check_polymarket_coded`] + [`RiskEngine::check_entry`]).
//!
//! ## Live money separation
//! LIVE entries are sized against a verified on-chain collateral read
//! ([`collateral`], ERC-20 `balanceOf`/`decimals` on Polygon, freshness-
//! bounded), and before any live order is broadcast the funder's balance AND
//! the settling exchange's ERC-20 allowance must cover the approved notional
//! PLUS the collateral already committed by resting orders. When any of that
//! cannot be verified the entry is REJECTED with a typed error — the demo
//! balance exists only for paper/simulate.
//!
//! ## Module map (one concern per file, the TASK 1–3 layout)
//! | file | concern |
//! |---|---|
//! | `lib.rs` | [`PolyBot`] construction, builders, the supervised run loop |
//! | `discover.rs` | Gamma discovery → quotes → strategy verdicts → frozen signals |
//! | `pipeline.rs` | the staged order pipeline: gates, the one risk decision, idempotency, ownership, sign, submit |
//! | `lifecycle.rs` | venue-order tracking: observations, fill accounting, polling, user-channel events, cancels |
//! | `reconcile.rs` | local order state vs the venue ([`ReconKind`], [`ReconFinding`]) |
//! | `recovery.rs` | restart re-adoption ([`RecoveryAction`], [`RecoveryReport`]) |
//! | `funding.rs` | collateral reads and pre-broadcast funding verification ([`CollateralSnapshot`]) |
//! | `store.rs` | durable journal contract ([`store::PolyStore`]) + in-memory implementation |
//! | `metrics.rs` | every `poly_*` series |
//! | `audit.rs` | the `poly.*` audit vocabulary ([`AUDIT_ACTOR`]) |
//! | `venue.rs` | credentials, authenticated client, heartbeat, kill-switch cancel / flatten |
//! | `orders.rs` | frozen intents, stages, reject reasons, the order state machine |
//! | `strategy.rs` | the two hardened strategies and their explicit verdicts |
//! | `gamma.rs` / `clob.rs` / `ws.rs` / `auth.rs` / `eip712.rs` / `ctf.rs` / `collateral.rs` / `error.rs` | venue clients, signing, chain reads, typed errors |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod clob;
pub mod collateral;
pub mod ctf;
pub mod eip712;
pub mod error;
pub mod gamma;
pub mod orders;
pub mod store;
pub mod strategy;
pub mod ws;

mod audit;
mod discover;
mod funding;
mod lifecycle;
mod metrics;
mod pipeline;
mod reconcile;
mod recovery;
mod venue;

pub use audit::AUDIT_ACTOR;
pub use funding::CollateralSnapshot;
pub use reconcile::{ReconFinding, ReconKind};
pub use recovery::{RecoveryAction, RecoveryReport};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use k256::ecdsa::SigningKey;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use bot_core::error::BotResult;
use bot_core::models::{BotModule, ExecutionMode, PolyMarket};
use bot_core::oms::OrderManager;
use bot_core::risk::RiskEngine;
use bot_core::state::Shared;

use crate::clob::ClobClient;
use crate::error::{PolyError, PolyResult};
use crate::gamma::GammaClient;
use crate::orders::TrackedOrder;
use crate::store::PolyStore;
use crate::strategy::Quote;
use crate::ws::{new_quote_map, run_market_feed, run_user_feed, QuoteMap, UserEvent};

/// Environment variable holding the Polygon private key (0x-hex, 32 bytes).
pub const PRIVATE_KEY_ENV: &str = "POLYMARKET_PRIVATE_KEY";
/// In-memory OMS capacity when the server did not attach a shared manager.
const LOCAL_OMS_CAP: usize = 2_000;
/// User-channel queue depth (events are tiny; a slow consumer must never
/// block the socket task for long).
const USER_EVENT_QUEUE: usize = 1_024;

/// The Polymarket bot.
pub struct PolyBot {
    state: Shared,
    gamma: GammaClient,
    clob: ClobClient,
    quotes: QuoteMap,
    risk: RiskEngine,
    /// Loaded from `POLYMARKET_PRIVATE_KEY` when present.
    signer: Option<SigningKey>,
    /// The signer's EOA address (lowercase 0x).
    address: Option<String>,
    /// API credentials for authenticated CLOB calls (derived when a key exists).
    api_key: Arc<RwLock<Option<auth::ApiKey>>>,
    /// On-chain CTF (ERC-1155) balance reader for fill-settlement truth
    /// (§Q). `None` when `[polymarket].ctf_rpc_url` is empty or invalid —
    /// the venue API then remains the only read, and reconciliation says
    /// "not configured" instead of guessing.
    ctf: Option<ctf::CtfClient>,
    /// On-chain collateral (ERC-20) reader for LIVE sizing and funding
    /// checks. Built from `[polymarket].ctf_rpc_url` +
    /// `collateral_address`. `None` when either is empty/invalid: paper and
    /// simulate keep working, but LIVE entries then REJECT with
    /// `BalanceUnavailable` instead of sizing against a demo balance.
    collateral: Option<collateral::CollateralClient>,
    /// Last verified collateral snapshot, reused only inside the freshness
    /// TTL. Written exclusively from real on-chain reads.
    collateral_cache: Arc<RwLock<Option<CollateralSnapshot>>>,
    /// Distributed execution ownership (Prompt 3 §B/§F), injected by the
    /// server. Entries claim the venue identity `poly:entry:{token_id}`.
    /// `None` = single-instance/legacy behaviour.
    ownership: Option<Arc<bot_core::ownership::OwnershipRegistry>>,
    /// The ONE order record + idempotency boundary: the server-attached OMS
    /// when present, else a private in-memory manager.
    orders: Arc<OrderManager>,
    /// Durable venue-level journal (migration 0014).
    store: Arc<dyn PolyStore>,
    /// Venue orders being tracked (non-terminal + recently terminal).
    tracked: Arc<RwLock<HashMap<String, TrackedOrder>>>,
    /// Last market snapshot per condition id (display + reprice checks).
    markets: Arc<RwLock<HashMap<String, PolyMarket>>>,
    /// Intent keys currently inside the pipeline (single-flight per intent
    /// within this process; the OMS key is the cross-process authority).
    /// A sync mutex: held for a few instructions, never across an await.
    inflight: std::sync::Mutex<HashSet<String>>,
    /// User-channel event queue (sender cloned into the feed task).
    user_tx: mpsc::Sender<UserEvent>,
    user_rx: Option<mpsc::Receiver<UserEvent>>,
}

impl PolyBot {
    /// Build the bot from shared state. Reads the optional private key from the
    /// environment; without it the bot runs read-only/paper.
    pub async fn new(state: Shared) -> PolyResult<Self> {
        let cfg = state.config_snapshot().await;
        let poly = cfg.polymarket.clone();
        let gamma = GammaClient::new(&poly.gamma_url)?;
        let clob = ClobClient::new(&poly.clob_url, poly.chain_id)?;
        let risk = RiskEngine::new(state.clone());

        let signer = load_signer()?;
        let address = signer.as_ref().map(eip712::address_from_signing_key);

        let ctf = if poly.ctf_rpc_url.trim().is_empty() {
            None
        } else {
            match ctf::CtfClient::new(&poly.ctf_rpc_url, &poly.conditional_tokens_address) {
                Ok(c) => Some(c),
                Err(e) => {
                    warn!(error = %e, "CTF balance reader disabled (bad rpc url/address)");
                    None
                }
            }
        };

        // Live collateral reader: same Polygon RPC as the CTF reader, but the
        // ERC-20 the CLOB actually settles in. Missing/invalid config disables
        // it — live entries then reject rather than guess a balance.
        let collateral = if poly.ctf_rpc_url.trim().is_empty()
            || poly.collateral_address.trim().is_empty()
        {
            None
        } else {
            match collateral::CollateralClient::new(&poly.ctf_rpc_url, &poly.collateral_address) {
                Ok(c) => Some(c),
                Err(e) => {
                    warn!(
                        error = %e,
                        "collateral reader disabled (bad rpc url/address) — live entries will reject"
                    );
                    None
                }
            }
        };

        let orders = match state.orders() {
            Some(mgr) => Arc::clone(mgr),
            None => OrderManager::new(None, LOCAL_OMS_CAP),
        };
        let (user_tx, user_rx) = mpsc::channel(USER_EVENT_QUEUE);

        Ok(PolyBot {
            state,
            gamma,
            clob,
            quotes: new_quote_map(),
            risk,
            signer,
            address,
            api_key: Arc::new(RwLock::new(None)),
            ctf,
            collateral,
            collateral_cache: Arc::new(RwLock::new(None)),
            ownership: None,
            orders,
            store: Arc::new(store::MemoryPolyStore::new()),
            tracked: Arc::new(RwLock::new(HashMap::new())),
            markets: Arc::new(RwLock::new(HashMap::new())),
            inflight: std::sync::Mutex::new(HashSet::new()),
            user_tx,
            user_rx: Some(user_rx),
        })
    }

    /// Attach the distributed execution-ownership registry (Prompt 3
    /// §B/§F/§P): every replica runs the same scanner over the same
    /// markets — the claim on `poly:entry:{token_id}` elects exactly one
    /// submitter per venue identity.
    #[must_use]
    pub fn with_ownership(mut self, reg: Arc<bot_core::ownership::OwnershipRegistry>) -> Self {
        self.ownership = Some(reg);
        self
    }

    /// Attach the durable journal (the server passes its Postgres-backed
    /// store; tests pass a [`store::MemoryPolyStore`]).
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn PolyStore>) -> Self {
        self.store = store;
        self
    }

    /// Use an explicit signing key instead of `POLYMARKET_PRIVATE_KEY`
    /// (embedding hosts and offline tests inject the key this way).
    #[must_use]
    pub fn with_signer(mut self, key: SigningKey) -> Self {
        self.address = Some(eip712::address_from_signing_key(&key));
        self.signer = Some(key);
        self
    }

    /// Use pre-derived CLOB API credentials instead of deriving them over
    /// L1 auth on first use.
    #[must_use]
    pub fn with_api_key(self, key: auth::ApiKey) -> Self {
        if let Ok(mut slot) = self.api_key.try_write() {
            *slot = Some(key);
        }
        self
    }

    /// The order manager this bot records every order in.
    pub fn orders(&self) -> &Arc<OrderManager> {
        &self.orders
    }

    /// Snapshot of every tracked venue order.
    pub async fn tracked_orders(&self) -> Vec<TrackedOrder> {
        let mut v: Vec<TrackedOrder> = self.tracked.read().await.values().cloned().collect();
        v.sort_by_key(|t| t.submitted_at);
        v
    }

    /// The websocket user-event sender (tests inject events the way the
    /// user channel would deliver them).
    pub fn user_event_sender(&self) -> mpsc::Sender<UserEvent> {
        self.user_tx.clone()
    }

    /// Feed one quote into the bot's quote cache — exactly what the market
    /// websocket does. Scans and the reprice check read from this cache
    /// before falling back to the REST book.
    pub async fn ingest_quote(&self, token_id: &str, quote: Quote) {
        self.quotes
            .write()
            .await
            .insert(token_id.to_string(), quote);
    }

    /// Whether the bot can sign (a private key is present).
    pub fn can_sign(&self) -> bool {
        self.signer.is_some()
    }

    /// TASK 5 — the account the global ledger attributes this module's
    /// exposure to: the signer EOA when a key is loaded, else the module
    /// name (paper / read-only runs have no wallet).
    pub(crate) fn ledger_wallet(&self) -> String {
        self.address
            .clone()
            .unwrap_or_else(|| BotModule::Polymarket.as_str().to_string())
    }

    /// TASK 5 — the strategy label the global ledger attributes fills to:
    /// the configured `[polymarket].strategy` (the only strategy that
    /// produces orders in this process).
    pub(crate) async fn strategy_label(&self) -> String {
        let cfg = self.state.config_snapshot().await;
        bot_core::global_risk::strategy_label(BotModule::Polymarket, Some(&cfg.polymarket.strategy))
    }

    /// On-chain settled balance of one outcome token (CTF ERC-1155
    /// `balanceOf`) for the funder wallet. `Ok(None)` = reader not
    /// configured; `Err` = could not read — per §O that is NEVER a zero.
    pub async fn ctf_balance(&self, token_id: &str) -> PolyResult<Option<u128>> {
        let Some(ctf) = &self.ctf else {
            return Ok(None);
        };
        let cfg = self.state.config_snapshot().await;
        let owner = cfg
            .polymarket
            .funder_address
            .clone()
            .or_else(|| self.address.clone())
            .ok_or_else(|| PolyError::not_configured("no funder/EOA address for CTF read"))?;
        ctf.balance_of(&owner, token_id).await.map(Some)
    }

    // ------------------------------------------------------------------
    // Run loop
    // ------------------------------------------------------------------

    /// Run the bot until the task is aborted.
    pub async fn run(&mut self) -> BotResult<()> {
        self.state
            .set_running(BotModule::Polymarket, true, false)
            .await;
        self.state
            .set_detail(BotModule::Polymarket, "starting")
            .await;

        let cfg = self.state.config_snapshot().await;
        let poly = cfg.polymarket.clone();
        if self.signer.is_some() {
            info!(address = ?self.address, "polymarket signer loaded");
        } else {
            warn!("no POLYMARKET_PRIVATE_KEY set — running read-only (paper fills only)");
        }

        // Authenticate (derive API creds) only when we can sign and are not in
        // pure paper mode. Failure is non-fatal: we fall back to read-only.
        if self.signer.is_some() && self.state.execution_mode().await != ExecutionMode::Paper {
            if let Err(e) = self.ensure_api_key().await {
                warn!(error = %e, "could not derive CLOB api key; authenticated calls disabled");
            }
        }

        // Restart recovery BEFORE the first scan: re-adopt open orders so the
        // exposure / open-order checks see them.
        match self.recover_after_restart().await {
            Ok(report) => {
                if !report.actions.is_empty() {
                    info!(
                        adopted = report.count(RecoveryAction::AdoptedJournalOrder)
                            + report.count(RecoveryAction::AdoptedOmsOrder),
                        held = report.count(RecoveryAction::HeldAmbiguous),
                        cleaned = report.count(RecoveryAction::FailedStalePaper)
                            + report.count(RecoveryAction::FailedUnsent),
                        "polymarket restart recovery complete"
                    );
                }
            }
            Err(e) => warn!(error = %e, "polymarket restart recovery failed"),
        }

        // Spawn the websocket feed once we know some token ids; we (re)start it
        // after the first scan populates the tracked set.
        let mut ws_started = false;
        let mut user_ws_started = false;
        let mut hb_started = false;
        let mut user_rx = self.user_rx.take();

        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
            poly.scan_interval_secs.max(5),
        ));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut poll = tokio::time::interval(std::time::Duration::from_secs(
            poly.order_poll_interval_secs.max(1),
        ));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut recon = tokio::time::interval(std::time::Duration::from_secs(
            poly.reconcile_interval_secs.max(1),
        ));
        recon.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            enum Tick {
                Scan,
                Poll,
                Recon,
                // Boxed: a user event (order + trade payloads) is far larger
                // than the timer arms and the select runs every tick.
                User(Box<UserEvent>),
                Shutdown,
            }
            let tick = tokio::select! {
                _ = ticker.tick() => Tick::Scan,
                _ = poll.tick() => Tick::Poll,
                _ = recon.tick() => Tick::Recon,
                ev = recv_user_event(&mut user_rx) => Tick::User(Box::new(ev)),
                _ = self.state.wait_shutdown() => Tick::Shutdown,
            };
            let cfg = self.state.config_snapshot().await;
            let poly = cfg.polymarket.clone();
            match tick {
                Tick::Shutdown => {
                    info!("module 3 (polymarket) stopping (shutdown)");
                    if poly.cancel_on_shutdown {
                        let n = self.cancel_all_tracked("shutdown").await;
                        if n > 0 {
                            info!(
                                cancelled = n,
                                "polymarket resting orders cancelled on shutdown"
                            );
                        }
                    }
                    break Ok(());
                }
                Tick::User(ev) => {
                    if let Err(e) = self.apply_user_event(&ev).await {
                        warn!(error = %e, "polymarket user event rejected");
                    }
                    continue;
                }
                Tick::Poll => {
                    if poly.enabled {
                        if let Err(e) = self.poll_orders_once(&poly).await {
                            debug!(error = %e, "polymarket order poll failed");
                        }
                        self.publish_gauges().await;
                    }
                    continue;
                }
                Tick::Recon => {
                    if poly.enabled && poly.reconcile_interval_secs > 0 {
                        if let Err(e) = self.reconcile_once(&poly).await {
                            debug!(error = %e, "polymarket reconciliation failed");
                        }
                    }
                    continue;
                }
                Tick::Scan => {}
            }
            if !poly.enabled {
                self.state
                    .set_detail(BotModule::Polymarket, "disabled")
                    .await;
                continue;
            }
            if self.state.kill_switch() {
                self.state
                    .set_detail(BotModule::Polymarket, "kill switch")
                    .await;
                continue;
            }
            if !self.state.is_enabled(BotModule::Polymarket).await {
                continue;
            }

            // Heartbeat task (dead-man's switch) once authenticated.
            if poly.heartbeat && !hb_started && self.api_key.read().await.is_some() {
                self.spawn_heartbeat(poly.heartbeat_interval_secs.max(2));
                hb_started = true;
            }

            match self.scan_once(&poly).await {
                Ok(tracked) => {
                    let open = self.open_order_count().await;
                    self.state
                        .set_detail(
                            BotModule::Polymarket,
                            format!("tracking {} tokens, {open} open orders", tracked.len()),
                        )
                        .await;
                    // (Re)start the websocket with the current tracked set.
                    if poly.use_websocket && !tracked.is_empty() && !ws_started {
                        let url = format!("{}market", poly.ws_url.trim_end_matches('/'));
                        let quotes = self.quotes.clone();
                        let state = self.state.clone();
                        let ids: Vec<String> = tracked.to_vec();
                        tokio::spawn(async move {
                            let _ = run_market_feed(url, ids, quotes, state).await;
                        });
                        ws_started = true;
                    }
                    // Authenticated user channel: order / trade events for
                    // the markets we scan.
                    if poly.use_user_websocket && !user_ws_started {
                        if let Some(ak) = self.api_key.read().await.clone() {
                            let markets: Vec<String> =
                                self.markets.read().await.keys().cloned().collect();
                            if !markets.is_empty() {
                                let url = format!("{}user", poly.ws_url.trim_end_matches('/'));
                                let tx = self.user_tx.clone();
                                let state = self.state.clone();
                                tokio::spawn(async move {
                                    let _ = run_user_feed(url, ak, markets, tx, state).await;
                                });
                                user_ws_started = true;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "polymarket scan failed");
                    self.state
                        .record_error(BotModule::Polymarket, &format!("scan: {e}"))
                        .await;
                }
            }
            self.state.heartbeat(BotModule::Polymarket).await;
        }
    }
}

/// Await the next user event, or forever when the channel is absent.
async fn recv_user_event(rx: &mut Option<mpsc::Receiver<UserEvent>>) -> UserEvent {
    match rx {
        Some(r) => match r.recv().await {
            Some(ev) => ev,
            None => {
                // All senders dropped: park forever (the select keeps the
                // other branches alive).
                *rx = None;
                std::future::pending().await
            }
        },
        None => std::future::pending().await,
    }
}

/// Load the Polygon signing key from the environment, if present.
fn load_signer() -> PolyResult<Option<SigningKey>> {
    let raw = match std::env::var(PRIVATE_KEY_ENV) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let hexstr = raw.trim().strip_prefix("0x").unwrap_or(raw.trim());
    if hexstr.is_empty() {
        return Ok(None);
    }
    let bytes =
        hex::decode(hexstr).map_err(|e| PolyError::signing(format!("private key hex: {e}")))?;
    let key = SigningKey::from_slice(&bytes)
        .map_err(|e| PolyError::signing(format!("private key: {e}")))?;
    Ok(Some(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recon_and_recovery_labels_are_stable() {
        for k in [
            ReconKind::OrphanVenueOrder,
            ReconKind::LocalOrderMissingOnVenue,
            ReconKind::MatchedSizeMismatch,
            ReconKind::AmbiguousSubmitResolved,
            ReconKind::PositionWithoutOrder,
            ReconKind::StaleOrder,
        ] {
            assert!(k
                .as_str()
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        let mut r = RecoveryReport::default();
        r.actions
            .push((RecoveryAction::HeldAmbiguous, "0x1".into()));
        r.actions
            .push((RecoveryAction::HeldAmbiguous, "0x2".into()));
        assert_eq!(r.count(RecoveryAction::HeldAmbiguous), 2);
        assert_eq!(r.count(RecoveryAction::AdoptedOmsOrder), 0);
        assert_eq!(
            RecoveryAction::FailedStalePaper.as_str(),
            "failed_stale_paper"
        );
        assert_eq!(audit::sanitize("a\nb"), "a b");
        assert!(audit::sanitize(&"x".repeat(500)).len() < 260);
    }
}
