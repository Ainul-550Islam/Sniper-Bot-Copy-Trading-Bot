//! Exit path for Module 2: manage TP/SL/trailing/max-hold on mirrored
//! positions, and sell when a whale we copied exits.
//!
//! [`sell_position`] is a free function so the background sweeper, the
//! mirrored-exit stage of the pipeline (`CopyBot::mirror_exit` in
//! `event.rs`) and the reconciler's auto-exit can call it with the
//! infrastructure they already hold. Every sell carries the hardened
//! deterministic exit id from [`crate::intent::exit_intent_id`] (position
//! id, mint, open time and the exact sell) and the unchanged `copy-exit-*`
//! label, and goes through the same executor / execution ledger /
//! write-ahead intent journal / ownership permit as before TASK 3.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::maths;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::risk::{ExitRule, RiskEngine};
use bot_core::state::Shared;

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::{exec_policy_from_config, ExecStatus, Executor};
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::layout::LayoutStore;
use solana_kit::pump::{self, BondingCurveState, BuildOptions, PumpContext};
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;
use solana_kit::tx::TxRequest;

use crate::intent::{exit_intent_id, exit_label, ExitRoute};

/// Sweep interval for mirrored positions.
const SWEEP_INTERVAL: Duration = Duration::from_secs(4);
/// Reference size for marking a graduated token via Jupiter (1 whole token).
const MARK_REFERENCE_RAW: u64 = 1_000_000;

/// Background manager for the copy module's open positions.
pub struct ExitSweeper {
    state: Shared,
    rpc: Rpc,
    wallet: Arc<Wallet>,
    layouts: Arc<RwLock<LayoutStore>>,
    executor: Executor,
    risk: RiskEngine,
    jupiter: Jupiter,
    /// Write-ahead intent journal (§I crash point C); exits move money too.
    intents: Option<Arc<dyn bot_core::recovery::IntentSink>>,
    /// Distributed execution ownership (Prompt 3 §F); exits claim
    /// `exit:{position.id}:{rule}` so exactly one replica sells.
    ownership: Option<Arc<bot_core::ownership::OwnershipRegistry>>,
}

/// One journaled broadcast: the sink plus the pre-built intent record.
type Journal<'a> = Option<(
    &'a Arc<dyn bot_core::recovery::IntentSink>,
    bot_core::db::repo::IntentRecord,
)>;

impl ExitSweeper {
    /// Build a sweeper. The executor/risk/jupiter are created here so the
    /// sweeper is self-contained on its own task.
    pub async fn new(
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        layouts: Arc<RwLock<LayoutStore>>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        intents: Option<Arc<dyn bot_core::recovery::IntentSink>>,
        ownership: Option<Arc<bot_core::ownership::OwnershipRegistry>>,
    ) -> Self {
        let cfg = state.config_snapshot().await;
        let mut executor =
            Executor::new(rpc.clone(), wallet.clone(), exec_policy_from_config(&cfg))
                .with_fee_policy(solana_kit::execute::fee_policy_from_config(&cfg));
        if let Some(reg) = &signers {
            executor = executor.with_signer_registry(Arc::clone(reg));
        }
        let risk = RiskEngine::new(state.clone());
        ExitSweeper {
            state,
            rpc,
            wallet,
            layouts,
            executor,
            risk,
            jupiter: Jupiter::new(),
            intents,
            ownership,
        }
    }

