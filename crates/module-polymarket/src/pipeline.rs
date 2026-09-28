//! The staged order pipeline (TASK 4).
//!
//! [`PolyBot::process_signal`] runs one frozen [`OrderSignal`] through the
//! stages `RECEIVED → VALIDATED → MARKET_RESOLVED → QUOTED → EXPOSURE_CHECKED
//! → SIZED → RISK_APPROVED → COLLATERAL_VERIFIED → IDEMPOTENT →
//! OWNERSHIP_CLAIMED → SIGNED → SUBMITTED → RESTING / FILLED` (or the
//! terminal `REJECTED` / `FAILED` / `AMBIGUOUS`). Every exit is an explicit
//! [`SignalOutcome`] with a [`RejectReason`], journaled (`poly_signals`),
//! metered (`poly_stage_total`, `poly_signals_total`, `poly_rejections_total`,
//! `poly_pipeline_latency_ms`) and audited (`poly.signal.<stage>`).
//!
//! Invariants enforced here: exactly ONE risk decision per signal (the
//! shared [`bot_core::risk::RiskEngine`]), exactly ONE idempotency key (the
//! OMS `idempotency_key` = [`OrderSignal::intent_key`]), one ownership claim
//! (`poly:entry:<token>`), write-ahead journaling of the venue claim before
//! the POST, and paper / simulate / no-key runs never leaving the process.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use bot_core::config::PolymarketConfig;
use bot_core::events::AppEvent;
use bot_core::models::{BotModule, ExecutionMode, PolyMarket, Venue};
use bot_core::oms::{ExecutionRecord, OrderDraft, OrderStatus};
use bot_core::risk::EntryRequest;

use crate::audit::sanitize;
use crate::error::{PolyError, PolyResult};
use crate::metrics;
use crate::orders::{
    sign_order_bundle, FillSource, LocalOrderState, OrderParams, OrderSignal, PolyStage,
    RejectReason, SignalOutcome, SignedOrderBundle, TrackedOrder, VenueObservation,
    VenueOrderState,
};
use crate::store::PolySignalRecord;
use crate::strategy::{self, OrderDecision, Quote};
use crate::PolyBot;

/// Outcome of a live POST.
struct LiveSubmit {
    venue_order_id: String,
    signature: String,
    venue_state: VenueOrderState,
    raw_status: String,
}

impl PolyBot {
    /// Run one signal through the staged pipeline. Never panics; every exit
    /// is an explicit [`SignalOutcome`] that is journaled, metered and
    /// audited. Exactly one risk decision, one OMS record, one ownership
    /// claim per signal.
    pub async fn process_signal(
        &self,
        signal: &OrderSignal,
        market: &PolyMarket,
        quotes: &HashMap<String, Quote>,
        poly: &PolymarketConfig,
    ) -> SignalOutcome {
        let started = Instant::now();
        let mut ctx = PipelineCtx::new(signal);
        let outcome = self
            .run_pipeline(signal, market, quotes, poly, &mut ctx)
            .await;
        let outcome = ctx.finish(outcome, started);
        self.record_outcome(signal, &outcome).await;
        outcome
    }

