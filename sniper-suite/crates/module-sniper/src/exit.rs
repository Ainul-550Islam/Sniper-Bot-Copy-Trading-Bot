//! Exit path for Module 1: mark open positions to market and close them when a
//! risk rule fires (TASK 2 §L exit hardening).
//!
//! The sweeper is deliberately independent of launch detection — if the feeds
//! drop, positions still have to be managed. Every pass it:
//!
//! 1. **cleans up failed entries** — a position whose entry intent the
//!    execution ledger settled as failed/expired never held tokens; it is
//!    closed as `Failed` without a sell (`sniper.failed_entry_cleanup`);
//! 2. **holds ambiguous entries** — while the entry's lifecycle record is
//!    still `submitted`/`pending`, reconciliation owns the outcome and no
//!    sell is attempted (the kill switch overrides this);
//! 3. **marks to market** on the position's own venue (curve → PumpSwap →
//!    Raydium → Jupiter, driven by `position.venue` / `position.market_id`);
//! 4. asks [`RiskEngine::check_exit`] what to do, adds the **stale-position**
//!    rule (`sniper.stale_position_exit_secs`: no successful mark for that
//!    long → flatten) and honours a per-position **retry backoff** after a
//!    failed sell (`sniper.exit_retry_backoff_secs`);
//! 5. sells through the same hardened execution engine as the entry, with a
//!    deterministic exit intent id per decision, the write-ahead journal and
//!    the distributed ownership permit.
//!
//! Polling rather than a per-position `subscribeTokenTrade` keeps the sweeper
//! robust: it works on any RPC, needs no extra subscriptions, and degrades
//! gracefully. `sniper.monitor_positions` is honoured as "sweep frequently".
//!
//! [`RiskEngine::check_exit`]: bot_core::risk::RiskEngine::check_exit

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::execution::ExecutionState;
use bot_core::maths;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::risk::{ExitDecision, ExitRule};

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::ExecStatus;
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::pump::{self, BondingCurveState, BuildOptions, PumpContext};
use solana_kit::pumpswap::{self, PumpSwapContext};
use solana_kit::raydium::RaydiumPool;
use solana_kit::tx::TxRequest;

use crate::pipeline::EntryRoute;
use crate::Sniper;

/// How often the sweeper runs when `monitor_positions` is on.
const SWEEP_FAST: Duration = Duration::from_secs(2);
/// How often when it is off (still manage risk, just less eagerly).
const SWEEP_SLOW: Duration = Duration::from_secs(10);
/// Reference size used to mark a graduated token via Jupiter (1 whole token).
const MARK_REFERENCE_RAW: u64 = 1_000_000;

/// Per-position bookkeeping the sweeper keeps between passes. Process-local
/// on purpose: it only shapes *when* the next attempt happens; the decisions
/// themselves come from durable state (positions, ledger, risk).
#[derive(Debug, Default)]
pub struct ExitTracker {
    /// position id → when marking first started failing (cleared on success).
    mark_failed_since: HashMap<String, DateTime<Utc>>,
    /// position id → when the last sell attempt failed (retry backoff).
    last_exit_failure: HashMap<String, DateTime<Utc>>,
}

impl ExitTracker {
    /// Note a successful mark: the position is priceable again.
    pub fn mark_ok(&mut self, position_id: &str) {
        self.mark_failed_since.remove(position_id);
    }

    /// Note a failed mark at `now`; returns how long marking has been
    /// failing for this position (seconds).
    pub fn mark_failed(&mut self, position_id: &str, now: DateTime<Utc>) -> i64 {
        let since = *self
            .mark_failed_since
            .entry(position_id.to_string())
            .or_insert(now);
        now.signed_duration_since(since).num_seconds().max(0)
    }

    /// Seconds the position has been unpriceable (0 when it is fine).
    pub fn unpriceable_secs(&self, position_id: &str, now: DateTime<Utc>) -> i64 {
        self.mark_failed_since
            .get(position_id)
            .map(|s| now.signed_duration_since(*s).num_seconds().max(0))
            .unwrap_or(0)
    }

    /// Note a failed sell at `now`.
    pub fn exit_failed(&mut self, position_id: &str, now: DateTime<Utc>) {
        self.last_exit_failure.insert(position_id.to_string(), now);
    }

    /// Note a sell that went through (any determinate or ambiguous outcome
    /// that left the process).
    pub fn exit_done(&mut self, position_id: &str) {
        self.last_exit_failure.remove(position_id);
        self.mark_failed_since.remove(position_id);
    }

    /// True while the retry backoff after a failed sell is still running.
    pub fn in_backoff(&self, position_id: &str, now: DateTime<Utc>, backoff_secs: u64) -> bool {
        if backoff_secs == 0 {
            return false;
        }
        match self.last_exit_failure.get(position_id) {
            Some(at) => now.signed_duration_since(*at).num_seconds() < backoff_secs as i64,
            None => false,
        }
    }

    /// Drop bookkeeping for positions that no longer exist.
    pub fn retain(&mut self, live: &[String]) {
        self.mark_failed_since.retain(|id, _| live.contains(id));
        self.last_exit_failure.retain(|id, _| live.contains(id));
    }
}

