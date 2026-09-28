//! Concurrency audit (TASK 3 test 19): several copy workers sharing one
//! state, one ledger and one mock node race on the same leader events; the
//! invariants — one position per leader trade, one intent per logical
//! mirror, no double exits, independent leaders do not interfere — must hold
//! without any test-only locking.

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use solana_sdk::pubkey::Pubkey;

use bot_core::models::{BotModule, PositionStatus};
use module_copy::event::{CopyStage, EventSource, RejectReason};
use module_copy::recovery::MemoryCopyStore;

use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_leader_trade_seen_by_many_workers_opens_exactly_one_position() {
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(copy_config());
    state.set_balances(Some(10.0), Some(1_000.0)).await;
    let store = Arc::new(MemoryCopyStore::new());
    let raw = leader_buy(LEADER, mint, 1, 0.5);

    let mut handles = Vec::new();
    for i in 0..6u64 {
        let (bot, _w) = copy_bot(state.clone(), mock_rpc(&url)).await;
        let mut bot = bot.with_copy_store(store.clone());
        // Same leader trade observed by six feeds / workers, alternating sources.
        let source = if i % 2 == 0 {
            EventSource::PumpPortal
        } else {
            EventSource::LogsPoll
        };
        let ev = event(&raw, source, i + 1);
        let st = state.clone();
        handles.push(tokio::spawn(async move {
            let cfg = st.config_snapshot().await;
            bot.process_event(&ev, &cfg).await
        }));
    }
    let mut outcomes = Vec::new();
    for h in handles {
        outcomes.push(h.await.unwrap());
    }
    let filled = outcomes
        .iter()
        .filter(|o| o.stage == CopyStage::Filled)
        .count();
    let dups = outcomes
        .iter()
        .filter(|o| o.rejection.as_ref().map(|r| r.reason) == Some(RejectReason::DuplicateEvent))
        .count();
    assert_eq!(filled, 1, "{outcomes:?}");
    assert_eq!(dups, 5, "{outcomes:?}");
    assert_eq!(state.open_positions_for(BotModule::Copy).await.len(), 1);
    assert_eq!(store.links().len(), 1);
    assert_eq!(store.events().len(), 1, "duplicates are not journaled");
    assert_eq!(state.seen_copy_event_count().await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distinct_leader_trades_in_parallel_all_mirror_and_share_nothing() {
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let creator = Pubkey::new_unique();
    let mut cfg = copy_config();
    cfg.copy.wallets.push(rule(LEADER_B));
    cfg.copy.strict_ordering = true;
    let state = state_with(cfg);
    state.set_balances(Some(10.0), Some(1_000.0)).await;
    let store = Arc::new(MemoryCopyStore::new());

    let mut handles = Vec::new();
    for i in 0..8u8 {
        let mint = Pubkey::new_unique();
        install_pump_token(&node, mint, creator, CurveSpec::fresh());
        let leader = if i % 2 == 0 { LEADER } else { LEADER_B };
        let (bot, _w) = copy_bot(state.clone(), mock_rpc(&url)).await;
        let mut bot = bot.with_copy_store(store.clone());
        let mut raw = leader_buy(leader, mint, 10 + i, 0.5);
        // Deliberately shuffled slots: each worker has its own ordering
        // tracker, so cross-worker order never rejects.
        raw.slot = 1_000 - i as u64;
        let ev = event(&raw, EventSource::PumpPortal, i as u64 + 1);
        let st = state.clone();
        handles.push(tokio::spawn(async move {
            let cfg = st.config_snapshot().await;
            bot.process_event(&ev, &cfg).await
        }));
    }
    let mut filled = 0;
    let mut intents = std::collections::HashSet::new();
    for h in handles {
        let o = h.await.unwrap();
        assert_eq!(o.stage, CopyStage::Filled, "{:?}", o.rejection);
        filled += 1;
        assert!(
            intents.insert(o.intent_id.clone().unwrap()),
            "intent ids are unique"
        );
    }
    assert_eq!(filled, 8);
    let positions = state.open_positions_for(BotModule::Copy).await;
    assert_eq!(positions.len(), 8);
    assert_eq!(
        positions
            .iter()
            .filter(|p| p.copied_wallet.as_deref() == Some(LEADER))
            .count(),
        4
    );
    assert_eq!(store.links().len(), 8);
    assert_eq!(store.events().len(), 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_mirrored_exits_sell_a_position_at_most_once() {
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(copy_config());
    state.set_balances(Some(10.0), Some(1_000.0)).await;
    let store = Arc::new(MemoryCopyStore::new());

    // One mirrored position.
    let (bot, _w) = copy_bot(state.clone(), mock_rpc(&url)).await;
    let mut bot = bot.with_copy_store(store.clone());
    let cfg = state.config_snapshot().await;
    let out = bot
        .process_event(
            &event(
                &leader_buy(LEADER, mint, 20, 0.5),
                EventSource::PumpPortal,
                1,
            ),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    let pos_id = out.position_id.clone().unwrap();

    // The same leader SELL seen by four workers at once.
    let raw_sell = leader_sell(LEADER, mint, 21, 1_000_000.0);
    let mut handles = Vec::new();
    for i in 0..4u64 {
        let (bot, _w) = copy_bot(state.clone(), mock_rpc(&url)).await;
        let mut bot = bot.with_copy_store(store.clone());
        let ev = event(&raw_sell, EventSource::PumpPortal, i + 2);
        let st = state.clone();
        handles.push(tokio::spawn(async move {
            let cfg = st.config_snapshot().await;
            bot.process_event(&ev, &cfg).await
        }));
    }
    let mut mirrored = 0;
    let mut dups = 0;
    for h in handles {
        let o = h.await.unwrap();
        match o.stage {
            CopyStage::ExitMirrored => mirrored += 1,
            CopyStage::Rejected
                if o.rejection.as_ref().map(|r| r.reason) == Some(RejectReason::DuplicateEvent) =>
            {
                dups += 1
            }
            other => panic!("unexpected {other:?}: {:?}", o.rejection),
        }
    }
    assert_eq!(mirrored, 1);
    assert_eq!(dups, 3);
    let p = state.position(&pos_id).await.unwrap();
    assert_eq!(p.status, PositionStatus::Closed);
    assert_eq!(store.link(&pos_id).unwrap().status, "closed");
    // Exactly one copy sell trade was booked.
    let sells = state
        .trades(50)
        .await
        .into_iter()
        .filter(|t| {
            t.source == bot_core::models::TradeSource::Copy
                && t.side == bot_core::models::PositionSide::Short
        })
        .count();
    assert_eq!(sells, 1);
    assert_eq!(
        node.sends.load(Ordering::SeqCst),
        0,
        "paper mode never broadcasts"
    );
}