    /// Run the sweeper until aborted.
    pub async fn run(&mut self) {
        info!("copy exit sweeper started");
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = self.state.wait_shutdown() => {
                    info!("copy exit sweeper stopping (shutdown)");
                    break;
                }
            }
            if let Err(e) = self.sweep_once().await {
                warn!(error = %e, "copy exit sweep failed");
                self.state
                    .record_error(BotModule::Copy, &format!("exit sweep: {e}"))
                    .await;
            }
            self.state.heartbeat(BotModule::Copy).await;
        }
    }

    /// One pass over every open copy position.
    pub async fn sweep_once(&mut self) -> BotResult<()> {
        let positions = self.state.open_positions_for(BotModule::Copy).await;
        if positions.is_empty() {
            return Ok(());
        }
        let kill = self.state.kill_switch();
        let cfg = self.state.config_snapshot().await;
        // Refresh the executor policy each pass (picks up mode/gate changes).
        self.executor.set_policy(exec_policy_from_config(&cfg));
        let fees = solana_kit::execute::fee_policy_from_config(&cfg);
        if *self.executor.fee_policy() != fees {
            self.executor.set_fee_policy(fees);
        }

        for mut position in positions {
            if !kill {
                match mark_price_sol(&self.rpc, &self.jupiter, &position.symbol).await {
                    Ok(mark) if mark.is_finite() && mark > 0.0 => {
                        position.last_mark = mark;
                        position.trailing_high_water =
                            Some(position.trailing_high_water.map_or(mark, |h| h.max(mark)));
                        position.updated_at = Utc::now();
                        self.state.upsert_position(position.clone()).await;
                        self.state
                            .set_unrealized(BotModule::Copy, position.unrealised())
                            .await;
                    }
                    Ok(_) => debug!(symbol = %position.symbol, "non-positive mark, holding"),
                    Err(e) => debug!(symbol = %position.symbol, error = %e, "could not mark"),
                }
            }

            let decision = self.risk.check_exit(&position, position.last_mark).await;
            if !decision.should_exit {
                continue;
            }
            info!(
                symbol = %position.symbol,
                rule = ?decision.rule,
                reason = %decision.reason,
                "copy exit rule fired"
            );
            // Distributed ownership per SELL DECISION (Prompt 3 §F/§P):
            // exactly one replica executes `exit:{position.id}:{rule}`.
            // Losers skip; store failures fail closed for this round (the
            // 4s sweep retries — exits are delayed, never double-executed).
            let rule_str = decision.rule.map(|r| r.as_str()).unwrap_or("manual");
            let mut permit = match bot_core::ownership::Permit::acquire(
                self.ownership.as_deref(),
                format!("exit:{}:{}", position.id, rule_str),
                "exit",
                "copy",
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
                        "copy exit ownership unavailable — failing closed this round (retry next sweep)"
                    );
                    continue;
                }
            };
            if !permit.proceed() {
                debug!(position = %position.id, rule = rule_str, "copy exit owned by another replica — skipping");
                continue;
            }

            if let Err(e) = sell_position(
                &self.state,
                &self.rpc,
                &self.wallet,
                &mut self.executor,
                &self.layouts,
                &self.risk,
                &position,
                decision.fraction,
                &decision.reason,
                decision.rule,
                self.intents.as_ref(),
                &mut permit,
            )
            .await
            {
                // Pre-broadcast failures release the claim so the next sweep
                // can re-decide at once (sell_position finishes it internally
                // once a broadcast outcome exists; double-finish is a no-op).
                permit.finish(false).await;
                warn!(symbol = %position.symbol, error = %e, "copy exit sell failed");
                self.state
                    .record_error(BotModule::Copy, &format!("exit {}: {e}", position.symbol))
                    .await;
            }
        }
        Ok(())
    }
}

/// Best available mark price in SOL per token (curve spot, else Jupiter).
pub async fn mark_price_sol(rpc: &Rpc, jupiter: &Jupiter, mint_str: &str) -> BotResult<f64> {
    let mint = Pubkey::try_from(mint_str)
        .map_err(|e| BotError::invalid(format!("position mint {mint_str}: {e}")))?;
    let curve_addr = pump::bonding_curve_pda(&mint);
    if let Ok(Some(data)) = rpc.get_account_processed(&curve_addr).await {
        if let Ok(curve) = BondingCurveState::parse(&data) {
            if !curve.complete {
                return Ok(maths::pump_spot_price_sol(
                    curve.virtual_sol_reserves,
                    curve.virtual_token_reserves,
                ));
            }
        }
    }
    let quote = jupiter
        .quote(&QuoteRequest::new(mint, *WSOL_MINT, MARK_REFERENCE_RAW))
        .await?;
    Ok(maths::lamports_to_sol(quote.out_amount_u64()?))
}

