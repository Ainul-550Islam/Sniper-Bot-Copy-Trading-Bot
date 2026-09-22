//! Staged order pipeline (TASK 4): every exit is an explicit, journaled
//! `SignalOutcome`; paper fills book through the OMS + ledger; each gate
//! rejects with the documented reason and nothing reaches the venue.

mod common;

use std::sync::Arc;

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::oms::OrderStatus;
use chrono::{Duration, Utc};
use common::*;
use module_polymarket::orders::{PolyStage, RejectReason};
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::strategy::Quote;

#[tokio::test]
async fn paper_signal_runs_every_stage_and_books_a_fill() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let sig = signal(decision(), ExecutionMode::Paper);
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;

    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert!(out.reject_reason.is_none());
    assert!(out.reached_venue());
    let order_id = out.order_id.clone().expect("oms order id");
    let venue_id = out.venue_order_id.clone().expect("paper order id");
    assert!(venue_id.starts_with("paper:"));
    assert_eq!(out.size_tokens, Some(25.0));
    assert!((out.approved_stake.unwrap() - 10.0).abs() < 1e-9);

    // OMS: one order, keyed by the intent, terminal Filled, with the paper
    // id attached and a fill execution recorded.
    let order = oms.get(&order_id).await.expect("oms order");
    assert_eq!(order.idempotency_key, sig.intent_key());
    assert_eq!(order.status, OrderStatus::Filled);
    assert_eq!(order.external_id.as_deref(), Some(venue_id.as_str()));
    assert_eq!(order.module, BotModule::Polymarket);
    assert_eq!(oms.list(50).await.len(), 1);

    // Position + trade booked through shared state.
    let pos = state
        .find_open(BotModule::Polymarket, YES)
        .await
        .expect("position opened");
    assert!((pos.qty - 25.0).abs() < 1e-9);
    assert_eq!(pos.market_id.as_deref(), Some(CONDITION));
    assert_eq!(out.position_id.as_deref(), Some(pos.id.as_str()));
    assert_eq!(fill_events(&state).await, 1);

    // Journal: signal, order and fill rows.
    let signals = store.signals().await;
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].stage, "FILLED");
    assert_eq!(signals[0].order_id.as_deref(), Some(order_id.as_str()));
    let orders = store.orders().await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].state, "filled");
    assert!(orders[0].closed_at.is_some());
    assert_eq!(store.fills().await.len(), 1);
    assert_eq!(store.fills().await[0].source, "paper");

    // Nothing was posted to the venue and the tracker holds the paper order.
    assert_eq!(venue.posted_orders(), 0);
    let tracked = bot.tracked_orders().await;
    assert_eq!(tracked.len(), 1);
    assert!(tracked[0].state.is_terminal());

    // Audit trail names the terminal stage.
    let actions = audit_actions(&state).await;
    assert!(
        actions.iter().any(|a| a == "poly.signal.filled"),
        "{actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "poly.order.filled"),
        "{actions:?}"
    );

    // TASK 5: the fill reached the global ledger exactly once as a typed
    // event that mirrors the module's own record (same quantity, position
    // id, venue fill id as reference) — the module did not touch the book.
    let ledger = state.ledger();
    assert_eq!(ledger.len().await, 1);
    let events = ledger.events().await;
    let ev = &events[0].event;
    assert_eq!(ev.kind, bot_core::accounting::EventKind::Fill);
    assert_eq!(ev.module, BotModule::Polymarket);
    assert_eq!(ev.asset, YES);
    assert_eq!(ev.quote_asset, "USDC");
    assert!((ev.quantity - 25.0).abs() < 1e-9);
    assert_eq!(ev.position_id.as_deref(), Some(pos.id.as_str()));
    assert_eq!(ev.correlation_id.as_deref(), Some(order_id.as_str()));
    assert_eq!(ev.reference_id, store.fills().await[0].fill_id);
    assert_eq!(ev.strategy, cfg.strategy);
    let book = ledger.book().await;
    let agg = book.open_positions().next().expect("aggregated position");
    assert!((agg.qty - pos.qty).abs() < 1e-9);
    assert!((agg.cost_basis - pos.cost_basis).abs() < 1e-9);
    // Module truth and the ledger reconcile without findings.
    let run = ledger
        .reconcile(
            &oms.list(50).await,
            &state.all_positions().await,
            &state.trades(50).await,
            state.started_at(),
            bot_core::reconciliation::QuantityTolerance::default(),
        )
        .await;
    assert!(run.findings.is_empty(), "{:?}", run.findings);
}

