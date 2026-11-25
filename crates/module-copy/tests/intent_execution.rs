//! Intent identity and execution through the shared engine (TASK 3 test 16):
//! live-mode mirrors go through the hardened executor with a deterministic
//! intent id, the write-ahead journal records before and links after
//! broadcast, the execution ledger refuses a second attempt at the same
//! logical trade, ambiguous broadcasts park without booking a position, and
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
async fn confirmed_buy_and_sell_report_unresolved_balance_metadata_without_changing_bookkeeping() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let mut events = w.state.events.subscribe();
    let buy = w.process(&w.buy(119)).await;
    assert_eq!(buy.stage, CopyStage::Filled, "{:?}", buy.rejection);
    let buy_audits = copy_audits(&mut events);
    let buy_observation = buy_audits
        .iter()
        .find(|(a, _)| a == "copy.transaction_balance_observation")
        .expect("buy observation");
    assert!(buy_observation
        .1
        .contains("status=unresolved_transaction_deltas"));
    assert!(buy_observation.1.contains("not_fill_or_fee_proof=true"));
    let sell = w.process(&w.sell(120)).await;
    assert_eq!(sell.stage, CopyStage::ExitMirrored, "{:?}", sell.rejection);
    let sell_audits = copy_audits(&mut events);
    let sell_observation = sell_audits
        .iter()
        .find(|(a, _)| a == "copy.transaction_balance_observation")
        .expect("sell observation");
    assert!(sell_observation
        .1
        .contains("status=unresolved_transaction_deltas"));
    assert_eq!(w.state.trades(10).await.len(), 2);
}

#[tokio::test]
async fn paper_and_ambiguous_buys_do_not_claim_live_balance_observation() {
    let mut paper = CopyWorld::new(copy_config()).await;
    let mut paper_events = paper.state.events.subscribe();
    assert_eq!(
        paper.process(&paper.buy(121)).await.stage,
        CopyStage::Filled
    );
    assert!(!copy_audits(&mut paper_events)
        .iter()
        .any(|(a, _)| a == "copy.transaction_balance_observation"));
    let mut live = CopyWorld::new(live_copy_config()).await;
    let mut live_events = live.state.events.subscribe();
    live.node.set_send(SendBehaviour::Drop);
    live.node.confirm.store(false, Ordering::SeqCst);
    assert_eq!(
        live.process(&live.buy(122)).await.stage,
        CopyStage::Ambiguous
    );
    assert!(!copy_audits(&mut live_events)
        .iter()
        .any(|(a, _)| a == "copy.transaction_balance_observation"));
}

#[tokio::test]
async fn balance_audit_reports_mock_match_and_mismatch_without_posting_fees() {
    use base64::Engine;
    use serde_json::json;
    use solana_kit::execute::ExecStatus;
    use solana_sdk::transaction::VersionedTransaction;
    let mut w = CopyWorld::new(live_copy_config()).await;
    let buy = w.process(&w.buy(123)).await;
    assert_eq!(buy.stage, CopyStage::Filled);
    let raw = base64::engine::general_purpose::STANDARD
        .decode(w.node.last_sent_transaction_b64().unwrap())
        .unwrap();
    let tx: VersionedTransaction = bincode::deserialize(&raw).unwrap();
    let keys = tx.message.static_account_keys();
    assert!(keys.len() > 1);
    let mut pre = vec![2_000_000u64; keys.len()];
    let mut post = pre.clone();
    pre[0] = 1_000_000_000;
    post[0] = 900_000_000;
    let token_row = |amount: &str| json!({"accountIndex": 1, "mint": w.mint.to_string(), "owner": w.wallet.pubkey.to_string(), "uiTokenAmount": {"amount": amount, "decimals": 6, "uiAmount": null, "uiAmountString": ""}});
    w.node.set_transaction_meta(Some(json!({"err": null, "status": {"Ok": null}, "fee": 5000, "preBalances": pre, "postBalances": post, "preTokenBalances": [token_row("10")], "postTokenBalances": [token_row("40")]})));
    let ledger_count = w.state.ledger().len().await;
    let rpc = mock_rpc(&w.url);
    let mut events = w.state.events.subscribe();
    for expected in [30, 31] {
        module_copy::balance_observation::audit_copy_balances(
            &w.state,
            &rpc,
            bot_core::models::ExecutionMode::Live,
            ExecStatus::Confirmed,
            buy.signature.as_deref(),
            &w.wallet.pubkey,
            &w.mint,
            "fixture-intent",
            expected,
        )
        .await;
    }
    let observed: Vec<_> = copy_audits(&mut events)
        .into_iter()
        .filter(|(a, _)| a == "copy.transaction_balance_observation")
        .collect();
    assert_eq!(observed.len(), 2);
    assert!(observed[0].1.contains("token_delta_raw=30"));
    assert!(observed[0].1.contains("matches_expected=true"));
    assert!(observed[1].1.contains("matches_expected=false"));
    assert!(observed[1].1.contains("not_fill_or_fee_proof=true"));
    assert_eq!(w.state.ledger().len().await, ledger_count);
}