/// What the ledger says about a position's entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryFate {
    /// Landed (confirmed / reconciled) or no ledger record to consult.
    Held,
    /// Still ambiguous: submitted / pending / not yet settled.
    Unknown,
    /// Provably never landed: failed / expired.
    Failed,
}

/// Classify an entry's ledger state for the sweeper. Pure.
pub fn entry_fate(state: Option<ExecutionState>) -> EntryFate {
    match state {
        None => EntryFate::Held,
        Some(ExecutionState::Confirmed) | Some(ExecutionState::Reconciled) => EntryFate::Held,
        Some(ExecutionState::Failed) | Some(ExecutionState::Expired) => EntryFate::Failed,
        Some(_) => EntryFate::Unknown,
    }
}

fn count_action(action: &str) {
    bot_core::obs::metrics::global()
        .counter(
            "sniper_exit_actions_total",
            "Exit sweeper actions (sold, failed_entry_cleanup, stale_exit, held_ambiguous, backoff_skip, mark_failed, sell_failed).",
            &[("action", action)],
        )
        .inc();
}

impl Sniper {
    /// The exit sweeper loop. Runs until the task is aborted.
    pub async fn exit_sweeper(&mut self) {
        info!("sniper exit sweeper started");
        loop {
            let cfg = self.state.config_snapshot().await;
            let interval = if cfg.sniper.monitor_positions {
                SWEEP_FAST
            } else {
                SWEEP_SLOW
            };

            if let Err(e) = self.sweep_once().await {
                warn!(error = %e, "exit sweep failed");
                self.state
                    .record_error(BotModule::Sniper, &format!("exit sweep: {e}"))
                    .await;
            }
            self.state.heartbeat(BotModule::Sniper).await;
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = self.state.wait_shutdown() => {
                    info!("sniper exit sweeper stopping (shutdown)");
                    break;
                }
            }
        }
    }

    /// One pass over every open sniper position.
    pub async fn sweep_once(&mut self) -> BotResult<()> {
        let positions = self.state.open_positions_for(BotModule::Sniper).await;
        let live: Vec<String> = positions.iter().map(|p| p.id.clone()).collect();
        self.exits.retain(&live);
        if positions.is_empty() {
            return Ok(());
        }
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let kill = self.state.kill_switch();

        for mut position in positions {
            let now = Utc::now();

            // 1./2. What does the execution ledger say about the entry?
            let fate = match &position.entry_signature {
                Some(sig) if !sig.is_empty() => {
                    let rec = self.executor.ledger().get_by_signature(sig).await;
                    entry_fate(rec.map(|r| r.state))
                }
                _ => EntryFate::Held,
            };
            match fate {
                EntryFate::Failed if sniper.failed_entry_cleanup => {
                    self.cleanup_failed_entry(&position).await;
                    continue;
                }
                EntryFate::Unknown if !kill => {
                    count_action("held_ambiguous");
                    debug!(
                        position = %position.id,
                        symbol = %position.symbol,
                        "entry outcome still ambiguous — holding until reconciliation settles it"
                    );
                    continue;
                }
                _ => {}
            }

            // Retry backoff after a failed sell (the kill switch ignores it).
            if !kill
                && self
                    .exits
                    .in_backoff(&position.id, now, sniper.exit_retry_backoff_secs)
            {
                count_action("backoff_skip");
                continue;
            }

            // 3. Mark to market (skip the fetch if the kill switch is
            //    flattening everything — we are selling regardless of price).
            if !kill {
                match self.mark_position(&position).await {
                    Ok(mark) if mark.is_finite() && mark > 0.0 => {
                        self.exits.mark_ok(&position.id);
                        position.last_mark = mark;
                        position.trailing_high_water =
                            Some(position.trailing_high_water.map_or(mark, |h| h.max(mark)));
                        position.updated_at = Utc::now();
                        self.state.upsert_position(position.clone()).await;
                        self.state
                            .set_unrealized(BotModule::Sniper, position.unrealised())
                            .await;
                    }
                    Ok(_) => {
                        count_action("mark_failed");
                        self.exits.mark_failed(&position.id, now);
                        debug!(symbol = %position.symbol, "non-positive mark, holding");
                    }
                    Err(e) => {
                        count_action("mark_failed");
                        let secs = self.exits.mark_failed(&position.id, now);
                        debug!(symbol = %position.symbol, error = %e, unpriceable_secs = secs, "could not mark position");
                    }
                }
            }

            // 4. Decide: risk rules first, then the stale-position rule.
            let mut decision = self.risk.check_exit(&position, position.last_mark).await;
            if !decision.should_exit && sniper.stale_position_exit_secs > 0 {
                let unpriceable = self.exits.unpriceable_secs(&position.id, now);
                if unpriceable >= sniper.stale_position_exit_secs {
                    decision = ExitDecision::forced(
                        ExitRule::StalePosition,
                        format!(
                            "no mark price for {unpriceable}s (max {}s)",
                            sniper.stale_position_exit_secs
                        ),
                    );
                    count_action("stale_exit");
                }
            }
            if !decision.should_exit {
                continue;
            }

            // Honour the operator's partial take-profit setting.
            let fraction = if matches!(decision.rule, Some(ExitRule::TakeProfit)) {
                sniper.take_profit_sell_fraction.clamp(0.0, 1.0)
            } else {
                decision.fraction
            };

            info!(
                symbol = %position.symbol,
                rule = ?decision.rule,
                fraction,
                mark = position.last_mark,
                reason = %decision.reason,
                "exit rule fired"
            );

            // Distributed ownership per SELL DECISION (§F/§P):
            // `exit:{position.id}:{rule}` — exactly one replica executes a
            // given exit. A replica that loses the claim skips (the owner is
            // selling); an unavailable store fails closed for this round —
            // the sweeper retries on the next tick, exits are never
            // permanently blocked.
            let rule_str = decision.rule.map(|r| r.as_str()).unwrap_or("manual");
            let mut permit = match bot_core::ownership::Permit::acquire(
                self.ownership.as_deref(),
                format!("exit:{}:{}", position.id, rule_str),
                "exit",
                "sniper",
                rule_str,
                &position.symbol,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        position = %position.id,
                        rule = rule_str,
                        error = %e,
                        "exit ownership unavailable — failing closed this round (retry next sweep)"
                    );
                    continue;
                }
            };
            if !permit.proceed() {
                debug!(position = %position.id, rule = rule_str, "exit owned by another replica — skipping");
                continue;
            }

            match self
                .sell_position(
                    &position,
                    fraction,
                    &decision.reason,
                    decision.rule,
                    &mut permit,
                )
                .await
            {
                Ok(()) => {
                    count_action("sold");
                    self.exits.exit_done(&position.id);
                }
                Err(e) => {
                    // The sell path releases the claim on determinate
                    // outcomes internally; this covers pre-broadcast
                    // failures (no money moved → release so the next sweep
                    // can re-decide after the backoff).
                    permit.finish(false).await;
                    count_action("sell_failed");
                    self.exits.exit_failed(&position.id, Utc::now());
                    warn!(symbol = %position.symbol, error = %e, "exit sell failed");
                    self.state
                        .record_error(BotModule::Sniper, &format!("exit {}: {e}", position.symbol))
                        .await;
                }
            }
        }
        Ok(())
    }

    /// Close a position whose entry provably never landed. No sell: there
    /// are no tokens. The cost basis is zeroed so no PnL is booked for a
    /// trade that did not happen.
    async fn cleanup_failed_entry(&mut self, position: &Position) {
        count_action("failed_entry_cleanup");
        self.state
            .with_position(&position.id, |p| {
                p.qty = 0.0;
                p.cost_basis = 0.0;
            })
            .await;
        self.state
            .close_position(
                &position.id,
                PositionStatus::Failed,
                "entry never landed (execution ledger: failed/expired)",
            )
            .await;
        self.state.events.publish(AppEvent::Audit {
            ts: Utc::now(),
            actor: "sniper".into(),
            action: "sniper.exit.failed_entry_cleanup".into(),
            target: Some(position.symbol.clone()),
            outcome: format!(
                "position {} closed as failed; entry signature {}",
                position.id,
                position.entry_signature.as_deref().unwrap_or("-")
            ),
        });
        info!(
            position = %position.id,
            symbol = %position.symbol,
            "failed entry cleaned up (no tokens were ever received)"
        );
    }

    /// Mark a position on its own venue first, then fall back through the
    /// other venues. `position.market_id` carries the pool the entry used.
    pub async fn mark_position(&self, position: &Position) -> BotResult<f64> {
        let mint = Pubkey::try_from(position.symbol.as_str())
            .map_err(|e| BotError::invalid(format!("position mint {}: {e}", position.symbol)))?;
        let pool = position
            .market_id
            .as_deref()
            .and_then(|p| Pubkey::try_from(p).ok());
        match position.venue {
            Venue::PumpSwap => {
                if let Ok(ctx) =
                    PumpSwapContext::load(&self.rpc, &mint, &self.wallet.pubkey, pool).await
                {
                    let price = ctx.price();
                    if price.is_finite() && price > 0.0 {
                        return Ok(price);
                    }
                }
            }
            Venue::RaydiumAmmV4 => {
                if let Some(amm) = pool {
                    if let Ok(p) = RaydiumPool::load(&self.rpc, &amm, true).await {
                        let price = raydium_price_in_sol(&p, &mint);
                        if price.is_finite() && price > 0.0 {
                            return Ok(price);
                        }
                    }
                }
            }
            _ => {}
        }
        self.mark_price_sol(&position.symbol).await
    }

    /// Best available mark price in SOL per token.
    ///
    /// Prefers the bonding-curve spot price (a pure read, no aggregation); falls
    /// back to a Jupiter quote once the token has graduated.
    pub async fn mark_price_sol(&self, mint_str: &str) -> BotResult<f64> {
        let mint = Pubkey::try_from(mint_str)
            .map_err(|e| BotError::invalid(format!("position mint {mint_str}: {e}")))?;

        // Try the bonding curve first.
        let curve_addr = pump::bonding_curve_pda(&mint);
        if let Ok(Some(data)) = self.rpc.get_account_processed(&curve_addr).await {
            if let Ok(curve) = BondingCurveState::parse(&data) {
                if !curve.complete {
                    return Ok(maths::pump_spot_price_sol(
                        curve.virtual_sol_reserves,
                        curve.virtual_token_reserves,
                    ));
                }
            }
        }

        // Graduated (or curve unreadable): mark via a Jupiter sell quote of a
        // reference size, so the price reflects real book depth.
        let jupiter = Jupiter::new();
        let quote = jupiter
            .quote(&QuoteRequest::new(mint, *WSOL_MINT, MARK_REFERENCE_RAW))
            .await?;
        let out_sol = maths::lamports_to_sol(quote.out_amount_u64()?);
        // Reference is 1 whole token (6dp), so out_sol is already SOL-per-token.
        Ok(out_sol)
    }

    /// Sell `fraction` of a position, book the PnL, and close or trim it.
    pub async fn sell_position(
        &mut self,
        position: &Position,
        fraction: f64,
        reason: &str,
        rule: Option<ExitRule>,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<()> {
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction <= 0.0 || position.qty <= 0.0 {
            permit.finish(false).await;
            return Ok(());
        }
        let cfg = self.state.config_snapshot().await;
        let base_decimals = 6u8;
        let sell_qty_human = position.qty * fraction;
        let sell_raw = maths::to_raw_amount(sell_qty_human, base_decimals);
        if sell_raw == 0 {
            permit.finish(false).await;
            return Ok(());
        }

        let mint = Pubkey::try_from(position.symbol.as_str())
            .map_err(|e| BotError::invalid(format!("position mint {}: {e}", position.symbol)))?;
        let slippage_pct = cfg.sniper.slippage_pct;

        // Resolve the exit route from the position's venue and the live
        // venue state; every path below is one hardened-executor call.
        let route = self.exit_route(position, &mint, &cfg.sniper).await?;

        // Fencing (§E): re-validate ownership immediately before the
        // money-moving branch — a taken-over replica must not sell.
        permit.fence().await?;

        let (quote_sol, signature, venue, status) = match route {
            ExitVenue::Curve(ctx) => {
                let intent_id =
                    exit_intent_id("sniper", position, sell_raw, EntryRoute::PumpCurve.as_str());
                self.sell_on_curve(&ctx, sell_raw, slippage_pct, &cfg, &intent_id)
                    .await?
            }
            ExitVenue::PumpSwap(ctx) => {
                let pct = cfg.sniper.pumpswap_slippage_pct.unwrap_or(slippage_pct);
                let intent_id = exit_intent_id(
                    "sniper",
                    position,
                    sell_raw,
                    EntryRoute::PumpSwapDirect.as_str(),
                );
                self.sell_on_pumpswap(&ctx, sell_raw, pct, &cfg, &intent_id)
                    .await?
            }
            ExitVenue::Raydium(pool) => {
                let pct = cfg.sniper.raydium_slippage_pct.unwrap_or(slippage_pct);
                let intent_id = exit_intent_id(
                    "sniper",
                    position,
                    sell_raw,
                    EntryRoute::RaydiumV4Direct.as_str(),
                );
                self.sell_on_raydium(&pool, &mint, sell_raw, pct, &cfg, &intent_id)
                    .await?
            }
            ExitVenue::Jupiter => {
                let intent_id =
                    exit_intent_id("sniper", position, sell_raw, EntryRoute::Jupiter.as_str());
                self.sell_via_jupiter(mint, sell_raw, &cfg, &intent_id)
                    .await?
            }
        };

        // Ownership terminal (§I/§M): unproven outcomes (Sent/SendUnknown)
        // hand off to reconciliation — the grace window stops any replica
        // from re-selling while the first sell may still land; determinate
        // outcomes (including unfilled) release.
        permit.finish(status.is_ambiguous()).await;

        let filled = matches!(
            status,
            ExecStatus::Confirmed
                | ExecStatus::Sent
                | ExecStatus::SendUnknown
                | ExecStatus::PaperFilled
        );
        if !filled {
            self.state.inc_orders_failed(BotModule::Sniper).await;
            return Err(BotError::solana(format!(
                "exit order did not fill ({status:?})"
            )));
        }
        self.state.inc_orders_sent(BotModule::Sniper).await;

        let price = if sell_qty_human > 0.0 {
            quote_sol / sell_qty_human
        } else {
            position.last_mark
        };

        // ---- Trade record -------------------------------------------------
        let trade = Trade {
            id: self.state.next_id("t"),
            ts: Utc::now(),
            source: TradeSource::Sniper,
            venue,
            mode: self.state.execution_mode().await,
            side: PositionSide::Short, // a sell
            symbol: position.symbol.clone(),
            symbol_display: position.symbol_display.clone(),
            amount_in: sell_qty_human,
            amount_out: quote_sol,
            quote_symbol: "SOL".into(),
            price,
            fee: 0.0,
            slippage_bps: (slippage_pct * 100.0).round() as u64,
            signature: signature.clone(),
            position_id: Some(position.id.clone()),
            note: Some(format!(
                "exit {} {}",
                rule.map(|r| r.as_str()).unwrap_or("manual"),
                reason
            )),
            latency_ms: None,
        };
        // TASK 5 — the typed accounting event for this exit fill (same
        // figures as the trade record; the signature is the reference for
        // live sells, a paper reference otherwise).
        let ledger_event = bot_core::accounting::fill_event_for_trade(
            &trade,
            self.wallet.pubkey.to_string(),
            bot_core::global_risk::strategy_label(BotModule::Sniper, None),
            None,
            None,
        );
        self.state.record_trade(trade.clone()).await;
        self.state.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade),
        });

        // ---- Apply to the position ---------------------------------------
        // `with_position` gives us &mut access under the state's write lock;
        // capture the realised slice through a mutable local.
        let mut realized = 0.0;
        let mut exit_sig = signature.clone();
        self.state
            .with_position(&position.id, |p| {
                realized = p.apply_sell(sell_qty_human, price, quote_sol);
                p.exit_signature = exit_sig.take();
            })
            .await;

        self.risk.book_pnl(BotModule::Sniper, realized).await;
        // The global ledger is the only mutator of global accounting state;
        // it books the realized slice from its own average cost.
        self.state.ledger().submit(ledger_event).await;

        // Close fully, or trim and keep managing the remainder.
        let remaining = position.qty - sell_qty_human;
        if remaining <= f64::EPSILON || fraction >= 1.0 {
            let status = if matches!(
                rule,
                Some(ExitRule::KillSwitch)
                    | Some(ExitRule::StopLoss)
                    | Some(ExitRule::TrailingStop)
                    | Some(ExitRule::StalePosition)
            ) {
                PositionStatus::StoppedOut
            } else {
                PositionStatus::Closed
            };
            self.state
                .close_position(&position.id, status, reason)
                .await;
            info!(
                symbol = %position.symbol,
                venue = venue.as_str(),
                realized,
                reason,
                "position closed"
            );
        } else {
            let updated = self.state.position(&position.id).await;
            if let Some(p) = updated {
                self.state.events.publish(AppEvent::PositionUpdate {
                    ts: Utc::now(),
                    position: Box::new(p),
                });
            }
            info!(
                symbol = %position.symbol,
                sold = sell_qty_human,
                remaining,
                realized,
                "partial exit"
            );
        }
        Ok(())
    }

    /// Choose the exit venue from the position's venue and the live state.
    /// Direct routes fall back to Jupiter when the pool cannot be loaded and
    /// the fallback is enabled; with no route at all the error names it.
    async fn exit_route(
        &self,
        position: &Position,
        mint: &Pubkey,
        sniper: &bot_core::config::SniperConfig,
    ) -> BotResult<ExitVenue> {
        let pool = position
            .market_id
            .as_deref()
            .and_then(|p| Pubkey::try_from(p).ok());
        let jupiter_or = |detail: String| -> BotResult<ExitVenue> {
            if sniper.use_jupiter_fallback {
                debug!(symbol = %position.symbol, %detail, "exit falling back to jupiter");
                Ok(ExitVenue::Jupiter)
            } else {
                Err(BotError::invalid(format!(
                    "INVALID_ROUTE: {detail} and jupiter fallback is off — cannot exit {}",
                    position.symbol
                )))
            }
        };
        match position.venue {
            Venue::PumpSwap => {
                if !sniper.trade_pumpswap {
                    return jupiter_or("trade_pumpswap is off".into());
                }
                match PumpSwapContext::load(&self.rpc, mint, &self.wallet.pubkey, pool).await {
                    Ok(ctx) if !ctx.config.sells_disabled() => {
                        Ok(ExitVenue::PumpSwap(Box::new(ctx)))
                    }
                    Ok(_) => jupiter_or("pumpswap sells disabled".into()),
                    Err(e) => jupiter_or(format!("pumpswap pool unavailable: {e}")),
                }
            }
            Venue::RaydiumAmmV4 => {
                if !sniper.trade_raydium {
                    return jupiter_or("trade_raydium is off".into());
                }
                let Some(amm) = pool else {
                    return jupiter_or("position carries no raydium pool id".into());
                };
                match RaydiumPool::load(&self.rpc, &amm, true).await {
                    Ok(p) if p.amm.is_swappable() => Ok(ExitVenue::Raydium(Box::new(p))),
                    Ok(_) => jupiter_or("raydium pool not swappable".into()),
                    Err(e) => jupiter_or(format!("raydium pool unavailable: {e}")),
                }
            }
            Venue::Jupiter => Ok(ExitVenue::Jupiter),
            _ => {
                // pump.fun (and anything else): the curve while it is live,
                // then the PumpSwap pool, then Jupiter.
                match PumpContext::load(&self.rpc, mint, &self.wallet.pubkey, None).await {
                    Ok(ctx) if !ctx.curve.complete => Ok(ExitVenue::Curve(Box::new(ctx))),
                    Ok(_) if sniper.trade_pumpswap => {
                        match PumpSwapContext::load(&self.rpc, mint, &self.wallet.pubkey, None)
                            .await
                        {
                            Ok(ctx) if !ctx.config.sells_disabled() => {
                                Ok(ExitVenue::PumpSwap(Box::new(ctx)))
                            }
                            Ok(_) => jupiter_or("pumpswap sells disabled".into()),
                            Err(e) => {
                                jupiter_or(format!("graduated; pumpswap pool unavailable: {e}"))
                            }
                        }
                    }
                    Ok(_) => jupiter_or("token graduated".into()),
                    Err(e) => jupiter_or(format!("bonding curve unavailable: {e}")),
                }
            }
        }
    }

    /// Sell on the bonding curve. Returns (quote_sol, signature, venue, status).
    async fn sell_on_curve(
        &mut self,
        ctx: &PumpContext,
        sell_raw: u64,
        slippage_pct: f64,
        cfg: &bot_core::config::Config,
        intent_id: &str,
    ) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
        let (amount, min_sol_output) = pump::plan_sell(&ctx.curve, sell_raw, slippage_pct)?;
        let opts = BuildOptions {
            extra_accounts: cfg.sniper.pump_extra_accounts.clone(),
            append_bonding_curve_v2: cfg.sniper.pump_append_bonding_curve_v2,
            ..BuildOptions::default()
        };
        let sell_ix = {
            let store = self.layouts.read().await;
            pump::build_sell_ix(ctx, &store, &opts, amount, min_sol_output)?
        };
        let req = TxRequest::new(format!("exit-{}", ctx.mint)).with_instruction(sell_ix);
        let result = self
            .run_exit_request(req, cfg, intent_id, &ctx.mint, sell_raw)
            .await?;
        let quote_sol = maths::lamports_to_sol(min_sol_output);
        Ok((
            quote_sol,
            signature_of(&result),
            Venue::PumpFun,
            result.status,
        ))
    }

    /// Sell on a PumpSwap pool (token → WSOL, unwrapped in the same tx).
    async fn sell_on_pumpswap(
        &mut self,
        ctx: &PumpSwapContext,
        sell_raw: u64,
        slippage_pct: f64,
        cfg: &bot_core::config::Config,
        intent_id: &str,
    ) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
        let (amount, min_quote_out) = pumpswap::plan_sell(ctx, sell_raw, slippage_pct)?;
        let sell_ix = {
            let store = self.layouts.read().await;
            pumpswap::build_sell_ix(ctx, &store, amount, min_quote_out)?
        };
        // The WSOL ATA must exist to receive the quote; `wrap_sol(0)` is not
        // a thing, so create it idempotently and sweep it afterwards.
        let (_wsol_ata, create_wsol) = self.wallet.ensure_ata(&self.rpc, &WSOL_MINT).await?;
        let mut req = TxRequest::new(format!("exit-ps-{}", ctx.base_mint));
        if let Some(ix) = create_wsol {
            req = req.with_instruction(ix);
        }
        let mut req = req.with_instruction(sell_ix);
        req.unwrap_sol = true;
        let result = self
            .run_exit_request(req, cfg, intent_id, &ctx.base_mint, sell_raw)
            .await?;
        Ok((
            maths::lamports_to_sol(min_quote_out),
            signature_of(&result),
            Venue::PumpSwap,
            result.status,
        ))
    }

    /// Sell on a Raydium AMM v4 pool (token → WSOL, unwrapped in the same tx).
    async fn sell_on_raydium(
        &mut self,
        pool: &RaydiumPool,
        mint: &Pubkey,
        sell_raw: u64,
        slippage_pct: f64,
        cfg: &bot_core::config::Config,
        intent_id: &str,
    ) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
        let expected_out = pool.quote(mint, sell_raw)?;
        let min_out = maths::minus_pct_u64(expected_out, slippage_pct);
        let (wsol_ata, create_wsol) = self.wallet.ensure_ata(&self.rpc, &WSOL_MINT).await?;
        let (token_ata, _create_token) = self.wallet.ensure_ata(&self.rpc, mint).await?;
        let swap_ix = {
            let store = self.layouts.read().await;
            pool.swap_base_in_learned(
                &store,
                &self.wallet.pubkey,
                &token_ata,
                &wsol_ata,
                sell_raw,
                min_out,
                pool.market.is_none(),
            )?
        };
        let mut req = TxRequest::new(format!("exit-ray-{mint}"));
        if let Some(ix) = create_wsol {
            req = req.with_instruction(ix);
        }
        let mut req = req.with_instruction(swap_ix);
        req.unwrap_sol = true;
        let result = self
            .run_exit_request(req, cfg, intent_id, mint, sell_raw)
            .await?;
        Ok((
            maths::lamports_to_sol(min_out),
            signature_of(&result),
            Venue::RaydiumAmmV4,
            result.status,
        ))
    }

    /// Common tail for the direct exit routes: fees, deterministic intent
    /// id, attribution, policy refresh, write-ahead journal, hardened
    /// executor.
    async fn run_exit_request(
        &mut self,
        req: TxRequest,
        cfg: &bot_core::config::Config,
        intent_id: &str,
        mint: &Pubkey,
        sell_raw: u64,
    ) -> BotResult<solana_kit::execute::ExecutionResult> {
        let mut req = req
            .priority_fee(cfg.execution.priority_fee_micro_lamports)
            .compute_units(cfg.execution.compute_unit_limit)
            // One exit decision → one lifecycle record (duplicate-protected).
            .with_intent_id(intent_id)
            .attributed("sniper", mint.to_string());
        if cfg.execution.use_jito {
            req = req.jito_tip(cfg.execution.jito_tip_lamports);
        }
        self.refresh_policy().await;
        // Write-ahead intent (§I crash point C) — exits move money too.
        let intent = self.intent_rec(&mint.to_string(), "sell", &sell_raw.to_string());
        bot_core::recovery::with_intent(
            self.intents.as_ref(),
            intent,
            self.executor.run(req),
            |r| r.broadcast_signature(),
        )
        .await
    }

    /// Sell a graduated token through Jupiter (mint → WSOL).
    async fn sell_via_jupiter(
        &mut self,
        mint: Pubkey,
        sell_raw: u64,
        cfg: &bot_core::config::Config,
        intent_id: &str,
    ) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
        let slippage_bps = (cfg.sniper.slippage_pct * 100.0).round() as u64;
        let jupiter = Jupiter::new();
        let quote = jupiter
            .quote(&QuoteRequest::new(mint, *WSOL_MINT, sell_raw).slippage_bps(slippage_bps))
            .await?;
        let expected_out = quote.out_amount_u64()?;
        let mode = self.state.execution_mode().await;

        // `outcome` is the executor's verdict once a live broadcast went
        // through the lifecycle (ledger, duplicate guard, expiry-aware
        // confirmation); `None` for paper (nothing built) and for
        // simulate-success (nothing broadcast).
        let outcome: Option<solana_kit::execute::ExecutionResult> = if mode == ExecutionMode::Paper
        {
            None
        } else {
            let recent = self.rpc.latest_blockhash(true).await?;
            let (_q, tx, last_valid) = jupiter
                .build_swap(
                    &self.wallet,
                    &QuoteRequest::new(mint, *WSOL_MINT, sell_raw).slippage_bps(slippage_bps),
                    Some(recent.blockhash),
                    Some(cfg.execution.priority_fee_micro_lamports),
                )
                .await?;
            if mode == ExecutionMode::Simulate {
                let sim = self.rpc.simulate(&tx).await?;
                if let Some(err) = sim.value.err {
                    return Err(BotError::solana(format!(
                        "jupiter exit simulate failed: {err}"
                    )));
                }
                None
            } else {
                self.state.may_broadcast().await?;
                self.refresh_policy().await;
                let built = solana_kit::tx::BuiltTx::from_signed(
                    format!("exit-jup-{mint}"),
                    tx,
                    recent.blockhash,
                    last_valid.or(Some(recent.last_valid_block_height)),
                )?
                .with_intent_id(intent_id)
                .attributed("sniper", mint.to_string());
                let intent = self.intent_rec(&mint.to_string(), "sell", &sell_raw.to_string());
                let result = bot_core::recovery::with_intent(
                    self.intents.as_ref(),
                    intent,
                    self.executor.send_prebuilt(&built),
                    |r| r.broadcast_signature(),
                )
                .await?;
                Some(result)
            }
        };

        // Proof-of-landing decides the status (§I): the executor reports
        // Confirmed only when the signature was observed on chain; an
        // unproven broadcast stays `Sent` (ambiguous) for reconciliation.
        let (signature, status) = match (&mode, outcome) {
            (ExecutionMode::Paper, _) => (None, ExecStatus::PaperFilled),
            (_, Some(result)) => (
                (!result.signature.is_empty()).then(|| result.signature.clone()),
                result.status,
            ),
            // Simulate-success: counts as filled for bookkeeping, as before.
            (_, None) => (None, ExecStatus::Sent),
        };
        Ok((
            maths::lamports_to_sol(expected_out),
            signature,
            Venue::Jupiter,
            status,
        ))
    }
}

