//! TASK 5 — global risk + accounting / ledger: deterministic offline suite.
//!
//! Everything runs against a real `AppState` (the same object the server
//! builds), the real `RiskEngine::check_entry` path the three modules call,
//! the real `GlobalLedger` and the in-memory journals. No network, no
//! database, no clock dependence beyond "today".
//!
//! Covered (spec §10): portfolio / wallet / venue / strategy exposure
//! limits, daily loss, drawdown, global + venue + strategy kill switches,
//! duplicate fill / fee / settlement, ledger replay, restart recovery,
//! position aggregation, PnL and fee calculation, ledger ↔ position
//! reconciliation (quantity mismatch, orphan event), concurrent identical
//! accounting events, and the end-to-end proof: the same financial event
//! submitted twice → exactly one ledger mutation, one position mutation,
//! one PnL effect.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;

use bot_core::accounting::{
    fill_event, AccountingEvent, AccountingFindingKind, Applied, EventKind, EventSide,
    GlobalLedger, MemoryLedgerStore, PositionKey,
};
use bot_core::config::AppConfig;
use bot_core::events::{AppEvent, EventBus};
use bot_core::global_risk::{
    DecisionContext, GlobalRejectReason, GlobalRiskRequest, GlobalVerdict, KillScope, SwitchOutcome,
};
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::reconciliation::QuantityTolerance;
use bot_core::risk::{EntryRequest, RiskCode, RiskEngine};
use bot_core::state::{AppState, Shared};

// ---------------------------------------------------------------- helpers --

fn state_with(configure: impl FnOnce(&mut bot_core::config::Config)) -> Shared {
    let mut cfg = AppConfig::from_defaults();
    cfg.raw.sniper.enabled = true;
    cfg.raw.copy.enabled = true;
    cfg.raw.polymarket.enabled = true;
    // Generous module limits so the GLOBAL layer is what decides here.
    cfg.raw.risk.max_open_positions = 100;
    cfg.raw.risk.max_position_quote = 1_000.0;
    cfg.raw.risk.max_position_fraction = 1.0;
    cfg.raw.risk.daily_loss_limit_quote = 0.0;
    cfg.raw.risk.min_sol_reserve = 0.0;
    cfg.raw.risk.reentry_cooldown_secs = 0;
    cfg.raw.global_risk.reference_rates =
        HashMap::from([("SOL".to_string(), 100.0), ("USDC".to_string(), 1.0)]);
    configure(&mut cfg.raw);
    AppState::new(cfg)
}

fn entry(
    module: BotModule,
    venue: Venue,
    symbol: &str,
    quote: f64,
    wallet: &str,
    strategy: &str,
) -> EntryRequest {
    EntryRequest {
        module,
        venue,
        symbol: symbol.into(),
        symbol_display: symbol.into(),
        requested_quote: quote,
        available_quote: 1_000_000.0,
        slippage_bps: 100,
        price: if venue == Venue::PolymarketClob {
            Some(0.5)
        } else {
            None
        },
        fair_value: None,
        liquidity: if venue == Venue::PolymarketClob {
            Some(100_000.0)
        } else {
            None
        },
        wallet: wallet.into(),
        strategy: strategy.into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn fill(
    module: BotModule,
    venue: Venue,
    wallet: &str,
    strategy: &str,
    asset: &str,
    side: EventSide,
    qty: f64,
    quote: f64,
    fee: f64,
    reference: &str,
) -> AccountingEvent {
    let quote_asset = if venue == Venue::PolymarketClob {
        "USDC"
    } else {
        "SOL"
    };
    fill_event(
        module,
        venue,
        wallet,
        strategy,
        asset,
        quote_asset,
        side,
        qty,
        quote / qty,
        quote,
        fee,
        ExecutionMode::Paper,
        reference,
        Some(format!("order-{reference}")),
        Some(format!("p-{asset}")),
        Utc::now(),
        "test",
    )
}

fn sol_buy(
    wallet: &str,
    strategy: &str,
    asset: &str,
    sol: f64,
    reference: &str,
) -> AccountingEvent {
    fill(
        BotModule::Sniper,
        Venue::PumpFun,
        wallet,
        strategy,
        asset,
        EventSide::Buy,
        sol * 1_000.0,
        sol,
        0.0,
        reference,
    )
}

/// Orders the OMS knows (empty when no manager is attached, as in these
/// state-only tests).
async fn oms_orders(state: &Shared) -> Vec<bot_core::oms::Order> {
    match state.orders() {
        Some(mgr) => mgr.list(500).await,
        None => Vec::new(),
    }
}

/// Audit actions with `prefix` received on a bus subscription taken BEFORE
/// the actions under test (the broadcast send is synchronous, so draining
/// the receiver is deterministic).
fn audit_actions(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<AppEvent>>,
    prefix: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let AppEvent::Audit { action, .. } = e.as_ref() {
            if action.starts_with(prefix) {
                out.push(action.clone());
            }
        }
    }
    out
}

// ------------------------------------------------------- exposure limits --

#[tokio::test]
async fn portfolio_exposure_limit_is_enforced_across_modules() {
    let state = state_with(|c| c.global_risk.max_portfolio_exposure_ref = 250.0);
    let mut rx = state.events.subscribe();
    let risk = RiskEngine::new(state.clone());
    // 1 SOL of sniper exposure (100 ref) + 100 USDC of Polymarket exposure.
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 1.0, "a"))
        .await;
    state
        .ledger()
        .submit(fill(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "0xeoa",
            "value",
            "TOKEN-1",
            EventSide::Buy,
            200.0,
            100.0,
            0.0,
            "poly-1",
        ))
        .await;
    // 200 used; a 0.4 SOL (40 ref) copy entry fits, a 0.6 SOL one does not.
    let ok = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-B",
            0.4,
            "w1",
            "copy:leader",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    let d = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-B",
            0.6,
            "w1",
            "copy:leader",
        ))
        .await;
    assert!(!d.allowed());
    assert_eq!(d.code, Some(RiskCode::GlobalExposure));
    assert!(d.reason.contains("portfolio_exposure"), "{}", d.reason);
    assert!(RiskCode::GlobalExposure.is_exposure_limit());
    // The decision is journaled with the figures it used.
    let recent = state.global_risk().recent_decisions(1).await;
    assert_eq!(recent[0].verdict, GlobalVerdict::Reject);
    assert_eq!(
        recent[0].reason,
        Some(GlobalRejectReason::PortfolioExposure)
    );
    assert!((recent[0].snapshot.portfolio_ref - 200.0).abs() < 1e-9);
    assert!((recent[0].snapshot.requested_ref - 60.0).abs() < 1e-9);
    // …and audited.
    let actions = audit_actions(&mut rx, "global.risk.reject");
    assert_eq!(actions.len(), 1);
}