#[tokio::test]
async fn kill_switch_after_live_buy_broadcast_does_not_book_ambiguous_fill() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let node = w.node.clone();
    let state = w.state.clone();
    let trip = tokio::spawn(async move {
        while node.sends.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        state.set_kill_switch(true, "test after broadcast").await;
    });
    let out = w.process(&w.buy(124)).await;
    trip.await.unwrap();
    assert_eq!(out.stage, CopyStage::Ambiguous, "{:?}", out.rejection);
    assert!(w.state.trades(10).await.is_empty());
    assert_eq!(w.state.ledger().len().await, 0);
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
}

#[tokio::test]
async fn confirmed_live_buy_keeps_live_mode_when_kill_switch_changes_after_send() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.node.latency_ms.store(25, Ordering::SeqCst);
    let node = w.node.clone();
    let state = w.state.clone();
    let trip = tokio::spawn(async move {
        while node.sends.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        state
            .set_kill_switch(true, "test after confirmed send")
            .await;
    });
    let out = w.process(&w.buy(129)).await;
    trip.await.unwrap();
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert_eq!(
        w.state.trades(10).await[0].mode,
        bot_core::models::ExecutionMode::Live
    );
    assert_eq!(
        w.state.ledger().events().await[0].event.mode,
        bot_core::models::ExecutionMode::Live
    );
}

#[tokio::test]
async fn kill_switch_after_live_exit_broadcast_does_not_book_ambiguous_sale() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let buy = w.process(&w.buy(125)).await;
    assert_eq!(buy.stage, CopyStage::Filled);
    let position = w
        .state
        .position(buy.position_id.as_deref().unwrap())
        .await
        .unwrap();
    let trades = w.state.trades(10).await.len();
    let ledger = w.state.ledger().len().await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let node = w.node.clone();
    let state = w.state.clone();
    let sends = node.sends.load(Ordering::SeqCst);
    let trip = tokio::spawn(async move {
        while node.sends.load(Ordering::SeqCst) == sends {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        state
            .set_kill_switch(true, "test after exit broadcast")
            .await;
    });
    let out = w.process(&w.sell(126)).await;
    trip.await.unwrap();
    assert_eq!(out.stage, CopyStage::Ambiguous, "{:?}", out.rejection);
    assert_eq!(
        w.state.position(&position.id).await.unwrap().qty,
        position.qty
    );
    assert_eq!(w.state.trades(10).await.len(), trades);
    assert_eq!(w.state.ledger().len().await, ledger);
}