/// The venue a sell executes on, with its loaded context.
enum ExitVenue {
    Curve(Box<PumpContext>),
    PumpSwap(Box<PumpSwapContext>),
    Raydium(Box<RaydiumPool>),
    Jupiter,
}

fn signature_of(result: &solana_kit::execute::ExecutionResult) -> Option<String> {
    if result.signature.is_empty() {
        None
    } else {
        Some(result.signature.clone())
    }
}

/// Price of `mint` in SOL from a loaded Raydium pool, whichever side it is on.
fn raydium_price_in_sol(pool: &RaydiumPool, mint: &Pubkey) -> f64 {
    let p = pool.price_pc_per_coin();
    if pool.amm.coin_mint == *mint && pool.amm.pc_mint == *WSOL_MINT {
        p
    } else if pool.amm.pc_mint == *mint && pool.amm.coin_mint == *WSOL_MINT && p > 0.0 {
        1.0 / p
    } else {
        0.0
    }
}

/// Deterministic execution-intent id for one exit decision. The position
/// (id, mint and opening time — ids alone recur across process lives), the
/// quantity being sold and the quantity held when the decision was made
/// identify the decision: a retry of the same exit maps onto the same ledger
/// record (duplicate-protected), while the next partial exit — taken from a
/// smaller position — gets a fresh id. The route is part of the id: a curve
/// sell and a Jupiter sell of the same decision are different transactions.
pub(crate) fn exit_intent_id(
    module: &str,
    position: &Position,
    sell_raw: u64,
    route: &str,
) -> String {
    bot_core::execution::intent_id(&[
        module,
        &position.id,
        &position.symbol,
        &position.opened_at.timestamp_millis().to_string(),
        "sell",
        route,
        &sell_raw.to_string(),
        &format!("{:.9}", position.qty),
    ])
}

