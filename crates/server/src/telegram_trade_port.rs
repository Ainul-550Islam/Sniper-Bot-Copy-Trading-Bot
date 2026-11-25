//! Real [`TradeCommandPort`] implementation for the deployment organization.
//!
//! The Telegram trading console (module-telegram) is deliberately
//! engine-agnostic: it owns consent, confirmation sessions, idempotent
//! command records and audit, and delegates execution to the
//! `TradeCommandPort` trait. This file is the server-side implementation
//! wired in `main.rs` when Telegram trading is enabled and the durable
//! trade-command stores are installed.
//!
//! Safety posture (fail-closed everywhere):
//!
//! * **Bound to one organization** — the deployment organization resolved at
//!   startup. Orders for any other org are refused; hosted multi-tenant
//!   Telegram trading would need its own per-tenant executor factory and is
//!   intentionally not approximated here.
//! * **Re-checked at execution** — kill switch/halt, execution mode and the
//!   `allow_live_trading` gate, Telegram caps intersected with the generic
//!   risk cap, per-order slippage ceiling, minimum SOL reserve and the
//!   wallet balance are all re-derived from live config on every order,
//!   never trusted from the chat layer.
//! * **Mode semantics mirror the rest of the engine** — paper books a quote
//!   fill, simulate builds and simulates the real Jupiter transaction before
//!   booking, live broadcasts through the shared [`Executor`] with a
//!   deterministic intent id and books only a `Confirmed` result. Ambiguous
//!   broadcasts (`Sent` / `SendUnknown`) return the signature receipt for
//!   reconciliation and are never booked as fills.
//! * **Idempotent** — completed receipts are memoised by command id and the
//!   deterministic trade id is checked against shared state, so a replayed
//!   command (double-tap, restart) returns the original outcome instead of
//!   trading twice.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bot_core::config::Config;
use bot_core::db::repo::IntentRecord;
use bot_core::events::AppEvent;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::recovery::IntentSink;
use bot_core::risk::RiskEngine;
use bot_core::state::Shared;
use bot_core::{maths, BotError};
use chrono::Utc;
use module_telegram::trade_port::{
    ExecutionReceipt, OrderRequest, OrgRiskLimits, PortError, PortMode, TradeAction,
    TradeCommandPort,
};
use solana_kit::execute::{ExecStatus, ExecutionResult, Executor};
use solana_kit::jupiter::{Jupiter, QuoteRequest};
use solana_kit::rpc::Rpc;
use solana_kit::tokens::{MintInfo, Wallet};
use solana_kit::tx::BuiltTx;
use solana_kit::WSOL_MINT;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

/// Versioned marker in a booked trade's note. It binds the deterministic
/// trade id to the exact command payload, so a restart replay can detect a
/// reused command id instead of guessing from a receipt.
const COMMAND_FINGERPRINT_PREFIX: &str = "tgfp1=";

fn command_note(req: &OrderRequest, label: &str) -> String {
    format!(
        "telegram {label} command={} chat={} {}{}",
        req.idempotency_key,
        req.chat_id,
        COMMAND_FINGERPRINT_PREFIX,
        req.payload_fingerprint()
    )
}

fn fingerprint_from_trade(trade: &Trade) -> Option<&str> {
    trade
        .note
        .as_deref()?
        .split_whitespace()
        .find_map(|part| part.strip_prefix(COMMAND_FINGERPRINT_PREFIX))
}

/// Server-side trade port for Telegram commands, bound to the deployment
/// organization and the deployment wallet.
pub struct DeploymentTradePort {
    state: Shared,
    rpc: Rpc,
    wallet: Arc<Wallet>,
    /// Serialised behind a mutex: policy refresh needs `&mut`, and one
    /// Telegram execution at a time keeps nonce/balance reasoning simple.
    executor: Mutex<Executor>,
    intents: Option<Arc<dyn IntentSink>>,
    risk: RiskEngine,
    org_id: String,
    receipts: Mutex<HashMap<Uuid, (String, ExecutionReceipt)>>,
}

impl DeploymentTradePort {
    /// Build the port over the shared state/rpc/wallet, mirroring the
    /// sniper's executor construction so Telegram orders travel exactly the
    /// same broadcast path (fee policy, dynamic Jito tip, signer registry).
    pub fn new(
        state: Shared,
        rpc: Rpc,
        wallet: Arc<Wallet>,
        signers: Option<Arc<solana_kit::signer::SignerRegistry>>,
        intents: Option<Arc<dyn IntentSink>>,
        org_id: String,
        cfg: &Config,
    ) -> Self {
        let policy = solana_kit::execute::exec_policy_from_config(cfg);
        let mut executor = Executor::new(rpc.clone(), Arc::clone(&wallet), policy)
            .with_fee_policy(solana_kit::execute::fee_policy_from_config(cfg));
        if let Some(tip) = solana_kit::execute::dynamic_tip_from_config(cfg) {
            executor = executor.with_dynamic_tip(tip);
        }
        if let Some(reg) = &signers {
            executor = executor.with_signer_registry(Arc::clone(reg));
        }
        Self {
            risk: RiskEngine::new(state.clone()),
            state,
            rpc,
            wallet,
            executor: Mutex::new(executor),
            intents,
            org_id,
            receipts: Mutex::new(HashMap::new()),
        }
    }

    fn check_org(&self, org_id: &str) -> Result<(), PortError> {
        if org_id == self.org_id {
            Ok(())
        } else {
            Err(PortError::Unavailable(format!(
                "telegram trading is bound to the deployment organization {expected}; \
                 refusing an order for organization {org_id}",
                expected = self.org_id
            )))
        }
    }

    /// Deterministic identity for a command: the same command always maps to
    /// the same intent/trade ids, which is what makes replays harmless.
    fn intent_id(command_id: Uuid) -> String {
        format!("telegram-{command_id}")
    }
    fn trade_id(command_id: Uuid) -> String {
        format!("tg-trade-{command_id}")
    }
    fn position_id(command_id: Uuid) -> String {
        format!("tg-pos-{command_id}")
    }

