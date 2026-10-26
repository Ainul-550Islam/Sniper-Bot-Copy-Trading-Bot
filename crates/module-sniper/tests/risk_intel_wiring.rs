//! Production wiring tests for the risk-intel service.
//!
//! These tests use the scripted mock node and the real
//! `Sniper::consider_event` pipeline. They prove that observed first-slot
//! buys reach the existing bundler gate, missing observations remain
//! unavailable (and therefore fail closed when the gate is enabled), and
//! a recorded rug outcome participates in the existing core repeat-offender
//! screen on the creator's next launch.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use solana_kit::token_safety::SellProbeOutcome;
use solana_sdk::pubkey::Pubkey;

use module_sniper::market::load_market_with_risk_intel;
use module_sniper::pipeline::RejectReason;
use module_sniper::risk_intel::bundler::FirstSlotBuy;
use module_sniper::risk_intel::creator_history::LaunchOutcome;
use module_sniper::risk_intel::service::RiskIntelService;

use common::*;

fn buys_from_one_funder(count: u8) -> Vec<FirstSlotBuy> {
    let funder = Pubkey::new_from_array([99; 32]);
    (1..=count)
        .map(|seed| FirstSlotBuy {
            buyer: Pubkey::new_from_array([seed; 32]),
            funder,
        })
        .collect()
}

#[tokio::test]
async fn observed_first_slot_buys_make_the_bundler_gate_reject() {
    let mut cfg = base_config();
    cfg.sniper.max_bundler_ratio = 0.5;
    let mut world = World::new(cfg).await;

    let mut event = world.event(41);
    event.first_slot_buys = buys_from_one_funder(4);

    let outcome = world.sniper.consider_event(event).await;
    let rejection = outcome.rejection.expect("bundled launch must be rejected");
    assert_eq!(rejection.reason, RejectReason::ConcentrationLimit);
    assert!(
        rejection.detail.contains("gate bundler_detection"),
        "{}",
        rejection.detail
    );
    assert!(
        outcome.gates.contains("bundler_detection=fail"),
        "{}",
        outcome.gates
    );
}

#[tokio::test]
async fn missing_first_slot_data_stays_fail_closed_when_gate_is_enabled() {
    let mut cfg = base_config();
    cfg.sniper.max_bundler_ratio = 0.5;
    let mut world = World::new(cfg).await;

    // The helper event carries no first-slot buyer/funder observations.
    let outcome = world.sniper.consider_event(world.event(42)).await;
    let rejection = outcome
        .rejection
        .expect("an enabled bundler gate must not pass without its signal");
    assert_eq!(rejection.reason, RejectReason::ConcentrationLimit);
    assert!(
        rejection.detail.contains("no bundler signal"),
        "{}",
        rejection.detail
    );
}

