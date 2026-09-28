//! Restart recovery through the real bot (TASK 3 test 18): the durable
//! journal re-seeds dedup and ordering so a replayed backlog is not
//! re-mirrored, live entries are held (never resubmitted), entries that
//! provably never landed are cleaned up without selling, links are repaired
//! against the position book, and the whole procedure is idempotent.

mod common;

use common::*;

use bot_core::execution::{ExecutionIntent, FailureClass};
use bot_core::models::{BotModule, ExecutionMode, Position, PositionStatus, TradeSource, Venue};
use bot_core::obs::metrics;
use module_copy::event::{CopyStage, RejectReason};
use module_copy::recovery::{link_for, CopyStore, MemoryCopyStore, RecoveryAction};
use std::sync::Arc;

#[tokio::test]
async fn replayed_backlog_after_restart_is_not_re_mirrored() {
    let store = Arc::new(MemoryCopyStore::new());
    let cfg = copy_config();
    let (mint, e1, e2, slot) = {
        let mut w = CopyWorld::new(cfg.clone()).await;
        w.attach_store(store.clone()).await;
        let e1 = w.buy(1);
        let mut e2 = w.buy(2);
        e2.slot = 900;
        assert_eq!(w.process(&e1).await.stage, CopyStage::Filled);
        assert_eq!(
            w.process(&e2).await.rejection.unwrap().reason,
            RejectReason::AlreadyMirroring
        );
        (w.mint, e1, e2, 900)
    };
    // "Restart": fresh in-memory state (empty dedup, empty book), same journal.
    let mut w2 = CopyWorld::new(cfg.clone()).await;
    install_pump_token(&w2.node, mint, w2.creator, CurveSpec::fresh());
    w2.attach_store(store.clone()).await;
    assert!(!w2.state.copy_event_seen(&e1.dedup_key()).await);
    let mut events = w2.state.events.subscribe();
    let plan = w2.bot.recover_after_restart(&cfg).await;
    assert_eq!(plan.count("seed_dedup"), 1);
    assert_eq!(plan.count("seed_cursor"), 1);
    assert!(plan.actions.contains(&RecoveryAction::SeedCursor {
        leader: LEADER.into(),
        slot,
        signature: e2.signature.clone(),
    }));
    assert!(w2.state.copy_event_seen(&e1.dedup_key()).await);
    assert!(w2.state.copy_event_seen(&e2.dedup_key()).await);
    assert_eq!(w2.bot.ordering().cursor(LEADER).unwrap().last_slot, slot);
    // The feed replays its backlog: both are duplicates, nothing is bought.
    assert_eq!(
        w2.process(&e1).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );
    assert_eq!(
        w2.process(&e2).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );
    assert!(w2
        .state
        .open_positions_for(BotModule::Copy)
        .await
        .is_empty());
    let audits = copy_audits(&mut events);
    assert!(audits
        .iter()
        .any(|(a, o)| a == "copy.recovery.seed_dedup" && o.contains("2 keys")));
    assert!(
        metrics::global()
            .counter(
                "copy_recovery_actions_total",
                "",
                &[("action", "seed_dedup")]
            )
            .get()
            >= 1
    );
    // Lookback 0 disables the re-seed (documented trade-off).
    let mut no_lookback = cfg.clone();
    no_lookback.copy.recovery_lookback_hours = 0;
    let w3 = CopyWorld::new(no_lookback.clone()).await;
    let mut bot3 = w3.bot;
    let plan = bot3.recover_after_restart(&no_lookback).await;
    assert_eq!(plan.count("seed_dedup"), 0);
}

