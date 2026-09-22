//! The staged pipeline end to end (TASK 3 test 13): a valid leader buy books
//! a position with a deterministic intent id, a journal row, a link and an
//! audit record; every rejection stage names its reason; the three
//! successful terminal states (`FILLED`, `AMBIGUOUS`, `EXIT_MIRRORED`) and
//! the two failing ones (`REJECTED`, `FAILED`) are reached in live mode
//! against the mock node; events, metrics and the legacy `mirror_trade`
//! door behave.

mod common;

use common::*;

use bot_core::config::AppConfig;
use bot_core::models::{BotModule, PositionSide, PositionStatus, Venue};
use bot_core::obs::metrics;
use bot_core::state::AppState;
use module_copy::event::{CopyStage, EventSource, RejectReason};
use module_copy::intent::{entry_intent_id_for, exit_intent_id, EntryRoute, ExitRoute};
use module_copy::recovery::MemoryCopyStore;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[tokio::test]
async fn paper_buy_walks_every_stage_and_books_a_linked_position() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    let mut events = w.state.events.subscribe();
    let e = w.buy(1);

    let out = w.process(&e).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert!(out.rejection.is_none());
    assert_eq!(out.requested_sol, Some(0.05));
    assert_eq!(out.sized_sol, Some(0.05));
    let intent = out.intent_id.clone().expect("intent id");
    assert_eq!(intent, entry_intent_id_for(&e, EntryRoute::Curve));
    assert!(intent.starts_with("int_"));

    // Position: copy source, curve venue, attributed to the leader.
    let positions = w.state.open_positions_for(BotModule::Copy).await;
    assert_eq!(positions.len(), 1);
    let p = &positions[0];
    assert_eq!(p.symbol, w.mint.to_string());
    assert_eq!(p.venue, Venue::PumpFun);
    assert_eq!(p.copied_wallet.as_deref(), Some(LEADER));
    assert!(p.qty > 0.0 && p.avg_entry > 0.0);
    assert!(p.stop_loss.is_some() && p.take_profit.is_some());
    assert_eq!(out.position_id.as_deref(), Some(p.id.as_str()));

    // Ledger: the confirmed intent under the deterministic id, module copy.
    let rec = bot_core::execution::ledger()
        .get(&intent)
        .await
        .expect("ledger record");
    assert_eq!(rec.module, "copy");
    assert!(rec.label.starts_with("copy-"));

    // Journal + link.
    let row = store.event(&e.event_id).expect("journal row");
    assert_eq!(row.stage, "FILLED");
    assert_eq!(row.intent_id.as_deref(), Some(intent.as_str()));
    assert_eq!(row.position_id.as_deref(), Some(p.id.as_str()));
    assert_eq!(row.source, "pumpportal");
    let link = store.link(&p.id).expect("link");
    assert_eq!(link.leader, LEADER);
    assert_eq!(link.entry_event_id, e.event_id);
    assert_eq!(link.entry_signature, e.signature);
    assert_eq!(link.status, "open");
    assert!((link.follower_qty - p.qty).abs() < 1e-9);

    // Dedup side effect: the event is decided.
    assert!(w.state.copy_event_seen(&e.dedup_key()).await);

    // Events: WalletTrade once, Signal, OrderSent, Fill, PositionUpdate, Audit.
    let kinds = drain_kinds(&mut events);
    assert_eq!(kinds.iter().filter(|k| *k == "wallet_trade").count(), 1);
    for k in ["signal", "order_sent", "fill", "position_update", "audit"] {
        assert!(kinds.iter().any(|x| x == k), "missing {k} in {kinds:?}");
    }
    // Audit content: the terminal record carries intent, position and size.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let recent = w.state.events.recent(50).await;
    let audit = recent
        .iter()
        .find_map(|ev| match ev {
            bot_core::events::AppEvent::Audit {
                action,
                outcome,
                target,
                ..
            } if action == "copy.entry.filled" => Some((outcome.clone(), target.clone())),
            _ => None,
        })
        .expect("copy.entry.filled audit");
    assert!(audit.0.contains(&intent), "{}", audit.0);
    assert!(audit.0.contains(&format!("position={}", p.id)));
    assert!(audit.0.contains("sized_sol=0.050000"));
    assert_eq!(
        audit.1.as_deref(),
        Some(format!("{}:{}", w.mint, e.event_id).as_str())
    );

    // Metrics: stages and total latency were recorded.
    let reg = metrics::global();
    assert!(
        reg.counter("copy_stage_total", "", &[("stage", "FILLED")])
            .get()
            >= 1
    );
    assert!(
        reg.counter(
            "copy_events_total",
            "",
            &[("source", "pumpportal"), ("side", "buy")]
        )
        .get()
            >= 1
    );
    assert!(
        reg.histogram(
            "copy_total_latency_ms",
            "",
            &[],
            metrics::LATENCY_BUCKETS_MS
        )
        .count()
            >= 1
    );
}