#[tokio::test]
async fn second_sell_probe_within_ttl_is_served_from_cache() {
    let service = RiskIntelService::new();
    let mint = Pubkey::new_unique();
    let now = Utc::now();
    let calls = Arc::new(AtomicUsize::new(0));

    let first_calls = Arc::clone(&calls);
    let first = service
        .sell_probe_with_cache(&mint, now, || async move {
            first_calls.fetch_add(1, Ordering::SeqCst);
            SellProbeOutcome::Sellable
        })
        .await;
    let second_calls = Arc::clone(&calls);
    let second = service
        .sell_probe_with_cache(&mint, now, || async move {
            second_calls.fetch_add(1, Ordering::SeqCst);
            SellProbeOutcome::NotSellable("second probe must not run".into())
        })
        .await;

    assert_eq!(first, SellProbeOutcome::Sellable);
    assert_eq!(second, SellProbeOutcome::Sellable);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn second_market_load_within_ttl_reuses_cached_sell_probe() {
    let mut cfg = base_config();
    cfg.sniper.simulate_sell = true;
    let sniper_cfg = cfg.sniper.clone();
    let platform_fee_cfg = cfg.platform_fee.clone();
    let world = World::new(cfg).await;
    let rpc = mock_rpc(&world.url);
    let event = world.event(44);
    let service = world.sniper.risk_intel_service();
    let now = Utc::now();

    let first = load_market_with_risk_intel(
        &rpc,
        &world.wallet.pubkey,
        &event,
        &sniper_cfg,
        &platform_fee_cfg,
        50_000_000,
        now,
        Some(&service),
    )
    .await
    .expect("first market load succeeds");
    let simulations_after_first = world.node.simulations.load(Ordering::SeqCst);
    assert_eq!(simulations_after_first, 1);

    let second = load_market_with_risk_intel(
        &rpc,
        &world.wallet.pubkey,
        &event,
        &sniper_cfg,
        &platform_fee_cfg,
        50_000_000,
        now,
        Some(&service),
    )
    .await
    .expect("second market load succeeds");

    assert_eq!(first.snapshot.sell_probe, second.snapshot.sell_probe);
    assert_eq!(
        world.node.simulations.load(Ordering::SeqCst),
        simulations_after_first,
        "the second load must reuse the cached probe instead of simulating again"
    );
}

#[tokio::test]
async fn recorded_rug_outcome_rejects_that_creators_next_launch() {
    let mut cfg = base_config();
    cfg.risk.block_repeat_offender_creators = true;
    let mut world = World::new(cfg).await;

    let service = world.sniper.risk_intel_service();
    let previous_mint = Pubkey::new_unique();
    service.note_launch(&previous_mint, Some(&world.creator));
    let recorded = service.record_position_close(&previous_mint, 1.0, 0.01, Utc::now());
    assert_eq!(recorded, Some(LaunchOutcome::Rugged));
    assert_eq!(service.rug_rate_bps(&world.creator), Some(10_000));

    let outcome = world.sniper.consider_event(world.event(43)).await;
    let rejection = outcome
        .rejection
        .expect("the recorded serial rugger's next launch must be rejected");
    assert_eq!(rejection.reason, RejectReason::RiskRejected);
    assert!(
        rejection.detail.contains("previously rugged"),
        "{}",
        rejection.detail
    );
}

#[tokio::test]
async fn full_position_close_records_the_creator_outcome() {
    let mut cfg = base_config();
    cfg.risk.block_repeat_offender_creators = true;
    let mut world = World::new(cfg).await;

    let opened = world.sniper.consider_event(world.event(45)).await;
    assert_eq!(
        opened.stage,
        module_sniper::pipeline::SniperStage::Confirmed,
        "{:?}",
        opened.rejection
    );
    let position_id = opened.position_id.expect("paper position is booked");

    // Force a stop exit whose realised price is far below the position's
    // booked entry. This uses the real sweeper and full-close hook; no
    // ledger outcome is written directly by the test.
    world
        .state
        .with_position(&position_id, |position| {
            position.stop_loss = Some(position.avg_entry * 2.0);
            position.avg_entry *= 20.0;
        })
        .await;
    world
        .sniper
        .sweep_once()
        .await
        .expect("exit sweep succeeds");
    let closed = world
        .state
        .position(&position_id)
        .await
        .expect("closed position remains readable");
    assert_eq!(closed.status, bot_core::models::PositionStatus::StoppedOut);

    let service = world.sniper.risk_intel_service();
    assert_eq!(service.rug_rate_bps(&world.creator), Some(10_000));

    let next_mint = Pubkey::new_unique();
    install_pump_token(&world.node, next_mint, world.creator, CurveSpec::fresh());
    let outcome = world
        .sniper
        .consider_event(pump_event(next_mint, world.creator, 46))
        .await;
    let rejection = outcome
        .rejection
        .expect("the creator's next launch must be rejected after the close");
    assert_eq!(rejection.reason, RejectReason::RiskRejected);
    assert!(
        rejection.detail.contains("previously rugged"),
        "{}",
        rejection.detail
    );
}