#[tokio::test]
async fn leader_fee_is_never_booked_as_follower_fee() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let mut event = w.buy(117);
    event.fee_sol = 0.002;
    let out = w.process(&event).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    let trades = w.state.trades(10).await;
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].fee, 0.0);
    let events = w.state.ledger().events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.fee, 0.0);
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
async fn ambiguous_broadcast_parks_without_booking_a_fill() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let e = w.buy(4);
    let out = w.process(&e).await;
    assert!(out.rejection.is_none(), "{:?}", out.rejection);
    assert_eq!(out.stage, CopyStage::Ambiguous);
    assert!(
        out.opened(),
        "an ambiguous broadcast may still create exposure"
    );
    assert!(out.position_id.is_none());
    let rec = bot_core::execution::ledger()
        .get(out.intent_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(rec.state.is_ambiguous(), "{:?}", rec.state);
    let positions = w.state.open_positions_for(BotModule::Copy).await;
    assert!(positions.is_empty());
    assert!(w.state.trades(10).await.is_empty());
    assert_eq!(w.state.ledger().len().await, 0);
    assert!(out.signature.is_some());
    // Ambiguous is not failed: no cooldown, journaled as AMBIGUOUS.
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_none());
    assert_eq!(store.event(&e.event_id).unwrap().stage, "AMBIGUOUS");
    // Even a different mint may consume capital already used by the
    // unresolved buy. A later local attempt must not broadcast.
    let sends = w.node.sends.load(Ordering::SeqCst);
    let another_mint = w.new_token();
    let subsequent = w.process(&w.buy_in(another_mint, 107)).await;
    assert_eq!(subsequent.stage, CopyStage::Failed);
    assert!(subsequent
        .rejection
        .unwrap()
        .detail
        .contains("unresolved copy buy"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
    // A lifecycle confirmation alone must not release the guard while the
    // confirmed transaction is still absent from the position book.
    bot_core::execution::ledger()
        .transition(
            out.intent_id.as_deref().unwrap(),
            ExecutionState::Confirmed,
            None,
        )
        .await
        .unwrap();
    let third_mint = w.new_token();
    let after_confirmation = w.process(&w.buy_in(third_mint, 108)).await;
    assert_eq!(after_confirmation.stage, CopyStage::Failed);
    assert!(after_confirmation
        .rejection
        .unwrap()
        .detail
        .contains("unresolved copy buy"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
    // Audit names the ambiguous terminal stage.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let recent = w.state.events.recent(30).await;
    assert!(recent.iter().any(|ev| matches!(
        ev,
        bot_core::events::AppEvent::Audit { action, .. } if action == "copy.entry.ambiguous"
    )));
}

struct UnavailableEventJournal {
    saturated: bool,
}

#[async_trait]
impl module_copy::recovery::CopyStore for UnavailableEventJournal {
    async fn upsert_leader(&self, _: bot_core::db::copy::LeaderRecord) -> bool {
        true
    }
    async fn append_leader_event(&self, _: bot_core::db::copy::LeaderEventRecord) -> bool {
        true
    }
    async fn load_leaders(&self) -> Option<Vec<bot_core::db::copy::LeaderRecord>> {
        Some(Vec::new())
    }
    async fn record_event(&self, _: bot_core::db::copy::CopyEventRecord) -> bool {
        true
    }
    async fn events_since(
        &self,
        _: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Option<Vec<bot_core::db::copy::CopyEventRecord>> {
        if !self.saturated {
            return None;
        }
        let now = chrono::Utc::now();
        let event = bot_core::db::copy::CopyEventRecord {
            event_id: "unrelated".into(),
            leader: "leader".into(),
            signature: "signature".into(),
            slot: 1,
            mint: "mint".into(),
            side: "buy".into(),
            venue: "venue".into(),
            token_amount: 1.0,
            sol_amount: 1.0,
            source: "test".into(),
            source_sequence: 1,
            event_at: Some(now),
            observed_at: now,
            stage: "FILLED".into(),
            reject_reason: None,
            detail: None,
            intent_id: None,
            position_id: None,
            created_at: now,
            updated_at: now,
        };
        Some(vec![event; limit])
    }
    async fn upsert_link(&self, _: bot_core::db::copy::CopyLinkRecord) -> bool {
        true
    }
    async fn open_links(&self) -> Option<Vec<bot_core::db::copy::CopyLinkRecord>> {
        Some(Vec::new())
    }
    async fn close_link(&self, _: &str, _: &str, _: Option<&str>, _: Option<&str>) -> bool {
        true
    }
}

struct FailedAmbiguousJournal(MemoryCopyStore);

#[async_trait]
impl module_copy::recovery::CopyStore for FailedAmbiguousJournal {
    async fn upsert_leader(&self, r: bot_core::db::copy::LeaderRecord) -> bool {
        self.0.upsert_leader(r).await
    }
    async fn append_leader_event(&self, r: bot_core::db::copy::LeaderEventRecord) -> bool {
        self.0.append_leader_event(r).await
    }
    async fn load_leaders(&self) -> Option<Vec<bot_core::db::copy::LeaderRecord>> {
        self.0.load_leaders().await
    }
    async fn record_event(&self, r: bot_core::db::copy::CopyEventRecord) -> bool {
        if r.stage == "AMBIGUOUS" {
            false
        } else {
            self.0.record_event(r).await
        }
    }
    async fn events_since(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Option<Vec<bot_core::db::copy::CopyEventRecord>> {
        self.0.events_since(since, limit).await
    }
    async fn upsert_link(&self, r: bot_core::db::copy::CopyLinkRecord) -> bool {
        self.0.upsert_link(r).await
    }
    async fn open_links(&self) -> Option<Vec<bot_core::db::copy::CopyLinkRecord>> {
        self.0.open_links().await
    }
    async fn close_link(
        &self,
        id: &str,
        status: &str,
        exit: Option<&str>,
        note: Option<&str>,
    ) -> bool {
        self.0.close_link(id, status, exit, note).await
    }
}

struct SwitchableJournal {
    inner: FailedAmbiguousJournal,
    read_mode: std::sync::atomic::AtomicU8,
}

#[async_trait]
impl module_copy::recovery::CopyStore for SwitchableJournal {
    async fn upsert_leader(&self, r: bot_core::db::copy::LeaderRecord) -> bool {
        self.inner.upsert_leader(r).await
    }
    async fn append_leader_event(&self, r: bot_core::db::copy::LeaderEventRecord) -> bool {
        self.inner.append_leader_event(r).await
    }
    async fn load_leaders(&self) -> Option<Vec<bot_core::db::copy::LeaderRecord>> {
        self.inner.load_leaders().await
    }
    async fn record_event(&self, r: bot_core::db::copy::CopyEventRecord) -> bool {
        self.inner.record_event(r).await
    }
    async fn events_since(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Option<Vec<bot_core::db::copy::CopyEventRecord>> {
        match self.read_mode.load(Ordering::SeqCst) {
            1 => None,
            2 => {
                module_copy::recovery::CopyStore::events_since(
                    &UnavailableEventJournal { saturated: true },
                    since,
                    limit,
                )
                .await
            }
            _ => self.inner.events_since(since, limit).await,
        }
    }
    async fn upsert_link(&self, r: bot_core::db::copy::CopyLinkRecord) -> bool {
        self.inner.upsert_link(r).await
    }
    async fn open_links(&self) -> Option<Vec<bot_core::db::copy::CopyLinkRecord>> {
        self.inner.open_links().await
    }
    async fn close_link(
        &self,
        id: &str,
        status: &str,
        exit: Option<&str>,
        note: Option<&str>,
    ) -> bool {
        self.inner.close_link(id, status, exit, note).await
    }
}

#[tokio::test]
async fn failed_ambiguous_journal_write_latches_live_buy_after_confirmation() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(FailedAmbiguousJournal(MemoryCopyStore::new()));
    w.attach_store(store.clone()).await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let first = w.process(&w.buy(117)).await;
    assert_eq!(first.stage, CopyStage::Ambiguous);
    bot_core::execution::ledger()
        .transition(
            first.intent_id.as_deref().unwrap(),
            ExecutionState::Confirmed,
            None,
        )
        .await
        .unwrap();
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let sends = w.node.sends.load(Ordering::SeqCst);
    let mut next = w.buy(118);
    next.mint = w.new_token().to_string();
    let result = w.process(&next).await;
    assert_eq!(result.stage, CopyStage::Failed);
    assert!(result
        .rejection
        .unwrap()
        .detail
        .contains("copy safety latch set"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
}

#[tokio::test]
async fn failed_journal_after_kill_switch_still_latches_unbooked_live_buy() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.attach_store(Arc::new(FailedAmbiguousJournal(MemoryCopyStore::new())))
        .await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let node = w.node.clone();
    let state = w.state.clone();
    let trip = tokio::spawn(async move {
        while node.sends.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        state.set_kill_switch(true, "test after send").await;
    });
    let first = w.process(&w.buy(127)).await;
    trip.await.unwrap();
    assert_eq!(first.stage, CopyStage::Ambiguous);
    bot_core::execution::ledger()
        .transition(
            first.intent_id.as_deref().unwrap(),
            ExecutionState::Confirmed,
            None,
        )
        .await
        .unwrap();
    w.state.set_kill_switch(false, "test resume").await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let sends = w.node.sends.load(Ordering::SeqCst);
    let mut next = w.buy(128);
    next.mint = w.new_token().to_string();
    let result = w.process(&next).await;
    assert_eq!(result.stage, CopyStage::Failed);
    assert!(result
        .rejection
        .unwrap()
        .detail
        .contains("copy safety latch set"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
}

#[tokio::test]
async fn failed_ambiguous_exit_journal_write_disables_reconciliation_auto_exit() {
    use module_copy::reconcile::{FindingKind, ReconAction};
    let mut cfg = live_copy_config();
    cfg.copy.mirror_exits = true;
    cfg.copy.reconcile_auto_exit = true;
    let mut w = CopyWorld::new(cfg.clone()).await;
    let store = Arc::new(FailedAmbiguousJournal(MemoryCopyStore::new()));
    w.attach_store(store).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    assert_eq!(w.process(&w.buy(119)).await.stage, CopyStage::Filled);
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let exit = w.process(&w.sell(120)).await;
    assert_eq!(exit.stage, CopyStage::Ambiguous);
    bot_core::execution::ledger()
        .transition(
            exit.intent_id.as_deref().unwrap(),
            ExecutionState::Confirmed,
            None,
        )
        .await
        .unwrap();
    cfg.copy.mirror_exits = false;
    w.reload(cfg.clone()).await;
    assert_eq!(
        w.process(&w.sell(121)).await.rejection.unwrap().reason,
        RejectReason::MirrorExitsDisabled
    );
    cfg.copy.mirror_exits = true;
    w.reload(cfg.clone()).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let sends = w.node.sends.load(Ordering::SeqCst);
    let report = w.bot.reconcile_once(&cfg).await;
    assert_eq!(report.count(FindingKind::LeaderExitedWeHold), 1);
    assert!(report
        .findings
        .iter()
        .all(|f| !matches!(f.action, ReconAction::MirrorExit { .. })));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_some());
}

#[tokio::test]
async fn paper_to_live_during_copy_build_cannot_bypass_live_journal_guard() {
    let mut w = CopyWorld::new(copy_config()).await;
    w.attach_store(Arc::new(UnavailableEventJournal { saturated: false }))
        .await;
    w.node.pause_multiple_accounts.store(true, Ordering::SeqCst);
    let node = w.node.clone();
    let state = w.state.clone();
    let change = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !node.multiple_accounts_paused.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("copy build reached the barrier");
        state
            .update_config(|c| {
                c.execution.mode = bot_core::models::ExecutionMode::Live;
                c.execution.allow_live_trading = true;
            })
            .await;
        node.resume_accounts();
    });
    let result = w.process(&w.buy(193)).await;
    change.await.unwrap();
    assert_ne!(
        result.stage,
        CopyStage::Filled,
        "must not send an unguarded live trade"
    );
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
    assert!(w.state.trades(10).await.is_empty());
}

#[tokio::test]
async fn kill_switch_during_copy_curve_build_stops_prebroadcast() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.node.pause_multiple_accounts.store(true, Ordering::SeqCst);
    let node = w.node.clone();
    let state = w.state.clone();
    let change = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !node.multiple_accounts_paused.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("copy build reached the barrier");
        state.set_kill_switch(true, "during copy build").await;
        node.resume_accounts();
    });
    let result = w.process(&w.buy(194)).await;
    change.await.unwrap();
    assert_eq!(result.stage, CopyStage::Failed);
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
    assert!(w.state.trades(10).await.is_empty());
}

#[tokio::test]
async fn live_buy_refuses_to_broadcast_if_copy_journal_is_unavailable() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.attach_store(Arc::new(UnavailableEventJournal { saturated: false }))
        .await;
    let out = w.process(&w.buy(109)).await;
    assert_eq!(out.stage, CopyStage::Failed);
    assert!(out
        .rejection
        .unwrap()
        .detail
        .contains("copy journal unavailable"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn live_buy_refuses_to_broadcast_when_journal_scan_is_saturated() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.attach_store(Arc::new(UnavailableEventJournal { saturated: true }))
        .await;
    let out = w.process(&w.buy(112)).await;
    assert_eq!(out.stage, CopyStage::Failed);
    assert!(out
        .rejection
        .unwrap()
        .detail
        .contains("copy journal scan limit reached"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn mirrored_exit_refuses_unbooked_ambiguous_exit_after_confirmation() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    assert_eq!(w.process(&w.buy(122)).await.stage, CopyStage::Filled);
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let prior = w.process(&w.sell(123)).await;
    assert_eq!(prior.stage, CopyStage::Ambiguous);
    bot_core::execution::ledger()
        .transition(
            prior.intent_id.as_deref().unwrap(),
            ExecutionState::Confirmed,
            None,
        )
        .await
        .unwrap();
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let sends = w.node.sends.load(Ordering::SeqCst);
    let next = w.process(&w.sell(124)).await;
    assert_eq!(next.stage, CopyStage::Failed);
    assert!(next
        .rejection
        .unwrap()
        .detail
        .contains("unresolved copy exit requires reconciliation"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
}

async fn test_sweeper(w: &CopyWorld) -> module_copy::exit::ExitSweeper {
    let layouts = solana_kit::layout::LayoutStore::load(&w.cfg.sniper.pump_layout_file).await;
    module_copy::exit::ExitSweeper::new(
        w.state.clone(),
        mock_rpc(&w.url),
        w.wallet.clone(),
        Arc::new(tokio::sync::RwLock::new(layouts)),
        None,
        None,
        None,
    )
    .await
}

async fn force_sweep_exit(w: &CopyWorld) {
    let mut position = w.state.open_positions_for(BotModule::Copy).await.remove(0);
    position.max_hold_secs = Some(0);
    w.state.upsert_position(position).await;
}

#[tokio::test]
async fn sweeper_blocks_journaled_ambiguous_mirror_exit_after_confirmation() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    assert_eq!(w.process(&w.buy(125)).await.stage, CopyStage::Filled);
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let exit = w.process(&w.sell(126)).await;
    assert_eq!(exit.stage, CopyStage::Ambiguous);
    bot_core::execution::ledger()
        .transition(
            exit.intent_id.as_deref().unwrap(),
            ExecutionState::Confirmed,
            None,
        )
        .await
        .unwrap();
    force_sweep_exit(&w).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let sends = w.node.sends.load(Ordering::SeqCst);
    let latch = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut sweeper = test_sweeper(&w).await.with_journal_guard(store, latch);
    sweeper.sweep_once().await.unwrap();
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_some());
}

#[tokio::test]
async fn standalone_live_sweeper_without_journal_refuses_exit() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    assert_eq!(w.process(&w.buy(128)).await.stage, CopyStage::Filled);
    force_sweep_exit(&w).await;
    let sends = w.node.sends.load(Ordering::SeqCst);
    test_sweeper(&w).await.sweep_once().await.unwrap();
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_some());
}

#[tokio::test]
async fn kill_switch_sweeper_cannot_bypass_missing_live_exit_journal() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let buy = w.process(&w.buy(190)).await;
    assert_eq!(buy.stage, CopyStage::Filled);
    let position = w
        .state
        .position(buy.position_id.as_deref().unwrap())
        .await
        .unwrap();
    let mut sweeper = test_sweeper(&w).await;
    w.state.set_kill_switch(true, "test protective exit").await;
    let sends = w.node.sends.load(Ordering::SeqCst);
    sweeper.sweep_once().await.unwrap();
    assert_eq!(
        w.node.sends.load(Ordering::SeqCst),
        sends,
        "missing journal must block live protective exit"
    );
    assert_eq!(
        w.state.position(&position.id).await.unwrap().qty,
        position.qty
    );
}

#[tokio::test]
async fn kill_switch_sweeper_checks_latch_and_allows_clear_journal() {
    for blocked in [true, false] {
        let mut w = CopyWorld::new(live_copy_config()).await;
        let store = Arc::new(MemoryCopyStore::new());
        w.attach_store(store.clone()).await;
        let buy = w.process(&w.buy(if blocked { 191 } else { 192 })).await;
        assert_eq!(buy.stage, CopyStage::Filled);
        let latch = Arc::new(std::sync::atomic::AtomicBool::new(blocked));
        let mut sweeper = test_sweeper(&w).await.with_journal_guard(store, latch);
        w.state.set_kill_switch(true, "test protective exit").await;
        let sends = w.node.sends.load(Ordering::SeqCst);
        sweeper.sweep_once().await.unwrap();
        let actual = w.node.sends.load(Ordering::SeqCst);
        if blocked {
            assert_eq!(actual, sends);
        } else {
            assert_eq!(actual, sends + 1);
        }
    }
}

#[tokio::test]
async fn mirrored_exit_and_sweeper_fail_closed_on_journal_outage_and_saturation() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(SwitchableJournal {
        inner: FailedAmbiguousJournal(MemoryCopyStore::new()),
        read_mode: std::sync::atomic::AtomicU8::new(0),
    });
    w.attach_store(store.clone()).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    assert_eq!(w.process(&w.buy(129)).await.stage, CopyStage::Filled);
    force_sweep_exit(&w).await;
    let mut sweeper = test_sweeper(&w).await.with_journal_guard(
        store.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let sends = w.node.sends.load(Ordering::SeqCst);
    for (mode, tag, detail) in [
        (1, 130, "journal unavailable"),
        (2, 131, "journal scan limit reached"),
    ] {
        store.read_mode.store(mode, Ordering::SeqCst);
        let out = w.process(&w.sell(tag)).await;
        assert_eq!(out.stage, CopyStage::Failed);
        assert!(out.rejection.unwrap().detail.contains(detail));
        sweeper.sweep_once().await.unwrap();
        assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
        assert!(w
            .state
            .find_open(BotModule::Copy, &w.mint.to_string())
            .await
            .is_some());
    }
}

#[tokio::test]
async fn sweeper_ambiguous_exit_sets_shared_latch_and_blocks_retry() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    let store = Arc::new(MemoryCopyStore::new());
    w.attach_store(store.clone()).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    assert_eq!(w.process(&w.buy(127)).await.stage, CopyStage::Filled);
    force_sweep_exit(&w).await;
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let latch = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut sweeper = test_sweeper(&w)
        .await
        .with_journal_guard(store, latch.clone());
    let sends_before = w.node.sends.load(Ordering::SeqCst);
    sweeper.sweep_once().await.unwrap();
    let sends_after_ambiguous = w.node.sends.load(Ordering::SeqCst);
    assert!(sends_after_ambiguous > sends_before);
    assert!(latch.load(Ordering::SeqCst));
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    sweeper.sweep_once().await.unwrap();
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends_after_ambiguous);
}