#[tokio::test]
async fn per_wallet_venue_and_strategy_limits_are_scoped() {
    let state = state_with(|c| {
        c.global_risk.max_wallet_exposure_ref = 150.0;
        c.global_risk.max_venue_exposure_ref = 120.0;
        c.global_risk.max_strategy_exposure_ref = 80.0;
    });
    let risk = RiskEngine::new(state.clone());
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 1.0, "a"))
        .await; // w1 / pump.fun / sniper = 100

    // Strategy cap (80): another sniper entry of 0.1 SOL exceeds 80 → reject.
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-B",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::GlobalExposure));
    assert!(d.reason.contains("strategy_exposure"), "{}", d.reason);

    // Different strategy, same venue: venue cap (120) allows 0.15 (15) …
    let ok = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::PumpFun,
            "MINT-C",
            0.15,
            "w1",
            "copy:l1",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    // … but not 0.25 (25 → 125 > 120).
    let d = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::PumpFun,
            "MINT-C",
            0.25,
            "w1",
            "copy:l1",
        ))
        .await;
    assert!(d.reason.contains("venue_exposure"), "{}", d.reason);

    // Different venue and strategy, same wallet: wallet cap (150) refuses 0.6 (60 → 160).
    let d = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-D",
            0.6,
            "w1",
            "copy:l2",
        ))
        .await;
    assert!(d.reason.contains("wallet_exposure"), "{}", d.reason);
    // Another wallet is untouched.
    let ok = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-D",
            0.6,
            "w2",
            "copy:l2",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
}

#[tokio::test]
async fn asset_open_position_and_order_notional_caps() {
    let state = state_with(|c| {
        c.global_risk.max_asset_exposure_ref = 100.0;
        c.global_risk.max_open_positions = 2;
        c.global_risk.max_order_notional_ref = 50.0;
    });
    let risk = RiskEngine::new(state.clone());
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-A",
            0.6,
            "w1",
            "sniper",
        ))
        .await;
    assert!(d.reason.contains("order_notional"), "{}", d.reason);
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 0.9, "a"))
        .await;
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-A",
            0.2,
            "w1",
            "sniper",
        ))
        .await;
    assert!(d.reason.contains("asset_exposure"), "{}", d.reason);
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-B", 0.1, "b"))
        .await;
    let d = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-C",
            0.1,
            "w1",
            "copy:l",
        ))
        .await;
    assert!(d.reason.contains("max_open_positions"), "{}", d.reason);
    // Adding to an already-open aggregated position is not a new position.
    let ok = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-B",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
}

#[tokio::test]
async fn reference_rate_missing_fails_closed_only_when_a_limit_needs_it() {
    // No SOL rate configured.
    let state = state_with(|c| {
        c.global_risk.reference_rates = HashMap::from([("USDC".to_string(), 1.0)]);
    });
    let risk = RiskEngine::new(state.clone());
    // No reference-denominated limit → Solana entries pass through.
    let ok = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-A",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    // Turn a limit on → the SOL entry is refused (cannot be evaluated).
    state
        .update_config(|c| c.global_risk.max_portfolio_exposure_ref = 1_000.0)
        .await;
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-A",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::GlobalUnavailable));
    assert!(d.reason.contains("reference_rate_missing"), "{}", d.reason);
    // USDC entries still evaluate — until SOL exposure exists without a rate.
    let ok = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 0.1, "a"))
        .await;
    let d = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(d.reason.contains("understated"), "{}", d.reason);
}

// ------------------------------------------------- daily loss / drawdown --