    async fn run_pipeline(
        &self,
        signal: &OrderSignal,
        market: &PolyMarket,
        quotes: &HashMap<String, Quote>,
        poly: &PolymarketConfig,
        ctx: &mut PipelineCtx,
    ) -> Result<PolyStage, (RejectReason, String)> {
        let d = &signal.decision;
        ctx.stage(PolyStage::Received);

        // ---- VALIDATED --------------------------------------------------
        if d.token_id.trim().is_empty() || !d.token_id.trim().chars().all(|c| c.is_ascii_digit()) {
            return Err((
                RejectReason::InvalidToken,
                format!("token id '{}' is not a decimal uint256", d.token_id),
            ));
        }
        if d.condition_id.trim().is_empty() {
            return Err((RejectReason::InvalidToken, "empty condition id".into()));
        }
        if !d.limit_price.is_finite() || d.limit_price <= 0.0 || d.limit_price >= 1.0 {
            return Err((
                RejectReason::InvalidPrice,
                format!("limit price {} outside (0, 1)", d.limit_price),
            ));
        }
        if !d.size_tokens.is_finite()
            || d.size_tokens <= 0.0
            || !d.stake_usd.is_finite()
            || d.stake_usd <= 0.0
        {
            return Err((
                RejectReason::InvalidSize,
                format!(
                    "size {} / stake {} not positive",
                    d.size_tokens, d.stake_usd
                ),
            ));
        }
        ctx.stage(PolyStage::Validated);

        // ---- MARKET_RESOLVED -------------------------------------------
        let now = Utc::now();
        if let Err((skip, detail)) = strategy::market_gate(market, poly, now) {
            return Err((RejectReason::from_skip(skip), detail));
        }
        ctx.stage(PolyStage::MarketResolved);

        // ---- QUOTED ----------------------------------------------------
        let Some(quote) = quotes.get(&d.token_id).copied() else {
            return Err((RejectReason::NoQuote, "no quote for token".into()));
        };
        if let Err((skip, detail)) = strategy::quote_gate(&quote, poly, now) {
            return Err((RejectReason::from_skip(skip), detail));
        }
        if let Some(age) = quote.age_secs(now) {
            metrics::observe_quote_age((age as u64).saturating_mul(1_000));
        }
        let tick = signal
            .tick_size
            .parse::<f64>()
            .ok()
            .filter(|t| *t > 0.0)
            .unwrap_or(0.01);
        let (low, high) = if d.is_buy {
            (quote.best_bid, quote.best_ask + tick)
        } else {
            (quote.best_bid - tick, quote.best_ask)
        };
        if d.limit_price < low - 1e-9 || d.limit_price > high + 1e-9 {
            return Err((
                RejectReason::PriceOutsideBand,
                format!(
                    "limit {:.4} outside [{:.4}, {:.4}] (bid {:.4} ask {:.4})",
                    d.limit_price, low, high, quote.best_bid, quote.best_ask
                ),
            ));
        }
        ctx.stage(PolyStage::Quoted);

        // ---- EXPOSURE_CHECKED ------------------------------------------
        if self
            .state
            .find_open(BotModule::Polymarket, &d.token_id)
            .await
            .is_some()
        {
            return Err((
                RejectReason::AlreadyInMarket,
                "already holding this outcome token".into(),
            ));
        }
        // Per-symbol reconciliation gate (§H): refuse NEW entries while this
        // outcome token has unresolved claims. Exits (flatten/cancel) are
        // never gated — reducing exposure is always safe.
        if self.state.is_symbol_blocked(&d.token_id).await {
            metrics::count_symbol_gated();
            return Err((
                RejectReason::SymbolGated,
                "symbol gated by unresolved reconciliation".into(),
            ));
        }
        let (open_orders, resting_quote, market_orders_quote, open_markets) = {
            let tracked = self.tracked.read().await;
            let mut open = 0usize;
            let mut resting = 0.0;
            let mut in_market = 0.0;
            let mut markets: HashSet<String> = HashSet::new();
            for t in tracked.values().filter(|t| !t.state.is_terminal()) {
                open += 1;
                resting += t.resting_quote();
                markets.insert(t.condition_id.to_ascii_lowercase());
                if t.condition_id.eq_ignore_ascii_case(&d.condition_id) {
                    in_market += t.resting_quote();
                }
                if t.token_id == d.token_id {
                    return Err((
                        RejectReason::OrderAlreadyOpen,
                        format!("order {} already open for this token", t.venue_order_id),
                    ));
                }
            }
            (open, resting, in_market, markets)
        };
        let positions = self.state.open_positions_for(BotModule::Polymarket).await;
        let mut open_markets = open_markets;
        let mut market_positions_quote = 0.0;
        for p in &positions {
            if let Some(m) = &p.market_id {
                open_markets.insert(m.to_ascii_lowercase());
                if m.eq_ignore_ascii_case(&d.condition_id) {
                    market_positions_quote += p.cost_basis.max(p.notional());
                }
            }
        }
        if poly.max_open_markets > 0
            && open_markets.len() >= poly.max_open_markets
            && !open_markets.contains(&d.condition_id.to_ascii_lowercase())
        {
            return Err((
                RejectReason::MaxOpenMarkets,
                format!(
                    "{} markets already open (max {})",
                    open_markets.len(),
                    poly.max_open_markets
                ),
            ));
        }
        ctx.stage(PolyStage::ExposureChecked);

        // ---- SIZED -----------------------------------------------------
        let mut size_tokens = strategy::round_size(d.size_tokens);
        if size_tokens < poly.min_order_size || size_tokens <= 0.0 {
            return Err((
                RejectReason::SizeTooSmall,
                format!(
                    "size {size_tokens:.2} below min_order_size {:.2}",
                    poly.min_order_size
                ),
            ));
        }
        ctx.stage(PolyStage::Sized);

        // ---- RISK_APPROVED (the ONE risk decision) ---------------------
        let mode = signal.mode;
        let available = match self.available_collateral(mode).await {
            Ok(v) => v,
            Err(e) => {
                self.state
                    .record_error(BotModule::Polymarket, &format!("{}: {e}", d.outcome))
                    .await;
                return Err((RejectReason::from_error(&e), e.to_string()));
            }
        };
        let risk_cfg = self.state.config_snapshot().await.risk;
        if let Err((code, reason)) = self
            .risk
            .check_polymarket_coded(
                &risk_cfg,
                d.stake_usd,
                open_orders,
                resting_quote,
                market_orders_quote + market_positions_quote,
            )
            .await
        {
            self.publish_risk_rejected(d, &reason).await;
            return Err((RejectReason::from_risk_code(Some(code)), reason));
        }
        let liquidity = (quote.best_bid + quote.best_ask) * market.liquidity.max(1.0);
        let req = EntryRequest {
            module: BotModule::Polymarket,
            venue: Venue::PolymarketClob,
            symbol: d.token_id.clone(),
            symbol_display: format!("{} {}", market.question, d.outcome),
            requested_quote: d.stake_usd,
            available_quote: available,
            slippage_bps: 0,
            price: Some(d.limit_price),
            fair_value: None,
            liquidity: Some(liquidity),
            // TASK 5 — attribution for the global layer: the signer EOA and
            // the strategy that produced the frozen signal.
            wallet: self.ledger_wallet(),
            strategy: signal.strategy.to_string(),
        };
        let risk_decision = self.risk.check_entry(&req).await;
        if !risk_decision.allowed() {
            self.publish_risk_rejected(d, &risk_decision.reason).await;
            return Err((
                RejectReason::from_risk_code(risk_decision.code),
                risk_decision.reason.clone(),
            ));
        }
        self.state.inc_signals(BotModule::Polymarket).await;
        let approved_stake = risk_decision.sized_quote;
        if approved_stake + 1e-9 < d.stake_usd {
            // Rescale size to the risk-approved notional (same price).
            size_tokens = strategy::round_size(approved_stake / d.limit_price);
            if size_tokens < poly.min_order_size || size_tokens <= 0.0 {
                return Err((
                    RejectReason::SizeTooSmall,
                    format!(
                        "risk-resized {size_tokens:.2} tokens below min_order_size {:.2}",
                        poly.min_order_size
                    ),
                ));
            }
        }
        let stake = size_tokens * d.limit_price;
        ctx.approved_stake = Some(stake);
        ctx.size_tokens = Some(size_tokens);
        self.state.events.publish(AppEvent::Signal {
            ts: Utc::now(),
            module: BotModule::Polymarket,
            symbol: d.outcome.clone(),
            side: signal.side_str().into(),
            reason: d.reason.clone(),
            strength: (1.0 - d.limit_price).max(0.0),
        });
        self.state.events.publish(AppEvent::Polymarket {
            ts: Utc::now(),
            message: format!("{} @ {:.3} (${:.2})", d.reason, d.limit_price, stake),
            market: Some(market.question.clone()),
            price: Some(d.limit_price),
        });
        ctx.stage(PolyStage::RiskApproved);

        // ---- COLLATERAL_VERIFIED (live only) ---------------------------
        let will_send = mode == ExecutionMode::Live
            && self.signer.is_some()
            && self.api_key.read().await.is_some();
        if will_send {
            if let Err(e) = self
                .ensure_live_funding(d, stake, resting_quote, poly)
                .await
            {
                return Err((RejectReason::from_error(&e), e.to_string()));
            }
        }
        ctx.stage(PolyStage::CollateralVerified);

        // ---- IDEMPOTENT (the ONE duplicate gate: OMS idempotency key) ---
        let key = signal.intent_key();
        if !self.claim_inflight(&key) {
            return Err((
                RejectReason::DuplicateIntent,
                "intent already in flight in this process".into(),
            ));
        }
        let _guard = InflightGuard {
            bot: self,
            key: key.clone(),
        };
        if let Some(existing) = self.orders.get_by_key(&key).await {
            metrics::count_duplicate_prevented("poly_pipeline");
            return Err((
                RejectReason::DuplicateIntent,
                format!(
                    "intent already recorded as order {} ({})",
                    existing.id,
                    existing.status.as_str()
                ),
            ));
        }
        let draft = OrderDraft {
            idempotency_key: key.clone(),
            module: BotModule::Polymarket,
            side: signal.side_str().into(),
            symbol: d.token_id.clone(),
            venue: Venue::PolymarketClob.as_str().to_string(),
            mode,
            qty: size_tokens,
            price: Some(d.limit_price),
            meta: serde_json::json!({
                "signal_id": signal.signal_id,
                "condition_id": d.condition_id,
                "outcome": d.outcome,
                "strategy": signal.strategy,
                "order_type": signal.order_type,
                "expiration": signal.expiration,
                "neg_risk": d.neg_risk,
                "question": signal.question,
            }),
        };
        let order = match self.orders.create(draft).await {
            Ok(o) => o,
            Err(e) => return Err((RejectReason::OmsRejected, e.to_string())),
        };
        if order.idempotency_key != key {
            return Err((
                RejectReason::OmsRejected,
                "order manager returned a foreign order".into(),
            ));
        }
        ctx.order_id = Some(order.id.clone());
        ctx.stage(PolyStage::Idempotent);

        // ---- OWNERSHIP_CLAIMED -----------------------------------------
        let mut permit = match bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("poly:entry:{}", d.token_id),
            "poly_entry",
            "polymarket",
            "clob",
            &d.token_id,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                self.fail_oms(&order.id, &format!("ownership unavailable: {e}"))
                    .await;
                return Err((
                    RejectReason::OwnershipUnavailable,
                    format!("ownership unavailable: {e}"),
                ));
            }
        };
        if !permit.proceed() {
            self.fail_oms(&order.id, "owned by another replica").await;
            return Err((
                RejectReason::OwnedByOtherReplica,
                "poly entry owned by another replica".into(),
            ));
        }
        ctx.stage(PolyStage::OwnershipClaimed);

        // ---- SIGNED → SUBMITTED ----------------------------------------
        let submitted_at = Utc::now();
        if will_send {
            if let Err(e) = permit.fence().await {
                permit.finish(false).await;
                self.fail_oms(&order.id, &format!("fencing rejected: {e}"))
                    .await;
                return Err((
                    RejectReason::OwnershipUnavailable,
                    format!("fencing rejected: {e}"),
                ));
            }
            let (bundle, venue_order_id) = match self.sign_signal(signal, size_tokens, poly) {
                Ok(v) => v,
                Err(e) => {
                    permit.finish(false).await;
                    self.fail_oms(&order.id, &format!("signing failed: {e}"))
                        .await;
                    return Err((RejectReason::SigningFailed, e.to_string()));
                }
            };
            let _ = self
                .orders
                .attach_external(
                    &order.id,
                    Some(venue_order_id.clone()),
                    Some(bundle.signature.clone()),
                )
                .await;
            ctx.venue_order_id = Some(venue_order_id.clone());
            ctx.stage(PolyStage::Signed);
            self.oms_transition(&order.id, OrderStatus::Submitted, "posting to clob")
                .await;
            let mut tracked = new_tracked(
                signal,
                market,
                &order.id,
                &venue_order_id,
                size_tokens,
                Some(bundle.signature.clone()),
                submitted_at,
            );
            self.insert_tracked(tracked.clone()).await;
            // Write-ahead: the venue claim (derived order id, size, price)
            // is journaled BEFORE the POST so a crash during the request
            // leaves a `submitted` row that restart recovery holds as
            // ambiguous and reconciliation resolves against the venue.
            self.journal_order(&tracked).await;
            // Persistence layer: OrderSent records the venue claim and
            // enqueues venue reconciliation (external id = venue order id).
            self.state.events.publish(AppEvent::OrderSent {
                ts: submitted_at,
                module: BotModule::Polymarket,
                symbol: d.token_id.clone(),
                venue: Venue::PolymarketClob.as_str().to_string(),
                mode: mode.as_str().to_string(),
                quote_amount: stake,
                signature: Some(venue_order_id.clone()),
                signer: self.address.clone(),
                attempts: Some(1),
                latency_ms: None,
            });
            self.state.inc_orders_sent(BotModule::Polymarket).await;
            let post_started = Instant::now();
            match self
                .post_live(&bundle, &venue_order_id, &signal.order_type)
                .await
            {
                Ok(submit) => {
                    self.orders
                        .record_execution(ExecutionRecord {
                            order_id: order.id.clone(),
                            ts: Utc::now(),
                            kind: "send".into(),
                            endpoint: Some("clob".into()),
                            latency_ms: Some(post_started.elapsed().as_millis() as u64),
                            ok: true,
                            detail: Some(format!("status={}", submit.raw_status)),
                        })
                        .await;
                    ctx.stage(PolyStage::Submitted);
                    // The order is POSTed and may rest on the book: HAND OFF
                    // (§I/§M). The grace window comfortably covers the
                    // book-sync interval, so no replica can double-post the
                    // same token before `find_open` converges.
                    permit.finish(true).await;
                    if !submit.venue_order_id.eq_ignore_ascii_case(&venue_order_id) {
                        // Track under the venue's handle so polling, the user
                        // channel and reconciliation all agree on the key.
                        self.tracked.write().await.remove(&venue_order_id);
                        tracked.venue_order_id = submit.venue_order_id.clone();
                        let _ = self
                            .orders
                            .attach_external(
                                &order.id,
                                Some(submit.venue_order_id.clone()),
                                Some(bundle.signature.clone()),
                            )
                            .await;
                        ctx.venue_order_id = Some(submit.venue_order_id.clone());
                    }
                    let obs = VenueObservation {
                        state: submit.venue_state,
                        raw_status: submit.raw_status.clone(),
                        size_matched: None,
                        fill_delta: None,
                        trade_id: None,
                        price: None,
                        source: FillSource::Poll,
                        at: Utc::now(),
                        associate_trades: Vec::new(),
                    };
                    tracked.signature = Some(submit.signature);
                    let effect = self.apply_observation(&mut tracked, &obs).await;
                    match effect {
                        Ok(_) => {}
                        Err(e) => {
                            warn!(order = %venue_order_id, error = %e, "post-submit observation rejected")
                        }
                    }
                    if tracked.is_fill_and_kill()
                        && submit.venue_state == VenueOrderState::Matched
                        && !tracked.state.is_terminal()
                    {
                        // FAK: the POST answer says `matched` but not how
                        // much — ask the venue for the quantity right away.
                        // If it cannot answer, polling / the user channel /
                        // reconciliation resolve it; nothing is assumed.
                        self.confirm_fill_and_kill_quantity(&mut tracked).await;
                    }
                    ctx.position_id = tracked.position_id.clone();
                    Ok(match tracked.state {
                        LocalOrderState::Filled => PolyStage::Filled,
                        _ => PolyStage::Resting,
                    })
                }
                Err(e) if e.is_ambiguous() => {
                    // SubmitUnknown = the signed order may be resting →
                    // reconciliation owns the outcome.
                    permit.finish(true).await;
                    self.orders
                        .record_execution(ExecutionRecord {
                            order_id: order.id.clone(),
                            ts: Utc::now(),
                            kind: "send".into(),
                            endpoint: Some("clob".into()),
                            latency_ms: Some(post_started.elapsed().as_millis() as u64),
                            ok: false,
                            detail: Some(format!("ambiguous: {e}")),
                        })
                        .await;
                    self.oms_transition(&order.id, OrderStatus::Unknown, &e.to_string())
                        .await;
                    let _ =
                        tracked.force_state(LocalOrderState::Unknown, "submit_unknown", Utc::now());
                    self.insert_tracked(tracked.clone()).await;
                    self.journal_order(&tracked).await;
                    warn!(
                        order_id = %venue_order_id,
                        error = %e,
                        "polymarket submit outcome unknown; held for reconciliation"
                    );
                    Err((RejectReason::SubmitUnknown, e.to_string()))
                }
                Err(e) => {
                    // Definite venue rejection: release for a fresh decision.
                    permit.finish(false).await;
                    self.orders
                        .record_execution(ExecutionRecord {
                            order_id: order.id.clone(),
                            ts: Utc::now(),
                            kind: "send".into(),
                            endpoint: Some("clob".into()),
                            latency_ms: Some(post_started.elapsed().as_millis() as u64),
                            ok: false,
                            detail: Some(e.to_string()),
                        })
                        .await;
                    let _ = tracked.force_state(LocalOrderState::Failed, "rejected", Utc::now());
                    self.insert_tracked(tracked.clone()).await;
                    self.journal_order(&tracked).await;
                    self.fail_oms(&order.id, &e.to_string()).await;
                    self.state.inc_orders_failed(BotModule::Polymarket).await;
                    self.state.note_failed_entry(&d.token_id).await;
                    Err((RejectReason::from_error(&e), e.to_string()))
                }
            }
        } else {
            // Paper / simulate / no-key: nothing leaves the process. The
            // paper book fills the order immediately at the limit price.
            permit.finish(false).await;
            let venue_order_id = signal.paper_order_id();
            let _ = self
                .orders
                .attach_external(&order.id, Some(venue_order_id.clone()), None)
                .await;
            ctx.venue_order_id = Some(venue_order_id.clone());
            ctx.stage(PolyStage::Signed);
            self.oms_transition(&order.id, OrderStatus::Submitted, "paper book")
                .await;
            self.state.inc_orders_sent(BotModule::Polymarket).await;
            ctx.stage(PolyStage::Submitted);
            let mut tracked = new_tracked(
                signal,
                market,
                &order.id,
                &venue_order_id,
                size_tokens,
                None,
                submitted_at,
            );
            self.insert_tracked(tracked.clone()).await;
            let obs = VenueObservation {
                state: VenueOrderState::Matched,
                raw_status: "paper".into(),
                size_matched: Some(size_tokens),
                fill_delta: None,
                trade_id: None,
                price: Some(d.limit_price),
                source: FillSource::Paper,
                at: Utc::now(),
                associate_trades: Vec::new(),
            };
            if let Err(e) = self.apply_observation(&mut tracked, &obs).await {
                return Err((RejectReason::Internal, e.to_string()));
            }
            ctx.position_id = tracked.position_id.clone();
            Ok(PolyStage::Filled)
        }
    }

    /// Sign the order for `signal` at the final size. Returns the bundle and
    /// the venue order id (EIP-712 struct hash) derivable before the POST.
    fn sign_signal(
        &self,
        signal: &OrderSignal,
        size_tokens: f64,
        poly: &PolymarketConfig,
    ) -> PolyResult<(SignedOrderBundle, String)> {
        let key = self
            .signer
            .as_ref()
            .ok_or_else(|| PolyError::not_configured("no signer for live order"))?;
        let d = &signal.decision;
        let params = OrderParams {
            token_id: &d.token_id,
            is_buy: d.is_buy,
            size: size_tokens,
            price: d.limit_price,
            tick_size: &signal.tick_size,
            neg_risk: d.neg_risk,
            chain_id: poly.chain_id,
            domain_version: &poly.exchange_domain_version,
            exchange_address: &poly.exchange_address,
            neg_risk_exchange_address: &poly.neg_risk_exchange_address,
            signature_type: poly.signature_type,
            funder: poly.funder_address.as_deref(),
            expiration_timestamp: signal.expiration,
            builder_code: poly.builder_code.as_deref(),
        };
        let bundle = sign_order_bundle(key, &params)?;
        let venue_order_id = bundle.derived_order_id()?;
        Ok((bundle, venue_order_id))
    }

    /// POST a signed bundle. Transport failures are surfaced as
    /// [`PolyError::SubmitUnknown`] carrying the derived order id.
    /// `order_type` is the FROZEN signal's (upper-cased) type — never
    /// re-read from config, so a config edit between freeze and post cannot
    /// change what the venue receives.
    async fn post_live(
        &self,
        bundle: &SignedOrderBundle,
        venue_order_id: &str,
        order_type: &str,
    ) -> PolyResult<LiveSubmit> {
        let client = self.authed_client().await?;
        let resp = match client.post_order(bundle, order_type).await {
            Ok(resp) => resp,
            Err(e @ PolyError::Http(_)) => {
                return Err(PolyError::SubmitUnknown {
                    order_id: venue_order_id.to_string(),
                    reason: e.to_string(),
                });
            }
            Err(e) => return Err(e),
        };
        if !resp.success.unwrap_or(false) {
            return Err(PolyError::clob(
                resp.error_msg
                    .clone()
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| "order rejected".into()),
            ));
        }
        if let Some(id) = resp.order_id.as_deref() {
            if !id.trim().is_empty() && !id.eq_ignore_ascii_case(venue_order_id) {
                // The venue's id must equal the struct hash we derived; a
                // mismatch means our encoding drifted from the exchange's —
                // keep the venue's id as the handle and say so loudly.
                warn!(
                    derived = %venue_order_id,
                    venue = %id,
                    "venue order id differs from the locally derived struct hash"
                );
            }
        }
        let raw_status = resp.status.clone().unwrap_or_else(|| "live".into());
        Ok(LiveSubmit {
            venue_order_id: resp
                .order_id
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| venue_order_id.to_string()),
            signature: bundle.signature.clone(),
            venue_state: VenueOrderState::parse(&raw_status),
            raw_status,
        })
    }

    async fn record_outcome(&self, signal: &OrderSignal, outcome: &SignalOutcome) {
        let d = &signal.decision;
        metrics::count_signal_outcome(signal.strategy, outcome.stage.as_str());
        if let Some(reason) = outcome.reject_reason {
            metrics::count_rejection(reason.as_str());
        }
        metrics::observe_pipeline_latency(outcome.stage.as_str(), outcome.total_ms);
        let rec = PolySignalRecord {
            signal_id: signal.signal_id.clone(),
            condition_id: d.condition_id.clone(),
            token_id: d.token_id.clone(),
            outcome: d.outcome.clone(),
            side: signal.side_str().into(),
            strategy: signal.strategy.into(),
            limit_price: d.limit_price,
            size_tokens: outcome.size_tokens.unwrap_or(d.size_tokens),
            stake_usd: outcome.approved_stake.unwrap_or(d.stake_usd),
            mode: signal.mode.as_str().to_string(),
            stage: outcome.stage.as_str().into(),
            reject_reason: outcome.reject_reason.map(|r| r.as_str().to_string()),
            detail: outcome.detail.clone(),
            order_id: outcome.order_id.clone(),
            venue_order_id: outcome.venue_order_id.clone(),
            position_id: outcome.position_id.clone(),
            created_at: signal.created_at,
            updated_at: Utc::now(),
        };
        if !self.store.record_signal(rec).await {
            metrics::count_journal_error("record_signal");
        }
        let action = format!(
            "poly.signal.{}",
            outcome.stage.as_str().to_ascii_lowercase()
        );
        let text = match outcome.reject_reason {
            Some(r) => format!(
                "{} reason={} token={} price={:.4} stake={:.2} detail={}",
                outcome.stage.as_str(),
                r.as_str(),
                d.token_id,
                d.limit_price,
                d.stake_usd,
                sanitize(&outcome.detail)
            ),
            None => format!(
                "{} token={} price={:.4} size={:.2} stake={:.2} order={} venue={} position={} ms={}",
                outcome.stage.as_str(),
                d.token_id,
                d.limit_price,
                outcome.size_tokens.unwrap_or(0.0),
                outcome.approved_stake.unwrap_or(0.0),
                outcome.order_id.as_deref().unwrap_or("-"),
                outcome.venue_order_id.as_deref().unwrap_or("-"),
                outcome.position_id.as_deref().unwrap_or("-"),
                outcome.total_ms
            ),
        };
        self.audit(
            &action,
            &format!("{}:{}", d.token_id, signal.signal_id),
            &text,
        );
        match outcome.stage {
            PolyStage::Rejected => debug!(
                token = %d.token_id,
                reason = ?outcome.reject_reason,
                detail = %outcome.detail,
                "polymarket signal rejected"
            ),
            PolyStage::Failed => {
                warn!(
                    token = %d.token_id,
                    reason = ?outcome.reject_reason,
                    detail = %outcome.detail,
                    "polymarket signal failed"
                );
                self.state
                    .record_error(
                        BotModule::Polymarket,
                        &format!("{}: {}", d.outcome, outcome.detail),
                    )
                    .await;
            }
            _ => info!(
                token = %d.token_id,
                stage = %outcome.stage,
                order = outcome.order_id.as_deref().unwrap_or("-"),
                venue = outcome.venue_order_id.as_deref().unwrap_or("-"),
                "polymarket signal processed"
            ),
        }
    }

    async fn publish_risk_rejected(&self, d: &OrderDecision, reason: &str) {
        self.state.inc_risk_rejected(BotModule::Polymarket).await;
        self.state.events.publish(AppEvent::RiskRejected {
            ts: Utc::now(),
            module: BotModule::Polymarket,
            symbol: d.outcome.clone(),
            reason: reason.to_string(),
        });
        debug!(token = %d.token_id, reason = %reason, "polymarket entry rejected");
    }

    /// Claim an intent key for this process (`false` = already in flight).
    fn claim_inflight(&self, key: &str) -> bool {
        let mut set = self
            .inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set.insert(key.to_string())
    }

    fn release_inflight(&self, key: &str) {
        let mut set = self
            .inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set.remove(key);
    }
}