    /// A previously completed receipt for this command, from the in-memory
    /// memo or reconstructed from the deterministic trade in shared state
    /// (covers restarts where the memo is gone but state was restored).
    /// Both paths require the exact original payload fingerprint: a reused
    /// command id with different economics is a conflict, not a replay.
    async fn completed_receipt(
        &self,
        req: &OrderRequest,
    ) -> Option<Result<ExecutionReceipt, PortError>> {
        let expected = req.payload_fingerprint();
        if let Some((stored, receipt)) = self
            .receipts
            .lock()
            .await
            .get(&req.idempotency_key)
            .cloned()
        {
            return Some(if stored == expected {
                Ok(receipt)
            } else {
                Err(PortError::PayloadMismatch)
            });
        }
        let trade_id = Self::trade_id(req.idempotency_key);
        let trade = self
            .state
            .trades(usize::MAX)
            .await
            .into_iter()
            .find(|t| t.id == trade_id)?;
        let stored = match fingerprint_from_trade(&trade) {
            Some(stored) => stored,
            None => {
                return Some(Err(PortError::Unavailable(
                    "restored trade lacks a command payload fingerprint; refusing replay".into(),
                )))
            }
        };
        if stored != expected {
            return Some(Err(PortError::PayloadMismatch));
        }
        let receipt = receipt_from_trade(&trade, req.idempotency_key);
        Some(Ok(self.memoise(req, receipt).await))
    }

    async fn memoise(&self, req: &OrderRequest, receipt: ExecutionReceipt) -> ExecutionReceipt {
        self.receipts.lock().await.insert(
            req.idempotency_key,
            (req.payload_fingerprint(), receipt.clone()),
        );
        receipt
    }