/// Public wrapper so tests and the replay engine compute the exact exit
/// intent id the live path would use.
pub fn exit_intent_id_for(position: &Position, sell_raw: u64, route: EntryRoute) -> String {
    exit_intent_id("sniper", position, sell_raw, route.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_fate_classifies_ledger_states() {
        assert_eq!(entry_fate(None), EntryFate::Held);
        assert_eq!(entry_fate(Some(ExecutionState::Confirmed)), EntryFate::Held);
        assert_eq!(
            entry_fate(Some(ExecutionState::Reconciled)),
            EntryFate::Held
        );
        assert_eq!(entry_fate(Some(ExecutionState::Failed)), EntryFate::Failed);
        assert_eq!(entry_fate(Some(ExecutionState::Expired)), EntryFate::Failed);
        for s in [
            ExecutionState::Created,
            ExecutionState::Validated,
            ExecutionState::Submitted,
            ExecutionState::Pending,
        ] {
            assert_eq!(entry_fate(Some(s)), EntryFate::Unknown, "{s:?}");
        }
    }

    #[test]
    fn exit_tracker_backoff_and_staleness() {
        let mut t = ExitTracker::default();
        let t0 = Utc::now();
        assert_eq!(t.unpriceable_secs("p", t0), 0);
        assert_eq!(t.mark_failed("p", t0), 0);
        let later = t0 + chrono::Duration::seconds(30);
        assert_eq!(t.mark_failed("p", later), 30);
        assert_eq!(t.unpriceable_secs("p", later), 30);
        t.mark_ok("p");
        assert_eq!(t.unpriceable_secs("p", later), 0);

        assert!(!t.in_backoff("p", t0, 10));
        t.exit_failed("p", t0);
        assert!(t.in_backoff("p", t0 + chrono::Duration::seconds(9), 10));
        assert!(!t.in_backoff("p", t0 + chrono::Duration::seconds(10), 10));
        assert!(!t.in_backoff("p", t0, 0), "0 disables the backoff");
        t.exit_done("p");
        assert!(!t.in_backoff("p", t0, 10));

        t.exit_failed("gone", t0);
        t.mark_failed("gone", t0);
        t.retain(&["p".to_string()]);
        assert!(!t.in_backoff("gone", t0, 10));
        assert_eq!(t.unpriceable_secs("gone", t0), 0);
    }

    #[test]
    fn exit_intent_ids_are_deterministic_per_decision_and_route() {
        let mut p = Position::new(
            "p-1".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            ExecutionMode::Paper,
            "So11111111111111111111111111111111111111112".into(),
            "WSOL".into(),
            "SOL".into(),
        );
        p.qty = 1_000.0;
        let a = exit_intent_id_for(&p, 500_000_000, EntryRoute::PumpCurve);
        let b = exit_intent_id_for(&p, 500_000_000, EntryRoute::PumpCurve);
        assert_eq!(a, b, "same decision → same id");
        assert_ne!(a, exit_intent_id_for(&p, 500_000_000, EntryRoute::Jupiter));
        assert_ne!(
            a,
            exit_intent_id_for(&p, 400_000_000, EntryRoute::PumpCurve)
        );
        p.qty = 500.0; // the next partial exit comes from a smaller position
        assert_ne!(
            a,
            exit_intent_id_for(&p, 500_000_000, EntryRoute::PumpCurve)
        );
        assert!(a.starts_with("int_"));
        // The same position id in another process life (different mint or
        // opening time) never collides with this decision.
        p.qty = 1_000.0;
        let mut other_life = p.clone();
        other_life.opened_at += chrono::Duration::seconds(1);
        assert_ne!(
            a,
            exit_intent_id_for(&other_life, 500_000_000, EntryRoute::PumpCurve)
        );
        let mut other_mint = p.clone();
        other_mint.symbol = "So11111111111111111111111111111111111111111".into();
        assert_ne!(
            a,
            exit_intent_id_for(&other_mint, 500_000_000, EntryRoute::PumpCurve)
        );
    }
}
