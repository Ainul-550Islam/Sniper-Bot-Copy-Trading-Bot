//! Intent identity and execution through the shared engine (TASK 3 test 16):
//! live-mode mirrors go through the hardened executor with a deterministic
//! intent id, the write-ahead journal records before and links after
//! broadcast, the execution ledger refuses a second attempt at the same
//! logical trade, ambiguous broadcasts book the position and park, and
//! definite failures never do.

mod common;

use common::*;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bot_core::db::repo::IntentRecord;
use bot_core::execution::{ExecutionIntent, ExecutionState};
use bot_core::models::BotModule;
use bot_core::recovery::IntentSink;
use module_copy::event::{CopyStage, RejectReason};
use module_copy::intent::{entry_intent_id_for, entry_label, EntryRoute};
use module_copy::recovery::MemoryCopyStore;

#[derive(Default)]
struct RecordingSink {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl IntentSink for RecordingSink {
    async fn record(&self, rec: IntentRecord) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("record:{}:{}:{}", rec.module, rec.side, rec.symbol));
    }
    async fn link(&self, intent_id: &str, signature: &str) {
        let _ = intent_id;
        self.calls
            .lock()
            .unwrap()
            .push(format!("link:{}", &signature[..6]));
    }
    async fn abandon(&self, intent_id: &str) {
        let _ = intent_id;
        self.calls.lock().unwrap().push("abandon".into());
    }
}

#[tokio::test]
async fn live_mirror_confirms_through_the_executor_with_a_deterministic_intent() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    let sink = Arc::new(RecordingSink::default());
    let (bot, wallet) = copy_bot(w.state.clone(), mock_rpc(&w.url)).await;
    w.bot = bot
        .with_copy_store(store.clone())
        .with_intent_sink(sink.clone());
    w.wallet = wallet;
    let e = w.buy(1);

    let out = w.process(&e).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert_eq!(
        w.node.sends.load(Ordering::SeqCst),
        1,
        "exactly one broadcast"
    );
    let intent = out.intent_id.clone().unwrap();
    assert_eq!(intent, entry_intent_id_for(&e, EntryRoute::Curve));
    let rec = bot_core::execution::ledger()
        .get(&intent)
        .await
        .expect("ledger");
    assert_eq!(rec.state, ExecutionState::Confirmed);
    assert_eq!(rec.module, "copy");
    assert_eq!(
        rec.label,
        entry_label(&w.mint.to_string(), EntryRoute::Curve)
    );
    let sent = w.node.last_sent_signature().expect("signature");
    assert_eq!(rec.signature.as_deref(), Some(sent.as_str()));
    assert_eq!(out.signature.as_deref(), Some(sent.as_str()));

    // Position carries the signature; link + journal carry the intent.
    let p = &w.state.open_positions_for(BotModule::Copy).await[0];
    assert_eq!(p.entry_signature.as_deref(), Some(sent.as_str()));
    assert_eq!(p.mode, bot_core::models::ExecutionMode::Live);
    let link = store.link(&p.id).unwrap();
    assert_eq!(link.intent_id.as_deref(), Some(intent.as_str()));
    assert_eq!(link.entry_signature, e.signature);
    assert_eq!(
        store.event(&e.event_id).unwrap().intent_id.as_deref(),
        Some(intent.as_str())
    );

    // Write-ahead journal: recorded BEFORE broadcast, linked AFTER.
    let calls = sink.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0], format!("record:copy:buy:{}", w.mint));
    assert_eq!(calls[1], format!("link:{}", &sent[..6]));
}

#[tokio::test]
async fn the_ledger_refuses_a_second_attempt_at_the_same_logical_trade() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let e = w.buy(2);
    // Another replica (or a previous life) already has this intent live —
    // and this replica's dedup knows nothing about it.
    let intent = entry_intent_id_for(&e, EntryRoute::Curve);
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: intent.clone(),
            module: "copy".into(),
            label: entry_label(&w.mint.to_string(), EntryRoute::Curve),
            wallet: w.wallet.pubkey.to_string(),
            symbol: w.mint.to_string(),
        })
        .await
        .unwrap();
    let out = w.process(&e).await;
    assert_eq!(out.stage, CopyStage::Failed);
    let r = out.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::ExecutionFailed);
    assert!(
        r.detail.to_ascii_lowercase().contains("duplicate"),
        "{}",
        r.detail
    );
    assert_eq!(
        w.node.sends.load(Ordering::SeqCst),
        0,
        "nothing was broadcast"
    );
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
    // The failed attempt notes the mint for the failed-entry cooldown.
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_some());
}

