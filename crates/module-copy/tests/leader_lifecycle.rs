//! Leader lifecycle through the real `CopyBot` (TASK 3 test 12): config
//! seeding, hot-reload sync (follow / rule change / pause / resume /
//! unfollow), journaling of every transition, audit records, counters, and
//! the effect of each state on the pipeline.

mod common;

use common::*;

use bot_core::config::CopyWallet;
use module_copy::event::{CopyStage, RejectReason};
use module_copy::leader::{LeaderEvent, LeaderRegistry, LeaderStatus};
use module_copy::recovery::MemoryCopyStore;
use std::sync::Arc;

#[tokio::test]
async fn config_seeds_the_registry_and_sync_journals_every_transition() {
    let mut cfg = copy_config();
    cfg.copy.wallets = vec![rule(LEADER), {
        let mut b = rule(LEADER_B);
        b.paused = true;
        b
    }];
    let store = Arc::new(MemoryCopyStore::new());
    let state = state_with(cfg.clone());
    let (bot, _) = copy_bot(state.clone(), offline_rpc()).await;
    let mut bot = bot.with_copy_store(store.clone());
    let mut events = state.events.subscribe();

    {
        let reg = bot.leaders();
        let reg = reg.read().await;
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.get(LEADER).unwrap().status, LeaderStatus::Active);
        assert_eq!(reg.get(LEADER_B).unwrap().status, LeaderStatus::Paused);
        assert_eq!(reg.counts(), (1, 1, 0));
    }

    // First sync against the same config: nothing changes, nothing journaled.
    assert!(bot.sync_leaders(&cfg).await.is_empty());
    assert!(store.leader_events().is_empty());

    // Hot reload: B resumes, A's rule changes, C appears paused, then A leaves.
    let mut a = rule(LEADER);
    a.max_sol = 0.42;
    let mut c = rule("Leader333333333333333333333333333333333333");
    c.paused = true;
    cfg.copy.wallets = vec![a, rule(LEADER_B), c];
    let ts = bot.sync_leaders(&cfg).await;
    let kinds: Vec<(&str, LeaderEvent)> =
        ts.iter().map(|t| (t.address.as_str(), t.event)).collect();
    assert!(kinds.contains(&(LEADER, LeaderEvent::RuleChanged)));
    assert!(kinds.contains(&(LEADER_B, LeaderEvent::Resumed)));
    assert!(kinds.contains(&(
        "Leader333333333333333333333333333333333333",
        LeaderEvent::Followed
    )));
    assert_eq!(ts.len(), 3);

    cfg.copy.wallets = vec![rule(LEADER_B)];
    let ts = bot.sync_leaders(&cfg).await;
    assert_eq!(ts.len(), 2);
    assert!(ts.iter().all(|t| t.event == LeaderEvent::Unfollowed));

    // Journal: one row per transition, leader rows reflect the final state.
    let journaled = store.leader_events();
    assert_eq!(journaled.len(), 5);
    assert!(journaled.iter().all(|e| !e.replica_id.is_empty()));
    let leaders = bot.store().load_leaders().await.unwrap();
    let a = leaders.iter().find(|l| l.address == LEADER).unwrap();
    assert_eq!(a.status, "removed");
    let b = leaders.iter().find(|l| l.address == LEADER_B).unwrap();
    assert_eq!(b.status, "active");

    // Audit: every transition is published as copy.leader.<event>.
    let audits = copy_audits(&mut events);
    let actions: Vec<&str> = audits.iter().map(|(a, _)| a.as_str()).collect();
    assert!(actions.contains(&"copy.leader.rule_changed"));
    assert!(actions.contains(&"copy.leader.resumed"));
    assert!(actions.contains(&"copy.leader.followed"));
    assert_eq!(
        actions
            .iter()
            .filter(|a| **a == "copy.leader.unfollowed")
            .count(),
        2
    );
    let (_, text) = audits
        .iter()
        .find(|(a, _)| a == "copy.leader.unfollowed")
        .unwrap();
    assert!(text.contains("reason=removed from config"), "{text}");
}

