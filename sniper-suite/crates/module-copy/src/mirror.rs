//! Mirroring a decoded whale trade — the legacy door and the execution paths.
//!
//! Before TASK 3 this file was the whole copy engine: [`CopyBot::mirror_trade`]
//! took a raw [`WalletTrade`], applied the wallet rule and bought. It is
//! kept for backward compatibility (the run loop, the tests and any external
//! caller still use it) but it no longer decides anything itself: it lifts
//! the trade into a [`LeaderTradeEvent`] and runs the staged pipeline
//! [`CopyBot::process_event`] (`event.rs`), which owns validation, the one
//! authoritative dedup, ordering, policy, sizing, the risk engine's decision
//! and the ownership claim.
//!
//! What stays here is **execution**, unchanged in behaviour:
//!
//! * `CopyBot::buy` — a pump.fun bonding-curve buy while the token is still
//!   on the curve (`buy_on_curve`), otherwise a Jupiter swap
//!   (`buy_via_jupiter`); both go through the shared executor / execution
//!   ledger with the deterministic entry intent id from `intent.rs`, the
//!   write-ahead intent journal (`bot_core::recovery`) and the cross-replica
//!   ownership permit's broadcast-time fence;
//! * `CopyBot::record_buy` — trade / position / event bookkeeping after a
//!   fill, reported back to the pipeline as a [`BuyReport`];
//! * [`journal_record`] — the durable `copy_events` row for a finished event;
//! * [`size_for`] / [`short`] — the pre-TASK-3 helpers, kept.
//!
//! Leader *sells* are mirrored by the pipeline's exit stage through
//! [`crate::exit::sell_position`] when `copy.mirror_exits` is on.

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use bot_core::config::Config;
use bot_core::config::CopyWallet;
use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::maths;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, Trade, TradeSource, Venue, WalletTrade,
};

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::{ExecStatus, ExecutionResult};
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::pump::{self, BuildOptions, PumpContext};
use solana_kit::tx::TxRequest;

use crate::event::{CopyOutcome, CopyStage, EventSource, LeaderTradeEvent};
use crate::intent::{self, EntryRoute};
use crate::sizing;
use crate::CopyBot;

/// What the execution path reports back to the pipeline.
#[derive(Debug, Clone)]
pub struct BuyReport {
    /// Ledger intent id of the attempt.
    pub intent_id: String,
    /// Broadcast signature when known.
    pub signature: Option<String>,
    /// Whether a position was booked (`Confirmed | Sent | SendUnknown | PaperFilled`).
    pub filled: bool,
    /// Whether the landing is unproven (`Sent | SendUnknown` in live mode).
    pub ambiguous: bool,
    /// Booked position id.
    pub position_id: Option<String>,
    /// Executor status.
    pub status: ExecStatus,
    /// Executor error when it did not fill.
    pub error: Option<String>,
    /// SOL actually spent (lamports → SOL).
    pub cost_sol: f64,
    /// Tokens booked.
    pub qty: f64,
}

impl CopyBot {
    /// Mirror one decoded whale trade — the pre-TASK-3 entry point, kept for
    /// backward compatibility. Lifts the raw trade into a [`LeaderTradeEvent`]
    /// (numbered by this bot, attributed to the configured feed) and runs
    /// the staged pipeline [`CopyBot::process_event`]; returns `Err` only
    /// when the event FAILED (an error before or during execution), never
    /// for a policy / risk / dedup rejection — those are decided, journaled
    /// and metered by the pipeline exactly as for any other event.
    pub async fn mirror_trade(&mut self, trade: &WalletTrade, cfg: &Config) -> BotResult<()> {
        let source = EventSource::from_feed(&cfg.copy.feed);
        let sequence = self.next_sequence();
        let event = LeaderTradeEvent::from_wallet_trade(trade, source, sequence);
        let outcome = self.process_event(&event, cfg).await;
        if outcome.stage == CopyStage::Failed {
            let detail = outcome
                .rejection
                .as_ref()
                .map(|r| r.detail.clone())
                .unwrap_or_else(|| "copy pipeline failed".into());
            return Err(BotError::solana(detail));
        }
        Ok(())
    }