#[tokio::test]
async fn live_terminal_states_filled_ambiguous_and_exit_mirrored() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    let reg = metrics::global();
    let stage_count = |stage: CopyStage| {
        reg.counter("copy_stage_total", "", &[("stage", stage.as_str())])
            .get()
    };
    let before: Vec<(CopyStage, u64)> = CopyStage::ALL
        .iter()
        .map(|s| (*s, stage_count(*s)))
        .collect();

    // 1. FILLED — the node accepts and confirms the broadcast.
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let buy = w.buy(1);
    let out = w.process(&buy).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert!(out.rejection.is_none());
    assert!(out.opened());
    let entry_intent = out.intent_id.clone().expect("intent id");
    assert_eq!(entry_intent, entry_intent_id_for(&buy, EntryRoute::Curve));
    let filled_sig = out
        .signature
        .clone()
        .expect("live fills carry the signature");
    let filled_pos = w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .expect("mirrored position");
    assert_eq!(
        filled_pos.entry_signature.as_deref(),
        Some(filled_sig.as_str())
    );
    assert_eq!(store.event(&buy.event_id).unwrap().stage, "FILLED");
    let filled_rec = bot_core::execution::ledger()
        .get(&entry_intent)
        .await
        .expect("ledger record");
    assert!(!filled_rec.state.is_ambiguous(), "{:?}", filled_rec.state);
    assert_eq!(filled_rec.module, "copy");

    // 2. AMBIGUOUS — the node swallows the broadcast and never confirms it:
    //    the position is booked, the intent parks, nothing is retried.
    let m2 = w.new_token();
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let ambiguous_buy = w.buy_in(m2, 2);
    let out = w.process(&ambiguous_buy).await;
    assert_eq!(out.stage, CopyStage::Ambiguous, "{:?}", out.rejection);
    assert!(out.rejection.is_none());
    assert!(out.opened());
    let ambiguous_intent = out.intent_id.clone().expect("intent id");
    assert_eq!(
        ambiguous_intent,
        entry_intent_id_for(&ambiguous_buy, EntryRoute::Curve)
    );
    assert!(bot_core::execution::ledger()
        .get(&ambiguous_intent)
        .await
        .unwrap()
        .state
        .is_ambiguous());
    assert!(w
        .state
        .find_open(BotModule::Copy, &m2.to_string())
        .await
        .is_some());
    assert_eq!(
        store.event(&ambiguous_buy.event_id).unwrap().stage,
        "AMBIGUOUS"
    );
    assert!(
        w.state.last_failed_entry(&m2.to_string()).await.is_none(),
        "ambiguous is not failed: no failed-entry cooldown"
    );

    // 3. EXIT_MIRRORED — the leader sells the first mint; our sell goes
    //    through the executor with the hardened exit id (position id + mint
    //    + open time + the exact sell).
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let sends_before_exit = w.node.sends.load(Ordering::SeqCst);
    let sell = w.sell(3);
    let sell_raw = bot_core::maths::to_raw_amount(filled_pos.qty, 6);
    let expected_exit_intent = exit_intent_id(&filled_pos, sell_raw, ExitRoute::Curve);
    let out = w.process(&sell).await;
    assert_eq!(out.stage, CopyStage::ExitMirrored, "{:?}", out.rejection);
    assert!(out.rejection.is_none());
    assert!(!out.opened());
    assert_eq!(out.position_id.as_deref(), Some(filled_pos.id.as_str()));
    assert!(
        w.node.sends.load(Ordering::SeqCst) > sends_before_exit,
        "the mirrored exit was broadcast through the executor"
    );
    let closed = w.state.position(&filled_pos.id).await.unwrap();
    assert_eq!(closed.status, PositionStatus::Closed);
    assert!(closed.exit_signature.is_some());
    let exit_rec = bot_core::execution::ledger()
        .get(&expected_exit_intent)
        .await
        .expect("exit ledger record under the hardened deterministic id");
    assert_eq!(exit_rec.module, "copy");
    assert!(
        module_copy::intent::is_exit_label(&exit_rec.label),
        "{}",
        exit_rec.label
    );
    assert_eq!(store.event(&sell.event_id).unwrap().stage, "EXIT_MIRRORED");
    let link = store.link(&filled_pos.id).unwrap();
    assert_eq!(link.status, "closed");
    assert_eq!(link.exit_event_id.as_deref(), Some(sell.event_id.as_str()));
    // The ambiguous position is untouched by someone else's exit.
    assert!(w
        .state
        .find_open(BotModule::Copy, &m2.to_string())
        .await
        .is_some());

    // 4. REJECTED — a redelivery of the filled buy is a duplicate.
    let out = w.process(&buy).await;
    assert_eq!(out.stage, CopyStage::Rejected);
    assert_eq!(out.rejection.unwrap().reason, RejectReason::DuplicateEvent);

    // 5. FAILED — the node rejects the broadcast of a third mint.
    let m3 = w.new_token();
    w.node.set_send(SendBehaviour::Reject);
    let out = w.process(&w.buy_in(m3, 4)).await;
    assert_eq!(out.stage, CopyStage::Failed, "{:?}", out.rejection);
    assert_eq!(out.rejection.unwrap().reason, RejectReason::ExecutionFailed);
    assert!(w
        .state
        .find_open(BotModule::Copy, &m3.to_string())
        .await
        .is_none());

    // Every one of the 15 stages was counted at least once by this flow.
    for (stage, was) in before {
        assert!(
            stage_count(stage) > was,
            "stage {stage} was not counted by the terminal-state flow"
        );
    }
    // Audit: the three successful terminal records exist with their targets.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let recent = w.state.events.recent(100).await;
    let actions: Vec<String> = recent
        .iter()
        .filter_map(|ev| match ev {
            bot_core::events::AppEvent::Audit { actor, action, .. } if actor == "copy" => {
                Some(action.clone())
            }
            _ => None,
        })
        .collect();
    for expected in [
        "copy.entry.filled",
        "copy.entry.ambiguous",
        "copy.exit.exit_mirrored",
        "copy.entry.failed",
    ] {
        assert!(
            actions.iter().any(|a| a == expected),
            "missing {expected} in {actions:?}"
        );
    }
    // Leader counters: two mirrors (filled + ambiguous); the execution
    // failure is counted as a rejection, the duplicate redelivery is not
    // (its first delivery already was counted as the fill).
    let leaders = w.bot.leaders();
    let leaders = leaders.read().await;
    let l = leaders.get(LEADER).unwrap();
    assert_eq!(l.stats.mirrored, 2);
    assert_eq!(l.stats.rejected, 1);
    assert_eq!(l.stats.last_rejection.as_deref(), Some("EXECUTION_FAILED"));
    assert_eq!(l.stats.events_seen, 5);
}