#[tokio::test]
async fn daily_loss_limit_reads_the_ledger_not_the_module() {
    let state = state_with(|c| c.global_risk.max_daily_loss_ref = 50.0);
    let risk = RiskEngine::new(state.clone());
    // Buy 1 SOL worth, sell for 0.4 → net realized today = −0.6 SOL = −60 ref.
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 1.0, "a"))
        .await;
    state
        .ledger()
        .submit(fill(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT-A",
            EventSide::Sell,
            1_000.0,
            0.4,
            0.0,
            "b",
        ))
        .await;
    let d = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-B",
            0.1,
            "w1",
            "copy:l",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::GlobalDailyLoss));
    assert!(d.reason.contains("daily_loss"), "{}", d.reason);
    let view = state.global_risk().portfolio(HashMap::new()).await;
    assert!((view.realized_today_ref + 60.0).abs() < 1e-9);
}

#[tokio::test]
async fn drawdown_limit_uses_peak_realized_and_current_unrealized() {
    let state = state_with(|c| {
        c.global_risk.capital_base_ref = 1_000.0;
        c.global_risk.max_drawdown_pct = 0.05; // 50 ref
    });
    let risk = RiskEngine::new(state.clone());
    // Win 1 SOL (peak = +100 ref), then lose 0.4 SOL (cumulative +60 → drawdown 40).
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 1.0, "a"))
        .await;
    state
        .ledger()
        .submit(fill(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT-A",
            EventSide::Sell,
            1_000.0,
            2.0,
            0.0,
            "b",
        ))
        .await;
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-B", 1.0, "c"))
        .await;
    state
        .ledger()
        .submit(fill(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT-B",
            EventSide::Sell,
            1_000.0,
            0.6,
            0.0,
            "d",
        ))
        .await;
    let ok = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-C",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert!(ok.allowed(), "drawdown 40 < 50: {}", ok.reason);
    // An open position marked down adds unrealized loss: buy 0.5 SOL of
    // MINT-C, mark it at 60% → unrealized −0.2 SOL = −20 → drawdown 60.
    state
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-C", 0.5, "e"))
        .await;
    let mut p = Position::new(
        "p-MINT-C".into(),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Paper,
        "MINT-C".into(),
        "MINT-C".into(),
        "SOL".into(),
    );
    p.apply_buy(500.0, 0.001, 0.5);
    p.last_mark = 0.0006;
    state.upsert_position(p).await;
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-D",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::GlobalDrawdown));
    assert!(d.reason.contains("drawdown"), "{}", d.reason);
    let view = state.global_risk().portfolio(state.marks().await).await;
    assert!(
        (view.drawdown_ref() - 60.0).abs() < 1e-6,
        "{}",
        view.drawdown_ref()
    );
    assert!(
        (view.utilization - 0.05).abs() < 1e-9,
        "0.5 SOL of 1000: {}",
        view.utilization
    );
}

// ---------------------------------------------------------- kill switches --

