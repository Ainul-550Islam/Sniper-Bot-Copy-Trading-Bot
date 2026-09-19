//! Mirroring a decoded whale trade.
//!
//! A tracked wallet's swap arrives as a [`WalletTrade`]. We look up that
//! wallet's rules, size the mirror, gate it through the risk engine, and execute
//! on the same venue family: a pump.fun bonding-curve buy while the token is
//! still on the curve, otherwise a Jupiter swap (which routes across PumpSwap /
//! Raydium / etc). Whale *sells* are handled in [`crate::exit`] when
//! `copy.mirror_exits` is on.

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use bot_core::config::Config;
use bot_core::config::CopyWallet;
use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::maths;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, Trade, TradeSource, Venue, WalletTrade,
};
use bot_core::risk::EntryRequest;

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::{ExecStatus, ExecutionResult};
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::pump::{self, BuildOptions, PumpContext};
use solana_kit::tx::TxRequest;

use crate::CopyBot;

impl CopyBot {
    /// Mirror one decoded whale trade.
    pub async fn mirror_trade(&mut self, trade: &WalletTrade, cfg: &Config) -> BotResult<()> {
        // Only act on wallets we are explicitly tracking.
        let rule = match cfg
            .copy
            .wallets
            .iter()
            .find(|w| w.address == trade.wallet)
            .cloned()
        {
            Some(r) => r,
            None => {
                debug!(wallet = %trade.wallet, "trade from untracked wallet, ignoring");
                return Ok(());
            }
        };

        // Surface every tracked trade for the dashboard/telegram.
        self.state.events.publish(AppEvent::WalletTrade {
            ts: Utc::now(),
            trade: Box::new(trade.clone()),
        });

        let label = rule
            .label
            .clone()
            .unwrap_or_else(|| trade.wallet[..8.min(trade.wallet.len())].to_string());

        // ---- Whale is selling: mirror the exit if configured --------------
        if trade.side == PositionSide::Short {
            if !cfg.copy.mirror_exits {
                debug!(wallet = %label, mint = %trade.mint, "whale sold but mirror_exits is off");
                return Ok(());
            }
            if rule.buys_only {
                debug!(wallet = %label, "wallet is buys_only, not mirroring the exit");
                return Ok(());
            }
            let held = self.state.find_open(BotModule::Copy, &trade.mint).await;
            let Some(position) = held else {
                debug!(wallet = %label, mint = %trade.mint, "whale sold but we do not hold it");
                return Ok(());
            };
            let fraction = if cfg.copy.full_exit_on_their_exit {
                1.0
            } else {
                // Mirror their exit proportionally: their sold tokens over the
                // tokens we hold, clamped to a full close.
                if position.qty > 0.0 && trade.token_amount > 0.0 {
                    (trade.token_amount / position.qty).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            };
            info!(
                wallet = %label,
                mint = %trade.mint,
                fraction,
                "mirroring whale exit"
            );
            // Distributed ownership (Prompt 3 §F/§H): every replica sees the
            // same whale exit on its own feed connection — exactly one may
            // sell our position. Fail closed on store errors: the position
            // stays under the sweeper's TP/SL/trailing management meanwhile.
            let mut permit = match bot_core::ownership::Permit::acquire(
                self.ownership.as_deref(),
                format!("exit:{}:mirror_exit", position.id),
                "exit",
                "copy",
                "mirror_exit",
                &position.symbol,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        position = %position.id,
                        error = %e,
                        "mirror-exit ownership unavailable — failing closed (sweeper still manages this position)"
                    );
                    return Ok(());
                }
            };
            if !permit.proceed() {
                debug!(position = %position.id, "mirror exit owned by another replica — skipping");
                return Ok(());
            }
            let res = crate::exit::sell_position(
                &self.state,
                &self.rpc,
                &self.wallet,
                &mut self.executor,
                &self.layouts,
                &self.risk,
                &position,
                fraction,
                "whale exit (mirror)",
                None,
                self.intents.as_ref(),
                &mut permit,
            )
            .await;
            if res.is_err() {
                permit.finish(false).await;
            }
            return res;
        }

        // ---- Whale is buying ---------------------------------------------
        if trade.sol_amount < rule.min_sol {
            debug!(
                wallet = %label,
                sol = trade.sol_amount,
                min = rule.min_sol,
                "whale buy below per-wallet minimum"
            );
            return Ok(());
        }

