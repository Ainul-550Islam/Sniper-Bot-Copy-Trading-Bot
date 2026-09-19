//! Entry path for Module 1: turn an accepted [`TokenLaunch`] into a position.
//!
//! The hot path is a pump.fun bonding-curve buy — that is where a token lives
//! in its first seconds, which is exactly the window the sniper targets. If the
//! curve has *already* completed by the time we act (we were beaten to it, or
//! the launch was relayed late), the token has graduated and we route through
//! Jupiter instead, provided `sniper.use_jupiter_fallback` is on.
//!
//! Every step is gated:
//!   1. dedup (`mark_launch_seen`),
//!   2. static screening (`risk.check_launch_with_lists`),
//!   3. sizing + hard limits (`risk.check_entry`),
//!   4. execution (`Executor`, which honours paper/simulate/live).

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::maths;
use bot_core::models::BotModule;
use bot_core::models::{
    ExecutionMode, Position, PositionSide, TokenLaunch, Trade, TradeSource, Venue,
};
use bot_core::risk::EntryRequest;

use solana_kit::consts::WSOL_MINT;
use solana_kit::execute::ExecStatus;
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::pump::{self, BuildOptions, PumpContext};
use solana_kit::tx::TxRequest;

use crate::{available_sol, Sniper};

impl Sniper {
    /// Evaluate one launch and, if it passes every gate, buy it.
    ///
    /// Returns `Ok(())` for "handled" (including "screened out") and `Err` only
    /// for infrastructure failures worth recording against the module.
    pub async fn consider_launch(&mut self, launch: TokenLaunch) -> BotResult<()> {
        // Per-symbol reconciliation gate (§H): refuse NEW entries while this
        // symbol has unresolved claims. Exits are never gated (reducing
        // exposure is always safe). Checked before dedup so a gated symbol
        // does not consume its one-shot launch slot.
        if self.state.is_symbol_blocked(&launch.mint.to_string()).await {
            bot_core::obs::metrics::global()
                .counter(
                    "bot_symbol_gated_entries_total",
                    "Entries refused because the symbol is gated by unresolved reconciliation.",
                    &[("module", "sniper")],
                )
                .inc();
            debug!(mint = %launch.mint, "symbol gated by unresolved reconciliation — skipping entry");
            return Ok(());
        }
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let risk_cfg = cfg.risk.clone();

        // 1. Dedup: both feeds race to report the same mint; only the first wins.
        if !self.state.mark_launch_seen(&launch.mint).await {
            debug!(mint = %launch.mint, "launch already seen, ignoring duplicate");
            return Ok(());
        }

        // Latency accounting: how long between the feed observing the launch and
        // us getting here. Purely informational at this point.
        let observe_age_ms = Utc::now()
            .signed_duration_since(launch.observed_at)
            .num_milliseconds()
            .max(0) as u64;

        // 2. Static screening (denylists, creator buy, market cap, age).
        let screen = self.risk.check_launch_with_lists(
            &launch,
            &risk_cfg,
            &sniper.creator_denylist,
            &sniper.keyword_denylist,
            &[], // known-bad-creator list is not persisted yet
            0,   // we are at the tip; the launch is ~0s old
            sniper.max_launch_age_secs,
        );
        let (accepted, reason) = match &screen {
            Ok(()) => (true, None),
            Err(r) => (false, Some(r.clone())),
        };

        self.state.events.publish(AppEvent::Launch {
            ts: Utc::now(),
            launch: Box::new(launch.clone()),
            accepted,
            reason: reason.clone(),
        });

        if screen.is_err() {
            debug!(
                mint = %launch.mint,
                symbol = %launch.symbol,
                reason = ?reason,
                "launch screened out"
            );
            return Ok(());
        }

        info!(
            mint = %launch.mint,
            symbol = %launch.symbol,
            name = %launch.name,
            cap_sol = launch.market_cap_sol,
            feed = %launch.feed,
            observe_age_ms,
            "launch accepted — evaluating entry"
        );

        // 3. Resolve the mint and load the bonding curve.
        let mint = Pubkey::try_from(launch.mint.as_str())
            .map_err(|e| BotError::invalid(format!("launch mint {}: {e}", launch.mint)))?;

        // Available SOL for sizing (risk needs the real balance).
        let available = available_sol(&self.state, &self.wallet, &self.rpc).await?;

        let slippage_bps = (sniper.slippage_pct * 100.0).round() as u64;
        let decision = self
            .risk
            .check_entry(&EntryRequest {
                module: BotModule::Sniper,
                venue: Venue::PumpFun,
                symbol: launch.mint.clone(),
                symbol_display: launch.symbol.clone(),
                requested_quote: sniper.buy_sol,
                available_quote: available,
                slippage_bps,
                price: None,
                fair_value: None,
                liquidity: None,
            })
            .await;

        if !decision.allowed() {
            self.state.inc_risk_rejected(BotModule::Sniper).await;
            self.state.events.publish(AppEvent::RiskRejected {
                ts: Utc::now(),
                module: BotModule::Sniper,
                symbol: launch.symbol.clone(),
                reason: decision.reason.clone(),
            });
            info!(symbol = %launch.symbol, reason = %decision.reason, "entry rejected by risk");
            return Ok(());
        }
        self.state.inc_signals(BotModule::Sniper).await;

        let sized_sol = decision.sized_quote;
        debug!(
            symbol = %launch.symbol,
            requested = sniper.buy_sol,
            sized = sized_sol,
            verdict = ?decision.verdict,
            "entry sized"
        );

        // 3.5 Distributed execution ownership (Prompt 3 §B/§F): exactly one
        //     replica may execute this launch, no matter how many observed it.
        //     The logical identity is the mint (`snipe:{mint}`), claimed AFTER
        //     risk (a rejected entry never consumes a claim). Loser replicas
        //     skip deterministically (§G); an unavailable ownership store
        //     aborts the entry — fail closed (§K), never "proceed locally".
        let mut permit = bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("snipe:{}", launch.mint),
            "entry",
            "sniper",
            "launch",
            &launch.symbol,
        )
        .await?;
        if !permit.proceed() {
            debug!(mint = %launch.mint, "snipe owned by another replica — skipping");
            return Ok(());
        }