#[tokio::test]
async fn ambiguous_exit_keeps_position_and_does_not_book_a_sell() {
    let mut w = CopyWorld::new(live_copy_config()).await;
    w.node.set_send(SendBehaviour::Accept);
    w.node.confirm.store(true, Ordering::SeqCst);
    let buy = w.process(&w.buy(104)).await;
    assert_eq!(buy.stage, CopyStage::Filled);
    let position = w
        .state
        .position(buy.position_id.as_deref().unwrap())
        .await
        .unwrap();
    let ledger_count = w.state.ledger().len().await;
    let trade_count = w.state.trades(100).await.len();
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let out = w.process(&w.sell(105)).await;
    assert_eq!(out.stage, CopyStage::Ambiguous, "{:?}", out.rejection);
    assert!(out.intent_id.is_some());
    assert!(out.signature.is_some());
    let current = w.state.position(&position.id).await.unwrap();
    assert_eq!(current.qty, position.qty);
    assert_eq!(current.status, position.status);
    assert_eq!(w.state.trades(100).await.len(), trade_count);
    assert_eq!(w.state.ledger().len().await, ledger_count);
    let sends = w.node.sends.load(Ordering::SeqCst);
    let retry = w.process(&w.sell(106)).await;
    assert_eq!(retry.stage, CopyStage::Failed);
    assert!(retry
        .rejection
        .unwrap()
        .detail
        .contains("unresolved copy exit"));
    assert_eq!(w.node.sends.load(Ordering::SeqCst), sends);
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