        // Per-wallet staleness: how long since we observed the trade.
        let age = Utc::now()
            .signed_duration_since(trade.observed_at)
            .num_seconds();
        if rule.max_staleness_secs > 0 && age > rule.max_staleness_secs {
            debug!(wallet = %label, age, "whale buy too stale to mirror");
            return Ok(());
        }

        // Per-symbol reconciliation gate (§H): refuse NEW entries while this
        // symbol has unresolved claims. Exits are never gated.
        if self.state.is_symbol_blocked(&trade.mint).await {
            bot_core::obs::metrics::global()
                .counter(
                    "bot_symbol_gated_entries_total",
                    "Entries refused because the symbol is gated by unresolved reconciliation.",
                    &[("module", "copy")],
                )
                .inc();
            debug!(mint = %trade.mint, "symbol gated by unresolved reconciliation — skipping copy entry");
            return Ok(());
        }

        // Skip tokens the sniper already holds (avoid double exposure).
        if cfg.copy.skip_if_sniper_holds
            && self
                .state
                .find_open(BotModule::Sniper, &trade.mint)
                .await
                .is_some()
        {
            debug!(mint = %trade.mint, "sniper already holds this mint, skipping copy");
            return Ok(());
        }

        // Already holding it ourselves? The risk engine also blocks duplicate
        // symbols, but short-circuit here for a clearer log.
        if self
            .state
            .find_open(BotModule::Copy, &trade.mint)
            .await
            .is_some()
        {
            debug!(mint = %trade.mint, "already mirroring this mint");
            return Ok(());
        }

        // Risk gate 1: copy-specific (preflight, cooldown, staleness).
        let risk_cfg = cfg.risk.clone();
        if let Err(reason) = self.risk.check_copy(trade, &risk_cfg).await {
            debug!(wallet = %label, mint = %trade.mint, %reason, "copy gated");
            return Ok(());
        }

        // Size the mirror from the wallet's rules.
        let requested = size_for(&rule, trade);
        if requested <= 0.0 {
            debug!(wallet = %label, "computed mirror size is zero");
            return Ok(());
        }

        let available = self.available_sol().await?;
        let slippage_pct = rule.slippage_pct.unwrap_or(cfg.copy.slippage_pct);
        let slippage_bps = (slippage_pct * 100.0).round() as u64;

        // Risk gate 2: sizing + hard limits.
        let decision = self
            .risk
            .check_entry(&EntryRequest {
                module: BotModule::Copy,
                venue: trade.venue,
                symbol: trade.mint.clone(),
                symbol_display: trade.symbol.clone().unwrap_or_else(|| short(&trade.mint)),
                requested_quote: requested,
                available_quote: available,
                slippage_bps,
                price: None,
                fair_value: None,
                liquidity: None,
            })
            .await;

        if !decision.allowed() {
            self.state.inc_risk_rejected(BotModule::Copy).await;
            self.state.events.publish(AppEvent::RiskRejected {
                ts: Utc::now(),
                module: BotModule::Copy,
                symbol: short(&trade.mint),
                reason: decision.reason.clone(),
            });
            info!(mint = %trade.mint, reason = %decision.reason, "copy entry rejected by risk");
            return Ok(());
        }

        self.state.inc_signals(BotModule::Copy).await;
        self.state.events.publish(AppEvent::Signal {
            ts: Utc::now(),
            module: BotModule::Copy,
            symbol: short(&trade.mint),
            side: "buy".into(),
            reason: format!("mirror {} ({:.4} SOL)", label, trade.sol_amount),
            strength: (requested / trade.sol_amount.max(1e-9)).min(1.0),
        });

        let sized = decision.sized_quote;
        info!(
            wallet = %label,
            mint = %trade.mint,
            whale_sol = trade.sol_amount,
            sized,
            "mirroring whale buy"
        );