    /// Execute the mirrored buy: bonding curve if still live, else Jupiter.
    /// Called by the pipeline (`CopyBot::process_event`, `event.rs`) after
    /// the ownership claim; returns `Err` only for pre-broadcast failures.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn buy(
        &mut self,
        event: &LeaderTradeEvent,
        rule: &CopyWallet,
        sized_sol: f64,
        slippage_pct: f64,
        slippage_bps: u64,
        decision: &bot_core::risk::RiskDecision,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<BuyReport> {
        let mint = Pubkey::try_from(event.mint.as_str())
            .map_err(|e| BotError::invalid(format!("copy mint {}: {e}", event.mint)))?;
        let cfg = self.state.config_snapshot().await;

        let ctx = PumpContext::load(&self.rpc, &mint, &self.wallet.pubkey, None).await?;
        if !ctx.curve.complete {
            self.buy_on_curve(
                event,
                rule,
                &ctx,
                &cfg,
                sized_sol,
                slippage_pct,
                decision,
                permit,
            )
            .await
        } else {
            self.buy_via_jupiter(
                event,
                rule,
                mint,
                &cfg,
                sized_sol,
                slippage_bps,
                decision,
                permit,
            )
            .await
        }
    }

    /// Bonding-curve buy (token still on pump.fun).
    #[allow(clippy::too_many_arguments)]
    async fn buy_on_curve(
        &mut self,
        event: &LeaderTradeEvent,
        rule: &CopyWallet,
        ctx: &PumpContext,
        cfg: &Config,
        sized_sol: f64,
        slippage_pct: f64,
        decision: &bot_core::risk::RiskDecision,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<BuyReport> {
        let sol_in = maths::sol_to_lamports(sized_sol);
        if sol_in == 0 {
            return Err(BotError::invalid("sized SOL rounds to zero lamports"));
        }
        let fee_bps = ctx.global_state.fee_basis_points;
        let (amount, max_sol_cost) = pump::plan_buy(&ctx.curve, sol_in, slippage_pct, fee_bps)?;
        if amount == 0 {
            return Err(BotError::solana(
                "bonding curve returns zero tokens — curve may be complete",
            ));
        }
        let opts = BuildOptions {
            extra_accounts: cfg.sniper.pump_extra_accounts.clone(),
            append_bonding_curve_v2: cfg.sniper.pump_append_bonding_curve_v2,
            ..BuildOptions::default()
        };
        let buy_ix = {
            let store = self.layouts.read().await;
            pump::build_buy_ix(ctx, &store, &opts, amount, max_sol_cost)?
        };
        let intent_id = intent::entry_intent_id_for(event, EntryRoute::Curve);
        let mut req = TxRequest::new(intent::entry_label(&event.mint, EntryRoute::Curve))
            .with_instruction(buy_ix)
            .priority_fee(cfg.execution.priority_fee_micro_lamports)
            .compute_units(cfg.execution.compute_unit_limit)
            // Deterministic lifecycle identity: one source trade → one mirror,
            // so a replayed feed event or a post-crash retry maps onto the
            // same ledger record and is refused as a duplicate.
            .with_intent_id(&intent_id)
            .attributed("copy", event.mint.clone());
        if cfg.execution.use_jito {
            req = req.jito_tip(cfg.execution.jito_tip_lamports);
        }
        self.refresh_policy().await;
        // Fencing (§E): ownership must still be ours at broadcast time.
        permit.fence().await?;
        let started = Utc::now();
        // Write-ahead intent (§I crash point C): journaled BEFORE broadcast.
        let intent = crate::copy_intent(
            &self.state,
            &self.wallet,
            &event.mint,
            "buy",
            &amount.to_string(),
        );
        let result = bot_core::recovery::with_intent(
            self.intents.as_ref(),
            intent,
            self.executor.run(req),
            |r| r.broadcast_signature(),
        )
        .await?;
        let exec_ms = (Utc::now() - started).num_milliseconds().max(0) as u64;
        // Ownership terminal (§I/§M).
        let ambiguous = result.status.is_ambiguous();
        permit.finish(ambiguous).await;

        self.record_buy(
            event,
            rule,
            Venue::PumpFun,
            &result,
            amount,
            sol_in,
            6,
            exec_ms,
            decision,
            ambiguous,
        )
        .await
    }

    /// Jupiter buy (token graduated; routes across PumpSwap/Raydium/etc).
    #[allow(clippy::too_many_arguments)]
    async fn buy_via_jupiter(
        &mut self,
        event: &LeaderTradeEvent,
        rule: &CopyWallet,
        mint: Pubkey,
        cfg: &Config,
        sized_sol: f64,
        slippage_bps: u64,
        decision: &bot_core::risk::RiskDecision,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<BuyReport> {
        let lamports = maths::sol_to_lamports(sized_sol);
        if lamports == 0 {
            return Err(BotError::invalid("sized SOL rounds to zero lamports"));
        }
        let jupiter = Jupiter::new();
        let quote = jupiter
            .quote(&QuoteRequest::new(*WSOL_MINT, mint, lamports).slippage_bps(slippage_bps))
            .await?;
        let out_amount = quote.out_amount_u64()?;
        if out_amount == 0 {
            return Err(BotError::solana("jupiter found no output for the copy buy"));
        }

        let mode = self.state.execution_mode().await;
        let started = Utc::now();
        let label = intent::entry_label(&event.mint, EntryRoute::Jupiter);
        // Deterministic lifecycle identity (same rule as the curve path).
        let intent_id = intent::entry_intent_id_for(event, EntryRoute::Jupiter);

        // Paper never builds/sends: the quote is the fill. Simulate builds +
        // simulates only. Live hands the Jupiter-signed transaction to the
        // executor's lifecycle — ledger, duplicate protection, expiry-aware
        // confirmation — instead of a bare send + confirm.
        let mut result: ExecutionResult = if mode == ExecutionMode::Paper {
            let mut r = ExecutionResult::empty(&label, &intent_id, true);
            r.status = ExecStatus::PaperFilled;
            r.state = bot_core::execution::ExecutionState::Confirmed;
            r.attempts = 1;
            r
        } else {
            let recent = self.rpc.latest_blockhash(true).await?;
            let (_q, tx, last_valid) = jupiter
                .build_swap(
                    &self.wallet,
                    &QuoteRequest::new(*WSOL_MINT, mint, lamports).slippage_bps(slippage_bps),
                    Some(recent.blockhash),
                    Some(cfg.execution.priority_fee_micro_lamports),
                )
                .await?;
            if mode == ExecutionMode::Simulate {
                let sim = self.rpc.simulate(&tx).await?;
                if let Some(err) = sim.value.err {
                    return Err(BotError::solana(format!(
                        "jupiter copy simulate failed: {err}"
                    )));
                }
                // Simulate-success lands as `Sent` for bookkeeping, as before;
                // the permit terminal below knows nothing was broadcast.
                let mut r = ExecutionResult::empty(&label, &intent_id, false);
                r.status = ExecStatus::Sent;
                r.state = bot_core::execution::ExecutionState::Validated;
                r.attempts = 1;
                r
            } else {
                self.state.may_broadcast().await?;
                self.refresh_policy().await;
                // Fencing (§E): ownership must still be ours at broadcast time.
                permit.fence().await?;
                let built = solana_kit::tx::BuiltTx::from_signed(
                    &label,
                    tx,
                    recent.blockhash,
                    last_valid.or(Some(recent.last_valid_block_height)),
                )?
                .with_intent_id(&intent_id)
                .attributed("copy", event.mint.clone());
                let intent = crate::copy_intent(
                    &self.state,
                    &self.wallet,
                    &mint.to_string(),
                    "buy",
                    &lamports.to_string(),
                );
                bot_core::recovery::with_intent(
                    self.intents.as_ref(),
                    intent,
                    self.executor.send_prebuilt(&built),
                    |r| r.broadcast_signature(),
                )
                .await?
            }
        };
        let exec_ms = (Utc::now() - started).num_milliseconds().max(0) as u64;
        // Wall-clock from quote to outcome (build included), as before.
        result.total_ms = exec_ms;
        // Ownership terminal (§I/§M): only a live broadcast whose landing is
        // unproven (Sent/SendUnknown) is ambiguous; paper/simulate never moved
        // money → release.
        let ambiguous = mode == ExecutionMode::Live && result.status.is_ambiguous();
        permit.finish(ambiguous).await;

        self.record_buy(
            event,
            rule,
            Venue::Jupiter,
            &result,
            out_amount,
            lamports,
            6,
            exec_ms,
            decision,
            ambiguous,
        )
        .await
    }

    /// Post-fill bookkeeping for a mirrored buy.
    #[allow(clippy::too_many_arguments)]
    async fn record_buy(
        &mut self,
        event: &LeaderTradeEvent,
        rule: &CopyWallet,
        venue: Venue,
        result: &ExecutionResult,
        amount_raw: u64,
        sol_in: u64,
        base_decimals: u8,
        latency_ms: u64,
        decision: &bot_core::risk::RiskDecision,
        ambiguous: bool,
    ) -> BotResult<BuyReport> {
        let cfg = self.state.config_snapshot().await;
        let mode = self.state.execution_mode().await;
        self.state.inc_orders_sent(BotModule::Copy).await;

        let filled = matches!(
            result.status,
            ExecStatus::Confirmed
                | ExecStatus::Sent
                | ExecStatus::SendUnknown
                | ExecStatus::PaperFilled
        );
        let signature = if result.signature.is_empty() {
            None
        } else {
            Some(result.signature.clone())
        };
        if !filled {
            self.state.inc_orders_failed(BotModule::Copy).await;
            self.state
                .record_error(
                    BotModule::Copy,
                    result.error.as_deref().unwrap_or("copy order did not fill"),
                )
                .await;
            warn!(mint = %event.mint, status = ?result.status, "copy buy did not fill");
            return Ok(BuyReport {
                intent_id: result.intent_id.clone(),
                signature,
                filled: false,
                ambiguous: false,
                position_id: None,
                status: result.status,
                error: result.error.clone(),
                cost_sol: 0.0,
                qty: 0.0,
            });
        }

        let qty = maths::from_raw_amount(amount_raw, base_decimals);
        let cost_sol = maths::lamports_to_sol(sol_in);
        let price = if qty > 0.0 { cost_sol / qty } else { 0.0 };
        let display = event.symbol.clone().unwrap_or_else(|| short(&event.mint));

        self.state.events.publish(AppEvent::OrderSent {
            ts: Utc::now(),
            module: BotModule::Copy,
            symbol: display.clone(),
            venue: venue.as_str().to_string(),
            mode: mode.as_str().to_string(),
            quote_amount: cost_sol,
            signature: signature.clone(),
            signer: Some(self.wallet.pubkey.to_string()),
            attempts: Some(result.attempts),
            latency_ms: Some(latency_ms),
        });

        let trade_rec = Trade {
            id: self.state.next_id("t"),
            ts: Utc::now(),
            source: TradeSource::Copy,
            venue,
            mode,
            side: PositionSide::Long,
            symbol: event.mint.clone(),
            symbol_display: display.clone(),
            amount_in: cost_sol,
            amount_out: qty,
            quote_symbol: "SOL".into(),
            price,
            fee: event.fee_sol,
            slippage_bps: (rule.slippage_pct.unwrap_or(cfg.copy.slippage_pct) * 100.0).round()
                as u64,
            signature: signature.clone(),
            position_id: None,
            note: Some(format!(
                "copy {} (whale {:.4} SOL)",
                rule.label.clone().unwrap_or_else(|| short(&event.leader)),
                event.sol_amount
            )),
            latency_ms: Some(latency_ms),
        };
        // TASK 5 — the typed accounting event for this mirrored fill (same
        // figures as the trade record; the deterministic copy intent id is
        // the correlation). Submitted after the position exists so the
        // event carries the position id.
        let mut ledger_event = bot_core::accounting::fill_event_for_trade(
            &trade_rec,
            self.wallet.pubkey.to_string(),
            bot_core::global_risk::strategy_label(BotModule::Copy, Some(&event.leader)),
            None,
            Some(result.intent_id.clone()),
        );
        self.state.record_trade(trade_rec.clone()).await;
        self.state.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade_rec),
        });