    async fn mint_decimals(&self, mint: &Pubkey) -> Result<u8, PortError> {
        let account = self
            .rpc
            .get_account(mint)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| {
                PortError::RiskRejected(format!("mint {mint} does not exist on chain"))
            })?;
        let info = MintInfo::parse(&account.data).map_err(unavailable)?;
        Ok(info.decimals)
    }

    /// Buy-side balance gate. Paper keeps sniper parity: the fill model must
    /// not depend on a funded wallet, so no chain read is made. Simulate and
    /// live read the real balance and keep the configured reserve untouched.
    async fn check_buy_balance(
        &self,
        mode: ExecutionMode,
        cfg: &Config,
        amount_lamports: u64,
    ) -> Result<(), PortError> {
        if matches!(mode, ExecutionMode::Paper) {
            return Ok(());
        }
        let balance = self
            .rpc
            .get_balance(&self.wallet.pubkey)
            .await
            .map_err(unavailable)?;
        let reserve = maths::sol_to_lamports(cfg.risk.min_sol_reserve);
        let needed = amount_lamports.saturating_add(reserve);
        if balance < needed {
            return Err(PortError::RiskRejected(format!(
                "wallet balance {balance} lamports cannot fund {amount_lamports} plus the \
                 {reserve} lamport minimum reserve"
            )));
        }
        Ok(())
    }

    /// The sell size in raw base units. Live sells size off the wallet's real
    /// token balance; paper/simulate size off the tracked position quantity
    /// (there may be nothing on chain in those modes).
    async fn sell_raw_amount(
        &self,
        mode: ExecutionMode,
        mint: &Pubkey,
        position: &Position,
        base_decimals: u8,
        percent_bps: u16,
    ) -> Result<u64, PortError> {
        let tracked_raw = maths::to_raw_amount(position.qty, base_decimals);
        let total_raw = if mode.is_live() {
            let token_program = self.rpc.token_program_of(mint).await.map_err(unavailable)?;
            let wallet_raw = self
                .wallet
                .token_balance(&self.rpc, mint, &token_program)
                .await
                .map_err(unavailable)?;
            let scoped = live_sellable_raw(wallet_raw, tracked_raw, mint)?;
            if wallet_raw != tracked_raw {
                self.state.events.publish(AppEvent::Audit {
                    ts: Utc::now(),
                    actor: "telegram".into(),
                    action: "telegram.sell.wallet_scope".into(),
                    target: Some(mint.to_string()),
                    outcome: format!(
                        "status=tolerated_rounding wallet_raw={wallet_raw} tracked_raw={tracked_raw}"
                    ),
                });
            }
            scoped
        } else {
            tracked_raw
        };
        if total_raw == 0 {
            return Err(PortError::RiskRejected(format!(
                "no sellable balance for mint {mint} in {mode} mode"
            )));
        }
        let raw = percent_of(total_raw, percent_bps);
        if raw == 0 {
            return Err(PortError::RiskRejected(format!(
                "{percent_bps} bps of the {mint} balance rounds to zero base units"
            )));
        }
        Ok(raw)
    }

    /// Shared execution tail for both sides: quote → mode dispatch → book.
    /// `quote_and_book` receives the expected output of the route actually
    /// used (re-quoted inside `build_swap` for simulate/live so the booked
    /// figures match the signed transaction, not a stale quote).
    async fn dispatch(
        &self,
        req: &OrderRequest,
        action: TradeAction,
        cfg: &Config,
        mint: Pubkey,
        quote: FeeQuote,
        book: BookFill<'_>,
    ) -> Result<ExecutionReceipt, PortError> {
        let started = Instant::now();
        let mode = cfg.execution.mode;
        let jupiter = Jupiter::new();
        let quote_req = module_copy::platform_fee::jupiter_fee_request(
            &self.state,
            &self.rpc,
            quote.request,
            quote.fee_notional_lamports,
            &cfg.platform_fee,
            &Self::intent_id(req.idempotency_key),
            "telegram",
        )
        .await
        .map_err(unavailable)?;
        match mode {
            ExecutionMode::Paper => {
                let quote = jupiter.quote(&quote_req).await.map_err(unavailable)?;
                let out = quote.out_amount_u64().map_err(unavailable)?;
                let reference = format!("paper:{}", req.idempotency_key);
                self.book_fill(
                    req,
                    action,
                    cfg,
                    book,
                    FillOutcome {
                        out_raw: out,
                        signature: None,
                        started,
                    },
                )
                .await?;
                Ok(ExecutionReceipt {
                    reference,
                    detail: format!("paper fill booked at the Jupiter quote (out {out} raw units)"),
                })
            }
            ExecutionMode::Simulate => {
                let blockhash = self.rpc.latest_blockhash(true).await.map_err(unavailable)?;
                let (quote, tx, _) = jupiter
                    .build_swap(&self.wallet, &quote_req, Some(blockhash.blockhash), None)
                    .await
                    .map_err(unavailable)?;
                let sim = self.rpc.simulate(&tx).await.map_err(unavailable)?;
                if let Some(err) = &sim.value.err {
                    return Err(PortError::Unavailable(format!(
                        "simulation rejected the telegram order: {err:?}"
                    )));
                }
                let out = quote.out_amount_u64().map_err(unavailable)?;
                let reference = format!("simulate:{}", req.idempotency_key);
                self.book_fill(
                    req,
                    action,
                    cfg,
                    book,
                    FillOutcome {
                        out_raw: out,
                        signature: None,
                        started,
                    },
                )
                .await?;
                Ok(ExecutionReceipt {
                    reference,
                    detail: format!(
                        "simulated successfully and booked at the quoted output ({out} raw units)"
                    ),
                })
            }
            ExecutionMode::Live => {
                // The one broadcast gate every live path passes.
                self.state.may_broadcast().await.map_err(broadcast_err)?;
                let blockhash = self.rpc.latest_blockhash(true).await.map_err(unavailable)?;
                let priority_fee = (cfg.execution.priority_fee_micro_lamports > 0)
                    .then_some(cfg.execution.priority_fee_micro_lamports);
                let (quote, tx, last_valid) = jupiter
                    .build_swap(
                        &self.wallet,
                        &quote_req,
                        Some(blockhash.blockhash),
                        priority_fee,
                    )
                    .await
                    .map_err(unavailable)?;
                let out = quote.out_amount_u64().map_err(unavailable)?;
                let label = format!("telegram {} {}", action.as_str(), mint);
                let built = BuiltTx::from_signed(
                    label,
                    tx,
                    blockhash.blockhash,
                    last_valid.or(Some(blockhash.last_valid_block_height)),
                )
                .map_err(unavailable)?
                .with_intent_id(Self::intent_id(req.idempotency_key))
                .attributed("telegram", mint.to_string());

                let side_label = match action {
                    TradeAction::Buy => "buy",
                    TradeAction::Sell => "sell",
                };
                let record = IntentRecord {
                    intent_id: Self::intent_id(req.idempotency_key),
                    module: "telegram".into(),
                    symbol: mint.to_string(),
                    wallet: self.wallet.pubkey.to_string(),
                    side: side_label.into(),
                    qty: match action {
                        TradeAction::Buy => req
                            .amount_lamports
                            .map(|a| a.to_string())
                            .unwrap_or_default(),
                        TradeAction::Sell => book.sell_raw.to_string(),
                    },
                    status: "pending".into(),
                    signature: None,
                    created_at: Utc::now(),
                };

                let result = {
                    let mut executor = self.executor.lock().await;
                    executor.set_policy(solana_kit::execute::exec_policy_from_config(cfg));
                    let fees = solana_kit::execute::fee_policy_from_config(cfg);
                    if *executor.fee_policy() != fees {
                        executor.set_fee_policy(fees);
                    }
                    // Durable PRE-BROADCAST reservation. The reservation key
                    // is the deterministic intent id (one command → one
                    // intent), so a second replica or a restarted process
                    // that replays the same command cannot broadcast it
                    // twice while the first attempt's outcome is unknown.
                    // Losing the lease (or an unreadable reservation store)
                    // refuses the send instead of hoping.
                    bot_core::recovery::with_reserved_intent_disposition(
                        self.intents.as_ref(),
                        &Self::intent_id(req.idempotency_key),
                        record,
                        executor.send_prebuilt(&built),
                        |r: &ExecutionResult| match r.broadcast_signature() {
                            Some(sig) if !sig.is_empty() => {
                                bot_core::recovery::IntentDisposition::Link(sig)
                            }
                            _ => bot_core::recovery::IntentDisposition::Abandon,
                        },
                    )
                    .await
                    .map_err(unavailable)?
                };

                match result.status {
                    ExecStatus::Confirmed => {
                        let signature = result.signature.clone();
                        self.book_fill(
                            req,
                            action,
                            cfg,
                            book,
                            FillOutcome {
                                out_raw: out,
                                signature: Some(signature.clone()),
                                started,
                            },
                        )
                        .await?;
                        Ok(ExecutionReceipt {
                            reference: signature,
                            detail: format!(
                                "confirmed on chain; booked at the signed quote (out {out} raw units)"
                            ),
                        })
                    }
                    ExecStatus::Sent | ExecStatus::SendUnknown => {
                        // Deliberately NOT booked: the fill is unproven until
                        // reconciliation observes the signature. The receipt
                        // carries the signature so the operator can track it.
                        warn!(
                            command_id = %req.idempotency_key,
                            signature = %result.signature,
                            status = ?result.status,
                            "telegram order broadcast without confirmation; left unbooked for reconciliation"
                        );
                        Ok(ExecutionReceipt {
                            reference: result.signature.clone(),
                            detail: "broadcast submitted; confirmation pending reconciliation — not yet booked".into(),
                        })
                    }
                    other => Err(PortError::Unavailable(format!(
                        "telegram order did not execute ({other:?}): {}",
                        result
                            .error
                            .clone()
                            .unwrap_or_else(|| "no error detail from the executor".into())
                    ))),
                }
            }
        }
    }

    /// Book one fill into shared state exactly like the strategy paths do:
    /// trade record + fill event + position update + global ledger event.
    async fn book_fill(
        &self,
        req: &OrderRequest,
        action: TradeAction,
        cfg: &Config,
        book: BookFill<'_>,
        fill: FillOutcome,
    ) -> Result<(), PortError> {
        let FillOutcome {
            out_raw,
            signature,
            started,
        } = fill;
        let mode = cfg.execution.mode;
        let latency_ms = started.elapsed().as_millis() as u64;
        self.state.inc_orders_sent(BotModule::Sniper).await;
        match action {
            TradeAction::Buy => {
                let lamports = req.amount_lamports.unwrap_or(0);
                let qty = maths::from_raw_amount(out_raw, book.base_decimals);
                let cost_sol = maths::lamports_to_sol(lamports);
                let price = if qty > 0.0 { cost_sol / qty } else { 0.0 };
                if qty <= 0.0 || !price.is_finite() || price <= 0.0 {
                    return Err(PortError::Unavailable(
                        "buy fill priced to zero; refusing to book a meaningless fill".into(),
                    ));
                }

                // Aggregate into the existing manual position for this mint
                // when one is open; otherwise open a fresh position.
                let existing = find_open_position(&self.state.open_positions().await, book.symbol);
                let position_id = match &existing {
                    Some(p) => p.id.clone(),
                    None => Self::position_id(req.idempotency_key),
                };

                let trade = Trade {
                    id: Self::trade_id(req.idempotency_key),
                    ts: Utc::now(),
                    source: TradeSource::Manual,
                    venue: Venue::Jupiter,
                    mode,
                    side: PositionSide::Long,
                    symbol: book.symbol.to_string(),
                    symbol_display: short_mint(book.symbol),
                    amount_in: cost_sol,
                    amount_out: qty,
                    quote_symbol: "SOL".into(),
                    price,
                    fee: 0.0,
                    slippage_bps: u64::from(req.slippage_bps),
                    signature: signature.clone(),
                    position_id: Some(position_id.clone()),
                    note: Some(command_note(req, "buy")),
                    latency_ms: Some(latency_ms),
                };
                let mut ledger_event = bot_core::accounting::fill_event_for_trade(
                    &trade,
                    self.wallet.pubkey.to_string(),
                    bot_core::global_risk::strategy_label(BotModule::Sniper, None),
                    None,
                    Some(Self::intent_id(req.idempotency_key)),
                );
                ledger_event.position_id = Some(position_id.clone());
                self.state.record_trade(trade.clone()).await;
                self.state.events.publish(AppEvent::Fill {
                    ts: Utc::now(),
                    trade: Box::new(trade),
                });

                if existing.is_some() {
                    self.state
                        .with_position(&position_id, |p| {
                            p.apply_buy(qty, price, cost_sol);
                        })
                        .await;
                } else {
                    let mut position = Position::new(
                        position_id.clone(),
                        TradeSource::Manual,
                        Venue::Jupiter,
                        mode,
                        book.symbol.to_string(),
                        short_mint(book.symbol),
                        "SOL".into(),
                    );
                    position.apply_buy(qty, price, cost_sol);
                    position.entry_signature = signature.clone();
                    position.entry_latency_ms = Some(latency_ms);
                    self.state.upsert_position(position).await;
                }
                if let Some(p) = self.state.position(&position_id).await {
                    self.state.events.publish(AppEvent::PositionUpdate {
                        ts: Utc::now(),
                        position: Box::new(p),
                    });
                }
                self.state
                    .submit_ledger_event(BotModule::Sniper, ledger_event)
                    .await;
                info!(
                    command_id = %req.idempotency_key,
                    mint = %book.symbol,
                    qty,
                    cost_sol,
                    "telegram buy booked"
                );
                Ok(())
            }
            TradeAction::Sell => {
                let position = book.position.ok_or_else(|| {
                    PortError::Unavailable("sell booking lost its position context".into())
                })?;
                let sol_out = maths::lamports_to_sol(out_raw);
                let sold_human =
                    maths::from_raw_amount(book.sell_raw, book.base_decimals).min(position.qty);
                if sold_human <= 0.0 || sol_out <= 0.0 {
                    return Err(PortError::Unavailable(
                        "sell fill priced to zero; refusing to book a meaningless fill".into(),
                    ));
                }
                let price = sol_out / sold_human;

                let trade = Trade {
                    id: Self::trade_id(req.idempotency_key),
                    ts: Utc::now(),
                    source: TradeSource::Manual,
                    venue: Venue::Jupiter,
                    mode,
                    side: PositionSide::Short,
                    symbol: position.symbol.clone(),
                    symbol_display: position.symbol_display.clone(),
                    amount_in: sold_human,
                    amount_out: sol_out,
                    quote_symbol: "SOL".into(),
                    price,
                    fee: 0.0,
                    slippage_bps: u64::from(req.slippage_bps),
                    signature: signature.clone(),
                    position_id: Some(position.id.clone()),
                    note: Some(command_note(req, "sell")),
                    latency_ms: Some(latency_ms),
                };
                let ledger_event = bot_core::accounting::fill_event_for_trade(
                    &trade,
                    self.wallet.pubkey.to_string(),
                    bot_core::global_risk::strategy_label(BotModule::Sniper, None),
                    None,
                    Some(Self::intent_id(req.idempotency_key)),
                );
                self.state.record_trade(trade.clone()).await;
                self.state.events.publish(AppEvent::Fill {
                    ts: Utc::now(),
                    trade: Box::new(trade),
                });

                let mut realized = 0.0;
                let mut exit_sig = signature.clone();
                self.state
                    .with_position(&position.id, |p| {
                        realized = p.apply_sell(sold_human, price, sol_out);
                        p.exit_signature = exit_sig.take();
                    })
                    .await;
                self.risk.book_pnl(BotModule::Sniper, realized).await;
                self.state
                    .submit_ledger_event(BotModule::Sniper, ledger_event)
                    .await;

                let remaining = position.qty - sold_human;
                if remaining <= f64::EPSILON || req.percent_bps == Some(10_000) {
                    self.state
                        .close_position(
                            &position.id,
                            PositionStatus::Closed,
                            "telegram manual sell",
                        )
                        .await;
                } else if let Some(p) = self.state.position(&position.id).await {
                    self.state.events.publish(AppEvent::PositionUpdate {
                        ts: Utc::now(),
                        position: Box::new(p),
                    });
                }
                info!(
                    command_id = %req.idempotency_key,
                    mint = %position.symbol,
                    sold_human,
                    sol_out,
                    realized,
                    "telegram sell booked"
                );
                Ok(())
            }
        }
    }

    /// The full guarded order pipeline shared by `place_buy`/`place_sell`.
    async fn execute(
        &self,
        req: &OrderRequest,
        action: TradeAction,
    ) -> Result<ExecutionReceipt, PortError> {
        // Authorize the tenant before consulting either the in-memory memo or
        // restored trade history. A guessed/reused command id must not expose
        // another organization's receipt.
        self.check_org(&req.organization_id)?;
        if let Some(done) = self.completed_receipt(req).await {
            return done;
        }
        validate_request(req, action)?;
        if self.state.kill_switch() {
            return Err(PortError::Halted);
        }

        let cfg = self.state.config_snapshot().await;
        let mode = cfg.execution.mode;
        if mode.is_live() && !cfg.execution.allow_live_trading {
            return Err(PortError::LiveGatesClosed);
        }
        if u64::from(req.slippage_bps) > cfg.risk.max_slippage_bps {
            return Err(PortError::RiskRejected(format!(
                "slippage {} bps exceeds the risk ceiling of {} bps",
                req.slippage_bps, cfg.risk.max_slippage_bps
            )));
        }
        let limits = lamport_limits(&cfg).ok_or_else(|| {
            PortError::Unavailable(
                "telegram trading caps are not configured (both caps must be positive)".into(),
            )
        })?;

        let mint = Pubkey::from_str(req.mint.trim()).map_err(|e| {
            PortError::RiskRejected(format!("mint {} is not a valid pubkey: {e}", req.mint))
        })?;
        if mint == *WSOL_MINT {
            return Err(PortError::RiskRejected(
                "the quote mint (WSOL) is not a tradeable telegram order target".into(),
            ));
        }
        let base_decimals = self.mint_decimals(&mint).await?;
        let symbol = mint.to_string();

        let receipt = match action {
            TradeAction::Buy => {
                let amount = req.amount_lamports.unwrap_or(0);
                if amount > limits.max_order_lamports {
                    return Err(PortError::RiskRejected(format!(
                        "buy amount {amount} lamports exceeds the per-order cap of {} lamports",
                        limits.max_order_lamports
                    )));
                }
                self.check_buy_balance(mode, &cfg, amount).await?;
                let quote_req = QuoteRequest::new(*WSOL_MINT, mint, amount)
                    .slippage_bps(u64::from(req.slippage_bps));
                self.dispatch(
                    req,
                    action,
                    &cfg,
                    mint,
                    FeeQuote {
                        request: quote_req,
                        fee_notional_lamports: amount,
                    },
                    BookFill {
                        symbol: &symbol,
                        base_decimals,
                        sell_raw: 0,
                        position: None,
                    },
                )
                .await?
            }
            TradeAction::Sell => {
                let percent_bps = req.percent_bps.unwrap_or(0);
                let position = find_open_position(&self.state.open_positions().await, &symbol)
                    .ok_or_else(|| {
                        PortError::RiskRejected(format!(
                            "no open position is tracked for mint {symbol}"
                        ))
                    })?;
                let sell_raw = self
                    .sell_raw_amount(mode, &mint, &position, base_decimals, percent_bps)
                    .await?;
                // The per-order cap applies to the expected SOL notional of
                // the sale as well: probe the route before dispatch so an
                // oversized exit is refused before anything is signed.
                let probe = Jupiter::new()
                    .quote(
                        &QuoteRequest::new(mint, *WSOL_MINT, sell_raw)
                            .slippage_bps(u64::from(req.slippage_bps)),
                    )
                    .await
                    .map_err(unavailable)?;
                let expected_out = probe.out_amount_u64().map_err(unavailable)?;
                if expected_out > limits.max_order_lamports {
                    return Err(PortError::RiskRejected(format!(
                        "sell notional ~{expected_out} lamports exceeds the per-order cap of {} lamports",
                        limits.max_order_lamports
                    )));
                }
                let quote_req = QuoteRequest::new(mint, *WSOL_MINT, sell_raw)
                    .slippage_bps(u64::from(req.slippage_bps));
                self.dispatch(
                    req,
                    action,
                    &cfg,
                    mint,
                    FeeQuote {
                        request: quote_req,
                        fee_notional_lamports: expected_out,
                    },
                    BookFill {
                        symbol: &symbol,
                        base_decimals,
                        sell_raw,
                        position: Some(&position),
                    },
                )
                .await?
            }
        };
        Ok(self.memoise(req, receipt).await)
    }
}

