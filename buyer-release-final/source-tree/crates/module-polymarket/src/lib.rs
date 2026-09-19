//! Module 3 — Polymarket betting bot.
//!
//! Pipeline: **discover** markets (Gamma) → **price** them (CLOB REST +
//! websocket) → **decide** (strategy) → **gate** (shared risk engine) →
//! **sign** (EIP-712 V2) → **submit** (CLOB). Paper mode is the default: the
//! full pipeline runs against live market data, but orders are only broadcast
//! when `execution.mode = "live"` *and* `execution.allow_live_trading = true`.
//! Without a `POLYMARKET_PRIVATE_KEY` the bot runs read-only and records paper
//! fills, so it is safe to demo with no funds at risk.
//!
//! ## Live money separation
//! LIVE entries are sized against a verified on-chain collateral read
//! ([`collateral`], ERC-20 `balanceOf`/`decimals` on Polygon, freshness-
//! bounded), and before any live order is broadcast the funder's balance AND
//! the settling exchange's ERC-20 allowance must cover the approved notional.
//! When any of that cannot be verified the entry is REJECTED with a typed
//! error — the demo balance exists only for paper/simulate.

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
pub mod strategy;
pub mod ws;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use k256::ecdsa::SigningKey;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use bot_core::config::PolymarketConfig;
use bot_core::error::BotResult;
use bot_core::events::AppEvent;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::risk::{EntryRequest, RiskEngine};
use bot_core::state::Shared;

use crate::clob::ClobClient;
use crate::error::{PolyError, PolyResult};
use crate::gamma::{GammaClient, MarketQuery};
use crate::orders::{sign_order_bundle, OrderParams};
use crate::strategy::{evaluate, OrderDecision, Quote};
use crate::ws::{new_quote_map, run_market_feed, QuoteMap};

/// Environment variable holding the Polygon private key (0x-hex, 32 bytes).
pub const PRIVATE_KEY_ENV: &str = "POLYMARKET_PRIVATE_KEY";
/// Demo USDC balance used for PAPER/SIMULATE sizing only. LIVE mode never
/// uses this figure: a live entry whose real collateral balance cannot be
/// verified is rejected with [`PolyError::BalanceUnavailable`].
const PAPER_USDC_BALANCE: f64 = 1_000.0;
/// Freshness bound for reusing one verified collateral snapshot across the
/// decisions of a single scan. Every live sizing/funding decision therefore
/// runs against an on-chain read at most this old.
const COLLATERAL_CACHE_TTL_SECS: i64 = 15;