        // 4. Load the curve to see whether the token is still on the bonding
        //    curve or has already graduated.
        let ctx = PumpContext::load(&self.rpc, &mint, &self.wallet.pubkey, None).await?;

        if ctx.curve.complete {
            // Graduated before we could act. Fall back to Jupiter if allowed.
            if !sniper.use_jupiter_fallback {
                info!(symbol = %launch.symbol, "token already graduated and jupiter fallback is off — skipping");
                // Nothing was broadcast: determinate → release the claim.
                permit.finish(false).await;
                return Ok(());
            }
            return self
                .buy_graduated_via_jupiter(
                    &launch,
                    mint,
                    sized_sol,
                    slippage_bps,
                    observe_age_ms,
                    &mut permit,
                )
                .await;
        }

        // 5. Bonding-curve buy.
        self.buy_on_curve(
            &launch,
            &ctx,
            sized_sol,
            sniper.slippage_pct,
            observe_age_ms,
            &mut permit,
        )
        .await
    }

    /// Execute a pump.fun bonding-curve buy and record the position.
    async fn buy_on_curve(
        &mut self,
        launch: &TokenLaunch,
        ctx: &PumpContext,
        sized_sol: f64,
        slippage_pct: f64,
        observe_age_ms: u64,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<()> {
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();

        let sol_in = maths::sol_to_lamports(sized_sol);
        if sol_in == 0 {
            return Err(BotError::invalid("sized SOL rounds to zero lamports"));
        }
        let fee_bps = ctx.global_state.fee_basis_points;

        // Expected tokens out and the lamport ceiling, from the curve math.
        let (amount, max_sol_cost) = pump::plan_buy(&ctx.curve, sol_in, slippage_pct, fee_bps)?;
        if amount == 0 {
            return Err(BotError::solana(
                "bonding curve returns zero tokens for this buy — curve may be complete",
            ));
        }

        // Build the instruction through the (possibly learned) layout.
        let opts = BuildOptions {
            extra_accounts: sniper.pump_extra_accounts.clone(),
            append_bonding_curve_v2: sniper.pump_append_bonding_curve_v2,
            ..BuildOptions::default()
        };
        let buy_ix = {
            let store = self.layouts.read().await;
            pump::build_buy_ix(ctx, &store, &opts, amount, max_sol_cost)?
        };

        let mut req = TxRequest::new(format!("snipe-{}", launch.symbol))
            .with_instruction(buy_ix)
            .priority_fee(cfg.execution.priority_fee_micro_lamports)
            .compute_units(cfg.execution.compute_unit_limit);
        if cfg.execution.use_jito {
            req = req.jito_tip(cfg.execution.jito_tip_lamports);
        }

        // Honour runtime mode/gate changes made since startup.
        self.refresh_policy().await;
        // Fencing (§E): re-validate ownership immediately before the
        // journal write + broadcast — a replica whose lease was taken over
        // while building the transaction must not send it.
        permit.fence().await?;
        let started = Utc::now();
        // Write-ahead intent (§I crash point C): journaled BEFORE broadcast,
        // linked to the signature (or abandoned) immediately after.
        let intent = self.intent_rec(&ctx.mint.to_string(), "buy", &amount.to_string());
        let result = bot_core::recovery::with_intent(
            self.intents.as_ref(),
            intent,
            self.executor.run(req),
            |r| r.broadcast_signature(),
        )
        .await?;
        let exec_ms = (Utc::now() - started).num_milliseconds().max(0) as u64;
        // Ownership terminal (§I/§M): an unproven outcome (Sent/SendUnknown)
        // hands the execution to reconciliation — the claim stays blocked for
        // the grace window so no replica resubmits; a determinate outcome
        // releases it.
        permit.finish(result.status.is_ambiguous()).await;

        self.record_execution(
            launch,
            Venue::PumpFun,
            &result,
            amount,
            sol_in,
            6,
            observe_age_ms + exec_ms,
            ctx.mint,
        )
        .await
    }

    /// Buy an already-graduated token through Jupiter (WSOL → mint).
    #[allow(clippy::too_many_arguments)]
    async fn buy_graduated_via_jupiter(
        &mut self,
        launch: &TokenLaunch,
        mint: Pubkey,
        sized_sol: f64,
        slippage_bps: u64,
        observe_age_ms: u64,
        permit: &mut bot_core::ownership::Permit,
    ) -> BotResult<()> {
        let cfg = self.state.config_snapshot().await;
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
            return Err(BotError::solana(
                "jupiter found no output for the graduated buy",
            ));
        }

        let mode = self.state.execution_mode().await;
        let started = Utc::now();

        // In paper mode we never build/send: the quote is the fill.
        // `sent` = (signature, confirmed) once a real broadcast happened.
        let sent: Option<(String, bool)> = if mode == ExecutionMode::Paper {
            None
        } else {
            let blockhash = self.rpc.latest_blockhash(true).await?.blockhash;
            let (_q, tx, _last_valid) = jupiter
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
                    return Err(BotError::solana(format!("jupiter simulate failed: {err}")));
                }
                None
            } else {
                // Live (already gated by may_broadcast inside execution_mode()).
                self.state.may_broadcast().await?;
                // Fencing (§E): ownership must still be ours at broadcast time.
                permit.fence().await?;
                let intent = self.intent_rec(&mint.to_string(), "buy", &lamports.to_string());
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
        // Proof-of-landing decides the status (§I): a broadcast whose
        // confirmation was NOT observed is `Sent` (ambiguous), never
        // optimistically `Confirmed`.
        let confirmed = sent.as_ref().map(|(_, c)| *c).unwrap_or(false);
        // Synthesise an ExecutionResult-shaped record via the shared recorder.
        let result = solana_kit::execute::ExecutionResult {
            signature: signature.clone().unwrap_or_default(),
            status: if mode == ExecutionMode::Paper {
                ExecStatus::PaperFilled
            } else if sent.is_some() && confirmed {
                ExecStatus::Confirmed
            } else {
                // Live-unconfirmed AND simulate-success both land here, as
                // before: `Sent` counts as filled for bookkeeping; the permit
                // terminal above distinguishes them via `sent`.
                ExecStatus::Sent
            },
            label: format!("snipe-jup-{}", launch.symbol),
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
        // Ownership terminal (§I/§M): only a broadcast whose landing is
        // unproven is ambiguous; paper/simulate never moved money → release.
        permit.finish(sent.is_some() && !confirmed).await;

        self.record_execution(
            launch,
            Venue::Jupiter,
            &result,
            out_amount,
            lamports,
            6,
            observe_age_ms + exec_ms,
            mint,
        )
        .await
    }

    /// Shared post-execution bookkeeping: position, trade, events, counters.
    ///
    /// `amount_raw` is the base tokens received (in `base_decimals`), `sol_in`
    /// the lamports spent. For paper/simulate the plan values *are* the fill;
    /// for live they are the intended fill (a confirmed-transaction decode can
    /// refine this later).
    #[allow(clippy::too_many_arguments)]
    async fn record_execution(
        &mut self,
        launch: &TokenLaunch,
        venue: Venue,
        result: &solana_kit::execute::ExecutionResult,
        amount_raw: u64,
        sol_in: u64,
        base_decimals: u8,
        latency_ms: u64,
        mint: Pubkey,
    ) -> BotResult<()> {
        let cfg = self.state.config_snapshot().await;
        let sniper = cfg.sniper.clone();
        let mode = self.state.execution_mode().await;

        self.state.inc_orders_sent(BotModule::Sniper).await;

        // Did it work?
        let filled = matches!(
            result.status,
            ExecStatus::Confirmed
                | ExecStatus::Sent
                | ExecStatus::SendUnknown
                | ExecStatus::PaperFilled
        );
        if !filled {
            self.state.inc_orders_failed(BotModule::Sniper).await;
            self.state
                .record_error(
                    BotModule::Sniper,
                    result.error.as_deref().unwrap_or("order did not fill"),
                )
                .await;
            self.state.events.publish(AppEvent::Error {
                ts: Utc::now(),
                module: Some(BotModule::Sniper),
                message: format!(
                    "snipe {} {:?}: {}",
                    launch.symbol,
                    result.status,
                    result.error.as_deref().unwrap_or("unknown")
                ),
                fatal: false,
            });
            warn!(symbol = %launch.symbol, status = ?result.status, error = ?result.error, "snipe did not fill");
            return Ok(());
        }

        // Compute the fill economics in human units.
        let qty = maths::from_raw_amount(amount_raw, base_decimals);
        let cost_sol = maths::lamports_to_sol(sol_in);
        let price = if qty > 0.0 { cost_sol / qty } else { 0.0 }; // SOL per token

        let signature = if result.signature.is_empty() {
            None
        } else {
            Some(result.signature.clone())
        };

        self.state.events.publish(AppEvent::OrderSent {
            ts: Utc::now(),
            module: BotModule::Sniper,
            symbol: launch.symbol.clone(),
            venue: venue.as_str().to_string(),
            mode: mode.as_str().to_string(),
            quote_amount: cost_sol,
            signature: signature.clone(),
            signer: Some(self.wallet.pubkey.to_string()),
            attempts: Some(result.attempts),
            latency_ms: Some(latency_ms),
        });

        // ---- Trade record -------------------------------------------------
        let trade = Trade {
            id: self.state.next_id("t"),
            ts: Utc::now(),
            source: TradeSource::Sniper,
            venue,
            mode,
            side: PositionSide::Long,
            symbol: launch.mint.clone(),
            symbol_display: launch.symbol.clone(),
            amount_in: cost_sol,
            amount_out: qty,
            quote_symbol: "SOL".into(),
            price,
            fee: 0.0,
            slippage_bps: (sniper.slippage_pct * 100.0).round() as u64,
            signature: signature.clone(),
            position_id: None,
            note: Some(format!("feed={} {}", launch.feed, launch.name)),
            latency_ms: Some(latency_ms),
        };
        let trade_id = trade.id.clone();
        self.state.record_trade(trade.clone()).await;
        self.state.events.publish(AppEvent::Fill {
            ts: Utc::now(),
            trade: Box::new(trade),
        });

        // ---- Position -----------------------------------------------------
        let pos_id = self.state.next_id("p");
        let mut position = Position::new(
            pos_id.clone(),
            TradeSource::Sniper,
            venue,
            mode,
            launch.mint.clone(),
            launch.symbol.clone(),
            "SOL".into(),
        );
        position.apply_buy(qty, price, cost_sol);
        position.entry_signature = signature.clone();
        position.entry_latency_ms = Some(latency_ms);
        position.market_id = None;

        // Exit parameters from the sniper config, expressed as absolute price
        // levels derived from the entry price (risk.check_exit prefers these).
        if let Some(tp) = sniper.take_profit_pct {
            position.take_profit = Some(price * (1.0 + tp));
        }
        if let Some(sl) = sniper.stop_loss_pct {
            position.stop_loss = Some(price * (1.0 - sl));
        }
        position.trailing_stop = sniper.trailing_stop_pct;
        position.max_hold_secs = sniper.max_hold_secs;

        self.state.upsert_position(position.clone()).await;
        self.state.events.publish(AppEvent::PositionUpdate {
            ts: Utc::now(),
            position: Box::new(position),
        });

        // Persist the learned layout store if a live buy confirmed (the layout
        // that just worked is worth keeping). Best-effort, never fatal.
        if sniper.pump_learn_account_layout && !sniper.pump_layout_file.trim().is_empty() {
            if let Err(e) = self.maybe_learn_layout(mint, signature.as_deref()).await {
                debug!(error = %e, "layout learning skipped");
            }
        }

        info!(
            symbol = %launch.symbol,
            qty,
            cost_sol,
            price,
            latency_ms,
            status = ?result.status,
            trade_id,
            pos_id,
            "SNIPED"
        );
        Ok(())
    }

    /// After a confirmed live buy, learn the account layout from the on-chain
    /// transaction so future builds reuse exactly what worked. No-op in paper
    /// mode (there is no confirmed transaction to learn from).
    async fn maybe_learn_layout(&mut self, mint: Pubkey, signature: Option<&str>) -> BotResult<()> {
        let mode = self.state.execution_mode().await;
        if mode == ExecutionMode::Paper {
            return Ok(());
        }
        let Some(sig) = signature else { return Ok(()) };
        let sig = sig
            .parse::<solana_sdk::signature::Signature>()
            .map_err(|e| BotError::encoding(format!("signature: {e}")))?;
        let Some(confirmed) = self.rpc.get_transaction(&sig).await? else {
            return Ok(());
        };
        // Decode the account keys and find the pump buy instruction's metas.
        let decoded = solana_kit::decode::decode_swap(
            &self.wallet.pubkey,
            &sig.to_string(),
            confirmed.slot,
            confirmed.block_time,
            &confirmed.transaction.transaction,
            confirmed
                .transaction
                .meta
                .as_ref()
                .ok_or_else(|| BotError::solana("confirmed tx has no meta"))?,
        )?;
        if decoded.is_none() {
            debug!(%mint, "confirmed snipe did not decode to a swap; nothing to learn");
        }
        // The heavy lifting (mapping metas back to named accounts) lives in the
        // layout store's `learn`; we only trigger a save here so a good template
        // is not lost on restart.
        let path = self
            .state
            .config_snapshot()
            .await
            .sniper
            .pump_layout_file
            .clone();
        let store = self.layouts.read().await;
        store.save(&path).await?;
        Ok(())
    }
}
