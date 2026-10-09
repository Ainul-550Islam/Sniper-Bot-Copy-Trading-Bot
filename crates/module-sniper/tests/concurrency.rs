//! Concurrency audit (TASK 2 §O): several sniper tasks sharing one state,
//! one ledger and one mock node race on the same launches; the invariants —
//! one position per launch, one confirmed entry intent per launch, no double
//! sells — must hold without any test-only locking.

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use solana_sdk::pubkey::Pubkey;

use bot_core::execution::ExecutionState;
use bot_core::models::{BotModule, PositionStatus};

use module_sniper::pipeline::{RejectReason, SniperStage};

use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_launch_seen_by_many_tasks_opens_exactly_one_position() {
    let mut cfg = base_config();
    cfg.risk.max_open_positions = 16;
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(cfg);

    let mut handles = Vec::new();
    for i in 0..6u8 {
        let (mut s, _w) = sniper(state.clone(), mock_rpc(&url)).await;
        // Same launch (same signature) observed by six feeds/tasks.
        let mut ev = pump_event(mint, creator, 1);
        ev.source_seq = i as u64 + 1;
        handles.push(tokio::spawn(async move { s.consider_event(ev).await }));
    }
    let mut outcomes = Vec::new();
    for h in handles {
        outcomes.push(h.await.unwrap());
    }
    let confirmed = outcomes
        .iter()
        .filter(|o| o.stage == SniperStage::Confirmed)
        .count();
    let duplicates = outcomes
        .iter()
        .filter(|o| o.rejection.as_ref().map(|r| r.reason) == Some(RejectReason::DuplicateEvent))
        .count();
    assert_eq!(confirmed, 1, "{outcomes:?}");
    assert_eq!(duplicates, 5, "{outcomes:?}");
    assert_eq!(state.open_positions_for(BotModule::Sniper).await.len(), 1);
    let intent = outcomes
        .iter()
        .find_map(|o| o.intent_id.clone())
        .expect("the winner published its intent");
    // TASK 5: exactly one typed fill event reached the global ledger (the
    // winner's), correlated to the snipe intent and mirroring the position.
    let ledger = state.ledger();
    assert_eq!(ledger.len().await, 1);
    let ev = &ledger.events().await[0].event;
    assert_eq!(ev.module, BotModule::Sniper);
    assert_eq!(ev.correlation_id.as_deref(), Some(intent.as_str()));
    assert_eq!(ev.quote_asset, "SOL");
    let position = &state.open_positions_for(BotModule::Sniper).await[0];
    assert_eq!(ev.position_id.as_deref(), Some(position.id.as_str()));
    let agg = ledger.open_positions().await;
    assert_eq!(agg.len(), 1);
    assert!((agg[0].qty - position.qty).abs() < 1e-9);
    let rec = bot_core::execution::ledger().get(&intent).await.unwrap();
    assert_eq!(rec.state, ExecutionState::Confirmed);
    assert_eq!(rec.attempts, 1, "no second attempt was ever begun");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_distinct_launches_in_parallel_each_get_one_position_and_one_intent() {
    let mut cfg = base_config();
    cfg.risk.max_open_positions = 64;
    cfg.risk.sniper_max_concurrent_positions = 64;
    cfg.risk.max_position_fraction = 1.0;
    let node = Arc::new(MockNode::default());
    node.default_balance.store(1_000 * SOL, Ordering::SeqCst);
    let url = spawn_mock_node(node.clone()).await;
    let state = state_with(cfg);
    let creator = Pubkey::new_unique();

    let mut mints = Vec::new();
    let mut handles = Vec::new();
    for tag in 1..=20u8 {
        let mint = Pubkey::new_unique();
        install_pump_token(&node, mint, creator, CurveSpec::fresh());
        mints.push(mint);
        let (mut s, _w) = sniper(state.clone(), mock_rpc(&url)).await;
        let ev = pump_event(mint, creator, tag);
        handles.push(tokio::spawn(async move { s.consider_event(ev).await }));
    }
    let mut ok = 0;
    for h in handles {
        let o = h.await.unwrap();
        assert!(o.rejection.is_none(), "{:?}", o.rejection);
        ok += 1;
    }
    assert_eq!(ok, 20);
    let positions = state.open_positions_for(BotModule::Sniper).await;
    assert_eq!(positions.len(), 20);
    let mut symbols: Vec<String> = positions.iter().map(|p| p.symbol.clone()).collect();
    symbols.sort();
    symbols.dedup();
    assert_eq!(symbols.len(), 20, "one position per mint");
    // One confirmed entry intent per mint in the shared ledger.
    let records = bot_core::execution::ledger().list(10_000).await;
    for mint in &mints {
        let entries: Vec<_> = records
            .iter()
            .filter(|r| r.symbol == mint.to_string() && r.label.starts_with("snipe"))
            .collect();
        assert_eq!(entries.len(), 1, "{mint}");
        assert_eq!(entries[0].state, ExecutionState::Confirmed);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entries_and_the_exit_sweeper_race_without_double_selling() {
    let mut cfg = base_config();
    cfg.risk.max_open_positions = 64;
    cfg.risk.sniper_max_concurrent_positions = 64;
    cfg.risk.max_position_fraction = 1.0;
    cfg.sniper.stop_loss_pct = None;
    cfg.sniper.take_profit_pct = None;
    let node = Arc::new(MockNode::default());
    node.default_balance.store(1_000 * SOL, Ordering::SeqCst);
    let url = spawn_mock_node(node.clone()).await;
    let state = state_with(cfg);
    let creator = Pubkey::new_unique();

    // A sweeper task hammering the book while entries land.
    let (mut sweeper, _w) = sniper(state.clone(), mock_rpc(&url)).await;
    let sweeper_state = state.clone();
    let sweeper_task = tokio::spawn(async move {
        loop {
            if sweeper_state.kill_switch() {
                // Final flatten pass, then stop.
                let _ = sweeper.sweep_once().await;
                let _ = sweeper.sweep_once().await;
                break;
            }
            let _ = sweeper.sweep_once().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });

    let mut handles = Vec::new();
    for tag in 1..=12u8 {
        let mint = Pubkey::new_unique();
        install_pump_token(&node, mint, creator, CurveSpec::fresh());
        let (mut s, _w) = sniper(state.clone(), mock_rpc(&url)).await;
        let ev = pump_event(mint, creator, tag);
        handles.push(tokio::spawn(async move { s.consider_event(ev).await }));
    }
    for h in handles {
        let o = h.await.unwrap();
        assert!(o.rejection.is_none(), "{:?}", o.rejection);
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // Flatten everything through the kill switch and let the sweeper finish.
    state.set_kill_switch(true, "test").await;
    sweeper_task.await.unwrap();

    let all = state.all_positions().await;
    let sniper_positions: Vec<_> = all
        .iter()
        .filter(|p| p.source == bot_core::models::TradeSource::Sniper)
        .collect();
    assert_eq!(sniper_positions.len(), 12);
    for p in &sniper_positions {
        assert_eq!(p.status, PositionStatus::StoppedOut, "{}", p.id);
        assert!(p.qty.abs() < 1e-9);
    }
    // Exactly one exit intent per position — no double sells.
    let records = bot_core::execution::ledger().list(10_000).await;
    for p in &sniper_positions {
        let exits: Vec<_> = records
            .iter()
            .filter(|r| r.symbol == p.symbol && r.label.starts_with("exit-"))
            .collect();
        assert_eq!(exits.len(), 1, "{}", p.symbol);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sweeps_over_the_same_position_sell_once() {
    // Two sweeper tasks (as two replicas without an ownership store would
    // be) decide the same exit at the same time: the deterministic exit
    // intent id makes the second attempt a ledger duplicate, so the chain
    // sees one sell.
    let mut cfg = live_config();
    cfg.risk.max_position_fraction = 1.0;
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(cfg);
    let (mut entry, wallet) = sniper(state.clone(), mock_rpc(&url)).await;
    let o = entry.consider_event(pump_event(mint, creator, 1)).await;
    assert_eq!(o.stage, SniperStage::Confirmed, "{:?}", o.rejection);
    let id = o.position_id.unwrap();
    let entry_sends = node.sends.load(Ordering::SeqCst);
    let e = state.position(&id).await.unwrap().avg_entry;
    state
        .with_position(&id, |p| p.stop_loss = Some(e * 2.0))
        .await;

    // Both sweepers share the wallet (same replica identity) and the ledger.
    let mut a = module_sniper::Sniper::new(state.clone(), mock_rpc(&url), wallet.clone(), None)
        .await
        .unwrap();
    let mut b = module_sniper::Sniper::new(state.clone(), mock_rpc(&url), wallet, None)
        .await
        .unwrap();
    let (ra, rb) = tokio::join!(a.sweep_once(), b.sweep_once());
    ra.unwrap();
    rb.unwrap();
    let sends = node.sends.load(Ordering::SeqCst) - entry_sends;
    assert_eq!(sends, 1, "exactly one sell transaction left the process");
    let p = state.position(&id).await.unwrap();
    assert_eq!(p.status, PositionStatus::StoppedOut);
}