#[tokio::test]
async fn validation_rejects_before_any_side_effect() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let cases: Vec<(
        &str,
        module_polymarket::strategy::OrderDecision,
        RejectReason,
    )> = vec![
        (
            "non-numeric token",
            decision_for("0xabc", "Yes", 0.40, 25.0),
            RejectReason::InvalidToken,
        ),
        (
            "price 0",
            decision_for(YES, "Yes", 0.0, 25.0),
            RejectReason::InvalidPrice,
        ),
        (
            "price 1",
            decision_for(YES, "Yes", 1.0, 25.0),
            RejectReason::InvalidPrice,
        ),
        (
            "nan size",
            decision_for(YES, "Yes", 0.40, f64::NAN),
            RejectReason::InvalidSize,
        ),
        (
            "zero size",
            decision_for(YES, "Yes", 0.40, 0.0),
            RejectReason::InvalidSize,
        ),
    ];
    for (name, d, expected) in cases {
        let sig = signal(d, ExecutionMode::Paper);
        let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
        assert_eq!(out.stage, PolyStage::Rejected, "{name}: {out:?}");
        assert_eq!(out.reject_reason, Some(expected), "{name}: {out:?}");
        assert!(
            out.order_id.is_none(),
            "{name}: no OMS order for a rejected signal"
        );
    }
    assert!(oms.list(10).await.is_empty());
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
    assert_eq!(
        store.signals().await.len(),
        5,
        "every rejection is journaled"
    );
    assert!(store.orders().await.is_empty());
}

#[tokio::test]
async fn market_and_quote_gates_reject_with_the_strategy_reason() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(base_config(&venue));
    let bot = paper_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let mut cfg = poly_cfg(&state).await;

    // Closed market.
    let mut closed = market();
    closed.closed = true;
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &closed,
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::MarketNotTradeable),
        "{out:?}"
    );

    // Resolving soon.
    cfg.min_time_to_resolution_secs = 3_600;
    let mut soon = market();
    soon.end_date = Some(Utc::now() + Duration::minutes(5));
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &soon,
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::ResolvingSoon),
        "{out:?}"
    );
    cfg.min_time_to_resolution_secs = 0;

    // No quote for the token.
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes_for("999", NO),
            &cfg,
        )
        .await;
    assert_eq!(out.reject_reason, Some(RejectReason::NoQuote), "{out:?}");

    // Stale quote (older than quote_max_age_secs).
    let mut stale = quotes();
    stale.insert(
        YES.into(),
        Quote::observed(0.39, 0.40, Utc::now() - Duration::seconds(600)),
    );
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &stale,
            &cfg,
        )
        .await;
    assert_eq!(out.reject_reason, Some(RejectReason::StaleQuote), "{out:?}");

    // Spread too wide.
    let mut wide = quotes();
    wide.insert(YES.into(), Quote::observed(0.20, 0.40, Utc::now()));
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &wide,
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::SpreadTooWide),
        "{out:?}"
    );

    // One-sided book.
    let mut one = quotes();
    one.insert(YES.into(), Quote::observed(0.0, 0.40, Utc::now()));
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &one,
            &cfg,
        )
        .await;
    assert_eq!(out.reject_reason, Some(RejectReason::BadBook), "{out:?}");

    // Limit price outside the [bid, ask + tick] band for a buy.
    let out = bot
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.45, 25.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::PriceOutsideBand),
        "{out:?}"
    );
    let out = bot
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.30, 25.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::PriceOutsideBand),
        "{out:?}"
    );
    // At the ask + one tick is still inside the band.
    let out = bot
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.41, 25.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_some());
}