        // Distributed ownership (Prompt 3 §F/§H): the whale trade arrives on
        // EVERY replica's feed connection — the claim on the logical identity
        // `copy:{wallet}:{mint}` elects exactly one executor. Losers skip
        // deterministically (§G); store failures fail closed (§K). The
        // process-local cooldown (mark_copied) stays as a rate guard, but the
        // claim is authoritative cross-replica.
        let mut permit = bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("copy:{}:{}", trade.wallet, trade.mint),
            "entry",
            "copy",
            "mirror",
            &trade.mint,
        )
        .await?;
        if !permit.proceed() {
            debug!(wallet = %label, mint = %trade.mint, "copy entry owned by another replica — skipping");
            return Ok(());
        }

        let res = self
            .buy(
                trade,
                &rule,
                sized,
                slippage_pct,
                slippage_bps,
                &decision,
                &mut permit,
            )
            .await;
        if res.is_err() {
            // Pre-broadcast failure (quote/build/parse): nothing moved —
            // release so a redelivered event or later whale buy can proceed.
            permit.finish(false).await;
        }
        res
    }

    /// Execute the mirrored buy: bonding curve if still live, else Jupiter.
    #[allow(clippy::too_many_arguments)]
    async fn buy(
        &mut self,
        trade: &WalletTrade,
        rule: &CopyWallet,
        sized_sol: f64,
        slippage_pct: f64,
        slippage_bps: u64,
        decision: &bot_core::risk::RiskDecision,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<()> {
        let mint = Pubkey::try_from(trade.mint.as_str())
            .map_err(|e| BotError::invalid(format!("copy mint {}: {e}", trade.mint)))?;
        let cfg = self.state.config_snapshot().await;

        let ctx = PumpContext::load(&self.rpc, &mint, &self.wallet.pubkey, None).await?;
        if !ctx.curve.complete {
            self.buy_on_curve(
                trade,
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
                trade,
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
    #[allow(clippy::too_many_arguments)]
    async fn buy_on_curve(
        &mut self,
        trade: &WalletTrade,
        rule: &CopyWallet,
        ctx: &PumpContext,
        cfg: &Config,
        sized_sol: f64,
        slippage_pct: f64,
        decision: &bot_core::risk::RiskDecision,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<()> {
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
        let mut req = TxRequest::new(format!("copy-{}", short(&trade.mint)))
            .with_instruction(buy_ix)
            .priority_fee(cfg.execution.priority_fee_micro_lamports)
            .compute_units(cfg.execution.compute_unit_limit);
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
            &trade.mint,
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
        permit.finish(result.status.is_ambiguous()).await;

        self.record_buy(
            trade,
            rule,
            Venue::PumpFun,
            &result,
            amount,
            sol_in,
            6,
            exec_ms,
            decision,
        )
        .await
    }

    /// Jupiter buy (token graduated; routes across PumpSwap/Raydium/etc).
    #[allow(clippy::too_many_arguments)]
    async fn buy_via_jupiter(
        &mut self,
        trade: &WalletTrade,
        rule: &CopyWallet,
        mint: Pubkey,
        cfg: &Config,
        sized_sol: f64,
        slippage_bps: u64,
        decision: &bot_core::risk::RiskDecision,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<()> {
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
        // `sent` = (signature, confirmed) once a real broadcast happened.
        let sent: Option<(String, bool)> = if mode == ExecutionMode::Paper {
            None
        } else {
            let blockhash = self.rpc.latest_blockhash(true).await?.blockhash;
            let (_q, tx, _lv) = jupiter
                .build_swap(
                    &self.wallet,
                    &QuoteRequest::new(*WSOL_MINT, mint, lamports).slippage_bps(slippage_bps),
                    Some(blockhash),
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
                None
            } else {
                self.state.may_broadcast().await?;
                // Fencing (§E): ownership must still be ours at broadcast time.
                permit.fence().await?;
                let intent = crate::copy_intent(
                    &self.state,
                    &self.wallet,
                    &mint.to_string(),
                    "buy",
                    &lamports.to_string(),
                );
                let sig = bot_core::recovery::with_intent(
                    self.intents.as_ref(),
                    intent,
                    self.rpc.send_transaction(&tx),
                    |s| Some(s.to_string()),
                )
                .await?;
                let confirmed = matches!(
                    self.rpc
                        .confirm(
                            &sig,
                            std::time::Duration::from_millis(cfg.execution.confirm_timeout_ms),
                            std::time::Duration::from_millis(cfg.execution.confirm_poll_ms.max(50)),
                        )
                        .await,
                    Ok(solana_kit::rpc::ConfirmOutcome::Confirmed { .. })
                );
                Some((sig.to_string(), confirmed))
            }
        };
        let exec_ms = (Utc::now() - started).num_milliseconds().max(0) as u64;

        let signature = sent.as_ref().map(|(s, _)| s.clone());
        // Proof-of-landing decides the status (§I): an unconfirmed broadcast
        // is `Sent` (ambiguous), never optimistically `Confirmed`.
        let confirmed = sent.as_ref().map(|(_, c)| *c).unwrap_or(false);
        let result = ExecutionResult {
            signature: signature.clone().unwrap_or_default(),
            status: match mode {
                ExecutionMode::Paper => ExecStatus::PaperFilled,
                _ if sent.is_some() && confirmed => ExecStatus::Confirmed,
                _ => ExecStatus::Sent,
            },
            label: format!("copy-jup-{}", short(&trade.mint)),
            total_ms: exec_ms,
            simulate_ms: None,
            send_ms: None,
            confirm_ms: None,
            tx_size: 0,
            logs: Vec::new(),
            error: None,
            paper: mode == ExecutionMode::Paper,
            attempts: 1,
        };
        // Ownership terminal (§I/§M): only a real, unproven broadcast is
        // ambiguous; paper/simulate never moved money → release.
        permit.finish(sent.is_some() && !confirmed).await;

        self.record_buy(
            trade,
            rule,
            Venue::Jupiter,
            &result,
            out_amount,
            lamports,
            6,
            exec_ms,
            decision,
        )
        .await
    }

    /// Post-fill bookkeeping for a mirrored buy.
    #[allow(clippy::too_many_arguments)]
    async fn record_buy(
        &mut self,
        trade: &WalletTrade,
        rule: &CopyWallet,
        venue: Venue,
        result: &ExecutionResult,
        amount_raw: u64,
        sol_in: u64,
        base_decimals: u8,
        latency_ms: u64,
        decision: &bot_core::risk::RiskDecision,
    ) -> BotResult<()> {
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
        if !filled {
            self.state.inc_orders_failed(BotModule::Copy).await;
            self.state
                .record_error(
                    BotModule::Copy,
                    result.error.as_deref().unwrap_or("copy order did not fill"),
                )
                .await;
            warn!(mint = %trade.mint, status = ?result.status, "copy buy did not fill");
            return Ok(());
        }

        let qty = maths::from_raw_amount(amount_raw, base_decimals);
        let cost_sol = maths::lamports_to_sol(sol_in);
        let price = if qty > 0.0 { cost_sol / qty } else { 0.0 };
        let signature = if result.signature.is_empty() {
            None
        } else {
            Some(result.signature.clone())
        };
        let display = trade.symbol.clone().unwrap_or_else(|| short(&trade.mint));

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
            symbol: trade.mint.clone(),
            symbol_display: display.clone(),
            amount_in: cost_sol,
            amount_out: qty,
            quote_symbol: "SOL".into(),
            price,
            fee: trade.fee_sol,
            slippage_bps: (rule.slippage_pct.unwrap_or(cfg.copy.slippage_pct) * 100.0).round()
                as u64,
            signature: signature.clone(),
            position_id: None,
            note: Some(format!(
                "copy {} (whale {:.4} SOL)",
                rule.label.clone().unwrap_or_else(|| short(&trade.wallet)),
                trade.sol_amount
            )),
            latency_ms: Some(latency_ms),
        };
        self.state.record_trade(trade_rec.clone()).await;
        self.state.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade_rec),
        });

        // ---- Position -----------------------------------------------------
        let pos_id = self.state.next_id("p");
        let mut position = Position::new(
            pos_id.clone(),
            TradeSource::Copy,
            venue,
            mode,
            trade.mint.clone(),
            display,
            "SOL".into(),
        );
        position.apply_buy(qty, price, cost_sol);
        position.entry_signature = signature;
        position.entry_latency_ms = Some(latency_ms);
        position.copied_wallet = Some(trade.wallet.clone());

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

        // Record the copy so the cooldown applies to repeat signals.
        self.state.mark_copied(&trade.wallet, &trade.mint).await;

        info!(
            mint = %trade.mint,
            wallet = %rule.label.clone().unwrap_or_else(|| short(&trade.wallet)),
            qty,
            cost_sol,
            price,
            pos_id,
            "COPIED"
        );
        Ok(())
    }
}

/// Apply a wallet's sizing rules to a whale trade.
pub fn size_for(rule: &CopyWallet, trade: &WalletTrade) -> f64 {
    let base = match rule.fixed_sol {
        Some(f) if f > 0.0 => f,
        _ => trade.sol_amount * rule.fraction_of_their_size.max(0.0),
    };
    if rule.max_sol > 0.0 {
        base.min(rule.max_sol)
    } else {
        base
    }
}

/// Short display form of an address (first 8 chars).
pub fn short(addr: &str) -> String {
    addr.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        assert!((size_for(&rule, &trade(2.0)) - 0.1).abs() < 1e-9);
    }
}