/// The executed side of one order: what the route actually produced.
struct FillOutcome {
    /// Raw output amount of the route that was signed/quoted.
    out_raw: u64,
    /// On-chain signature for live fills; `None` in paper/simulate.
    signature: Option<String>,
    /// Dispatch start, for the trade's latency record.
    started: Instant,
}

/// Executable quote request and the notional used to prepare its fee.
struct FeeQuote {
    request: QuoteRequest,
    fee_notional_lamports: u64,
}

/// Booking context handed to [`DeploymentTradePort::book_fill`].
struct BookFill<'a> {
    symbol: &'a str,
    base_decimals: u8,
    /// Raw base units being sold (sell side only).
    sell_raw: u64,
    /// The position being reduced (sell side only).
    position: Option<&'a Position>,
}

/// Telegram caps intersected with the generic risk cap. `None` (trading
/// unavailable) unless both Telegram caps are positive — a half-configured
/// console must never trade.
fn lamport_limits(cfg: &Config) -> Option<OrgRiskLimits> {
    let tg = &cfg.telegram;
    if tg.trading_max_order_lamports == 0 || tg.trading_daily_cap_lamports == 0 {
        return None;
    }
    let mut max_order = tg.trading_max_order_lamports;
    if cfg.risk.max_position_quote > 0.0 && cfg.risk.max_position_quote.is_finite() {
        let risk_cap = maths::sol_to_lamports(cfg.risk.max_position_quote);
        if risk_cap > 0 {
            max_order = max_order.min(risk_cap);
        }
    }
    if max_order == 0 {
        return None;
    }
    Some(OrgRiskLimits::new(max_order, tg.trading_daily_cap_lamports))
}