#[tokio::test]
async fn global_venue_and_strategy_kill_switches_refuse_new_entries() {
    let state = state_with(|c| c.global_risk.killed_strategies = vec!["copy:bad-leader".into()]);
    let mut rx = state.events.subscribe();
    let risk = RiskEngine::new(state.clone());
    // Configured strategy switch.
    let d = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-A",
            0.1,
            "w1",
            "copy:bad-leader",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::GlobalKillSwitch));
    assert!(d.reason.contains("strategy_kill_switch"), "{}", d.reason);
    let ok = risk
        .check_entry(&entry(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "MINT-A",
            0.1,
            "w1",
            "copy:good-leader",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    // Runtime venue switch (durable + audited), then released.
    let engine = state.global_risk();
    assert_eq!(
        engine
            .engage(KillScope::Venue(Venue::PolymarketClob), "incident", "op")
            .await,
        SwitchOutcome::Changed
    );
    let d = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(d.reason.contains("venue_kill_switch"), "{}", d.reason);
    assert_eq!(
        engine
            .release(KillScope::Venue(Venue::PolymarketClob), "resolved", "op")
            .await,
        SwitchOutcome::Changed
    );
    let ok = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    // The configured switch cannot be released at runtime.
    assert_eq!(
        engine
            .release(KillScope::Strategy("copy:bad-leader".into()), "x", "op")
            .await,
        SwitchOutcome::PinnedByConfig
    );
    let actions = audit_actions(&mut rx, "global.kill_switch.");
    assert_eq!(actions.len(), 2, "{actions:?}");
    // The process-wide kill switch is still the first gate (existing code).
    state.set_kill_switch(true, "test").await;
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-Z",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::KillSwitch));
    // …and the global engine itself also refuses it when asked directly.
    let direct = engine
        .decide(
            &GlobalRiskRequest {
                module: BotModule::Sniper,
                venue: Venue::PumpFun,
                wallet: "w1".into(),
                strategy: "sniper".into(),
                asset: "MINT-Z".into(),
                quote_asset: "SOL".into(),
                requested_quote: 0.1,
                mode: ExecutionMode::Paper,
            },
            &DecisionContext {
                global_kill: true,
                marks: HashMap::new(),
            },
        )
        .await;
    assert_eq!(direct.reason, Some(GlobalRejectReason::GlobalKillSwitch));
}

#[tokio::test]
async fn runtime_kill_switches_survive_a_restart_through_the_journal() {
    let store = Arc::new(bot_core::global_risk::MemoryRiskStore::default());
    let life1 = state_with(|_| {});
    life1.global_risk().attach_store(store.clone()).await;
    life1
        .global_risk()
        .engage(
            KillScope::Strategy("copy:leader-x".into()),
            "incident",
            "op",
        )
        .await;
    assert_eq!(store.kill_switch_events().await.len(), 1);

    let life2 = state_with(|_| {});
    life2.global_risk().attach_store(store.clone()).await;
    assert!(life2
        .global_risk()
        .switches()
        .strategy_killed("copy:leader-x")
        .is_none());
    assert_eq!(life2.global_risk().restore().await, 1);
    assert!(life2
        .global_risk()
        .switches()
        .strategy_killed("copy:leader-x")
        .is_some());
}

// ---------------------------------------------------------- idempotency --

#[tokio::test]
async fn same_financial_event_twice_yields_one_ledger_one_position_one_pnl_effect() {
    let state = state_with(|_| {});
    let mut rx = state.events.subscribe();
    let ledger = state.ledger();
    let buy = sol_buy("w1", "sniper", "MINT-A", 1.0, "sig-buy");
    let sell = fill(
        BotModule::Sniper,
        Venue::PumpFun,
        "w1",
        "sniper",
        "MINT-A",
        EventSide::Sell,
        1_000.0,
        1.5,
        0.0,
        "sig-sell",
    );

    assert!(ledger.submit(buy.clone()).await.is_new());
    assert_eq!(ledger.submit(buy.clone()).await, Applied::Duplicate);
    assert!(ledger.submit(sell.clone()).await.is_new());
    // The SAME sell again: replayed confirmation / recovery re-submission.
    assert_eq!(ledger.submit(sell.clone()).await, Applied::Duplicate);
    assert_eq!(ledger.submit(sell.clone()).await, Applied::Duplicate);

    // Exactly one ledger mutation per fact.
    assert_eq!(ledger.len().await, 2);
    let store = ledger.store().await;
    assert_eq!(store.load_events().await.unwrap().len(), 2);
    // Exactly one position mutation per fact.
    let book = ledger.book().await;
    let key = PositionKey::of(&buy);
    let p = book.get(&key).unwrap();
    assert_eq!(p.event_count, 2);
    assert_eq!(p.qty, 0.0);
    // Exactly one PnL effect: +0.5 SOL, not +1.0 or +1.5.
    assert!((p.realized - 0.5).abs() < 1e-12, "{}", p.realized);
    let series = ledger.realized_series().await;
    assert!((series.total["SOL"] - 0.5).abs() < 1e-12);
    // Audit shows two ledger mutations and two position transitions only.
    let all: Vec<String> = {
        let mut v = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let AppEvent::Audit { action, .. } = e.as_ref() {
                v.push(action.clone());
            }
        }
        v
    };
    let ledger_audits: Vec<&String> = all
        .iter()
        .filter(|a| a.starts_with("global.ledger."))
        .collect();
    assert_eq!(ledger_audits.len(), 2, "{ledger_audits:?}");
    let position_audits: Vec<&String> = all
        .iter()
        .filter(|a| a.starts_with("global.position."))
        .collect();
    assert_eq!(
        position_audits,
        vec!["global.position.opened", "global.position.closed"]
    );
}

#[tokio::test]
async fn duplicate_fee_and_settlement_events_never_double_book() {
    let state = state_with(|_| {});
    let ledger = state.ledger();
    let mut fee = sol_buy("w1", "sniper", "SOL", 0.0, "fee-ref-1");
    fee.kind = EventKind::Fee;
    fee.side = None;
    fee.quantity = 0.0;
    fee.quote_amount = 0.0;
    fee.fee = 0.01;
    fee.price = None;
    assert!(ledger.submit(fee.clone()).await.is_new());
    assert_eq!(ledger.submit(fee.clone()).await, Applied::Duplicate);
    let book = ledger.book().await;
    assert!((book.standalone_fees()[&("w1".to_string(), "SOL".to_string())] - 0.01).abs() < 1e-12);
    assert!((book.cash()[&("w1".to_string(), "SOL".to_string())] + 0.01).abs() < 1e-12);

    // A Polymarket position settled at resolution: the settlement is booked once.
    ledger
        .submit(fill(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "0xeoa",
            "value",
            "TOKEN",
            EventSide::Buy,
            100.0,
            40.0,
            0.0,
            "poly-fill-1",
        ))
        .await;
    let mut settlement = fill(
        BotModule::Polymarket,
        Venue::PolymarketClob,
        "0xeoa",
        "value",
        "TOKEN",
        EventSide::Sell,
        100.0,
        100.0,
        0.0,
        "redeem-tx-1",
    );
    settlement.kind = EventKind::Settlement;
    assert!(ledger.submit(settlement.clone()).await.is_new());
    assert_eq!(ledger.submit(settlement.clone()).await, Applied::Duplicate);
    let p = ledger
        .book()
        .await
        .get(&PositionKey::of(&settlement))
        .cloned()
        .unwrap();
    assert_eq!(p.qty, 0.0);
    assert!((p.realized - 60.0).abs() < 1e-9);
    assert_eq!(p.event_count, 2);
}