#[tokio::test]
async fn malformed_events_are_refused_at_the_door() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    let base = w.buy(1);

    let mut nan = base.clone();
    nan.sol_amount = f64::NAN;
    let out = w.process(&nan).await;
    let r = out.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::InvalidEvent);
    assert_eq!(r.stage, CopyStage::Received);
    assert!(r.detail.contains("non_finite_amount"), "{}", r.detail);
    assert!(
        !w.state.copy_event_seen(&nan.dedup_key()).await,
        "invalid events never consume dedup"
    );
    assert!(
        store.event(&nan.event_id).is_none(),
        "nothing journaled before the leader is known"
    );

    let mut empty_mint = base.clone();
    empty_mint.mint = String::new();
    assert_eq!(
        w.process(&empty_mint).await.rejection.unwrap().reason,
        RejectReason::InvalidEvent
    );
    let mut negative = base.clone();
    negative.token_amount = -1.0;
    assert_eq!(
        w.process(&negative).await.rejection.unwrap().reason,
        RejectReason::InvalidEvent
    );
    let mut future = base.clone();
    future.block_time = Some(chrono::Utc::now() + chrono::Duration::hours(1));
    assert_eq!(
        w.process(&future).await.rejection.unwrap().reason,
        RejectReason::InvalidEvent
    );
    let mut foreign = base;
    foreign.venue = Venue::PolymarketClob;
    assert_eq!(
        w.process(&foreign).await.rejection.unwrap().reason,
        RejectReason::InvalidEvent
    );
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
    assert!(
        metrics::global()
            .counter(
                "copy_rejections_total",
                "",
                &[("reason", "INVALID_EVENT"), ("stage", "RECEIVED")]
            )
            .get()
            >= 5
    );
}