        // ---- Position -----------------------------------------------------
        let pos_id = self.state.next_id("p");
        ledger_event.position_id = Some(pos_id.clone());
        let mut position = Position::new(
            pos_id.clone(),
            TradeSource::Copy,
            venue,
            mode,
            event.mint.clone(),
            display,
            "SOL".into(),
        );
        position.apply_buy(qty, price, cost_sol);
        position.entry_signature = signature.clone();
        position.entry_latency_ms = Some(latency_ms);
        position.copied_wallet = Some(event.leader.clone());

        // Derive SL/TP from the risk defaults (copy config has none of its own),
        // then let the decision apply trailing/max-hold.
        let risk = &cfg.risk;
        if price > 0.0 {
            position.stop_loss = Some(price * (1.0 - risk.default_stop_loss_pct));
            position.take_profit = Some(price * (1.0 + risk.default_take_profit_pct));
        }
        decision.apply_to(&mut position);

        self.state.upsert_position(position.clone()).await;
        self.state.events.publish(AppEvent::PositionUpdate {
            ts: Utc::now(),
            position: Box::new(position),
        });
        // The global ledger is the only mutator of global accounting state;
        // the module hands over the typed event and keeps its own record.
        self.state.ledger().submit(ledger_event).await;

        // Record the copy so the cooldown applies to repeat signals.
        self.state.mark_copied(&event.leader, &event.mint).await;

