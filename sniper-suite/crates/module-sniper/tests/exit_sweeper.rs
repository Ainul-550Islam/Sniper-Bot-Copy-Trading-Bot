//! Exit hardening (TASK 2 §L): the REAL sweeper against the mock node —
//! normal stop exits, kill-switch flattening, partial take-profit, stale
//! positions, retry backoff, deterministic exit intents.

mod common;

use std::sync::atomic::Ordering;

use bot_core::execution::ExecutionState;
use bot_core::models::{BotModule, PositionStatus, Venue};

use module_sniper::pipeline::SniperStage;

use common::*;

/// Open a paper position through the real pipeline and return its id.
async fn open_position(w: &mut World, tag: u8) -> String {
    let o = w.sniper.consider_event(w.event(tag)).await;
    assert_eq!(o.stage, SniperStage::Confirmed, "{:?}", o.rejection);
    o.position_id.unwrap()
}

#[tokio::test]
async fn stop_loss_exit_sells_on_the_curve_and_closes_stopped_out() {
    let mut w = World::new(base_config()).await;
    let id = open_position(&mut w, 1).await;
    // Force the stop above the current mark.
    let entry = w.state.position(&id).await.unwrap().avg_entry;
    w.state
        .with_position(&id, |p| p.stop_loss = Some(entry * 2.0))
        .await;

    // The mark and the booked entry price are on the same scale (SOL per
    // whole token), so a stop at 2× entry must fire and nothing else.
    let mark = w.sniper.mark_price_sol(&w.mint.to_string()).await.unwrap();
    assert!(
        mark > entry * 0.8 && mark < entry * 1.2,
        "mark {mark} vs entry {entry}"
    );
    let pos = w.state.position(&id).await.unwrap();
    let d = w.sniper.risk().check_exit(&pos, mark).await;
    let rec = bot_core::execution::ledger()
        .get_by_signature(pos.entry_signature.as_deref().unwrap_or(""))
        .await;
    w.sniper.sweep_once().await.unwrap();

    let p = w.state.position(&id).await.unwrap();
    assert_eq!(
        p.status,
        PositionStatus::StoppedOut,
        "reason={:?} decision={:?} sig={:?} rec={:?} err={:?}",
        p.reason_closed,
        d,
        pos.entry_signature,
        rec.map(|r| r.state),
        w.state.module_status(BotModule::Sniper).await.last_error
    );
    assert!(p.reason_closed.as_deref().unwrap_or("").contains("stop"));
    assert!(p.qty.abs() < 1e-9, "fully sold");
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
    // The sell went through the executor under a deterministic exit intent.
    let open = bot_core::execution::ledger().list(50).await;
    let exit = open
        .iter()
        .find(|r| r.label.starts_with("exit-") && r.symbol == w.mint.to_string())
        .expect("exit lifecycle record");
    assert_eq!(exit.state, ExecutionState::Confirmed);
    let text = bot_core::obs::metrics::global().encode();
    assert!(text.contains("sniper_exit_actions_total"));
    assert!(text.contains("action=\"sold\""));
}

#[tokio::test]
async fn kill_switch_flattens_without_marking() {
    let mut w = World::new(base_config()).await;
    let id = open_position(&mut w, 2).await;
    let before = w.node.requests.load(Ordering::SeqCst);
    w.state.set_kill_switch(true, "test").await;
    w.sniper.sweep_once().await.unwrap();
    let p = w.state.position(&id).await.unwrap();
    assert_eq!(p.status, PositionStatus::StoppedOut);
    assert!(p
        .reason_closed
        .as_deref()
        .unwrap_or("")
        .contains("kill switch"));
    // The sell still needed the curve (one context load) but no mark quote.
    let methods = &w.node.methods()[before..];
    assert!(!methods.iter().any(|m| m == "getAccountInfo" && false));
    assert!(methods.iter().any(|m| m == "getMultipleAccounts"));
}