/// Pipeline bookkeeping for one signal.
struct PipelineCtx {
    signal_id: String,
    last_stage: PolyStage,
    order_id: Option<String>,
    venue_order_id: Option<String>,
    position_id: Option<String>,
    approved_stake: Option<f64>,
    size_tokens: Option<f64>,
}

impl PipelineCtx {
    fn new(signal: &OrderSignal) -> Self {
        PipelineCtx {
            signal_id: signal.signal_id.clone(),
            last_stage: PolyStage::Received,
            order_id: None,
            venue_order_id: None,
            position_id: None,
            approved_stake: None,
            size_tokens: None,
        }
    }

    fn stage(&mut self, stage: PolyStage) {
        self.last_stage = stage;
        metrics::count_stage(stage.as_str());
    }

    fn finish(
        self,
        result: Result<PolyStage, (RejectReason, String)>,
        started: Instant,
    ) -> SignalOutcome {
        let (stage, reject_reason, detail) = match result {
            Ok(stage) => (stage, None, String::new()),
            Err((reason, detail)) => {
                // Anything refused before the ownership claim is a clean
                // rejection; after it, a definite error is a failure and an
                // ambiguous submit is held.
                let stage = if reason == RejectReason::SubmitUnknown {
                    PolyStage::Ambiguous
                } else if self.last_stage >= PolyStage::OwnershipClaimed {
                    PolyStage::Failed
                } else {
                    PolyStage::Rejected
                };
                (stage, Some(reason), detail)
            }
        };
        metrics::count_stage(stage.as_str());
        SignalOutcome {
            signal_id: self.signal_id,
            stage,
            reject_reason,
            detail,
            order_id: self.order_id,
            venue_order_id: self.venue_order_id,
            position_id: self.position_id,
            approved_stake: self.approved_stake,
            size_tokens: self.size_tokens,
            total_ms: started.elapsed().as_millis() as u64,
        }
    }
}