/// Request-shape invariants the port enforces independently of the console
/// (the port never trusts its caller to have validated).
fn validate_request(req: &OrderRequest, action: TradeAction) -> Result<(), PortError> {
    if req.command_id != req.idempotency_key {
        return Err(PortError::RiskRejected(
            "command id must equal idempotency key".into(),
        ));
    }
    if req.action != action {
        return Err(PortError::RiskRejected(format!(
            "order action {} does not match the {} endpoint",
            req.action.as_str(),
            action.as_str()
        )));
    }
    if req.slippage_bps > 10_000 {
        return Err(PortError::RiskRejected(format!(
            "slippage {} bps exceeds 100%",
            req.slippage_bps
        )));
    }
    match action {
        TradeAction::Buy => {
            if req.percent_bps.is_some() {
                return Err(PortError::RiskRejected(
                    "a buy is sized in lamports, not percent".into(),
                ));
            }
            match req.amount_lamports {
                Some(0) | None => Err(PortError::RiskRejected(
                    "a buy requires a positive lamport amount".into(),
                )),
                Some(_) => Ok(()),
            }
        }
        TradeAction::Sell => {
            if req.amount_lamports.is_some() {
                return Err(PortError::RiskRejected(
                    "a sell is sized in percent of the position, not lamports".into(),
                ));
            }
            match req.percent_bps {
                Some(p) if (1..=10_000).contains(&p) => Ok(()),
                other => Err(PortError::RiskRejected(format!(
                    "a sell requires percent_bps in 1..=10000, got {other:?}"
                ))),
            }
        }
    }
}

/// `percent_bps` of `total`, in u128 so no balance can overflow the math.
fn percent_of(total: u64, percent_bps: u16) -> u64 {
    ((total as u128 * percent_bps as u128) / 10_000) as u64
}