#[tokio::test]
async fn concurrent_identical_events_from_many_tasks_book_once() {
    let state = state_with(|_| {});
    let event = sol_buy("w1", "sniper", "MINT-C", 0.3, "sig-concurrent");
    let mut handles = Vec::new();
    for _ in 0..32 {
        let state = state.clone();
        let event = event.clone();
        handles.push(tokio::spawn(
            async move { state.ledger().submit(event).await },
        ));
    }
    let mut new = 0;
    for h in handles {
        if h.await.unwrap().is_new() {
            new += 1;
        }
    }
    assert_eq!(new, 1);
    assert_eq!(state.ledger().len().await, 1);
    let p = state.ledger().open_positions().await;
    assert_eq!(p.len(), 1);
    assert!((p[0].qty - 300.0).abs() < 1e-9);
}

// ---------------------------------------------------- aggregation / pnl --

#[tokio::test]
async fn positions_aggregate_across_modules_with_pnl_and_fees() {
    let state = state_with(|c| c.global_risk.capital_base_ref = 2_000.0);
    let ledger = state.ledger();
    // Sniper: buy 1 SOL, sell half for 0.8 (gain 0.3), fee 0.01 on the sell.
    ledger
        .submit(sol_buy("w1", "sniper", "MINT-A", 1.0, "s1"))
        .await;
    ledger
        .submit(fill(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT-A",
            EventSide::Sell,
            500.0,
            0.8,
            0.01,
            "s2",
        ))
        .await;
    // Copy: buy 0.5 SOL with a 0.005 fee embedded.
    ledger
        .submit(fill(
            BotModule::Copy,
            Venue::RaydiumAmmV4,
            "w1",
            "copy:leader",
            "MINT-B",
            EventSide::Buy,
            500.0,
            0.5,
            0.005,
            "c1",
        ))
        .await;
    // Polymarket: buy 100 tokens for 40 USDC.
    ledger
        .submit(fill(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "0xeoa",
            "value",
            "TOKEN",
            EventSide::Buy,
            100.0,
            40.0,
            0.0,
            "p1",
        ))
        .await;

    let marks = HashMap::from([("TOKEN".to_string(), 0.5)]); // 100 × 0.5 = 50 USDC notional
    let view = state.global_risk().portfolio(marks).await;
    assert_eq!(view.reference_asset, "USD");
    assert_eq!(view.open_positions, 3);
    // Exposure = max(cost, notional at the mark); assets without a module
    // mark use their last fill price:
    //   sniper  500 × 0.0016 (last sell) = 0.8 SOL  vs cost 0.5  → 0.8 SOL = 80
    //   copy    500 × 0.001 = 0.5 SOL           vs cost 0.495 → 0.5 SOL = 50
    //   poly    100 × 0.5 = 50 USDC             vs cost 40    → 50
    assert!(
        (view.total_exposure_ref - 180.0).abs() < 1e-6,
        "{}",
        view.total_exposure_ref
    );
    assert!((view.by_module["sniper"].exposure_ref - 80.0).abs() < 1e-6);
    assert!((view.by_module["copy"].exposure_ref - 50.0).abs() < 1e-6);
    assert!((view.by_module["polymarket"].exposure_ref - 50.0).abs() < 1e-6);
    assert!((view.by_venue["polymarket"].unrealized_ref - 10.0).abs() < 1e-6);
    assert!((view.by_wallet["w1"].exposure_ref - 130.0).abs() < 1e-6);
    assert!((view.by_strategy["copy:leader"].exposure_ref - 50.0).abs() < 1e-6);
    // Gross realized = (net 0.8 + fee 0.01) − cost of slice 0.5 = 0.31 SOL.
    assert!(
        (view.by_asset["MINT-A"].realized_ref - 31.0).abs() < 1e-6,
        "{}",
        view.by_asset["MINT-A"].realized_ref
    );
    // Fees: 0.01 + 0.005 SOL = 1.5 ref.
    assert!((view.fees_ref - 1.5).abs() < 1e-6, "{}", view.fees_ref);
    // Unrealized: sniper 0.3 SOL (30) + copy 0.005 SOL (0.5) + poly 10 = 40.5.
    assert!(
        (view.unrealized_ref - 40.5).abs() < 1e-6,
        "{}",
        view.unrealized_ref
    );
    // Net = realized 31 + unrealized 40.5 − fees 1.5 (the sell fee is
    // inside the gross realized figure, so it is not lost and not counted twice).
    assert!(
        (view.net_pnl_ref - 70.0).abs() < 1e-6,
        "{}",
        view.net_pnl_ref
    );
    assert!((view.utilization - 180.0 / 2_000.0).abs() < 1e-9);
    assert!(view.missing_rates.is_empty());
    // Native view keeps per-quote figures without any conversion.
    assert!((view.native["USDC"].exposure - 50.0).abs() < 1e-9);
    assert!((view.native["SOL"].fees - 0.015).abs() < 1e-9);
}

// --------------------------------------------------------- reconciliation --

fn module_position(
    id: &str,
    source: TradeSource,
    venue: Venue,
    symbol: &str,
    qty: f64,
    cost: f64,
) -> Position {
    let mut p = Position::new(
        id.into(),
        source,
        venue,
        ExecutionMode::Paper,
        symbol.into(),
        symbol.into(),
        "SOL".into(),
    );
    p.qty = qty;
    p.cost_basis = cost;
    p.avg_entry = if qty > 0.0 { cost / qty } else { 0.0 };
    p.last_mark = p.avg_entry;
    p
}