        info!(
            mint = %event.mint,
            wallet = %rule.label.clone().unwrap_or_else(|| short(&event.leader)),
            qty,
            cost_sol,
            price,
            pos_id,
            "COPIED"
        );
        Ok(BuyReport {
            intent_id: result.intent_id.clone(),
            signature,
            filled: true,
            ambiguous,
            position_id: Some(pos_id),
            status: result.status,
            error: None,
            cost_sol,
            qty,
        })
    }
}

/// Durable journal row for a finished event.
pub fn journal_record(
    event: &LeaderTradeEvent,
    outcome: &CopyOutcome,
) -> bot_core::db::copy::CopyEventRecord {
    let now = Utc::now();
    bot_core::db::copy::CopyEventRecord {
        event_id: event.event_id.clone(),
        leader: event.leader.clone(),
        signature: event.signature.clone(),
        slot: event.slot,
        mint: event.mint.clone(),
        side: event.side_str().to_string(),
        venue: event.venue.as_str().to_string(),
        token_amount: event.token_amount,
        sol_amount: event.sol_amount,
        source: event.source.as_str().to_string(),
        source_sequence: event.source_sequence,
        event_at: event.block_time,
        observed_at: event.observed_at,
        stage: outcome.stage.as_str().to_string(),
        reject_reason: outcome
            .rejection
            .as_ref()
            .map(|r| r.reason.as_str().to_string()),
        detail: outcome.rejection.as_ref().map(|r| r.detail.clone()),
        intent_id: outcome.intent_id.clone(),
        position_id: outcome.position_id.clone(),
        created_at: now,
        updated_at: now,
    }
}