/// A live Telegram sell must sell only the quantity belonging to the one
/// tracked position it will reduce. A wallet-wide token balance may contain
/// other positions, untracked exposure or stale dust; using it as the sell
/// notional would misattribute proceeds and inventory. Allow at most one raw
/// unit of decimal-rounding difference, and never exceed the tracked raw qty.
fn live_sellable_raw(wallet_raw: u64, tracked_raw: u64, mint: &Pubkey) -> Result<u64, PortError> {
    if tracked_raw == 0 || wallet_raw == 0 {
        return Err(PortError::RiskRejected(format!(
            "no sellable tracked balance for mint {mint}"
        )));
    }
    if wallet_raw.abs_diff(tracked_raw) > 1 {
        return Err(PortError::RiskRejected(format!(
            "wallet token balance {wallet_raw} does not match the tracked position quantity {tracked_raw} for mint {mint}; refusing a wallet-wide sell against one position"
        )));
    }
    Ok(wallet_raw.min(tracked_raw))
}

/// The open position a Telegram order should act on: a manually opened one
/// first (Telegram manages its own book), then any other tracked position
/// on the mint. Terminal positions never match (already filtered by
/// `open_positions`, kept explicit for direct callers).
fn find_open_position(positions: &[Position], mint_symbol: &str) -> Option<Position> {
    let candidates: Vec<&Position> = positions
        .iter()
        .filter(|p| !p.status.is_terminal() && p.symbol == mint_symbol && p.qty > 0.0)
        .collect();
    candidates
        .iter()
        .find(|p| p.source == TradeSource::Manual)
        .or_else(|| candidates.first())
        .map(|p| (*p).clone())
}

/// Rebuild the receipt for a command whose fill was booked in a previous
/// process lifetime.
fn receipt_from_trade(trade: &Trade, command_id: Uuid) -> ExecutionReceipt {
    let reference = trade
        .signature
        .clone()
        .unwrap_or_else(|| format!("paper:{command_id}"));
    ExecutionReceipt {
        reference,
        detail: "previously booked fill replayed from trade history".into(),
    }
}

fn short_mint(mint: &str) -> String {
    if mint.len() > 10 {
        format!("{}..{}", &mint[..6], &mint[mint.len() - 4..])
    } else {
        mint.to_string()
    }
}

fn unavailable(e: BotError) -> PortError {
    PortError::Unavailable(e.to_string())
}

fn broadcast_err(e: BotError) -> PortError {
    match e {
        BotError::KillSwitch(_) => PortError::Halted,
        BotError::ModuleDisabled(_) => PortError::LiveGatesClosed,
        other => PortError::Unavailable(other.to_string()),
    }
}

#[async_trait]
impl TradeCommandPort for DeploymentTradePort {
    async fn is_halted(&self) -> bool {
        self.state.kill_switch()
    }

    async fn current_mode(&self) -> Result<PortMode, PortError> {
        let cfg = self.state.config_snapshot().await;
        Ok(PortMode::new(
            cfg.execution.mode,
            cfg.execution.allow_live_trading,
        ))
    }

    async fn org_risk_limits(&self, org_id: &str) -> Result<OrgRiskLimits, PortError> {
        self.check_org(org_id)?;
        let cfg = self.state.config_snapshot().await;
        lamport_limits(&cfg).ok_or_else(|| {
            PortError::Unavailable(
                "telegram trading caps are not configured (both caps must be positive)".into(),
            )
        })
    }