/// Sell `fraction` of a position, book the PnL, and close or trim it.
///
/// Shared by the sweeper (TP/SL/trailing/max-hold) and the mirror-exit path
/// (whale sold). Routes through the bonding curve while the token is on it,
/// otherwise through Jupiter.
#[allow(clippy::too_many_arguments)]
pub async fn sell_position(
    state: &Shared,
    rpc: &Rpc,
    wallet: &Arc<Wallet>,
    executor: &mut Executor,
    layouts: &Arc<RwLock<LayoutStore>>,
    risk: &RiskEngine,
    position: &Position,
    fraction: f64,
    reason: &str,
    rule: Option<ExitRule>,
    intents: Option<&Arc<dyn bot_core::recovery::IntentSink>>,
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
    let cfg = state.config_snapshot().await;
    let slippage_pct = cfg.copy.slippage_pct;

    let ctx = PumpContext::load(rpc, &mint, &wallet.pubkey, None).await?;
    // Write-ahead intent (§I crash point C), rebuilt per branch because the
    // record is consumed by whichever broadcast actually runs.
    let mk_journal = || -> Journal<'_> {
        intents.map(|s| {
            (
                s,
                crate::copy_intent(
                    state,
                    wallet,
                    &position.symbol,
                    "sell",
                    &sell_raw.to_string(),
                ),
            )
        })
    };
    // Fencing (§E): re-validate ownership immediately before the
    // money-moving branch.
    permit.fence().await?;

    let (quote_sol, signature, venue, status) = if !ctx.curve.complete {
        sell_on_curve(
            executor,
            layouts,
            &ctx,
            &cfg,
            sell_raw,
            slippage_pct,
            mk_journal(),
            &exit_intent_id(position, sell_raw, ExitRoute::Curve),
        )
        .await?
    } else if cfg.sniper.use_jupiter_fallback {
        sell_via_jupiter(
            state,
            executor,
            rpc,
            wallet,
            mint,
            sell_raw,
            &cfg,
            mk_journal(),
            &exit_intent_id(position, sell_raw, ExitRoute::Jupiter),
        )
        .await?
    } else {
        return Err(BotError::solana(
            "token graduated and jupiter fallback is off — cannot exit copy position",
        ));
    };

    // Ownership terminal (§I/§M): unproven outcomes hand off to
    // reconciliation; determinate outcomes (incl. unfilled) release.
    permit.finish(status.is_ambiguous()).await;

    let filled = matches!(
        status,
        ExecStatus::Confirmed
            | ExecStatus::Sent
            | ExecStatus::SendUnknown
            | ExecStatus::PaperFilled
    );
    if !filled {
        state.inc_orders_failed(BotModule::Copy).await;
        return Err(BotError::solana(format!(
            "copy exit order did not fill ({status:?})"
        )));
    }
    state.inc_orders_sent(BotModule::Copy).await;

    let price = if sell_qty_human > 0.0 {
        quote_sol / sell_qty_human
    } else {
        position.last_mark
    };

    let trade = Trade {
        id: state.next_id("t"),
        ts: Utc::now(),
        source: TradeSource::Copy,
        venue,
        mode: state.execution_mode().await,
        side: PositionSide::Short,
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
            "copy exit {} {}",
            rule.map(|r| r.as_str()).unwrap_or("mirror"),
            reason
        )),
        latency_ms: None,
    };
    // TASK 5 — the typed accounting event for this exit fill (same figures
    // as the trade record; the leader-scoped strategy label keeps the
    // exposure attributed to the leader that opened it).
    let ledger_event = bot_core::accounting::fill_event_for_trade(
        &trade,
        wallet.pubkey.to_string(),
        bot_core::global_risk::strategy_label(BotModule::Copy, position.copied_wallet.as_deref()),
        None,
        None,
    );
    state.record_trade(trade.clone()).await;
    state.events.publish(AppEvent::Fill {
        ts: Utc::now(),
        trade: Box::new(trade),
    });

    let mut realized = 0.0;
    let mut exit_sig = signature.clone();
    state
        .with_position(&position.id, |p| {
            realized = p.apply_sell(sell_qty_human, price, quote_sol);
            p.exit_signature = exit_sig.take();
        })
        .await;
    risk.book_pnl(BotModule::Copy, realized).await;
    // The global ledger is the only mutator of global accounting state; it
    // books the realized slice from its own average cost.
    state.ledger().submit(ledger_event).await;

    let remaining = position.qty - sell_qty_human;
    if remaining <= f64::EPSILON || fraction >= 1.0 {
        let status = if matches!(
            rule,
            Some(ExitRule::KillSwitch) | Some(ExitRule::StopLoss) | Some(ExitRule::TrailingStop)
        ) {
            PositionStatus::StoppedOut
        } else {
            PositionStatus::Closed
        };
        state.close_position(&position.id, status, reason).await;
        info!(symbol = %position.symbol, realized, reason, "copy position closed");
    } else if let Some(p) = state.position(&position.id).await {
        state.events.publish(AppEvent::PositionUpdate {
            ts: Utc::now(),
            position: Box::new(p),
        });
        info!(symbol = %position.symbol, sold = sell_qty_human, remaining, realized, "copy partial exit");
    }
    Ok(())
}