#[tokio::test]
async fn leader_state_gates_the_pipeline() {
    let mut w = CopyWorld::new(copy_config()).await;

    // Unknown leader: refused before dedup, no journal row, no audit noise.
    let mut events = w.state.events.subscribe();
    let mut stranger = w.buy(1);
    stranger.leader = "Stranger11111111111111111111111111111111111".into();
    let out = w.process(&stranger).await;
    assert_eq!(out.stage, CopyStage::Rejected);
    assert_eq!(out.rejection.unwrap().reason, RejectReason::LeaderUnknown);
    assert!(
        !w.state.copy_event_seen(&stranger.dedup_key()).await,
        "unknown leaders never consume dedup"
    );
    assert!(copy_audits(&mut events).is_empty());

    // Paused leader: observed and counted, buys refused as LEADER_PAUSED.
    let mut cfg = w.cfg.clone();
    cfg.copy.wallets[0].paused = true;
    w.reload(cfg.clone()).await;
    let out = w.process(&w.buy(2)).await;
    assert_eq!(
        out.rejection.as_ref().unwrap().reason,
        RejectReason::LeaderPaused
    );
    {
        let reg = w.bot.leaders();
        let reg = reg.read().await;
        let l = reg.get(LEADER).unwrap();
        assert_eq!(l.status, LeaderStatus::Paused);
        assert_eq!(l.stats.events_seen, 1);
        assert_eq!(l.stats.rejected, 1);
        assert_eq!(l.stats.last_rejection.as_deref(), Some("LEADER_PAUSED"));
        assert_eq!(l.stats.last_slot, Some(502));
    }
    assert!(w
        .state
        .open_positions_for(bot_core::models::BotModule::Copy)
        .await
        .is_empty());

    // Resume: the next buy mirrors (paper fill) and the counters follow.
    cfg.copy.wallets[0].paused = false;
    w.reload(cfg.clone()).await;
    let out = w.process(&w.buy(3)).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    {
        let reg = w.bot.leaders();
        let reg = reg.read().await;
        let l = reg.get(LEADER).unwrap();
        assert_eq!(l.stats.events_seen, 2);
        assert_eq!(l.stats.mirrored, 1);
    }

    // A paused leader's SELL is still mirrored (reducing exposure is allowed).
    cfg.copy.wallets[0].paused = true;
    w.reload(cfg.clone()).await;
    let out = w.process(&w.sell(4)).await;
    assert_eq!(out.stage, CopyStage::ExitMirrored, "{:?}", out.rejection);
    assert!(w
        .state
        .open_positions_for(bot_core::models::BotModule::Copy)
        .await
        .is_empty());

    // Removed leader: the ordering cursor is dropped with the unfollow and
    // everything is refused as LEADER_REMOVED.
    cfg.copy.wallets = vec![];
    w.reload(cfg).await;
    assert!(w.bot.ordering().cursor(LEADER).is_none());
    let out = w.process(&w.buy(5)).await;
    assert_eq!(out.rejection.unwrap().reason, RejectReason::LeaderRemoved);
    assert!(w
        .state
        .open_positions_for(bot_core::models::BotModule::Copy)
        .await
        .is_empty());
}

#[tokio::test]
async fn invalid_transitions_are_refused_and_reported() {
    let mut reg = LeaderRegistry::from_config(&[rule(LEADER)]);
    let err = reg.resume(LEADER, None).unwrap_err();
    assert!(
        err.to_string().contains("cannot resumed from active"),
        "{err}"
    );
    let err = reg.pause("nobody", None).unwrap_err();
    assert!(err.to_string().contains("unknown leader nobody"));
    reg.unfollow(LEADER, Some("bye")).unwrap();
    assert!(reg.pause(LEADER, None).is_err());
    assert!(reg.resume(LEADER, None).is_err());
    assert!(reg.unfollow(LEADER, None).is_err());
    // Re-follow restores an active leader with the new rule.
    let mut fresh = rule(LEADER);
    fresh.fixed_sol = Some(0.01);
    let t = reg.follow(fresh, "config", None).unwrap().unwrap();
    assert_eq!(t.event, LeaderEvent::Followed);
    assert_eq!(reg.get(LEADER).unwrap().rule.fixed_sol, Some(0.01));
    assert!(reg.get(LEADER).unwrap().is_active());
    let empty = CopyWallet {
        address: String::new(),
        ..CopyWallet::default()
    };
    assert!(reg.follow(empty, "config", None).is_err());
}

#[tokio::test]
async fn restart_restores_counters_from_the_journal_but_config_owns_membership() {
    let store = Arc::new(MemoryCopyStore::new());
    let cfg = copy_config();
    {
        let mut w = CopyWorld::new(cfg.clone()).await;
        w.attach_store(store.clone()).await;
        w.bot.sync_leaders(&cfg).await;
        assert_eq!(w.process(&w.buy(1)).await.stage, CopyStage::Filled);
        assert_eq!(
            w.process(&w.buy(2)).await.rejection.unwrap().reason,
            RejectReason::AlreadyMirroring
        );
    }
    // "Restart": a new bot with the same journal but a config that only
    // knows LEADER_B — LEADER's row is ignored, B starts fresh.
    let mut cfg2 = cfg.clone();
    cfg2.copy.wallets = vec![rule(LEADER_B)];
    let state = state_with(cfg2.clone());
    let (bot, _) = copy_bot(state.clone(), offline_rpc()).await;
    let mut bot = bot.with_copy_store(store.clone());
    bot.recover_after_restart(&cfg2).await;
    {
        let reg = bot.leaders();
        let reg = reg.read().await;
        assert!(reg.get(LEADER).is_none());
        assert_eq!(reg.get(LEADER_B).unwrap().stats.events_seen, 0);
    }
    // And with the original config the counters come back.
    let state = state_with(cfg.clone());
    let (bot, _) = copy_bot(state.clone(), offline_rpc()).await;
    let mut bot = bot.with_copy_store(store);
    bot.recover_after_restart(&cfg).await;
    let reg = bot.leaders();
    let reg = reg.read().await;
    let l = reg.get(LEADER).unwrap();
    assert_eq!(l.stats.events_seen, 2);
    assert_eq!(l.stats.mirrored, 1);
    assert_eq!(l.stats.rejected, 1);
}