#[tokio::test]
async fn exposure_gates_reject_second_entries_and_market_caps() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let bot = paper_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let mut cfg = poly_cfg(&state).await;

    // First entry fills.
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled);

    // Same token again (different price so the intent key differs): already
    // in market.
    let out = bot
        .process_signal(
            &signal(decision_for(YES, "Yes", 0.39, 25.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::AlreadyInMarket),
        "{out:?}"
    );

    // Max open markets: one market open, cap 1, a different market rejects
    // while the same market's other outcome is still allowed.
    cfg.max_open_markets = 1;
    let mut d = decision_for(YES_B, "Yes", 0.40, 25.0);
    d.condition_id = CONDITION_B.into();
    let out = bot
        .process_signal(
            &signal(d, ExecutionMode::Paper),
            &market_b(),
            &quotes_for(YES_B, NO_B),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::MaxOpenMarkets),
        "{out:?}"
    );
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert_eq!(
        oms.list(10).await.len(),
        2,
        "exactly the two fills created orders"
    );

    // Size below the venue minimum.
    let out = bot
        .process_signal(
            &signal(decision_for(YES_B, "Yes", 0.40, 3.0), ExecutionMode::Paper),
            &market(),
            &quotes_for(YES_B, NO_B),
            &cfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::SizeTooSmall),
        "{out:?}"
    );
}

#[tokio::test]
async fn the_shared_risk_engine_is_the_only_risk_decision() {
    let venue = mock_venue().await;
    let mut cfg = base_config(&venue);
    // Per-order cap below the requested stake → the ONE risk engine resizes.
    cfg.risk.poly_max_position_quote = 4.0;
    let (state, oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let pcfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    // 4 USDC / 0.40 = 10 tokens.
    assert_eq!(out.size_tokens, Some(10.0));
    assert!((out.approved_stake.unwrap() - 4.0).abs() < 1e-9);
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 10.0).abs() < 1e-9);
    assert_eq!(
        oms.list(10).await[0].qty,
        10.0,
        "OMS records the approved size"
    );

    // A cap that resizes below the venue minimum rejects as SizeTooSmall
    // (never a tiny order).
    state
        .update_config(|c| c.risk.poly_max_position_quote = 1.0)
        .await;
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::SizeTooSmall),
        "{out:?}"
    );

    // Kill switch → preflight rejection with the risk code mapped. (Each
    // case uses a distinct size so its signal identity — and journal row —
    // is distinct.)
    state
        .update_config(|c| c.risk.poly_max_position_quote = 0.0)
        .await;
    state.set_kill_switch(true, "test").await;
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 21.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.reject_reason, Some(RejectReason::KillSwitch), "{out:?}");
    state.set_kill_switch(false, "test").await;

    // Module emergency disable → EmergencyDisabled.
    state
        .update_config(|c| c.risk.poly_emergency_disable = true)
        .await;
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 22.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::EmergencyDisabled),
        "{out:?}"
    );
    state
        .update_config(|c| c.risk.poly_emergency_disable = false)
        .await;

    // Price outside the risk floor/ceiling → VenueCheck maps to RiskRejected.
    state
        .update_config(|c| c.risk.poly_price_ceiling = 0.50)
        .await;
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 23.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::RiskRejected),
        "{out:?}"
    );

    // Every rejection above was journaled with its reason.
    let rejected: Vec<String> = store
        .signals()
        .await
        .into_iter()
        .filter_map(|s| s.reject_reason)
        .collect();
    assert_eq!(rejected.len(), 4, "{rejected:?}");
    assert!(rejected.iter().any(|r| r == "SIZE_TOO_SMALL"));
    assert!(rejected.iter().any(|r| r == "KILL_SWITCH"));
    assert!(rejected.iter().any(|r| r == "EMERGENCY_DISABLED"));
    assert!(rejected.iter().any(|r| r == "RISK_REJECTED"));

    // The same decision re-issued keeps ONE journal row (deterministic
    // signal identity), updated in place.
    let before = store.signals().await.len();
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 23.0), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.reject_reason, Some(RejectReason::RiskRejected));
    assert_eq!(store.signals().await.len(), before);
}