#[tokio::test]
async fn live_entries_are_held_and_failed_entries_cleaned_up_without_selling() {
    let cfg = copy_config();
    let w = CopyWorld::new(cfg.clone()).await;
    let store = Arc::new(MemoryCopyStore::new());
    let mut bot = w.sibling().await.with_copy_store(store.clone());
    let wallet = w.wallet.pubkey.to_string();

    // Position restored from the DB whose entry is still live in the ledger.
    let live_sig = signature(41);
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: "int_copy_recovery_live".into(),
            module: "copy".into(),
            label: "copy-LIVE".into(),
            wallet: wallet.clone(),
            symbol: "LiveMint".into(),
        })
        .await
        .unwrap();
    bot_core::execution::ledger()
        .attach_submission(
            "int_copy_recovery_live",
            &live_sig,
            Some("hash".into()),
            Some(1),
            0,
        )
        .await
        .unwrap();
    let mut live = position("p-recovery-live", "LiveMint", Some(&live_sig));
    live.copied_wallet = Some(LEADER.into());
    w.state.upsert_position(live).await;

    // Position whose entry the ledger settled as expired after the crash.
    let dead_sig = signature(42);
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: "int_copy_recovery_dead".into(),
            module: "copy".into(),
            label: "copy-DEAD".into(),
            wallet: wallet.clone(),
            symbol: "DeadMint".into(),
        })
        .await
        .unwrap();
    bot_core::execution::ledger()
        .attach_submission(
            "int_copy_recovery_dead",
            &dead_sig,
            Some("hash".into()),
            Some(1),
            0,
        )
        .await
        .unwrap();
    bot_core::execution::ledger()
        .fail(
            "int_copy_recovery_dead",
            FailureClass::BlockhashExpired,
            "expired",
        )
        .await
        .unwrap();
    let mut dead = position("p-recovery-dead", "DeadMint", Some(&dead_sig));
    dead.copied_wallet = Some(LEADER.into());
    w.state.upsert_position(dead).await;
    store
        .upsert_link(link_for(
            "p-recovery-dead",
            LEADER,
            "DeadMint",
            "ev-dead",
            &dead_sig,
            None,
            1.0,
            100.0,
        ))
        .await;

    // A healthy confirmed position without a link, and an open link whose
    // position is gone.
    let mut healthy = position("p-recovery-ok", "OkMint", Some(&signature(43)));
    healthy.copied_wallet = Some(LEADER.into());
    w.state.upsert_position(healthy).await;
    store
        .upsert_link(link_for(
            "p-gone",
            LEADER,
            "GoneMint",
            "ev-gone",
            &signature(44),
            None,
            1.0,
            1.0,
        ))
        .await;

    let mut events = w.state.events.subscribe();
    let plan = bot.recover_after_restart(&cfg).await;
    assert_eq!(plan.count("hold_ambiguous"), 1, "{:?}", plan.actions);
    assert_eq!(plan.count("cleanup_failed_entry"), 1);
    assert_eq!(
        plan.count("restore_link"),
        2,
        "live + healthy positions had no link"
    );
    assert_eq!(plan.count("close_link"), 1);

    // Held: still open, untouched, nothing sold, no new broadcast.
    let held = w.state.position("p-recovery-live").await.unwrap();
    assert_eq!(held.status, PositionStatus::Open);
    assert!(held.qty > 0.0);
    assert_eq!(w.node.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    // Cleaned: closed as failed with zero qty / cost, link closed.
    let cleaned = w.state.position("p-recovery-dead").await.unwrap();
    assert_eq!(cleaned.status, PositionStatus::Failed);
    assert_eq!(cleaned.qty, 0.0);
    assert_eq!(cleaned.cost_basis, 0.0);
    assert_eq!(store.link("p-recovery-dead").unwrap().status, "closed");
    // Links repaired.
    let restored = store.link("p-recovery-ok").unwrap();
    assert_eq!(restored.leader, LEADER);
    assert_eq!(restored.status, "open");
    assert!(restored.note.as_deref().unwrap_or("").contains("restored"));
    assert_eq!(store.link("p-gone").unwrap().status, "orphaned");
    // Audit trail for each action.
    let audits = copy_audits(&mut events);
    for action in [
        "copy.recovery.hold_ambiguous",
        "copy.recovery.cleanup_failed_entry",
        "copy.recovery.restore_link",
        "copy.recovery.close_link",
    ] {
        assert!(
            audits.iter().any(|(a, _)| a == action),
            "missing {action} in {audits:?}"
        );
    }
    // Idempotent: a second run has nothing to repair (the held entry is
    // reported again — it is still live — but no link/cleanup work remains).
    let again = bot.recover_after_restart(&cfg).await;
    assert_eq!(again.count("cleanup_failed_entry"), 0);
    assert_eq!(again.count("restore_link"), 0);
    assert_eq!(again.count("close_link"), 0);
    assert_eq!(again.count("hold_ambiguous"), 1);
}

#[tokio::test]
async fn recovery_runs_before_the_pipeline_accepts_new_events() {
    // The dedup facade is process-local here: without recovery, a replay
    // would be mirrored again. With recovery it is refused — and new events
    // for other mints still flow.
    let store = Arc::new(MemoryCopyStore::new());
    let cfg = copy_config();
    let (mint, e1) = {
        let mut w = CopyWorld::new(cfg.clone()).await;
        w.attach_store(store.clone()).await;
        let e1 = w.buy(51);
        assert_eq!(w.process(&e1).await.stage, CopyStage::Filled);
        (w.mint, e1)
    };
    let mut w2 = CopyWorld::new(cfg.clone()).await;
    install_pump_token(&w2.node, mint, w2.creator, CurveSpec::fresh());
    w2.attach_store(store.clone()).await;
    w2.bot.recover_after_restart(&cfg).await;
    assert_eq!(
        w2.process(&e1).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );
    let fresh = w2.buy_in(w2.new_token(), 52);
    assert_eq!(w2.process(&fresh).await.stage, CopyStage::Filled);
    assert_eq!(w2.state.open_positions_for(BotModule::Copy).await.len(), 1);
}

fn position(id: &str, mint: &str, sig: Option<&str>) -> Position {
    let mut p = Position::new(
        id.to_string(),
        TradeSource::Copy,
        Venue::PumpFun,
        ExecutionMode::Live,
        mint.to_string(),
        mint.to_string(),
        "SOL".into(),
    );
    p.apply_buy(100.0, 0.001, 0.1);
    p.entry_signature = sig.map(|s| s.to_string());
    p
}