/// Releases the in-flight intent key when the pipeline returns (any path).
struct InflightGuard<'a> {
    bot: &'a PolyBot,
    key: String,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.bot.release_inflight(&self.key);
    }
}

/// Build a tracker entry for a freshly submitted order.
fn new_tracked(
    signal: &OrderSignal,
    market: &PolyMarket,
    order_id: &str,
    venue_order_id: &str,
    size_tokens: f64,
    signature: Option<String>,
    submitted_at: DateTime<Utc>,
) -> TrackedOrder {
    let d = &signal.decision;
    TrackedOrder {
        venue_order_id: venue_order_id.to_string(),
        order_id: order_id.to_string(),
        signal_id: signal.signal_id.clone(),
        condition_id: d.condition_id.clone(),
        token_id: d.token_id.clone(),
        outcome: d.outcome.clone(),
        question: market.question.clone(),
        is_buy: d.is_buy,
        neg_risk: d.neg_risk,
        order_type: signal.order_type.clone(),
        limit_price: d.limit_price,
        size_tokens,
        size_matched: 0.0,
        mode: signal.mode,
        state: LocalOrderState::Submitted,
        venue_status: String::new(),
        expiration: signal.expiration,
        position_id: None,
        signature,
        submitted_at,
        updated_at: submitted_at,
        booked_trade_ids: Vec::new(),
        trade_matched: 0.0,
    }
}