#[tokio::test]
async fn policy_rejections_name_their_rule() {
    let mut w = CopyWorld::new(copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;

    // Stale by the wallet rule (observed long ago).
    let mut stale = w.buy(1);
    stale.observed_at = ago(600);
    stale.block_time = Some(ago(600));
    let out = w.process(&stale).await;
    let r = out.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::StaleEvent);
    assert_eq!(r.stage, CopyStage::PolicyPassed);
    let row = store.event(&stale.event_id).unwrap();
    assert_eq!(row.stage, "REJECTED");
    assert_eq!(row.reject_reason.as_deref(), Some("STALE_EVENT"));

    // Global max_event_age_secs with a loose wallet rule.
    let mut cfg = w.cfg.clone();
    cfg.copy.wallets[0].max_staleness_secs = 0;
    cfg.copy.max_event_age_secs = 5;
    w.reload(cfg.clone()).await;
    let mut old_chain = w.buy(2);
    old_chain.block_time = Some(ago(30));
    let r = w.process(&old_chain).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::StaleEvent);
    assert!(r.detail.contains("max_event_age_secs"), "{}", r.detail);

    // Below the wallet minimum.
    cfg.copy.max_event_age_secs = 300;
    cfg.copy.wallets[0].min_sol = 1.0;
    w.reload(cfg.clone()).await;
    assert_eq!(
        w.process(&w.buy(3)).await.rejection.unwrap().reason,
        RejectReason::BelowLeaderMin
    );

    // Venue decoder off.
    cfg.copy.wallets[0].min_sol = 0.0;
    cfg.copy.decode_pumpfun = false;
    w.reload(cfg.clone()).await;
    assert_eq!(
        w.process(&w.buy(4)).await.rejection.unwrap().reason,
        RejectReason::VenueDisabled
    );

    // Replay source never trades.
    cfg.copy.decode_pumpfun = true;
    w.reload(cfg.clone()).await;
    let mut replay = w.buy(5);
    replay.source = EventSource::Replay;
    assert_eq!(
        w.process(&replay).await.rejection.unwrap().reason,
        RejectReason::ReplayOnly
    );

    // Symbol gated by unresolved reconciliation.
    w.state.set_blocked_symbols(vec![w.mint.to_string()]).await;
    let before = metrics::global()
        .counter("bot_symbol_gated_entries_total", "", &[("module", "copy")])
        .get();
    assert_eq!(
        w.process(&w.buy(6)).await.rejection.unwrap().reason,
        RejectReason::SymbolGated
    );
    assert_eq!(
        metrics::global()
            .counter("bot_symbol_gated_entries_total", "", &[("module", "copy")])
            .get(),
        before + 1
    );
    w.state.set_blocked_symbols(Vec::new()).await;

    // Sniper holds the mint.
    cfg.copy.skip_if_sniper_holds = true;
    w.reload(cfg.clone()).await;
    let mut sniper_pos = bot_core::models::Position::new(
        "p-sniper".into(),
        bot_core::models::TradeSource::Sniper,
        Venue::PumpFun,
        bot_core::models::ExecutionMode::Paper,
        w.mint.to_string(),
        "HRN".into(),
        "SOL".into(),
    );
    sniper_pos.apply_buy(10.0, 0.001, 0.01);
    w.state.upsert_position(sniper_pos).await;
    assert_eq!(
        w.process(&w.buy(7)).await.rejection.unwrap().reason,
        RejectReason::SniperHolds
    );
    cfg.copy.skip_if_sniper_holds = false;
    w.reload(cfg.clone()).await;

    // Now a real fill, then ALREADY_MIRRORING for the next buy of the mint.
    assert_eq!(w.process(&w.buy(8)).await.stage, CopyStage::Filled);
    assert_eq!(
        w.process(&w.buy(9)).await.rejection.unwrap().reason,
        RejectReason::AlreadyMirroring
    );

    // Leader sells: mirrored exit closes our position, link closed.
    let sell = w.sell(10);
    let out = w.process(&sell).await;
    assert_eq!(out.stage, CopyStage::ExitMirrored, "{:?}", out.rejection);
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_none());
    let pos_id = out.position_id.unwrap();
    let link = store.link(&pos_id).unwrap();
    assert_eq!(link.status, "closed");
    assert_eq!(link.exit_event_id.as_deref(), Some(sell.event_id.as_str()));

    // Sell with nothing held / buys_only / mirror_exits off.
    assert_eq!(
        w.process(&w.sell(11)).await.rejection.unwrap().reason,
        RejectReason::NoPositionToExit
    );
    cfg.copy.wallets[0].buys_only = true;
    w.reload(cfg.clone()).await;
    assert_eq!(
        w.process(&w.sell(12)).await.rejection.unwrap().reason,
        RejectReason::SellsNotMirrored
    );
    cfg.copy.wallets[0].buys_only = false;
    cfg.copy.mirror_exits = false;
    w.reload(cfg).await;
    assert_eq!(
        w.process(&w.sell(13)).await.rejection.unwrap().reason,
        RejectReason::MirrorExitsDisabled
    );
    // Leader stats: every rejection counted with the last reason.
    let reg = w.bot.leaders();
    let reg = reg.read().await;
    let l = reg.get(LEADER).unwrap();
    assert_eq!(
        l.stats.last_rejection.as_deref(),
        Some("MIRROR_EXITS_DISABLED")
    );
    assert!(l.stats.rejected >= 10);
    assert_eq!(l.stats.mirrored, 1);
}