/// One verified on-chain collateral read (never a cache seed, never paper).
#[derive(Clone, Debug)]
pub struct CollateralSnapshot {
    /// Balance in raw token units (as returned by `balanceOf`).
    pub raw: u128,
    /// On-chain `decimals()` of the collateral token, validated 1..=18.
    pub decimals: u8,
    /// `raw` converted to whole-token (USD) units via `decimals`.
    pub usd: f64,
    /// When the read happened — the freshness bound.
    pub ts: chrono::DateTime<Utc>,
}

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

    /// Whether the bot can sign (a private key is present).
    pub fn can_sign(&self) -> bool {
        self.signer.is_some()
    }

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

        // Spawn the websocket feed once we know some token ids; we (re)start it
        // after the first scan populates the tracked set.
        let mut ws_started = false;
        let mut hb_started = false;

        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
            poly.scan_interval_secs.max(5),
        ));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = self.state.wait_shutdown() => {
                    info!("module 3 (polymarket) stopping (shutdown)");
                    break Ok(());
                }
            }
            let cfg = self.state.config_snapshot().await;
            let poly = cfg.polymarket.clone();
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
                    self.state
                        .set_detail(
                            BotModule::Polymarket,
                            format!("tracking {} tokens", tracked.len()),
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

    /// One catalogue scan: discover → price → decide → act. Returns the set of
    /// token ids we are now tracking (for the websocket).
    async fn scan_once(&self, poly: &PolymarketConfig) -> PolyResult<Vec<String>> {
        let markets = self.discover(poly).await?;
        if markets.is_empty() {
            return Ok(Vec::new());
        }

        let mut tracked = Vec::new();
        let mut acted = 0usize;

        for market in &markets {
            for o in &market.outcomes {
                tracked.push(o.token_id.clone());
            }
            let quotes = self.quotes_for(market, poly).await;
            if quotes.is_empty() {
                continue;
            }
            let decisions = evaluate(market, &quotes, poly);
            for decision in decisions {
                if acted >= poly.max_open_markets.saturating_mul(2) {
                    break;
                }
                match self.act_on_decision(&decision, market, &quotes, poly).await {
                    Ok(true) => acted += 1,
                    Ok(false) => {}
                    Err(e) => {
                        warn!(token = %decision.token_id, error = %e, "polymarket order failed");
                        self.state
                            .record_error(
                                BotModule::Polymarket,
                                &format!("{}: {e}", decision.outcome),
                            )
                            .await;
                    }
                }
            }
        }
        Ok(tracked)
    }

    /// Discover candidate markets per the configured strategy.
    async fn discover(
        &self,
        _poly: &PolymarketConfig,
    ) -> PolyResult<Vec<bot_core::models::PolyMarket>> {
        let query = MarketQuery {
            active: Some(true),
            closed: Some(false),
            limit: Some(50),
            order: Some("volume24hr".into()),
            ascending: Some(false),
            ..Default::default()
        };
        // The search strategy narrows by keyword client-side; keep the query
        // broad and let `evaluate` filter.
        self.gamma.markets(&query).await
    }

    /// Build a per-token quote map for a market: websocket first, REST fallback.
    async fn quotes_for(
        &self,
        market: &bot_core::models::PolyMarket,
        _poly: &PolymarketConfig,
    ) -> HashMap<String, Quote> {
        let mut out = HashMap::new();
        let cached = self.quotes.read().await;
        for o in &market.outcomes {
            if let Some(q) = cached.get(&o.token_id) {
                out.insert(o.token_id.clone(), *q);
            }
        }
        drop(cached);

        // Fill any gaps from the REST book.
        for o in &market.outcomes {
            if out.contains_key(&o.token_id) {
                continue;
            }
            match self.clob.order_book(&o.token_id).await {
                Ok(book) => {
                    out.insert(o.token_id.clone(), book.to_quote());
                }
                Err(e) => debug!(token = %o.token_id, error = %e, "could not fetch book"),
            }
        }
        out
    }

    /// Gate + (optionally) submit one decision. Returns `true` if a position was
    /// opened (or paper-filled).
    async fn act_on_decision(
        &self,
        decision: &OrderDecision,
        market: &bot_core::models::PolyMarket,
        quotes: &HashMap<String, Quote>,
        poly: &PolymarketConfig,
    ) -> PolyResult<bool> {
        // Already holding this token? Skip (one position per outcome).
        if self
            .state
            .find_open(BotModule::Polymarket, &decision.token_id)
            .await
            .is_some()
        {
            return Ok(false);
        }

        // Per-symbol reconciliation gate (§H): refuse NEW entries while this
        // outcome token has unresolved claims. Exits (flatten/cancel) are
        // never gated — reducing exposure is always safe.
        if self.state.is_symbol_blocked(&decision.token_id).await {
            bot_core::obs::metrics::global()
                .counter(
                    "bot_symbol_gated_entries_total",
                    "Entries refused because the symbol is gated by unresolved reconciliation.",
                    &[("module", "polymarket")],
                )
                .inc();
            debug!(token = %decision.token_id, "symbol gated by unresolved reconciliation — skipping entry");
            return Ok(false);
        }

        let mode = self.state.execution_mode().await;
        // LIVE sizing REQUIRES a verified on-chain collateral read; a failure
        // here rejects the entry with a typed error (never a paper figure).
        let available = self.available_collateral(mode).await?;
        let liquidity = quotes
            .get(&decision.token_id)
            .map(|q| (q.best_bid + q.best_ask) * market.liquidity.max(1.0))
            .unwrap_or(market.liquidity);

        let req = EntryRequest {
            module: BotModule::Polymarket,
            venue: Venue::PolymarketClob,
            symbol: decision.token_id.clone(),
            symbol_display: format!("{} {}", market.question, decision.outcome),
            requested_quote: decision.stake_usd,
            available_quote: available,
            slippage_bps: 0,
            price: Some(decision.limit_price),
            fair_value: None,
            liquidity: Some(liquidity),
        };
        let risk_decision = self.risk.check_entry(&req).await;
        if !risk_decision.allowed() {
            self.state.inc_risk_rejected(BotModule::Polymarket).await;
            self.state.events.publish(AppEvent::RiskRejected {
                ts: Utc::now(),
                module: BotModule::Polymarket,
                symbol: decision.outcome.clone(),
                reason: risk_decision.reason.clone(),
            });
            debug!(token = %decision.token_id, reason = %risk_decision.reason, "polymarket entry rejected");
            return Ok(false);
        }
        self.state.inc_signals(BotModule::Polymarket).await;

        // Rescale size to the risk-approved notional.
        let approved_stake = risk_decision.sized_quote;
        let size_tokens = if decision.limit_price > 0.0 {
            approved_stake / decision.limit_price
        } else {
            decision.size_tokens
        };

        self.state.events.publish(AppEvent::Signal {
            ts: Utc::now(),
            module: BotModule::Polymarket,
            symbol: decision.outcome.clone(),
            side: if decision.is_buy { "buy" } else { "sell" }.into(),
            reason: decision.reason.clone(),
            strength: (1.0 - decision.limit_price).max(0.0),
        });
        self.state.events.publish(AppEvent::Polymarket {
            ts: Utc::now(),
            message: format!(
                "{} @ {:.3} (${:.2})",
                decision.reason, decision.limit_price, approved_stake
            ),
            market: Some(market.question.clone()),
            price: Some(decision.limit_price),
        });

        // Live funding gate: when this decision will actually be broadcast,
        // the funder's on-chain collateral must cover the approved notional
        // AND the settling exchange must hold the ERC-20 allowance to pull
        // it. Both are real reads — any failure rejects the order with a
        // typed error before an ownership permit is even taken.
        let will_send = mode == ExecutionMode::Live
            && self.signer.is_some()
            && self.api_key.read().await.is_some();
        if will_send {
            self.ensure_live_funding(decision, approved_stake, poly)
                .await?;
        }

        // Distributed execution ownership (Prompt 3 §B/§F/§P): claim the
        // venue identity BEFORE any submission. Loser replicas skip
        // deterministically (§G); store failures fail closed (§K).
        let mut permit = bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("poly:entry:{}", decision.token_id),
            "poly_entry",
            "polymarket",
            "clob",
            &decision.token_id,
        )
        .await
        .map_err(|e| PolyError::invalid(format!("ownership unavailable: {e}")))?;
        if !permit.proceed() {
            debug!(token = %decision.token_id, "poly entry owned by another replica — skipping");
            return Ok(false);
        }

        // Build + sign (if we can), then submit only in live mode.
        // `will_send` was computed BEFORE the ownership claim so the live
        // funding check runs before any permit is taken.
        let (signature, order_id, status) = if will_send {
            // Fencing (§E): ownership must still be ours at submission time.
            permit
                .fence()
                .await
                .map_err(|e| PolyError::invalid(format!("fencing rejected: {e}")))?;
            match self.submit_live(decision, market, size_tokens, poly).await {
                Ok(v) => {
                    // The order is POSTed and may rest on the book: HAND OFF
                    // (§I/§M). The grace window (default 15 min) comfortably
                    // covers the book-sync interval (30 s), so no replica can
                    // double-post the same token before `find_open` converges.
                    permit.finish(true).await;
                    v
                }
                Err(e) => {
                    // SubmitUnknown = the signed order may be resting →
                    // reconciliation owns the outcome; every other error is a
                    // definite rejection → release for a fresh decision.
                    permit
                        .finish(matches!(e, PolyError::SubmitUnknown { .. }))
                        .await;
                    return Err(e);
                }
            }
        } else {
            // Paper / simulate / no-key: nothing left the process → release.
            permit.finish(false).await;
            (
                None,
                None,
                crate::clob::PostOrderResponse {
                    order_id: None,
                    success: Some(true),
                    error_msg: None,
                    status: Some("paper".into()),
                    taking_amount: None,
                    making_amount: None,
                },
            )
        };
        let _ = status;

        self.record_position(
            decision,
            market,
            size_tokens,
            approved_stake,
            signature,
            order_id,
        )
        .await;
        Ok(true)
    }

    /// Sign and POST the order to the CLOB (live only).
    async fn submit_live(
        &self,
        decision: &OrderDecision,
        market: &bot_core::models::PolyMarket,
        size_tokens: f64,
        poly: &PolymarketConfig,
    ) -> PolyResult<(
        Option<String>,
        Option<String>,
        crate::clob::PostOrderResponse,
    )> {
        let key = self
            .signer
            .as_ref()
            .ok_or_else(|| PolyError::not_configured("no signer for live order"))?;
        let tick = self
            .clob
            .tick_size(&market.condition_id)
            .await
            .unwrap_or_else(|_| "0.01".into());
        let expiration = if poly.order_type.eq_ignore_ascii_case("GTD") && poly.expiration_secs > 0
        {
            (Utc::now().timestamp() + poly.expiration_secs).max(0) as u64
        } else {
            0
        };
        let params = OrderParams {
            token_id: &decision.token_id,
            is_buy: decision.is_buy,
            size: size_tokens,
            price: decision.limit_price,
            tick_size: &tick,
            neg_risk: decision.neg_risk,
            chain_id: poly.chain_id,
            domain_version: &poly.exchange_domain_version,
            exchange_address: &poly.exchange_address,
            neg_risk_exchange_address: &poly.neg_risk_exchange_address,
            signature_type: poly.signature_type,
            funder: poly.funder_address.as_deref(),
            expiration_timestamp: expiration,
            builder_code: poly.builder_code.as_deref(),
        };
        let bundle = sign_order_bundle(key, &params)?;

        let clob = self.clob.clone();
        if let Some(ak) = self.api_key.read().await.clone() {
            let clob = clob.with_auth(self.address.clone().unwrap_or_default(), ak);
            let resp = match clob.post_order(&bundle, &poly.order_type).await {
                Ok(resp) => resp,
                Err(e @ PolyError::Http(_)) => {
                    // Transport failure: the signed order may or may not have
                    // reached the book. Derive the CLOB order id locally (it
                    // is the EIP-712 struct hash), publish OrderSent so the
                    // persistence layer records the claim and enqueues venue
                    // reconciliation, then surface SubmitUnknown. The
                    // deterministic salt means a retry of the same intent
                    // reuses the same order id — no duplicate resting order.
                    let derived = bundle.derived_order_id()?;
                    warn!(
                        order_id = %derived,
                        error = %e,
                        "polymarket submit outcome unknown; queued for reconciliation"
                    );
                    self.state.events.publish(AppEvent::OrderSent {
                        ts: Utc::now(),
                        module: BotModule::Polymarket,
                        symbol: decision.token_id.clone(),
                        venue: Venue::PolymarketClob.as_str().to_string(),
                        mode: self.state.execution_mode().await.as_str().to_string(),
                        quote_amount: decision.limit_price * size_tokens,
                        signature: Some(derived.clone()),
                        signer: self.address.clone(),
                        attempts: None,
                        latency_ms: None,
                    });
                    return Err(PolyError::SubmitUnknown {
                        order_id: derived,
                        reason: e.to_string(),
                    });
                }
                Err(e) => return Err(e),
            };
            let ok = resp.success.unwrap_or(false);
            if !ok {
                return Err(PolyError::clob(
                    resp.error_msg
                        .clone()
                        .unwrap_or_else(|| "order rejected".into()),
                ));
            }
            return Ok((Some(bundle.signature.clone()), resp.order_id.clone(), resp));
        }
        Err(PolyError::not_configured("no api key for live order"))
    }

    /// Record a (paper or live) fill as a position + trade + events.
    #[allow(clippy::too_many_arguments)]
    async fn record_position(
        &self,
        decision: &OrderDecision,
        market: &bot_core::models::PolyMarket,
        size_tokens: f64,
        stake_usd: f64,
        signature: Option<String>,
        order_id: Option<String>,
    ) {
        let mode = self.state.execution_mode().await;
        self.state.inc_orders_sent(BotModule::Polymarket).await;

        let price = decision.limit_price;
        let trade = Trade {
            id: self.state.next_id("t"),
            ts: Utc::now(),
            source: TradeSource::Polymarket,
            venue: Venue::PolymarketClob,
            mode,
            side: if decision.is_buy {
                PositionSide::Long
            } else {
                PositionSide::Short
            },
            symbol: decision.token_id.clone(),
            symbol_display: format!("{} {}", market.question, decision.outcome),
            amount_in: stake_usd,
            amount_out: size_tokens,
            quote_symbol: "USDC".into(),
            price,
            fee: 0.0,
            slippage_bps: 0,
            signature: signature.clone(),
            position_id: None,
            note: Some(format!(
                "{}{}",
                decision.reason,
                order_id.map(|o| format!(" order={o}")).unwrap_or_default()
            )),
            latency_ms: None,
        };
        self.state.record_trade(trade.clone()).await;
        self.state.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade),
        });

        let pos_id = self.state.next_id("p");
        let mut position = Position::new(
            pos_id.clone(),
            TradeSource::Polymarket,
            Venue::PolymarketClob,
            mode,
            decision.token_id.clone(),
            decision.outcome.clone(),
            "USDC".into(),
        );
        position.apply_buy(size_tokens, price, stake_usd);
        position.market_id = Some(market.condition_id.clone());
        position.outcome = Some(decision.outcome.clone());
        position.entry_signature = signature;
        // Exit at redemption (1.0) or a stop below entry.
        position.take_profit = Some(0.99);
        position.stop_loss = Some((price * 0.5).max(0.01));
        self.state.upsert_position(position.clone()).await;
        self.state.events.publish(AppEvent::PositionUpdate {
            ts: Utc::now(),
            position: Box::new(position),
        });

        info!(
            outcome = %decision.outcome,
            token = %decision.token_id,
            price,
            size_tokens,
            stake_usd,
            mode = %mode.as_str(),
            "polymarket order placed"
        );
    }

    /// Collateral available for sizing, with STRICT paper/live separation.
    ///
    /// * LIVE: a verified on-chain read is REQUIRED. The cached dashboard
    ///   balance is ignored (it may be a paper seed left over from a
    ///   paper→live mode switch) and the paper figure is never returned. Any
    ///   failure yields [`PolyError::BalanceUnavailable`], which rejects the
    ///   entry upstream — there is no fallback path.
    /// * PAPER / SIMULATE (demo paths that never broadcast): the seeded demo
    ///   balance first, a real read when one is possible, else the paper
    ///   figure, so demo mode keeps working with no funds and no RPC.
    async fn available_collateral(&self, mode: ExecutionMode) -> PolyResult<f64> {
        let cached = self.state.balances().await.usdc_polygon;
        if mode != ExecutionMode::Live && cached > 0.0 {
            return Ok(cached);
        }
        let live = if self.collateral.is_some() {
            Some(self.read_collateral().await.map(|s| s.usd))
        } else {
            None
        };
        resolve_sizing_balance(mode, cached, live)
    }

    /// Verified on-chain collateral snapshot for the funder wallet, reused
    /// only within [`COLLATERAL_CACHE_TTL_SECS`]. Every failure mode is
    /// surfaced as [`PolyError::BalanceUnavailable`] — a caller in live mode
    /// MUST reject rather than substitute any other figure. Successful reads
    /// mirror the REAL balance into shared state (dashboard/telemetry).
    async fn read_collateral(&self) -> PolyResult<CollateralSnapshot> {
        let client = self.collateral.as_ref().ok_or_else(|| {
            PolyError::balance_unavailable(
                "collateral reader not configured ([polymarket].ctf_rpc_url / collateral_address)",
            )
        })?;
        if let Some(snap) = self.collateral_cache.read().await.clone() {
            if Utc::now().signed_duration_since(snap.ts).num_seconds() < COLLATERAL_CACHE_TTL_SECS {
                return Ok(snap);
            }
        }
        let cfg = self.state.config_snapshot().await;
        let owner = cfg
            .polymarket
            .funder_address
            .clone()
            .or_else(|| self.address.clone())
            .ok_or_else(|| {
                PolyError::balance_unavailable("no funder/EOA address for collateral read")
            })?;
        let raw = client.balance_of(&owner).await.map_err(|e| {
            PolyError::balance_unavailable(format!("collateral balanceOf failed: {e}"))
        })?;
        let decimals = client.decimals().await.map_err(|e| {
            PolyError::balance_unavailable(format!("collateral decimals read failed: {e}"))
        })?;
        // Identity/plausibility: the read targeted exactly the configured
        // `collateral_address` (validated 0x+40 hex at construction), and a
        // Polymarket collateral token is a stablecoin with sane decimals —
        // anything outside 1..=18 means the config points at the wrong
        // contract and MUST NOT be used to scale a balance.
        if !(1..=18).contains(&decimals) {
            return Err(PolyError::balance_unavailable(format!(
                "collateral decimals {decimals} implausible for a stablecoin (expected 1..=18)"
            )));
        }
        let usd = collateral::raw_to_usd(raw, decimals);
        let snap = CollateralSnapshot {
            raw,
            decimals,
            usd,
            ts: Utc::now(),
        };
        *self.collateral_cache.write().await = Some(snap.clone());
        self.state.set_balances(None, Some(usd)).await;
        Ok(snap)
    }

    /// Pre-broadcast funding verification for LIVE orders:
    ///
    /// * the funder's on-chain collateral balance (fresh read, same TTL
    ///   bound as sizing) must cover the risk-approved notional;
    /// * for EOA signing (`signature_type == 0`) the exchange contract that
    ///   will settle this order (`neg_risk` selects between the two) must
    ///   also hold an ERC-20 allowance of at least the approved notional —
    ///   without it the CLOB cannot pull the funds and the order would fail
    ///   on-chain or rest unfundable. Proxy-wallet flows (`signature_type`
    ///   1/2/3) move funds through the funder proxy itself, so only the
    ///   balance check applies there.
    ///
    /// Read failures reject with [`PolyError::BalanceUnavailable`];
    /// insufficient funds/allowance reject with
    /// [`PolyError::InsufficientFunding`]. No fallback exists.
    async fn ensure_live_funding(
        &self,
        decision: &OrderDecision,
        approved_stake: f64,
        poly: &PolymarketConfig,
    ) -> PolyResult<()> {
        let client = self.collateral.as_ref().ok_or_else(|| {
            PolyError::balance_unavailable(
                "collateral reader not configured ([polymarket].ctf_rpc_url / collateral_address)",
            )
        })?;
        let owner = poly
            .funder_address
            .clone()
            .or_else(|| self.address.clone())
            .ok_or_else(|| {
                PolyError::balance_unavailable("no funder/EOA address for funding check")
            })?;

        let balance = self.read_collateral().await?;
        let required = collateral::usd_to_raw(approved_stake, balance.decimals)?;
        if balance.raw < required {
            return Err(PolyError::insufficient_funding(format!(
                "collateral balance {} raw < approved {} raw ({} decimals)",
                balance.raw, required, balance.decimals
            )));
        }

        if poly.signature_type == 0 {
            let spender = if decision.neg_risk {
                &poly.neg_risk_exchange_address
            } else {
                &poly.exchange_address
            };
            let allowance = client.allowance(&owner, spender).await.map_err(|e| {
                PolyError::balance_unavailable(format!("collateral allowance read failed: {e}"))
            })?;
            if allowance < required {
                return Err(PolyError::insufficient_funding(format!(
                    "exchange {spender} allowance {allowance} raw < approved {required} raw — approve the exchange first"
                )));
            }
        }
        Ok(())
    }

    /// Provider-side status of one order (reconciliation truth source).
    /// `Ok(None)` when this bot cannot authenticate (no signer / no key) —
    /// the caller treats that as "retry later", not as an answer.
    pub async fn order_status(&self, order_id: &str) -> PolyResult<Option<serde_json::Value>> {
        if self.ensure_api_key().await.is_err() {
            return Ok(None);
        }
        let Some(ak) = self.api_key.read().await.clone() else {
            return Ok(None);
        };
        let client = self
            .clob
            .clone()
            .with_auth(self.address.clone().unwrap_or_default(), ak);
        Ok(Some(client.order_status(order_id).await?))
    }

    /// Derive API credentials from the signer (L1 auth) if not already present.
    async fn ensure_api_key(&self) -> PolyResult<()> {
        if self.api_key.read().await.is_some() {
            return Ok(());
        }
        let key = self
            .signer
            .as_ref()
            .ok_or_else(|| PolyError::not_configured("no signer"))?;
        let address = self
            .address
            .clone()
            .ok_or_else(|| PolyError::not_configured("no address"))?;
        let cfg = self.state.config_snapshot().await;
        let creds = ClobClient::derive_api_key(
            &cfg.polymarket.clob_url,
            cfg.polymarket.chain_id,
            key,
            &address,
        )
        .await?;
        *self.api_key.write().await = Some(creds);
        info!("derived CLOB api credentials");
        Ok(())
    }

    /// Spawn the heartbeat task (dead-man's switch).
    fn spawn_heartbeat(&self, interval_secs: u64) {
        let clob = self.clob.clone();
        let address = self.address.clone().unwrap_or_default();
        let api_key = self.api_key.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = state.wait_shutdown() => break,
                }
                let Some(ak) = api_key.read().await.clone() else {
                    continue;
                };
                let client = clob.clone().with_auth(address.clone(), ak);
                if let Err(e) = client.heartbeat().await {
                    debug!(error = %e, "polymarket heartbeat failed");
                    state
                        .record_error(BotModule::Polymarket, &format!("heartbeat: {e}"))
                        .await;
                }
            }
        });
    }

    /// Cancel all open CLOB orders (used by the kill switch path).
    pub async fn cancel_all(&self) -> PolyResult<()> {
        let Some(ak) = self.api_key.read().await.clone() else {
            return Err(PolyError::not_configured("no api key"));
        };
        let client = self
            .clob
            .clone()
            .with_auth(self.address.clone().unwrap_or_default(), ak);
        client.cancel_all().await?;
        Ok(())
    }

    /// Mark all open polymarket positions as stopped (kill switch flatten).
    pub async fn flatten(&self, reason: &str) {
        let positions = self.state.open_positions_for(BotModule::Polymarket).await;
        for p in positions {
            self.state
                .close_position(&p.id, PositionStatus::StoppedOut, reason)
                .await;
        }
    }
}