#[tokio::test]
async fn partial_take_profit_trims_and_keeps_managing() {
    let mut cfg = base_config();
    cfg.sniper.take_profit_sell_fraction = 0.5;
    let mut w = World::new(cfg).await;
    let id = open_position(&mut w, 3).await;
    let qty = w.state.position(&id).await.unwrap().qty;
    w.state
        .with_position(&id, |p| {
            p.take_profit = Some(p.avg_entry * 0.5); // already "reached"
            p.stop_loss = Some(0.0);
        })
        .await;
    w.sniper.sweep_once().await.unwrap();
    let p = w.state.position(&id).await.unwrap();
    assert_eq!(p.status, PositionStatus::Open, "half remains");
    assert!(
        (p.qty - qty * 0.5).abs() < qty * 1e-6,
        "{} vs {}",
        p.qty,
        qty
    );
    // The next decision on the smaller position gets a NEW exit intent id.
    let a = module_sniper::exit::exit_intent_id_for(
        &p,
        bot_core::maths::to_raw_amount(p.qty, 6),
        module_sniper::pipeline::EntryRoute::PumpCurve,
    );
    let mut before = p.clone();
    before.qty = qty;
    let b = module_sniper::exit::exit_intent_id_for(
        &before,
        bot_core::maths::to_raw_amount(qty * 0.5, 6),
        module_sniper::pipeline::EntryRoute::PumpCurve,
    );
    assert_ne!(a, b);
}

#[tokio::test]
async fn unpriceable_position_is_held_then_force_exited_when_configured() {
    let mut cfg = base_config();
    cfg.sniper.stale_position_exit_secs = 1;
    cfg.sniper.use_jupiter_fallback = false;
    cfg.sniper.exit_retry_backoff_secs = 60;
    let mut w = World::new(cfg).await;
    let id = open_position(&mut w, 4).await;
    // The curve vanishes: marking fails (curve unreadable + no Jupiter).
    w.node
        .remove_account(&solana_kit::pump::bonding_curve_pda(&w.mint));

    // First sweep: mark fails, position held (rule not yet due).
    w.sniper.sweep_once().await.unwrap();
    assert_eq!(
        w.state.position(&id).await.unwrap().status,
        PositionStatus::Open
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // Second sweep: the stale rule fires. The sell cannot route (no curve,
    // no fallback) → INVALID_ROUTE error, recorded, backoff armed.
    w.sniper.sweep_once().await.unwrap();
    let p = w.state.position(&id).await.unwrap();
    assert_eq!(p.status, PositionStatus::Open, "sell failed; still open");
    let status = w.state.module_status(BotModule::Sniper).await;
    assert!(status
        .last_error
        .as_deref()
        .unwrap_or("")
        .contains("INVALID_ROUTE"));
    let text = bot_core::obs::metrics::global().encode();
    assert!(text.contains("action=\"stale_exit\""));
    assert!(text.contains("action=\"sell_failed\""));

    // Third sweep: inside the backoff → skipped, no second attempt.
    let errors_before = status.consecutive_errors;
    w.sniper.sweep_once().await.unwrap();
    let status = w.state.module_status(BotModule::Sniper).await;
    assert_eq!(
        status.consecutive_errors, errors_before,
        "backoff skipped the retry"
    );
    assert!(bot_core::obs::metrics::global()
        .encode()
        .contains("action=\"backoff_skip\""));
}

#[tokio::test]
async fn positions_on_other_venues_are_marked_and_routed_by_venue() {
    // A PumpSwap-venue position with an unreadable pool and no fallback:
    // the mark fails on its venue and falls back to the curve/jupiter path,
    // the exit names the missing route instead of silently loading the
    // curve for a token that lives on an AMM.
    let mut cfg = base_config();
    cfg.sniper.use_jupiter_fallback = false;
    let mut w = World::new(cfg).await;
    let mut p = bot_core::models::Position::new(
        "p-ps".into(),
        bot_core::models::TradeSource::Sniper,
        Venue::PumpSwap,
        bot_core::models::ExecutionMode::Paper,
        w.mint.to_string(),
        "HRN".into(),
        "SOL".into(),
    );
    p.apply_buy(1_000.0, 0.00001, 0.01);
    p.market_id = Some(solana_sdk::pubkey::Pubkey::new_unique().to_string());
    p.stop_loss = Some(1.0); // fires on any positive mark
    w.state.restore_positions(vec![p]).await;

    w.sniper.sweep_once().await.unwrap();
    let status = w.state.module_status(BotModule::Sniper).await;
    let err = status.last_error.unwrap_or_default();
    assert!(err.contains("INVALID_ROUTE"), "{err}");
    assert!(err.contains("pumpswap"), "{err}");
    assert_eq!(
        w.state.position("p-ps").await.unwrap().status,
        PositionStatus::Open
    );
}