#[tokio::test]
async fn simulation_failure_never_broadcasts_and_abandons_the_journal_entry() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let sink = Arc::new(RecordingSink::default());
    let (bot, wallet) = copy_bot(w.state.clone(), mock_rpc(&w.url)).await;
    w.bot = bot.with_intent_sink(sink.clone());
    w.wallet = wallet;
    w.node
        .set_simulate_error(Some("custom program error: 0x1771"));
    let out = w.process(&w.buy(3)).await;
    assert_eq!(out.stage, CopyStage::Failed, "{:?}", out.rejection);
    assert_eq!(out.rejection.unwrap().reason, RejectReason::ExecutionFailed);
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
    let calls = sink.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].starts_with("record:copy:buy:"));
    assert_eq!(calls[1], "abandon");
    // The mint is noted as a failed entry; module error counter bumped.
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_some());
    // The event is decided: a redelivery is a duplicate, not a retry.
    w.node.set_simulate_error(None);
    assert_eq!(
        w.process(&w.buy(3)).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ambiguous_broadcast_books_the_position_and_parks_the_intent() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let e = w.buy(4);
    let out = w.process(&e).await;
    assert!(out.rejection.is_none(), "{:?}", out.rejection);
    assert_eq!(out.stage, CopyStage::Ambiguous);
    assert!(out.opened());
    let rec = bot_core::execution::ledger()
        .get(out.intent_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(rec.state.is_ambiguous(), "{:?}", rec.state);
    let positions = w.state.open_positions_for(BotModule::Copy).await;
    assert_eq!(positions.len(), 1);
    assert!(positions[0].entry_signature.is_some());
    // Ambiguous is not failed: no cooldown, journaled as AMBIGUOUS, linked.
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_none());
    assert_eq!(store.event(&e.event_id).unwrap().stage, "AMBIGUOUS");
    assert_eq!(store.link(&positions[0].id).unwrap().status, "open");
    // Audit names the ambiguous terminal stage.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let recent = w.state.events.recent(30).await;
    assert!(recent.iter().any(|ev| matches!(
        ev,
        bot_core::events::AppEvent::Audit { action, .. } if action == "copy.entry.ambiguous"
    )));
}

#[tokio::test]
async fn node_rejection_is_a_definite_failure_with_nothing_booked() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.node.set_send(SendBehaviour::Reject);
    let out = w.process(&w.buy(5)).await;
    assert_eq!(out.stage, CopyStage::Failed, "{:?}", out.rejection);
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
    let reg = w.bot.leaders();
    let reg = reg.read().await;
    let l = reg.get(LEADER).unwrap();
    assert_eq!(l.stats.mirrored, 0);
    assert_eq!(l.stats.rejected, 1);
    assert_eq!(l.stats.last_rejection.as_deref(), Some("EXECUTION_FAILED"));
}

#[tokio::test]
async fn pending_execution_cap_throttles_new_mirrors() {
    // The execution ledger is process-wide: other tests in this binary may
    // have left live copy entries (e.g. the ambiguous broadcast). Size the
    // cap relative to what is already in flight.
    let already = bot_core::execution::ledger()
        .open()
        .await
        .iter()
        .filter(|r| r.module == "copy" && module_copy::intent::is_entry_label(&r.label))
        .count();
    let mut cfg = live_copy_config();
    cfg.risk.copy_max_pending_executions = already + 1;
    let mut w = CopyWorld::new(cfg).await;
    // A live copy ENTRY intent from elsewhere.
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: "int_copy_pending_cap_test".into(),
            module: "copy".into(),
            label: "copy-PENDING".into(),
            wallet: w.wallet.pubkey.to_string(),
            symbol: "PendingMint".into(),
        })
        .await
        .unwrap();
    let r = w.process(&w.buy(6)).await.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::ExposureLimit);
    assert!(r.detail.contains("in flight"), "{}", r.detail);
    assert!(
        r.detail.contains(&format!("max {}", already + 1)),
        "{}",
        r.detail
    );
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
    // Exits never count against the cap: a live copy-exit intent alone
    // does not throttle.
    bot_core::execution::ledger()
        .fail(
            "int_copy_pending_cap_test",
            bot_core::execution::FailureClass::BlockhashExpired,
            "expired",
        )
        .await
        .unwrap();
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: "int_copy_exit_pending_cap_test".into(),
            module: "copy".into(),
            label: "copy-exit-PENDING".into(),
            wallet: w.wallet.pubkey.to_string(),
            symbol: "PendingMint".into(),
        })
        .await
        .unwrap();
    let m = w.new_token();
    let out = w.process(&w.buy_in(m, 7)).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
}