#[tokio::test]
async fn live_mode_without_a_verified_collateral_read_rejects() {
    let venue = mock_venue().await;
    let mut cfg = live_config(&venue);
    // No RPC → no collateral reader → live sizing has nothing verified.
    cfg.polymarket.ctf_rpc_url = String::new();
    let (state, oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let pcfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Rejected, "{out:?}");
    assert_eq!(out.reject_reason, Some(RejectReason::CollateralUnavailable));
    assert!(
        oms.list(10).await.is_empty(),
        "rejected before the OMS record"
    );
    assert_eq!(venue.posted_orders(), 0, "nothing was signed or posted");
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_none());
}

#[tokio::test]
async fn live_sizing_uses_the_verified_balance_not_the_paper_seed() {
    let venue = mock_venue().await;
    // 3 USDC on chain; the shared state carries a 1 000 USDC demo seed.
    venue.set_balance(3_000_000, 3_000_000);
    let (state, oms) = state_with_oms(live_config(&venue));
    state.set_balances(None, Some(1_000.0)).await;
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let pcfg = poly_cfg(&state).await;

    // The ONE risk engine sizes against the verified 3 USDC: 7.5 tokens.
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    assert_eq!(out.size_tokens, Some(7.5));
    assert!((out.approved_stake.unwrap() - 3.0).abs() < 1e-9);
    assert_eq!(oms.list(10).await[0].qty, 7.5);
    let posted = venue.last_body("POST", "/clob/order").unwrap();
    // BUY makerAmount is the USDC side in 6-dp raw units: 3 USDC.
    assert_eq!(posted["order"]["makerAmount"], "3000000");
}

#[tokio::test]
async fn live_funding_shortfall_rejects_before_signing() {
    let venue = mock_venue().await;
    // 15 USDC balance + allowance.
    venue.set_balance(15_000_000, 15_000_000);
    let (state, oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let pcfg = poly_cfg(&state).await;

    // First order rests and commits 10 USDC of collateral.
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    assert_eq!(oms.list(10).await.len(), 1);

    // Second order (11 USDC) fits the balance alone but not on top of the
    // resting commitment → rejected BEFORE any OMS record or signature.
    let out = bot
        .process_signal(
            &signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Live),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Rejected, "{out:?}");
    assert_eq!(out.reject_reason, Some(RejectReason::InsufficientFunding));
    assert!(out.detail.contains("reserved"), "{}", out.detail);
    assert_eq!(
        oms.list(10).await.len(),
        1,
        "no OMS record for the rejected signal"
    );
    assert_eq!(venue.posted_orders(), 1);
}

#[tokio::test]
async fn missing_exchange_allowance_rejects_eoa_orders() {
    let venue = mock_venue().await;
    // Balance fine but the exchange allowance is missing (EOA signing).
    venue.set_balance(1_000_000_000, 0);
    let (state, oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let pcfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::InsufficientFunding),
        "{out:?}"
    );
    assert!(out.detail.contains("allowance"), "{}", out.detail);
    assert!(oms.list(10).await.is_empty());
    assert_eq!(venue.posted_orders(), 0);
    // The allowance was read against the exchange that settles this order.
    let allowance_calls = venue
        .captures()
        .into_iter()
        .filter(|c| c.path == "/rpc" && c.body.contains("dd62ed3e"))
        .count();
    assert_eq!(allowance_calls, 1);
}

#[tokio::test]
async fn process_market_turns_strategy_verdicts_into_outcomes() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let bot = paper_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let cfg = poly_cfg(&state).await;

    // Value strategy on the fixture: both legs enter (basket edge 0.05).
    let outcomes = bot.process_market(&market(), &quotes(), &cfg).await;
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert!(outcomes.iter().all(|o| o.stage == PolyStage::Filled));
    assert_eq!(oms.list(10).await.len(), 2);
    assert!(state.find_open(BotModule::Polymarket, YES).await.is_some());
    assert!(state.find_open(BotModule::Polymarket, NO).await.is_some());

    // No edge → no decisions, no outcomes, nothing new in the OMS.
    let mut flat = quotes();
    flat.insert(YES.into(), Quote::observed(0.49, 0.50, Utc::now()));
    flat.insert(NO.into(), Quote::observed(0.49, 0.50, Utc::now()));
    let outcomes = bot.process_market(&market(), &flat, &cfg).await;
    assert!(outcomes.is_empty());
    assert_eq!(oms.list(10).await.len(), 2);
}