/// Pure paper/live separation rule for the sizing balance — the single place
/// where a demo figure may be chosen, so the invariant is unit-testable
/// without state or network:
///
/// * LIVE: only a verified live read counts. The cached state balance is
///   IGNORED (it may be a paper seed left over from a mode switch), a failed
///   or missing read is `BalanceUnavailable`, and an implausible read
///   (negative / non-finite) is rejected too.
/// * PAPER / SIMULATE: cached demo balance first, then a live read when one
///   was possible, else [`PAPER_USDC_BALANCE`].
pub(crate) fn resolve_sizing_balance(
    mode: ExecutionMode,
    cached_state: f64,
    live: Option<PolyResult<f64>>,
) -> PolyResult<f64> {
    if mode == ExecutionMode::Live {
        let v = match live {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                return Err(match e {
                    PolyError::BalanceUnavailable(_) | PolyError::InsufficientFunding(_) => e,
                    other => PolyError::balance_unavailable(other.to_string()),
                });
            }
            None => {
                return Err(PolyError::balance_unavailable(
                    "live mode requires a verified on-chain collateral read; reader not configured",
                ));
            }
        };
        if !v.is_finite() || v < 0.0 {
            return Err(PolyError::balance_unavailable(format!(
                "live collateral read implausible: {v}"
            )));
        }
        return Ok(v);
    }
    if cached_state > 0.0 {
        return Ok(cached_state);
    }
    if let Some(Ok(v)) = live {
        if v.is_finite() && v >= 0.0 {
            return Ok(v);
        }
    }
    Ok(PAPER_USDC_BALANCE)
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

    // ------------------------------------------------------------------
    // resolve_sizing_balance: the paper/live separation invariant.
    // ------------------------------------------------------------------

    #[test]
    fn live_mode_uses_only_the_verified_read() {
        // A real read of 42.5 wins regardless of any cached value.
        let got = resolve_sizing_balance(ExecutionMode::Live, 999.0, Some(Ok(42.5))).unwrap();
        assert!((got - 42.5).abs() < f64::EPSILON);
        // Zero is a legitimate verified balance (risk gate will reject the
        // order) — it is NOT replaced by the paper figure.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Live, 1000.0, Some(Ok(0.0))).unwrap(),
            0.0
        );
    }

    #[test]
    fn live_mode_ignores_a_poisoned_cache_seed() {
        // Regression for the paper→live mode-switch defect: a 1000 USDC demo
        // seed left in shared state must never size a live order when the
        // real read says the wallet holds 3 USDC.
        let got = resolve_sizing_balance(ExecutionMode::Live, 1000.0, Some(Ok(3.0))).unwrap();
        assert!((got - 3.0).abs() < f64::EPSILON);
        // And with no live read at all, the seed must NOT leak through.
        let err = resolve_sizing_balance(ExecutionMode::Live, 1000.0, None)
            .expect_err("live without a reader must reject");
        assert!(matches!(err, PolyError::BalanceUnavailable(_)));
    }

    #[test]
    fn live_mode_rejects_failed_and_missing_reads() {
        // Read failed -> typed error, never the paper figure.
        let err = resolve_sizing_balance(
            ExecutionMode::Live,
            0.0,
            Some(Err(PolyError::http("rpc down"))),
        )
        .expect_err("failed read must reject");
        assert!(matches!(err, PolyError::BalanceUnavailable(_)));
        assert!(err.to_string().contains("rpc down"));
        // Already-typed balance errors pass through unchanged.
        let err = resolve_sizing_balance(
            ExecutionMode::Live,
            0.0,
            Some(Err(PolyError::balance_unavailable("decimals implausible"))),
        )
        .expect_err("must reject");
        assert!(err.to_string().contains("decimals implausible"));
        // No reader configured -> reject.
        assert!(matches!(
            resolve_sizing_balance(ExecutionMode::Live, 0.0, None),
            Err(PolyError::BalanceUnavailable(_))
        ));
    }

    #[test]
    fn live_mode_rejects_implausible_reads() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                resolve_sizing_balance(ExecutionMode::Live, 0.0, Some(Ok(bad))),
                Err(PolyError::BalanceUnavailable(_))
            ));
        }
    }

    #[test]
    fn paper_and_simulate_prefer_cache_then_live_then_paper_figure() {
        // Seeded demo balance wins (paper mode works offline with no funds).
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 1000.0, None).unwrap(),
            1000.0
        );
        // No seed + successful read -> real value.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, Some(Ok(7.25))).unwrap(),
            7.25
        );
        // No seed + failed/absent read -> paper figure (demo keeps working).
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, Some(Err(PolyError::http("x"))))
                .unwrap(),
            PAPER_USDC_BALANCE
        );
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, None).unwrap(),
            PAPER_USDC_BALANCE
        );
        // Simulate is a demo path (never broadcasts) and behaves like paper.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Simulate, 55.0, None).unwrap(),
            55.0
        );
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Simulate, 0.0, None).unwrap(),
            PAPER_USDC_BALANCE
        );
        // An implausible live value in paper mode falls through to the demo
        // figure rather than propagating NaN into sizing.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, Some(Ok(f64::NAN))).unwrap(),
            PAPER_USDC_BALANCE
        );
    }
}