/// Apply a wallet's sizing rules to a whale trade (pre-TASK-3 helper, kept;
/// `sizing::size_mirror` adds the global caps and floors on top).
pub fn size_for(rule: &CopyWallet, trade: &WalletTrade) -> f64 {
    sizing::rule_size(rule, trade.sol_amount).1
}

/// Short display form of an address (first 8 chars).
pub fn short(addr: &str) -> String {
    addr.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{RejectReason, Rejection};

    fn trade(sol: f64) -> WalletTrade {
        WalletTrade {
            wallet: "whale".into(),
            signature: "sig".into(),
            slot: 1,
            block_time: None,
            side: PositionSide::Long,
            mint: "mint".into(),
            symbol: None,
            token_amount: 100.0,
            sol_amount: sol,
            venue: Venue::PumpFun,
            fee_sol: 0.0,
            discriminator: None,
            observed_at: Utc::now(),
        }
    }

    #[test]
    fn fixed_size_wins_over_fraction() {
        let rule = CopyWallet {
            address: "whale".into(),
            label: None,
            fixed_sol: Some(0.25),
            fraction_of_their_size: 0.1,
            max_sol: 10.0,
            min_sol: 0.0,
            buys_only: true,
            slippage_pct: None,
            max_staleness_secs: 0,
            paused: false,
            max_exposure_sol: 0.0,
            max_open_positions: 0,
        };
        assert!((size_for(&rule, &trade(5.0)) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn fraction_is_capped_by_max_sol() {
        let rule = CopyWallet {
            address: "whale".into(),
            label: None,
            fixed_sol: None,
            fraction_of_their_size: 0.1,
            max_sol: 0.3,
            min_sol: 0.0,
            buys_only: true,
            slippage_pct: None,
            max_staleness_secs: 0,
            paused: false,
            max_exposure_sol: 0.0,
            max_open_positions: 0,
        };
        // 10% of 5 SOL = 0.5, capped to 0.3.
        assert!((size_for(&rule, &trade(5.0)) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn fraction_without_cap() {
        let rule = CopyWallet {
            address: "whale".into(),
            label: None,
            fixed_sol: None,
            fraction_of_their_size: 0.05,
            max_sol: 0.0,
            min_sol: 0.0,
            buys_only: true,
            slippage_pct: None,
            max_staleness_secs: 0,
            paused: false,
            max_exposure_sol: 0.0,
            max_open_positions: 0,
        };
        assert!((size_for(&rule, &trade(2.0)) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn journal_record_carries_outcome_fields() {
        let e = LeaderTradeEvent::from_wallet_trade(&trade(1.0), EventSource::LogsPoll, 3);
        let outcome = CopyOutcome {
            event_id: e.event_id.clone(),
            stage: CopyStage::Filled,
            rejection: None,
            intent_id: Some("int_x".into()),
            position_id: Some("p-1".into()),
            requested_sol: Some(0.1),
            sized_sol: Some(0.1),
            signature: None,
            total_ms: 12,
        };
        let rec = journal_record(&e, &outcome);
        assert_eq!(rec.event_id, e.event_id);
        assert_eq!(rec.side, "buy");
        assert_eq!(rec.stage, "FILLED");
        assert_eq!(rec.source, "logs_poll");
        assert_eq!(rec.source_sequence, 3);
        assert_eq!(rec.intent_id.as_deref(), Some("int_x"));
        assert_eq!(rec.position_id.as_deref(), Some("p-1"));
        assert!(rec.reject_reason.is_none());
        let rejected = CopyOutcome::rejected(
            &e.event_id,
            Rejection::new(RejectReason::StaleEvent, CopyStage::PolicyPassed, "old"),
            1,
        );
        let rec = journal_record(&e, &rejected);
        assert_eq!(rec.stage, "REJECTED");
        assert_eq!(rec.reject_reason.as_deref(), Some("STALE_EVENT"));
        assert_eq!(rec.detail.as_deref(), Some("old"));
    }
}