#[tokio::test]
async fn risk_gates_are_the_authority_and_are_labelled() {
    let mut cfg = copy_config();
    cfg.risk.copy_emergency_disable = true;
    let mut w = CopyWorld::new(cfg.clone()).await;
    let r = w.process(&w.buy(1)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::StrategyDisabled);
    assert_eq!(r.stage, CopyStage::Sized);
    assert!(r.detail.contains("copy_emergency_disable"), "{}", r.detail);

    cfg.risk.copy_emergency_disable = false;
    cfg.risk.copy_cooldown_secs = 600;
    w.reload(cfg.clone()).await;
    assert_eq!(w.process(&w.buy(2)).await.stage, CopyStage::Filled);
    // Same leader + mint again after the position closed → cooldown.
    w.state
        .close_position(
            &w.state.open_positions_for(BotModule::Copy).await[0].id,
            bot_core::models::PositionStatus::Closed,
            "test",
        )
        .await;
    let r = w.process(&w.buy(3)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::CopyCooldown);

    // Kill switch.
    cfg.risk.copy_cooldown_secs = 0;
    w.reload(cfg.clone()).await;
    w.state.set_kill_switch(true, "test").await;
    let killed = w.buy_in(w.new_token(), 4);
    let r = w.process(&killed).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::KillSwitch);
    w.state.set_kill_switch(false, "test").await;

    // Per-leader exposure cap (rule): one open mirror of 0.05 SOL, cap 0.06.
    cfg.copy.wallets[0].max_exposure_sol = 0.06;
    w.reload(cfg.clone()).await;
    let m1 = w.new_token();
    assert_eq!(w.process(&w.buy_in(m1, 5)).await.stage, CopyStage::Filled);
    let m2 = w.new_token();
    let r = w.process(&w.buy_in(m2, 6)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::LeaderExposure);
    assert_eq!(r.stage, CopyStage::Sized);

    // Per-leader open-position cap.
    cfg.copy.wallets[0].max_exposure_sol = 0.0;
    cfg.copy.wallets[0].max_open_positions = 1;
    w.reload(cfg.clone()).await;
    let r = w.process(&w.buy_in(m2, 7)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::LeaderExposure);
    assert!(r.detail.contains("open positions"), "{}", r.detail);

    // Generic risk engine caps still apply (check_entry): copy position limit.
    cfg.copy.wallets[0].max_open_positions = 0;
    cfg.risk.copy_max_concurrent_positions = 1;
    w.reload(cfg.clone()).await;
    let r = w.process(&w.buy_in(m2, 8)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::ExposureLimit);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let recent = w.state.events.recent(20).await;
    assert!(
        recent.iter().any(|e| matches!(
            e,
            bot_core::events::AppEvent::RiskRejected {
                module: BotModule::Copy,
                ..
            }
        )),
        "check_entry rejections publish RiskRejected"
    );

    // Slippage above the cap.
    cfg.risk.copy_max_concurrent_positions = 0;
    cfg.risk.max_slippage_bps = 100;
    w.reload(cfg.clone()).await;
    let r = w.process(&w.buy_in(m2, 9)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::SlippageLimit);

    // Dust / zero size from sizing.
    cfg.risk.max_slippage_bps = 3000;
    cfg.copy.min_mirror_sol = 0.1;
    w.reload(cfg.clone()).await;
    let r = w.process(&w.buy_in(m2, 10)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::DustSize);
    cfg.copy.min_mirror_sol = 0.0;
    cfg.copy.wallets[0].fixed_sol = None;
    cfg.copy.wallets[0].fraction_of_their_size = 0.0;
    w.reload(cfg).await;
    let r = w.process(&w.buy_in(m2, 11)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::ZeroSize);
}