fn module_trade(
    id: &str,
    sig: &str,
    symbol: &str,
    buy: bool,
    qty: f64,
    quote: f64,
    position_id: &str,
) -> Trade {
    Trade {
        id: id.into(),
        ts: Utc::now(),
        source: TradeSource::Sniper,
        venue: Venue::PumpFun,
        mode: ExecutionMode::Paper,
        side: if buy {
            PositionSide::Long
        } else {
            PositionSide::Short
        },
        symbol: symbol.into(),
        symbol_display: symbol.into(),
        amount_in: if buy { quote } else { qty },
        amount_out: if buy { qty } else { quote },
        quote_symbol: "SOL".into(),
        price: quote / qty,
        fee: 0.0,
        slippage_bps: 0,
        signature: Some(sig.into()),
        position_id: Some(position_id.into()),
        note: None,
        latency_ms: None,
    }
}

#[tokio::test]
async fn ledger_and_positions_reconcile_and_mismatches_are_reported_not_repaired() {
    let state = state_with(|_| {});
    let mut rx = state.events.subscribe();
    let ledger = state.ledger();
    // Module truth: one open position of 1000 MINT-A for 1 SOL, one trade.
    state
        .upsert_position(module_position(
            "p-MINT-A",
            TradeSource::Sniper,
            Venue::PumpFun,
            "MINT-A",
            1_000.0,
            1.0,
        ))
        .await;
    state
        .record_trade(module_trade(
            "t-1", "sig-1", "MINT-A", true, 1_000.0, 1.0, "p-MINT-A",
        ))
        .await;
    // Ledger: the same fill, referenced by the signature and the trade id.
    let mut e = sol_buy("w1", "sniper", "MINT-A", 1.0, "sig-1");
    e.trade_id = Some("t-1".into());
    ledger.submit(e).await;

    let orders = oms_orders(&state).await;
    let positions = state.all_positions().await;
    let trades = state.trades(100).await;
    let run = ledger
        .reconcile(
            &orders,
            &positions,
            &trades,
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    assert!(run.findings.is_empty(), "{:?}", run.findings);

    // A second trade the ledger never saw → missing entry; the module
    // position grows → quantity mismatch. Nothing is repaired.
    state
        .record_trade(module_trade(
            "t-2", "sig-2", "MINT-A", true, 500.0, 0.5, "p-MINT-A",
        ))
        .await;
    state
        .with_position("p-MINT-A", |p| p.apply_buy(500.0, 0.001, 0.5))
        .await;
    let orders = oms_orders(&state).await;
    let positions = state.all_positions().await;
    let trades = state.trades(100).await;
    let run = ledger
        .reconcile(
            &orders,
            &positions,
            &trades,
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    let kinds: Vec<_> = run.findings.iter().map(|f| f.kind).collect();
    assert!(
        kinds.contains(&AccountingFindingKind::MissingLedgerEntry),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&AccountingFindingKind::QuantityMismatch),
        "{kinds:?}"
    );
    assert!(run.findings.iter().all(|f| f.action == "reported"));
    let book_qty = ledger.book().await.positions().next().unwrap().qty;
    assert!(
        (book_qty - 1_000.0).abs() < 1e-9,
        "ledger untouched by reconciliation"
    );
    // New findings are journaled once; a second run reports them but does
    // not journal them again.
    assert_eq!(run.new.len(), run.findings.len());
    let again = ledger
        .reconcile(
            &orders,
            &positions,
            &trades,
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    assert_eq!(again.findings.len(), run.findings.len());
    assert!(again.new.is_empty());
    let journaled = ledger.store().await.recent_findings(10).await.unwrap();
    assert_eq!(journaled.len(), run.findings.len());
    let audits = audit_actions(&mut rx, "global.recon.");
    assert_eq!(audits.len(), run.findings.len());
}

#[tokio::test]
async fn the_order_layer_is_reconciled_against_the_ledger() {
    use bot_core::oms::{OrderDraft, OrderManager, OrderStatus};

    let state = state_with(|_| {});
    let oms = OrderManager::new(None, 256);
    state.attach_orders(oms.clone());
    let ledger = state.ledger();

    // An order that filled and whose fill WAS booked (correlation = order id).
    let booked = oms
        .create(OrderDraft {
            idempotency_key: "intent-booked".into(),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "MINT-A".into(),
            venue: Venue::PumpFun.as_str().into(),
            mode: ExecutionMode::Paper,
            qty: 1_000.0,
            price: Some(0.001),
            meta: serde_json::Value::Null,
        })
        .await
        .expect("order created");
    oms.transition(&booked.id, OrderStatus::Submitted, None)
        .await
        .expect("submitted");
    oms.transition(&booked.id, OrderStatus::Filled, Some("test"))
        .await
        .expect("filled");
    let mut ev = sol_buy("w1", "sniper", "MINT-A", 1.0, "sig-booked");
    ev.correlation_id = Some(booked.id.clone());
    ev.position_id = Some("p-MINT-A".into());
    assert!(ledger.submit(ev).await.is_new());
    state
        .upsert_position(module_position(
            "p-MINT-A",
            TradeSource::Sniper,
            Venue::PumpFun,
            "MINT-A",
            1_000.0,
            1.0,
        ))
        .await;

    // An order that reports a fill but whose money never reached the ledger.
    let unbooked = oms
        .create(OrderDraft {
            idempotency_key: "intent-unbooked".into(),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "MINT-B".into(),
            venue: Venue::PumpFun.as_str().into(),
            mode: ExecutionMode::Paper,
            qty: 500.0,
            price: Some(0.002),
            meta: serde_json::Value::Null,
        })
        .await
        .expect("order created");
    oms.transition(&unbooked.id, OrderStatus::Submitted, None)
        .await
        .expect("submitted");
    oms.transition(&unbooked.id, OrderStatus::Filled, Some("test"))
        .await
        .expect("filled");

    // An order that never moved money (submitted, then failed) — intent only.
    let intent_only = oms
        .create(OrderDraft {
            idempotency_key: "intent-failed".into(),
            module: BotModule::Copy,
            side: "buy".into(),
            symbol: "MINT-C".into(),
            venue: Venue::RaydiumAmmV4.as_str().into(),
            mode: ExecutionMode::Paper,
            qty: 10.0,
            price: None,
            meta: serde_json::Value::Null,
        })
        .await
        .expect("order created");
    oms.transition(&intent_only.id, OrderStatus::Submitted, None)
        .await
        .expect("submitted");
    oms.transition(&intent_only.id, OrderStatus::Failed, Some("rejected"))
        .await
        .expect("failed");

    let orders = oms_orders(&state).await;
    assert_eq!(orders.len(), 3);
    let run = ledger
        .reconcile(
            &orders,
            &state.all_positions().await,
            &state.trades(100).await,
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    // Exactly one finding: the filled order with no ledger event. The booked
    // order and the never-executed order are silent.
    let order_findings: Vec<_> = run
        .findings
        .iter()
        .filter(|f| f.order_id.is_some())
        .collect();
    assert_eq!(order_findings.len(), 1, "{:?}", run.findings);
    let f = order_findings[0];
    assert_eq!(f.kind, AccountingFindingKind::UnresolvedFinancialEvent);
    assert_eq!(f.order_id.as_deref(), Some(unbooked.id.as_str()));
    assert_eq!(f.action, "reported");
    assert!(
        f.detail.contains("no ledger event references it"),
        "{}",
        f.detail
    );
    // Nothing was repaired: the ledger still holds exactly the one event.
    assert_eq!(ledger.len().await, 1);

    // Booking the missing fill clears the finding on the next run.
    let mut fix = sol_buy("w1", "sniper", "MINT-B", 1.0, "sig-unbooked");
    fix.correlation_id = Some(unbooked.id.clone());
    fix.position_id = Some("p-MINT-B".into());
    assert!(ledger.submit(fix).await.is_new());
    state
        .upsert_position(module_position(
            "p-MINT-B",
            TradeSource::Sniper,
            Venue::PumpFun,
            "MINT-B",
            1_000.0,
            1.0,
        ))
        .await;
    let run = ledger
        .reconcile(
            &oms_orders(&state).await,
            &state.all_positions().await,
            &state.trades(100).await,
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    assert!(
        run.findings.iter().all(|f| f.order_id.is_none()),
        "{:?}",
        run.findings
    );
}

#[tokio::test]
async fn orphan_events_are_reported() {
    let state = state_with(|_| {});
    // A fill event that names a position and a trade no module knows.
    let mut e = sol_buy("w1", "sniper", "GHOST", 0.1, "sig-ghost");
    e.trade_id = Some("t-ghost".into());
    e.position_id = Some("p-ghost".into());
    state.ledger().submit(e).await;
    let run = state
        .ledger()
        .reconcile(
            &[],
            &[],
            &[],
            state.started_at(),
            QuantityTolerance::default(),
        )
        .await;
    assert_eq!(run.findings.len(), 1);
    assert_eq!(
        run.findings[0].kind,
        AccountingFindingKind::OrphanAccountingEvent
    );
    assert_eq!(run.findings[0].position_id.as_deref(), Some("p-ghost"));
}

// ------------------------------------------------------------- recovery --

#[tokio::test]
async fn restart_rebuilds_risk_state_from_the_journal_and_refuses_replays() {
    let store = Arc::new(MemoryLedgerStore::new());
    let life1 = state_with(|c| c.global_risk.max_daily_loss_ref = 50.0);
    life1.ledger().attach_store(store.clone()).await;
    life1
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-A", 1.0, "a"))
        .await;
    life1
        .ledger()
        .submit(fill(
            BotModule::Sniper,
            Venue::PumpFun,
            "w1",
            "sniper",
            "MINT-A",
            EventSide::Sell,
            1_000.0,
            0.3,
            0.0,
            "b",
        ))
        .await; // −0.7 SOL = −70 ref today
    life1
        .ledger()
        .submit(sol_buy("w1", "sniper", "MINT-B", 0.2, "c"))
        .await;
    assert_eq!(store.len().await, 3);

    // New process life: the module positions come back from the DB (here:
    // one open position with ledger history, one without).
    let life2 = state_with(|c| c.global_risk.max_daily_loss_ref = 50.0);
    let mut rx = life2.events.subscribe();
    life2.ledger().attach_store(store.clone()).await;
    life2
        .upsert_position(module_position(
            "p-MINT-B",
            TradeSource::Sniper,
            Venue::PumpFun,
            "MINT-B",
            200.0,
            0.2,
        ))
        .await;
    life2
        .upsert_position(module_position(
            "p-OLD",
            TradeSource::Copy,
            Venue::RaydiumAmmV4,
            "OLD",
            5.0,
            0.05,
        ))
        .await;
    let positions = life2.all_positions().await;
    let report = life2.ledger().recover(&positions).await;
    assert!(report.journal_available);
    assert_eq!(report.rebuilt, 3);
    assert_eq!(report.duplicates_skipped, 0);
    assert_eq!(report.open_positions, 1);
    assert_eq!(
        report.gaps,
        vec!["p-OLD".to_string()],
        "gap reported, never synthesised"
    );
    assert_eq!(store.len().await, 3, "recovery journals nothing");

    // Risk state is back: today's loss blocks new entries.
    let risk = RiskEngine::new(life2.clone());
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-C",
            0.1,
            "w1",
            "sniper",
        ))
        .await;
    assert_eq!(d.code, Some(RiskCode::GlobalDailyLoss));
    // Replaying a journaled fill after the restart is a duplicate.
    assert_eq!(
        life2
            .ledger()
            .submit(fill(
                BotModule::Sniper,
                Venue::PumpFun,
                "w1",
                "sniper",
                "MINT-A",
                EventSide::Sell,
                1_000.0,
                0.3,
                0.0,
                "b"
            ))
            .await,
        Applied::Duplicate
    );
    assert_eq!(store.len().await, 3);
    // Ledger events audited as recovery actions.
    let audits = audit_actions(&mut rx, "global.recovery.");
    assert!(
        audits.iter().any(|a| a == "global.recovery.gap_reported"),
        "{audits:?}"
    );
    assert!(
        audits.iter().any(|a| a == "global.recovery.completed"),
        "{audits:?}"
    );
}

#[tokio::test]
async fn ledger_replay_of_the_whole_journal_is_idempotent() {
    let store = Arc::new(MemoryLedgerStore::new());
    let life1 = GlobalLedger::new(EventBus::new(64), "r1");
    life1.attach_store(store.clone()).await;
    for i in 0..20 {
        life1
            .submit(sol_buy(
                "w1",
                "sniper",
                &format!("MINT-{}", i % 4),
                0.1,
                &format!("sig-{i}"),
            ))
            .await;
    }
    let life2 = GlobalLedger::new(EventBus::new(64), "r2");
    life2.attach_store(store.clone()).await;
    let first = life2.recover(&[]).await;
    assert_eq!(first.rebuilt, 20);
    let second = life2.recover(&[]).await;
    assert_eq!(second.rebuilt, 0);
    assert_eq!(second.duplicates_skipped, 20);
    let book = life2.book().await;
    assert_eq!(book.open_count(), 4);
    for p in book.open_positions() {
        assert!((p.qty - 500.0).abs() < 1e-9);
        assert!((p.cost_basis - 0.5).abs() < 1e-9);
    }
    assert_eq!(store.len().await, 20);
}

// ------------------------------------------------------- module wiring --

#[tokio::test]
async fn entry_request_attribution_reaches_the_global_decision() {
    let state = state_with(|c| c.global_risk.max_wallet_exposure_ref = 10.0);
    let risk = RiskEngine::new(state.clone());
    state
        .ledger()
        .submit(sol_buy("wallet-x", "sniper", "MINT-A", 0.1, "a"))
        .await; // 10 on wallet-x
    let d = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-B",
            0.01,
            "wallet-x",
            "sniper",
        ))
        .await;
    assert!(d.reason.contains("wallet_exposure"), "{}", d.reason);
    // An empty wallet label falls back to the module name — a different account.
    let ok = risk
        .check_entry(&entry(
            BotModule::Sniper,
            Venue::PumpFun,
            "MINT-B",
            0.01,
            "",
            "",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    let recent = state.global_risk().recent_decisions(1).await;
    assert_eq!(recent[0].request.wallet, "sniper");
    assert_eq!(recent[0].request.strategy, "sniper");
    assert_eq!(recent[0].request.quote_asset, "SOL");
}

#[tokio::test]
async fn config_reload_updates_limits_and_kill_lists_live() {
    let state = state_with(|_| {});
    let risk = RiskEngine::new(state.clone());
    let ok = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    state
        .update_config(|c| c.global_risk.killed_venues = vec!["polymarket".into()])
        .await;
    let d = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(d.reason.contains("venue_kill_switch"), "{}", d.reason);
    state
        .update_config(|c| c.global_risk.killed_venues.clear())
        .await;
    let ok = risk
        .check_entry(&entry(
            BotModule::Polymarket,
            Venue::PolymarketClob,
            "TOKEN",
            10.0,
            "0xeoa",
            "value",
        ))
        .await;
    assert!(ok.allowed(), "{}", ok.reason);
    assert_eq!(state.global_risk().config().await.killed_venues.len(), 0);
    assert_eq!(state.global_risk().switches().active().len(), 0);
    // Module status stays untouched by global decisions.
    assert!(state.is_enabled(BotModule::Polymarket).await);
    assert!(state.module_status(BotModule::Polymarket).await.enabled);
    let _ = PositionStatus::Open;
}