/// Sell on the bonding curve. Returns (quote_sol, signature, venue, status).
#[allow(clippy::too_many_arguments)]
async fn sell_on_curve(
    executor: &mut Executor,
    layouts: &Arc<RwLock<LayoutStore>>,
    ctx: &PumpContext,
    cfg: &bot_core::config::Config,
    sell_raw: u64,
    slippage_pct: f64,
    journal: Journal<'_>,
    intent_id: &str,
) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
    let (amount, min_sol_output) = pump::plan_sell(&ctx.curve, sell_raw, slippage_pct)?;
    let opts = BuildOptions {
        extra_accounts: cfg.sniper.pump_extra_accounts.clone(),
        append_bonding_curve_v2: cfg.sniper.pump_append_bonding_curve_v2,
        ..BuildOptions::default()
    };
    let sell_ix = {
        let store = layouts.read().await;
        pump::build_sell_ix(ctx, &store, &opts, amount, min_sol_output)?
    };
    let mut req = TxRequest::new(exit_label(&ctx.mint.to_string(), ExitRoute::Curve))
        .with_instruction(sell_ix)
        .priority_fee(cfg.execution.priority_fee_micro_lamports)
        .compute_units(cfg.execution.compute_unit_limit)
        // One exit decision → one lifecycle record (duplicate-protected).
        .with_intent_id(intent_id)
        .attributed("copy", ctx.mint.to_string());
    if cfg.execution.use_jito {
        req = req.jito_tip(cfg.execution.jito_tip_lamports);
    }
    let result = match journal {
        Some((sink, rec)) => {
            bot_core::recovery::with_intent(Some(sink), rec, executor.run(req), |r| {
                r.broadcast_signature()
            })
            .await?
        }
        None => executor.run(req).await?,
    };
    let quote_sol = maths::lamports_to_sol(min_sol_output);
    let signature = if result.signature.is_empty() {
        None
    } else {
        Some(result.signature.clone())
    };
    Ok((quote_sol, signature, Venue::PumpFun, result.status))
}

/// Sell a graduated token through Jupiter (mint → WSOL).
#[allow(clippy::too_many_arguments)]
async fn sell_via_jupiter(
    state: &Shared,
    executor: &Executor,
    rpc: &Rpc,
    wallet: &Arc<Wallet>,
    mint: Pubkey,
    sell_raw: u64,
    cfg: &bot_core::config::Config,
    journal: Journal<'_>,
    intent_id: &str,
) -> BotResult<(f64, Option<String>, Venue, ExecStatus)> {
    let slippage_bps = (cfg.copy.slippage_pct * 100.0).round() as u64;
    let jupiter = Jupiter::new();
    let quote = jupiter
        .quote(&QuoteRequest::new(mint, *WSOL_MINT, sell_raw).slippage_bps(slippage_bps))
        .await?;
    let expected_out = quote.out_amount_u64()?;
    let mode = state.execution_mode().await;

    // `outcome` is the executor's verdict once a live broadcast went through
    // the lifecycle (ledger, duplicate guard, expiry-aware confirmation);
    // `None` for paper (nothing built) and simulate-success (nothing sent).
    let outcome: Option<solana_kit::execute::ExecutionResult> = if mode == ExecutionMode::Paper {
        None
    } else {
        let recent = rpc.latest_blockhash(true).await?;
        let (_q, tx, last_valid) = jupiter
            .build_swap(
                wallet,
                &QuoteRequest::new(mint, *WSOL_MINT, sell_raw).slippage_bps(slippage_bps),
                Some(recent.blockhash),
                Some(cfg.execution.priority_fee_micro_lamports),
            )
            .await?;
        if mode == ExecutionMode::Simulate {
            let sim = rpc.simulate(&tx).await?;
            if let Some(err) = sim.value.err {
                return Err(BotError::solana(format!(
                    "jupiter copy exit simulate failed: {err}"
                )));
            }
            None
        } else {
            state.may_broadcast().await?;
            let built = solana_kit::tx::BuiltTx::from_signed(
                exit_label(&mint.to_string(), ExitRoute::Jupiter),
                tx,
                recent.blockhash,
                last_valid.or(Some(recent.last_valid_block_height)),
            )?
            .with_intent_id(intent_id)
            .attributed("copy", mint.to_string());
            let result = match journal {
                Some((sink, rec)) => {
                    bot_core::recovery::with_intent(
                        Some(sink),
                        rec,
                        executor.send_prebuilt(&built),
                        |r| r.broadcast_signature(),
                    )
                    .await?
                }
                None => executor.send_prebuilt(&built).await?,
            };
            Some(result)
        }
    };

    // Proof-of-landing decides the status (§I): the executor reports
    // Confirmed only when the signature was observed on chain; an unproven
    // broadcast stays `Sent` (ambiguous) for reconciliation.
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