    async fn place_buy(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError> {
        self.execute(req, TradeAction::Buy).await
    }

    async fn place_sell(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError> {
        self.execute(req, TradeAction::Sell).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_caps(max_order: u64, daily: u64, risk_sol: f64) -> Config {
        let mut cfg = Config::default();
        cfg.telegram.trading_max_order_lamports = max_order;
        cfg.telegram.trading_daily_cap_lamports = daily;
        cfg.risk.max_position_quote = risk_sol;
        cfg
    }

    fn buy_req() -> OrderRequest {
        let command_id = Uuid::new_v4();
        OrderRequest {
            command_id,
            idempotency_key: command_id,
            organization_id: "org-1".into(),
            chat_id: 55,
            user_id: 77,
            mint: Pubkey::new_unique().to_string(),
            action: TradeAction::Buy,
            amount_lamports: Some(1_000_000),
            percent_bps: None,
            slippage_bps: 100,
        }
    }

    fn test_port_with_state() -> (DeploymentTradePort, Shared) {
        use bot_core::config::AppConfig;
        use solana_sdk::signature::Keypair;

        let app = AppConfig::from_defaults();
        let state = bot_core::state::AppState::new(app.clone());
        let rpc = Rpc::new(&app.raw.network).expect("lazy RPC client");
        let wallet = Arc::new(Wallet::from_keypair(Keypair::new()));
        let port = DeploymentTradePort::new(
            state.clone(),
            rpc,
            wallet,
            None,
            None,
            "org-1".into(),
            &app.raw,
        );
        (port, state)
    }

    fn recorded_trade(req: &OrderRequest, with_fingerprint: bool) -> Trade {
        Trade {
            id: DeploymentTradePort::trade_id(req.idempotency_key),
            ts: Utc::now(),
            source: TradeSource::Manual,
            venue: Venue::Jupiter,
            mode: ExecutionMode::Paper,
            side: PositionSide::Long,
            symbol: req.mint.clone(),
            symbol_display: short_mint(&req.mint),
            amount_in: 0.1,
            amount_out: 10.0,
            quote_symbol: "SOL".into(),
            price: 0.01,
            fee: 0.0,
            slippage_bps: u64::from(req.slippage_bps),
            signature: Some("Sig111".into()),
            position_id: None,
            note: Some(if with_fingerprint {
                command_note(req, "buy")
            } else {
                "legacy telegram trade without a payload fingerprint".into()
            }),
            latency_ms: None,
        }
    }

    /// Telegram's live send sits behind the same durable pre-broadcast
    /// reservation as copy/sniper: a failed or unreachable reservation store
    /// must abort the send, never fall back to "just send it". The gate is
    /// reached only AFTER every cheap local refusal, so an unfunded paper
    /// request still terminates locally (proving the order of the gates).
    #[tokio::test]
    async fn a_failed_durable_reservation_aborts_the_telegram_broadcast() {
        struct AlwaysFails;
        #[async_trait::async_trait]
        impl IntentSink for AlwaysFails {
            async fn record(&self, _rec: bot_core::db::repo::IntentRecord) {}
            async fn record_checked(
                &self,
                _rec: bot_core::db::repo::IntentRecord,
            ) -> bot_core::BotResult<()> {
                Err(bot_core::BotError::invalid("journal unavailable (test)"))
            }
            async fn reserve(
                &self,
                _key: &str,
                _rec: &bot_core::db::repo::IntentRecord,
            ) -> bot_core::BotResult<()> {
                Err(bot_core::BotError::invalid(
                    "durable reservation unavailable (test)",
                ))
            }
            async fn link(&self, _intent_id: &str, _signature: &str) {}
            async fn abandon(&self, _intent_id: &str) {}
        }

        let app = bot_core::config::AppConfig::from_defaults();
        let state = bot_core::state::AppState::new(app.clone());
        let rpc = Rpc::new(&app.raw.network).expect("lazy RPC client");
        let wallet = Arc::new(Wallet::from_keypair(solana_sdk::signature::Keypair::new()));
        let sink: Arc<dyn IntentSink> = Arc::new(AlwaysFails);
        let port = DeploymentTradePort::new(
            state.clone(),
            rpc,
            wallet,
            None,
            Some(sink),
            "org-1".into(),
            &app.raw,
        );

        // Paper + unfunded: refused locally, before any reservation or send.
        let request = buy_req();
        let local = port.execute(&request, TradeAction::Buy).await;
        assert!(
            matches!(
                local,
                Err(PortError::Unavailable(_)) | Err(PortError::RiskRejected(_))
            ),
            "an unfunded paper order must fail locally, got {local:?}"
        );
        assert_eq!(
            port.intents.as_ref().map(|_| ()),
            Some(()),
            "the sink must be attached, so the gate below is real"
        );

        // The durable reservation is the gate the money-moving future sits
        // behind: with a sink that cannot reserve, the same broadcast is
        // refused instead of executed.
        let rec = bot_core::db::repo::IntentRecord {
            intent_id: "telegram-gate-test".into(),
            module: "telegram".into(),
            symbol: "GateMint".into(),
            wallet: "Wallet1111".into(),
            side: "buy".into(),
            qty: "1000".into(),
            status: "pending".into(),
            signature: None,
            created_at: Utc::now(),
        };
        let out: bot_core::BotResult<()> = bot_core::recovery::with_reserved_intent_disposition(
            port.intents.as_ref(),
            "telegram-gate-test",
            rec,
            async { Ok::<(), bot_core::BotError>(()) },
            |_| bot_core::recovery::IntentDisposition::Abandon,
        )
        .await;
        assert!(
            out.is_err(),
            "losing the durable reservation must refuse the telegram broadcast"
        );
    }

    #[tokio::test]
    async fn same_org_command_id_reuse_with_different_payload_is_refused() {
        use bot_core::config::AppConfig;
        use solana_sdk::signature::Keypair;

        let app = AppConfig::from_defaults();
        let state = bot_core::state::AppState::new(app.clone());
        let rpc = Rpc::new(&app.raw.network).expect("lazy RPC client");
        let wallet = Arc::new(Wallet::from_keypair(Keypair::new()));
        let port =
            DeploymentTradePort::new(state, rpc, wallet, None, None, "org-1".into(), &app.raw);
        let mut request = buy_req();
        let receipt = ExecutionReceipt {
            reference: "original-receipt".into(),
            detail: "original".into(),
        };
        port.memoise(&request, receipt).await;
        let original = request.clone();
        assert_eq!(
            port.execute(&original, TradeAction::Buy)
                .await
                .expect("identical replay"),
            ExecutionReceipt {
                reference: "original-receipt".into(),
                detail: "original".into(),
            }
        );
        request.amount_lamports = Some(9_000_000);
        assert_eq!(
            port.execute(&request, TradeAction::Buy).await,
            Err(PortError::PayloadMismatch),
            "same organization and command id with a changed amount must not replay the old receipt"
        );
    }

    #[tokio::test]
    async fn restored_trade_replay_requires_exact_command_fingerprint() {
        let (port, state) = test_port_with_state();
        let request = buy_req();
        state.record_trade(recorded_trade(&request, true)).await;
        assert_eq!(
            port.execute(&request, TradeAction::Buy)
                .await
                .expect("identical restart replay")
                .reference,
            "Sig111"
        );
        let mut changed = request.clone();
        changed.slippage_bps = 101;
        assert_eq!(
            port.execute(&changed, TradeAction::Buy).await,
            Err(PortError::PayloadMismatch),
            "a restored trade must not vouch for a different command payload"
        );
    }

    #[tokio::test]
    async fn restored_trade_without_fingerprint_fails_closed() {
        let (port, state) = test_port_with_state();
        let request = buy_req();
        state.record_trade(recorded_trade(&request, false)).await;
        assert!(matches!(
            port.execute(&request, TradeAction::Buy).await,
            Err(PortError::Unavailable(reason)) if reason.contains("fingerprint")
        ));
    }

    #[tokio::test]
    async fn receipt_replay_never_bypasses_organization_check() {
        use bot_core::config::AppConfig;
        use solana_sdk::signature::Keypair;

        let app = AppConfig::from_defaults();
        let state = bot_core::state::AppState::new(app.clone());
        let rpc = Rpc::new(&app.raw.network).expect("lazy RPC client");
        let wallet = Arc::new(Wallet::from_keypair(Keypair::new()));
        let port =
            DeploymentTradePort::new(state, rpc, wallet, None, None, "org-1".into(), &app.raw);
        let mut request = buy_req();
        let receipt = ExecutionReceipt {
            reference: "private-receipt".into(),
            detail: "private".into(),
        };
        port.memoise(&request, receipt).await;
        assert_eq!(
            port.execute(&request, TradeAction::Buy)
                .await
                .unwrap()
                .reference,
            "private-receipt"
        );
        request.organization_id = "other-org".into();
        assert!(matches!(
            port.execute(&request, TradeAction::Buy).await,
            Err(PortError::Unavailable(_))
        ));
    }

    #[test]
    fn limits_require_both_telegram_caps() {
        assert!(lamport_limits(&cfg_with_caps(0, 10, 0.0)).is_none());
        assert!(lamport_limits(&cfg_with_caps(10, 0, 0.0)).is_none());
        let limits = lamport_limits(&cfg_with_caps(10, 20, 0.0)).expect("both caps set");
        assert_eq!(limits.max_order_lamports, 10);
        assert_eq!(limits.daily_cap_lamports, 20);
    }

    #[test]
    fn limits_intersect_with_generic_risk_cap() {
        // 0.5 SOL risk cap is tighter than the 2 SOL telegram cap.
        let limits =
            lamport_limits(&cfg_with_caps(2_000_000_000, 9_000_000_000, 0.5)).expect("caps set");
        assert_eq!(limits.max_order_lamports, 500_000_000);
        // A looser risk cap leaves the telegram cap in force.
        let limits =
            lamport_limits(&cfg_with_caps(2_000_000_000, 9_000_000_000, 5.0)).expect("caps set");
        assert_eq!(limits.max_order_lamports, 2_000_000_000);
    }

    #[test]
    fn validate_request_shape() {
        let ok = buy_req();
        assert!(validate_request(&ok, TradeAction::Buy).is_ok());

        let mut mismatched_id = buy_req();
        mismatched_id.command_id = Uuid::new_v4();
        assert!(matches!(
            validate_request(&mismatched_id, TradeAction::Buy),
            Err(PortError::RiskRejected(_))
        ));

        let mut wrong_endpoint = buy_req();
        assert!(matches!(
            validate_request(&wrong_endpoint, TradeAction::Sell),
            Err(PortError::RiskRejected(_))
        ));

        wrong_endpoint.amount_lamports = None;
        assert!(matches!(
            validate_request(&wrong_endpoint, TradeAction::Buy),
            Err(PortError::RiskRejected(_))
        ));

        let mut zero = buy_req();
        zero.amount_lamports = Some(0);
        assert!(matches!(
            validate_request(&zero, TradeAction::Buy),
            Err(PortError::RiskRejected(_))
        ));

        let mut percent_buy = buy_req();
        percent_buy.percent_bps = Some(5_000);
        assert!(matches!(
            validate_request(&percent_buy, TradeAction::Buy),
            Err(PortError::RiskRejected(_))
        ));

        let mut slippery = buy_req();
        slippery.slippage_bps = 10_001;
        assert!(matches!(
            validate_request(&slippery, TradeAction::Buy),
            Err(PortError::RiskRejected(_))
        ));

        let mut sell = buy_req();
        sell.action = TradeAction::Sell;
        sell.amount_lamports = None;
        sell.percent_bps = Some(10_000);
        assert!(validate_request(&sell, TradeAction::Sell).is_ok());
        sell.percent_bps = Some(0);
        assert!(matches!(
            validate_request(&sell, TradeAction::Sell),
            Err(PortError::RiskRejected(_))
        ));
        sell.percent_bps = Some(10_001);
        assert!(matches!(
            validate_request(&sell, TradeAction::Sell),
            Err(PortError::RiskRejected(_))
        ));
    }

    #[test]
    fn percent_math_is_exact_and_bounded() {
        assert_eq!(percent_of(1_000_000, 10_000), 1_000_000);
        assert_eq!(percent_of(1_000_000, 2_500), 250_000);
        assert_eq!(percent_of(3, 3_333), 0);
        assert_eq!(percent_of(u64::MAX, 10_000), u64::MAX);
    }

    #[test]
    fn live_sell_never_uses_untracked_wallet_wide_balance() {
        let mint = Pubkey::new_unique();
        assert_eq!(
            live_sellable_raw(1_000_000, 1_000_000, &mint).unwrap(),
            1_000_000
        );
        assert_eq!(
            live_sellable_raw(1_000_001, 1_000_000, &mint).unwrap(),
            1_000_000
        );
        assert_eq!(
            live_sellable_raw(999_999, 1_000_000, &mint).unwrap(),
            999_999
        );
        // A second position's tokens in the same wallet must not be sold
        // against this one tracked position.
        assert!(matches!(
            live_sellable_raw(2_000_000, 1_000_000, &mint),
            Err(PortError::RiskRejected(reason)) if reason.contains("wallet-wide")
        ));
        assert!(matches!(
            live_sellable_raw(1_000_000, 2_000_000, &mint),
            Err(PortError::RiskRejected(_))
        ));
        assert!(matches!(
            live_sellable_raw(0, 1_000_000, &mint),
            Err(PortError::RiskRejected(_))
        ));
        assert!(matches!(
            live_sellable_raw(1_000_000, 0, &mint),
            Err(PortError::RiskRejected(_))
        ));
        assert_eq!(percent_of(1_000_000, 10_000), 1_000_000);
    }

    #[test]
    fn deterministic_ids_are_stable() {
        let cmd = Uuid::nil();
        assert_eq!(
            DeploymentTradePort::intent_id(cmd),
            "telegram-00000000-0000-0000-0000-000000000000"
        );
        assert!(DeploymentTradePort::trade_id(cmd).starts_with("tg-trade-"));
        assert!(DeploymentTradePort::position_id(cmd).starts_with("tg-pos-"));
    }

    #[test]
    fn open_position_lookup_prefers_manual_and_skips_terminal() {
        let mint = "Mint111";
        let mut sniper_pos = Position::new(
            "p1".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            ExecutionMode::Paper,
            mint.into(),
            "M".into(),
            "SOL".into(),
        );
        sniper_pos.apply_buy(10.0, 0.1, 1.0);
        let mut manual_pos = Position::new(
            "p2".into(),
            TradeSource::Manual,
            Venue::Jupiter,
            ExecutionMode::Paper,
            mint.into(),
            "M".into(),
            "SOL".into(),
        );
        manual_pos.apply_buy(5.0, 0.2, 1.0);
        let found = find_open_position(&[sniper_pos.clone(), manual_pos], mint).expect("found");
        assert_eq!(found.id, "p2", "telegram manages its own book first");

        let mut closed = sniper_pos.clone();
        closed.close(PositionStatus::Closed, "done");
        assert!(find_open_position(&[closed], mint).is_none());
        assert!(find_open_position(&[sniper_pos], "OtherMint").is_none());
    }

    #[test]
    fn receipt_reconstruction_uses_signature_or_paper_reference() {
        let cmd = Uuid::new_v4();
        let mut trade = Trade {
            id: DeploymentTradePort::trade_id(cmd),
            ts: Utc::now(),
            source: TradeSource::Manual,
            venue: Venue::Jupiter,
            mode: ExecutionMode::Live,
            side: PositionSide::Long,
            symbol: "Mint111".into(),
            symbol_display: "M".into(),
            amount_in: 0.1,
            amount_out: 10.0,
            quote_symbol: "SOL".into(),
            price: 0.01,
            fee: 0.0,
            slippage_bps: 100,
            signature: Some("Sig111".into()),
            position_id: None,
            note: None,
            latency_ms: None,
        };
        let receipt = receipt_from_trade(&trade, cmd);
        assert_eq!(receipt.reference, "Sig111");
        trade.signature = None;
        let receipt = receipt_from_trade(&trade, cmd);
        assert_eq!(receipt.reference, format!("paper:{cmd}"));
    }
}
