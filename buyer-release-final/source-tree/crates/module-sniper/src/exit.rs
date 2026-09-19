//! Exit path for Module 1: mark open positions to market and close them when a
//! risk rule fires.
//!
//! The sweeper is deliberately independent of launch detection — if the feeds
//! drop, positions still have to be managed. It polls a mark price for each
//! open position (bonding-curve spot first, Jupiter once graduated), updates the
//! trailing high-water mark, and asks [`RiskEngine::check_exit`] what to do.
//!
//! Polling rather than a per-position `subscribeTokenTrade` keeps the sweeper
//! robust: it works on any RPC, needs no extra subscriptions, and degrades
//! gracefully. `sniper.monitor_positions` is honoured as "sweep frequently".

use std::time::Duration;

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::maths;
use bot_core::models::{
    BotModule, ExecutionMode, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::risk::ExitRule;

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::ExecStatus;
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::pump::{self, BondingCurveState, BuildOptions, PumpContext};
use solana_kit::tx::TxRequest;

use crate::Sniper;

/// How often the sweeper runs when `monitor_positions` is on.
const SWEEP_FAST: Duration = Duration::from_secs(2);
/// How often when it is off (still manage risk, just less eagerly).
const SWEEP_SLOW: Duration = Duration::from_secs(10);
/// Reference size used to mark a graduated token via Jupiter (1 whole token).
const MARK_REFERENCE_RAW: u64 = 1_000_000;

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
        if positions.is_empty() {
            return Ok(());
        }
        let kill = self.state.kill_switch();

        for mut position in positions {
            // Mark to market (skip the fetch if the kill switch is flattening
            // everything — we are selling regardless of price).
            if !kill {
                match self.mark_price_sol(&position.symbol).await {
                    Ok(mark) if mark.is_finite() && mark > 0.0 => {
                        position.last_mark = mark;
                        position.trailing_high_water =
                            Some(position.trailing_high_water.map_or(mark, |h| h.max(mark)));
                        position.updated_at = Utc::now();
                        self.state.upsert_position(position.clone()).await;
                        self.state
                            .set_unrealized(BotModule::Sniper, position.unrealised())
                            .await;
                    }
                    Ok(_) => debug!(symbol = %position.symbol, "non-positive mark, holding"),
                    Err(e) => {
                        debug!(symbol = %position.symbol, error = %e, "could not mark position");
                    }
                }
            }

            let decision = self.risk.check_exit(&position, position.last_mark).await;
            if !decision.should_exit {
                continue;
            }

            // Honour the operator's partial take-profit setting.
            let fraction = if matches!(decision.rule, Some(ExitRule::TakeProfit)) {
                let f = self
                    .state
                    .config_snapshot()
                    .await
                    .sniper
                    .take_profit_sell_fraction;
                f.clamp(0.0, 1.0)
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

            // Distributed ownership per SELL DECISION (Prompt 3 §F/§P):
            // `exit:{position.id}:{rule}` — exactly one replica executes a
            // given exit. Position ids converge across replicas through the
            // shared DB (restore + book sync), so the logical id is stable.
            // A replica that loses the claim skips (the owner is selling);
            // an unavailable store fails closed for this round — the sweeper
            // retries on the next tick, exits are never permanently blocked.
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

            if let Err(e) = self
                .sell_position(
                    &position,
                    fraction,
                    &decision.reason,
                    decision.rule,
                    &mut permit,
                )
                .await
            {
                // The sell path releases the claim on determinate outcomes
                // internally; this covers pre-broadcast failures (no money
                // moved → release so the next sweep can re-decide at once).
                permit.finish(false).await;
                warn!(symbol = %position.symbol, error = %e, "exit sell failed");
                self.state
                    .record_error(BotModule::Sniper, &format!("exit {}: {e}", position.symbol))
                    .await;
            }
        }
        Ok(())
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
        position: &bot_core::models::Position,
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
        let sell_qty_human = position.qty * fraction;
        let sell_raw = maths::to_raw_amount(sell_qty_human, 6);
        if sell_raw == 0 {
            permit.finish(false).await;
            return Ok(());
        }

        let mint = Pubkey::try_from(position.symbol.as_str())
            .map_err(|e| BotError::invalid(format!("position mint {}: {e}", position.symbol)))?;
        let cfg = self.state.config_snapshot().await;
        let slippage_pct = cfg.sniper.slippage_pct;

        // Is the token still on the bonding curve?
        let ctx = PumpContext::load(&self.rpc, &mint, &self.wallet.pubkey, None).await?;

        // Fencing (§E): re-validate ownership immediately before the
        // money-moving branch — a taken-over replica must not sell.
        permit.fence().await?;

        let (quote_sol, signature, venue, status) = if !ctx.curve.complete {
            self.sell_on_curve(&ctx, sell_raw, slippage_pct, &cfg)
                .await?
        } else if cfg.sniper.use_jupiter_fallback {
            self.sell_via_jupiter(mint, sell_raw, &cfg).await?
        } else {
            return Err(BotError::solana(
                "token graduated and jupiter fallback is off — cannot exit",
            ));
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

        // Close fully, or trim and keep managing the remainder.
        let remaining = position.qty - sell_qty_human;
        if remaining <= f64::EPSILON || fraction >= 1.0 {
            let status = if matches!(
                rule,
                Some(ExitRule::KillSwitch)
                    | Some(ExitRule::StopLoss)
                    | Some(ExitRule::TrailingStop)
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

    /// Sell on the bonding curve. Returns (quote_sol, signature, venue, status).
    async fn sell_on_curve(
        &mut self,
        ctx: &PumpContext,
        sell_raw: u64,
        slippage_pct: f64,
        cfg: &bot_core::config::Config,
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

        let mut req = TxRequest::new(format!("exit-{}", ctx.mint))
            .with_instruction(sell_ix)
            .priority_fee(cfg.execution.priority_fee_micro_lamports)
            .compute_units(cfg.execution.compute_unit_limit);
        if cfg.execution.use_jito {
            req = req.jito_tip(cfg.execution.jito_tip_lamports);
        }
        self.refresh_policy().await;
        // Write-ahead intent (§I crash point C) — exits move money too.
        let intent = self.intent_rec(&ctx.mint.to_string(), "sell", &sell_raw.to_string());
        let result = bot_core::recovery::with_intent(
            self.intents.as_ref(),
            intent,
            self.executor.run(req),
            |r| r.broadcast_signature(),
        )
        .await?;

        let quote_sol = maths::lamports_to_sol(min_sol_output);
        let signature = if result.signature.is_empty() {
            None
        } else {
            Some(result.signature.clone())
        };
        Ok((quote_sol, signature, Venue::PumpFun, result.status))
    }

    /// Sell a graduated token through Jupiter (mint → WSOL).
    async fn sell_via_jupiter(
        &mut self,
        mint: Pubkey,
        sell_raw: u64,
        cfg: &bot_core::config::Config,
    ) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
        let slippage_bps = (cfg.sniper.slippage_pct * 100.0).round() as u64;
        let jupiter = Jupiter::new();
        let quote = jupiter
            .quote(&QuoteRequest::new(mint, *WSOL_MINT, sell_raw).slippage_bps(slippage_bps))
            .await?;
        let expected_out = quote.out_amount_u64()?;
        let mode = self.state.execution_mode().await;

        let signature = if mode == ExecutionMode::Paper {
            None
        } else {
            let blockhash = self.rpc.latest_blockhash(true).await?.blockhash;
            let (_q, tx, _lv) = jupiter
                .build_swap(
                    &self.wallet,
                    &QuoteRequest::new(mint, *WSOL_MINT, sell_raw).slippage_bps(slippage_bps),
                    Some(blockhash),
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
                let intent = self.intent_rec(&mint.to_string(), "sell", &sell_raw.to_string());
                let sig = bot_core::recovery::with_intent(
                    self.intents.as_ref(),
                    intent,
                    self.rpc.send_transaction(&tx),
                    |s| Some(s.to_string()),
                )
                .await?;
                let _ = self
                    .rpc
                    .confirm(
                        &sig,
                        Duration::from_millis(cfg.execution.confirm_timeout_ms),
                        Duration::from_millis(cfg.execution.confirm_poll_ms.max(50)),
                    )
                    .await;
                Some(sig.to_string())
            }
        };

        let status = match mode {
            ExecutionMode::Paper => ExecStatus::PaperFilled,
            _ if signature.is_some() => ExecStatus::Confirmed,
            _ => ExecStatus::Sent,
        };
        Ok((
            maths::lamports_to_sol(expected_out),
            signature,
            Venue::Jupiter,
            status,
        ))
    }
}