#[tokio::test]
async fn legacy_mirror_trade_door_runs_the_pipeline() {
    let mut w = CopyWorld::new(copy_config()).await;
    let cfg = w.state.config_snapshot().await;
    let trade = leader_buy(LEADER, w.mint, 21, 0.5);
    w.bot
        .mirror_trade(&trade, &cfg)
        .await
        .expect("rejections and fills are Ok");
    assert_eq!(w.state.open_positions_for(BotModule::Copy).await.len(), 1);
    // A redelivery of the same raw trade is a duplicate, still Ok.
    w.bot
        .mirror_trade(&trade, &cfg)
        .await
        .expect("duplicate is not an error");
    assert_eq!(w.state.open_positions_for(BotModule::Copy).await.len(), 1);
    // A sell through the legacy door mirrors the exit.
    let sell = leader_sell(LEADER, w.mint, 22, 1.0);
    assert_eq!(module_copy::CopyBot::trade_side(&sell), PositionSide::Short);
    w.bot.mirror_trade(&sell, &cfg).await.expect("exit ok");
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
}

#[tokio::test]
async fn execution_failures_are_failed_not_rejected_and_note_the_mint() {
    // A dead RPC: the paper path still needs the curve context → FAILED.
    let cfg = copy_config();
    let state = AppState::new(AppConfig {
        raw: cfg.clone(),
        source_path: None,
        warnings: Vec::new(),
    });
    state.set_balances(Some(10.0), Some(1_000.0)).await;
    let (mut bot, _) = copy_bot(state.clone(), offline_rpc()).await;
    let mint = solana_sdk::pubkey::Pubkey::new_unique();
    let e = event(&leader_buy(LEADER, mint, 31, 0.5), EventSource::LogsPoll, 1);
    let out = bot.process_event(&e, &cfg).await;
    assert_eq!(out.stage, CopyStage::Failed);
    let r = out.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::ExecutionFailed);
    assert_eq!(r.stage, CopyStage::Submitted);
    assert!(state.last_failed_entry(&mint.to_string()).await.is_some());
    // The legacy door surfaces the failure as Err.
    let trade = leader_buy(LEADER, mint, 32, 0.5);
    assert!(bot.mirror_trade(&trade, &cfg).await.is_err());
    // With the failed-entry cooldown on (the risk engine reads the shared
    // state's config), the next event is throttled by risk.
    let mut cfg2 = cfg.clone();
    cfg2.risk.copy_failed_entry_cooldown_secs = 120;
    let next = cfg2.clone();
    state.update_config(move |c| *c = next).await;
    // The failure above happened before the cooldown existed; note it again
    // under the new TTL as a fresh failed attempt would.
    state.note_failed_entry(&mint.to_string()).await;
    let e = event(&leader_buy(LEADER, mint, 33, 0.5), EventSource::LogsPoll, 2);
    let out = bot.process_event(&e, &cfg2).await;
    assert_eq!(out.stage, CopyStage::Rejected);
    assert_eq!(out.rejection.unwrap().reason, RejectReason::RiskRejected);
}
