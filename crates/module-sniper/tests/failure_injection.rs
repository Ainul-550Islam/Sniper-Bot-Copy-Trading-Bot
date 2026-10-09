//! Failure-injection suite for the sniper engine (TASK 2 §N).
//!
//! Every scenario runs the REAL detector / pipeline / risk engine / hardened
//! executor against scripted fakes (mock RPC node, mock websocket, failing
//! sinks). Nothing here touches a network. Scenario numbers follow the task
//! list; see `docs/SNIPER-ENGINE.md` §12 ("Failure scenarios → behaviour →
//! test") for the mapping.

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use bot_core::config::{AppConfig, Config};
use bot_core::db::repo::IntentRecord;
use bot_core::execution::{ExecutionIntent, ExecutionState};
use bot_core::models::{BotModule, LaunchFeed, PositionStatus};
use bot_core::recovery::IntentSink;
use bot_core::state::AppState;

use module_sniper::event::LaunchProtocol;
use module_sniper::pipeline::{RejectReason, SniperStage};
use module_sniper::LaunchDetector;

use common::*;

// ---------------------------------------------------------------------------
// Mock Solana websocket speaking `logsSubscribe`
// ---------------------------------------------------------------------------

/// Script for one connection: notifications to push, then whether to drop.
#[derive(Clone)]
struct ConnScript {
    notifications: Vec<Value>,
    drop_after: bool,
}

async fn spawn_mock_logs_ws(scripts: Vec<ConnScript>) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let conns = connections.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let idx = conns.fetch_add(1, Ordering::SeqCst);
            let script = scripts
                .get(idx)
                .cloned()
                .unwrap_or_else(|| scripts.last().cloned().unwrap());
            tokio::spawn(handle_logs_conn(stream, script));
        }
    });
    (addr, connections)
}

async fn handle_logs_conn(stream: TcpStream, script: ConnScript) {
    let Ok(mut ws) = accept_async(stream).await else {
        return;
    };
    let mut pushed = false;
    loop {
        match tokio::time::timeout(Duration::from_millis(250), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if v["method"] == "logsSubscribe" && !pushed {
                    pushed = true;
                    let ack = json!({"jsonrpc": "2.0", "id": v["id"].clone(), "result": 77});
                    if ws.send(Message::Text(ack.to_string())).await.is_err() {
                        return;
                    }
                    for n in &script.notifications {
                        let frame = json!({
                            "jsonrpc": "2.0",
                            "method": "logsNotification",
                            "params": {"subscription": 77, "result": n},
                        });
                        if ws.send(Message::Text(frame.to_string())).await.is_err() {
                            return;
                        }
                    }
                    let _ = ws.flush().await;
                    if script.drop_after {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let _ = ws.close(None).await;
                        return;
                    }
                }
            }
            Ok(Some(Ok(Message::Ping(p)))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => {}
        }
    }
}

fn logs_notification(slot: u64, signature: &str, logs: Vec<String>) -> Value {
    json!({
        "context": {"slot": slot},
        "value": {"signature": signature, "err": null, "logs": logs}
    })
}

fn detector_config(ws_url: &str) -> Config {
    let mut cfg = Config::default();
    cfg.sniper.use_pumpportal = false;
    cfg.sniper.use_log_subscription = true;
    cfg.sniper.use_transaction_subscribe = false;
    cfg.sniper.trade_pumpswap = false;
    cfg.sniper.trade_raydium = false;
    cfg.network.rpc_url = "http://127.0.0.1:9".into();
    cfg.network.ws_url = ws_url.to_string();
    cfg.network.retry_base_backoff_ms = 250;
    cfg.network.retry_max_backoff_ms = 500;
    cfg.network.retry_jitter = false;
    cfg
}

fn logs_rpc(ws_url: &str) -> solana_kit::rpc::Rpc {
    solana_kit::rpc::Rpc::with_urls(
        "http://127.0.0.1:9".into(),
        ws_url.to_string(),
        Vec::new(),
        solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        0,
        Duration::from_millis(300),
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// 1./2./18. Websocket disconnects, gaps and ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_disconnect_during_detection_reconnects_and_keeps_detecting() {
    let mint_a = Pubkey::new_unique();
    let mint_b = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let (addr, connections) = spawn_mock_logs_ws(vec![
        ConnScript {
            notifications: vec![logs_notification(
                100,
                &signature(1),
                pump_create_logs(mint_a, creator),
            )],
            drop_after: true,
        },
        ConnScript {
            notifications: vec![logs_notification(
                103,
                &signature(2),
                pump_create_logs(mint_b, creator),
            )],
            drop_after: false,
        },
    ])
    .await;
    let ws_url = format!("ws://{addr}/");
    let state = AppState::new(AppConfig {
        raw: detector_config(&ws_url),
        source_path: None,
        warnings: Vec::new(),
    });
    let mut rx = LaunchDetector::spawn(state.clone(), logs_rpc(&ws_url))
        .await
        .expect("detector starts");

    let a = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("first launch")
        .expect("channel open");
    assert_eq!(a.mint, mint_a.to_string());
    assert_eq!(a.source, LaunchFeed::SolanaLogs);
    assert_eq!(a.slot, Some(100));
    assert_eq!(a.source_seq, 1);

    // The server dropped the socket; the feed reconnects (backoff ≥ 250 ms)
    // and the next launch flows through the SAME channel.
    let b = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("launch after reconnect")
        .expect("channel open");
    assert_eq!(b.mint, mint_b.to_string());
    assert_eq!(b.slot, Some(103));
    assert_eq!(b.source_seq, 2, "sequence continues across the reconnect");
    assert!(
        connections.load(Ordering::SeqCst) >= 2,
        "a second connection was made"
    );
    let text = bot_core::obs::metrics::global().encode();
    assert!(text.contains("sniper_feed_reconnects_total"));
    assert!(text.contains("sniper_feed_events_total"));
}

#[tokio::test]
async fn ws_disconnect_after_detection_does_not_affect_the_entry() {
    // The launch was already handed to the pipeline; killing the feed
    // afterwards must not stop the entry — the pipeline talks to the RPC.
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let (addr, _) = spawn_mock_logs_ws(vec![ConnScript {
        notifications: vec![logs_notification(
            200,
            &signature(3),
            pump_create_logs(mint, creator),
        )],
        drop_after: true,
    }])
    .await;
    let ws_url = format!("ws://{addr}/");
    let mut cfg = base_config();
    cfg.sniper.use_log_subscription = true;
    cfg.sniper.trade_pumpswap = false;
    cfg.sniper.trade_raydium = false;
    cfg.network.ws_url = ws_url.clone();
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(cfg);
    let rpc = solana_kit::rpc::Rpc::with_urls(
        url,
        ws_url,
        Vec::new(),
        solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        1,
        Duration::from_secs(2),
    )
    .unwrap();
    let mut rx = LaunchDetector::spawn(state.clone(), rpc.clone())
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    // Socket is gone by now (drop_after); run the pipeline anyway.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (mut sniper, _wallet) = sniper(state.clone(), rpc).await;
    let o = sniper.consider_event(event).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.stage, SniperStage::Confirmed);
}

#[tokio::test]
async fn ws_slot_regression_after_reconnect_is_counted_not_traded_twice() {
    // Connection 2 replays the launch from connection 1 at an OLDER slot
    // (a provider replaying its buffer): the detector counts the regression
    // and the pipeline's dedup refuses the duplicate.
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let (addr, _) = spawn_mock_logs_ws(vec![
        ConnScript {
            notifications: vec![
                logs_notification(300, &signature(4), pump_create_logs(mint, creator)),
                logs_notification(
                    305,
                    &signature(5),
                    pump_create_logs(Pubkey::new_unique(), creator),
                ),
            ],
            drop_after: true,
        },
        ConnScript {
            notifications: vec![logs_notification(
                300,
                &signature(4),
                pump_create_logs(mint, creator),
            )],
            drop_after: false,
        },
    ])
    .await;
    let ws_url = format!("ws://{addr}/");
    let state = AppState::new(AppConfig {
        raw: detector_config(&ws_url),
        source_path: None,
        warnings: Vec::new(),
    });
    let mut rx = LaunchDetector::spawn(state.clone(), logs_rpc(&ws_url))
        .await
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..3 {
        let e = tokio::time::timeout(Duration::from_secs(15), rx.recv())
            .await
            .expect("event")
            .expect("open");
        seen.push(e);
    }
    assert_eq!(seen[0].event_id, seen[2].event_id, "same launch, same id");
    assert_eq!(seen[2].source_seq, 3);
    // Same id from two observations → consistent (no slot disagreement).
    assert!(seen[0].consistent_with(&seen[2]));
    assert!(state.mark_launch_seen(&seen[0].dedup_key()).await);
    assert!(
        !state.mark_launch_seen(&seen[2].dedup_key()).await,
        "the replayed launch is a duplicate for the pipeline"
    );
    let text = bot_core::obs::metrics::global().encode();
    assert!(text.contains("sniper_feed_out_of_order_total"));
}

// ---------------------------------------------------------------------------
// 3./4. RPC timeout and failover
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rpc_timeout_without_fallback_is_execution_unavailable() {
    let cfg = base_config();
    let state = state_with(cfg);
    let drop_url = spawn_drop_server().await;
    let (mut s, _w) = sniper(state.clone(), mock_rpc(&drop_url)).await;
    let mint = Pubkey::new_unique();
    let o = s
        .consider_event(pump_event(mint, Pubkey::new_unique(), 6))
        .await;
    assert!(o.infra_failure());
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::ExecutionUnavailable);
    assert_eq!(r.stage, SniperStage::Validated);
    assert!(state.open_positions_for(BotModule::Sniper).await.is_empty());
}

#[tokio::test]
async fn rpc_failover_to_a_healthy_provider_completes_the_entry() {
    let node = Arc::new(MockNode::default());
    let good = spawn_mock_node(node.clone()).await;
    let bad = spawn_drop_server().await;
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(base_config());
    let (mut s, _w) = sniper(state.clone(), mock_rpc_with_fallback(&bad, &good)).await;
    let o = s.consider_event(pump_event(mint, creator, 7)).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.stage, SniperStage::Confirmed);
    assert!(
        node.requests.load(Ordering::SeqCst) > 0,
        "the fallback served the reads"
    );
}

#[tokio::test]
async fn tripped_provider_pool_is_reported_before_any_read() {
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    node.fail_all.store(true, Ordering::SeqCst);
    let state = state_with(base_config());
    let rpc = mock_rpc(&url);
    // Trip the breaker with a few failing calls.
    for _ in 0..4 {
        let _ = rpc.get_slot().await;
    }
    assert!(rpc.unhealthy(), "every provider tripped");
    let (mut s, _w) = sniper(state.clone(), rpc).await;
    let before = node.requests.load(Ordering::SeqCst);
    let o = s
        .consider_event(pump_event(Pubkey::new_unique(), Pubkey::new_unique(), 8))
        .await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::ExecutionUnavailable);
    assert!(r.detail.contains("tripped"), "{}", r.detail);
    assert_eq!(
        node.requests.load(Ordering::SeqCst),
        before,
        "no read attempted"
    );
}

// ---------------------------------------------------------------------------
// 5. Stale blockhash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_blockhash_is_replaced_before_broadcast() {
    let mut w = World::new(live_config()).await;
    // First blockhash already expired (last valid height < current height);
    // the second is fresh.
    w.node.block_height.store(5_000, Ordering::SeqCst);
    w.node.push_blockhash(Hash::new_unique(), 4_000);
    w.node.push_blockhash(Hash::new_unique(), 9_000);
    let o = w.sniper.consider_event(w.event(9)).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.stage, SniperStage::Confirmed);
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 1);
    let fetches = w
        .node
        .methods()
        .iter()
        .filter(|m| *m == "getLatestBlockhash")
        .count();
    assert!(fetches >= 1);
}

#[tokio::test]
async fn blockhash_rejected_by_the_node_is_a_definite_failure_with_cooldown() {
    let mut cfg = live_config();
    cfg.risk.sniper_failed_entry_cooldown_secs = 60;
    let mut w = World::new(cfg).await;
    w.node.set_send(SendBehaviour::Reject); // "Blockhash not found"
    let o = w.sniper.consider_event(w.event(10)).await;
    assert_eq!(o.stage, SniperStage::Failed);
    assert!(o
        .rejection
        .as_ref()
        .unwrap()
        .detail
        .contains("blockhash_expired"));
    let rec = bot_core::execution::ledger()
        .get(o.intent_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(matches!(
        rec.state,
        ExecutionState::Expired | ExecutionState::Failed
    ));
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_some());
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
}

// ---------------------------------------------------------------------------
// 6. Failed simulation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_simulation_never_broadcasts_and_marks_the_entry_failed() {
    let mut w = World::new(live_config()).await;
    w.node
        .set_simulate_error(Some("custom program error: 0x1771"));
    let o = w.sniper.consider_event(w.event(11)).await;
    assert_eq!(o.stage, SniperStage::Failed);
    let r = o.rejection.unwrap();
    assert_eq!(r.reason, RejectReason::ExecutionUnavailable);
    assert!(r.detail.contains("SimulationFailed"), "{}", r.detail);
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0, "nothing was sent");
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_some());
}

// ---------------------------------------------------------------------------
// 7. Submit failure (transport ambiguity)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ambiguous_send_books_the_position_and_leaves_the_intent_pending() {
    let mut w = World::new(live_config()).await;
    // The socket dies on send AND the node never shows the transaction:
    // the outcome is genuinely unknown for the whole confirmation window.
    w.node.set_send(SendBehaviour::Drop);
    w.node.confirm.store(false, Ordering::SeqCst);
    let o = w.sniper.consider_event(w.event(12)).await;
    // SendUnknown: the transaction MAY have landed → position booked, stage
    // stays SUBMITTED (never CONFIRMED), ledger parks the intent.
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.stage, SniperStage::Submitted);
    assert_eq!(o.timeline.confirmation_ms(), None);
    let rec = bot_core::execution::ledger()
        .get(o.intent_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(rec.state.is_ambiguous(), "{:?}", rec.state);
    let positions = w.state.open_positions_for(BotModule::Sniper).await;
    assert_eq!(positions.len(), 1);
    assert!(positions[0].entry_signature.is_some());
    // No failed-entry cooldown: ambiguous is not failed.
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_none());
}

// ---------------------------------------------------------------------------
// 8. Confirmation timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn confirmation_timeout_parks_the_entry_and_the_sweeper_holds_it() {
    let mut w = World::new(live_config()).await;
    w.node.confirm.store(false, Ordering::SeqCst);
    let o = w.sniper.consider_event(w.event(13)).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.stage, SniperStage::Submitted);
    let rec = bot_core::execution::ledger()
        .get(o.intent_id.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(rec.state, ExecutionState::Pending);
    // The exit sweeper must not sell an entry whose outcome is unknown.
    w.sniper.sweep_once().await.unwrap();
    let p = &w.state.open_positions_for(BotModule::Sniper).await[0];
    assert_eq!(p.status, PositionStatus::Open);
    let text = bot_core::obs::metrics::global().encode();
    assert!(text.contains("held_ambiguous"));
}

// ---------------------------------------------------------------------------
// 9. Duplicate event / duplicate intent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_intent_is_refused_by_the_execution_ledger() {
    let mut w = World::new(live_config()).await;
    let event = w.event(14);
    let intent = module_sniper::entry::entry_intent_id(
        &event,
        module_sniper::pipeline::EntryRoute::PumpCurve,
    )
    .unwrap();
    // Another replica (or a previous life) already has this intent live.
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: intent.clone(),
            module: "sniper".into(),
            label: "snipe-HRN".into(),
            wallet: w.wallet.pubkey.to_string(),
            symbol: w.mint.to_string(),
        })
        .await
        .unwrap();
    let o = w.sniper.consider_event(event).await;
    assert_eq!(o.stage, SniperStage::Failed);
    assert!(
        o.rejection.unwrap().detail.contains("duplicate"),
        "duplicate guard"
    );
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
}

// ---------------------------------------------------------------------------
// 10. Restart mid-execution: the sweeper resolves restored positions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restart_with_a_failed_entry_cleans_the_position_up_without_selling() {
    let mut w = World::new(live_config()).await;
    // A position restored from the DB whose entry the ledger settled as
    // failed (e.g. expired after the crash).
    let sig = signature(15);
    bot_core::execution::ledger()
        .begin(ExecutionIntent {
            intent_id: "int_restart_failed".into(),
            module: "sniper".into(),
            label: "snipe-HRN".into(),
            wallet: w.wallet.pubkey.to_string(),
            symbol: w.mint.to_string(),
        })
        .await
        .unwrap();
    bot_core::execution::ledger()
        .attach_submission("int_restart_failed", &sig, Some("hash".into()), Some(1), 0)
        .await
        .unwrap();
    bot_core::execution::ledger()
        .fail(
            "int_restart_failed",
            bot_core::execution::FailureClass::BlockhashExpired,
            "expired",
        )
        .await
        .unwrap();
    let mut p = bot_core::models::Position::new(
        "p-restart".into(),
        bot_core::models::TradeSource::Sniper,
        bot_core::models::Venue::PumpFun,
        bot_core::models::ExecutionMode::Live,
        w.mint.to_string(),
        "HRN".into(),
        "SOL".into(),
    );
    p.apply_buy(1_000.0, 0.00001, 0.01);
    p.entry_signature = Some(sig);
    w.state.restore_positions(vec![p]).await;

    w.sniper.sweep_once().await.unwrap();
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
    let closed = w.state.position("p-restart").await.unwrap();
    assert_eq!(closed.status, PositionStatus::Failed);
    assert_eq!(closed.qty, 0.0);
    assert_eq!(
        w.node.sends.load(Ordering::SeqCst),
        0,
        "no sell was attempted"
    );
    assert_eq!(
        w.state.daily_realized(BotModule::Sniper).await,
        0.0,
        "no PnL booked"
    );
}

// ---------------------------------------------------------------------------
// 11. Journal (DB) path: record/link/abandon ordering under failure
// ---------------------------------------------------------------------------

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
            .push(format!("record:{}:{}", rec.side, rec.symbol));
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
async fn intent_journal_records_before_broadcast_and_abandons_on_rejection() {
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    install_pump_token(&node, mint, creator, CurveSpec::fresh());
    let state = state_with(live_config());
    let wallet = Arc::new(solana_kit::tokens::Wallet::generate());
    let sink = Arc::new(RecordingSink::default());
    let mut s = module_sniper::Sniper::new(state.clone(), mock_rpc(&url), wallet, None)
        .await
        .unwrap()
        .with_intent_sink(sink.clone());

    // Simulation rejects: the transaction never left the process → the
    // journal entry is recorded first and then abandoned.
    node.set_simulate_error(Some("custom program error: 0x1"));
    let o = s.consider_event(pump_event(mint, creator, 16)).await;
    assert_eq!(o.stage, SniperStage::Failed);
    {
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].starts_with(&format!("record:buy:{mint}")));
        assert_eq!(calls[1], "abandon");
    }
    // The node answers "no" to the broadcast: the signed transaction DID
    // leave the process, so it is linked (reconciliation can check it),
    // never abandoned.
    node.set_simulate_error(None);
    node.set_send(SendBehaviour::Reject);
    let mint2 = Pubkey::new_unique();
    install_pump_token(&node, mint2, creator, CurveSpec::fresh());
    let o = s.consider_event(pump_event(mint2, creator, 17)).await;
    assert_eq!(o.stage, SniperStage::Failed);
    {
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert!(calls[2].starts_with("record:buy:"));
        assert!(calls[3].starts_with("link:"), "{}", calls[3]);
    }
    // Accepted broadcast: record then link.
    node.set_send(SendBehaviour::Accept);
    let mint3 = Pubkey::new_unique();
    install_pump_token(&node, mint3, creator, CurveSpec::fresh());
    let o = s.consider_event(pump_event(mint3, creator, 18)).await;
    assert_eq!(o.stage, SniperStage::Confirmed);
    let calls = sink.calls.lock().unwrap();
    assert_eq!(calls.len(), 6, "{calls:?}");
    assert!(calls[4].starts_with("record:buy:"));
    assert!(calls[5].starts_with("link:"));
}

// ---------------------------------------------------------------------------
// 12. Risk rejection (covered in pipeline.rs; here: daily sniper loss)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sniper_daily_loss_limit_blocks_new_entries() {
    let mut cfg = base_config();
    cfg.risk.sniper_daily_loss_limit_quote = 0.1;
    let mut w = World::new(cfg).await;
    w.state.add_realized(BotModule::Sniper, -0.2).await;
    let o = w.sniper.consider_event(w.event(18)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::ExposureLimit);
    assert!(r.detail.contains("sniper_daily_loss"), "{}", r.detail);
}

// ---------------------------------------------------------------------------
// 13. Kill switch mid-execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kill_switch_engaged_mid_pipeline_stops_the_hand_off() {
    let mut w = World::new(live_config()).await;
    // Every RPC answer takes 80 ms; the switch flips ~120 ms in, i.e. after
    // validation started but before the transaction is handed over.
    w.node.latency_ms.store(80, Ordering::SeqCst);
    let state = w.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        state.set_kill_switch(true, "operator").await;
    });
    let o = w.sniper.consider_event(w.event(19)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::KillSwitch, "{}", r.detail);
    assert!(matches!(
        r.stage,
        SniperStage::Detected | SniperStage::Validated | SniperStage::RiskApproved
    ));
    assert_eq!(
        w.node.sends.load(Ordering::SeqCst),
        0,
        "nothing was broadcast"
    );
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
}

// ---------------------------------------------------------------------------
// 14./15. Malformed and stale protocol data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_curve_account_is_execution_unavailable_not_a_trade() {
    let mut w = World::new(base_config()).await;
    // Garbage where the bonding curve should be.
    w.node.set_account(
        solana_kit::pump::bonding_curve_pda(&w.mint),
        *solana_kit::consts::PUMP_PROGRAM_ID,
        vec![0xAB; 40],
    );
    let o = w.sniper.consider_event(w.event(20)).await;
    assert!(o.infra_failure());
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::ExecutionUnavailable);
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
}

#[tokio::test]
async fn stale_market_snapshot_is_refused_at_submit_time() {
    let mut cfg = base_config();
    cfg.sniper.max_snapshot_age_ms = 1;
    let mut w = World::new(cfg).await;
    w.node.latency_ms.store(30, Ordering::SeqCst);
    let o = w.sniper.consider_event(w.event(21)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::StaleEvent);
    assert!(
        r.detail.contains("snapshot"),
        "the stale datum is named: {}",
        r.detail
    );
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_feed_payloads_are_dropped_by_the_detector() {
    // A logs notification whose `Program data:` is not a Create event and
    // one whose base64 is corrupt: neither becomes a launch; the healthy
    // one after them does.
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let (addr, _) = spawn_mock_logs_ws(vec![ConnScript {
        notifications: vec![
            logs_notification(400, &signature(22), vec!["Program data: !!!notbase64!!!".into()]),
            logs_notification(401, &signature(23), vec!["Program log: Instruction: Buy".into()]),
            json!({"context": {"slot": 402}, "value": {"signature": "x", "err": {"InstructionError": [0, "Custom"]}, "logs": pump_create_logs(Pubkey::new_unique(), creator)}}),
            logs_notification(403, &signature(24), pump_create_logs(mint, creator)),
        ],
        drop_after: false,
    }])
    .await;
    let ws_url = format!("ws://{addr}/");
    let state = AppState::new(AppConfig {
        raw: detector_config(&ws_url),
        source_path: None,
        warnings: Vec::new(),
    });
    let mut rx = LaunchDetector::spawn(state, logs_rpc(&ws_url))
        .await
        .unwrap();
    let e = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.mint, mint.to_string());
    assert_eq!(e.slot, Some(403));
    assert_eq!(
        e.source_seq, 3,
        "failed txs are skipped before sequencing; junk logs are sequenced but yield nothing"
    );
    assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv())
        .await
        .is_err());
}

// ---------------------------------------------------------------------------
// 16./17. Missing liquidity and invalid token state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_liquidity_is_insufficient_liquidity() {
    let mut w = World::new(base_config()).await;
    let mut spec = CurveSpec::fresh();
    spec.virtual_sol = 0;
    spec.real_sol = 0;
    w.node.set_account(
        solana_kit::pump::bonding_curve_pda(&w.mint),
        *solana_kit::consts::PUMP_PROGRAM_ID,
        bonding_curve_bytes(w.creator, spec),
    );
    let o = w.sniper.consider_event(w.event(25)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::InsufficientLiquidity);
    assert!(o.gates.contains("min_liquidity=fail"));
}

#[tokio::test]
async fn unreadable_mint_is_skipped_by_default_and_rejected_under_strict_gates() {
    let mut w = World::new(base_config()).await;
    w.node.remove_account(&w.mint);
    // Default: the freeze gate is skipped (datum unavailable) and the
    // launch still trades.
    let o = w.sniper.consider_event(w.event(26)).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert!(o.gates.contains("freeze_authority=skip"), "{}", o.gates);
    // Strict: a skip is a failure.
    w.state
        .update_config(|c| c.sniper.strict_gates = true)
        .await;
    let mint2 = Pubkey::new_unique();
    install_pump_token(&w.node, mint2, w.creator, CurveSpec::fresh());
    w.node.remove_account(&mint2);
    let o = w
        .sniper
        .consider_event(pump_event(mint2, w.creator, 27))
        .await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::TokenStateInvalid);
    assert!(r.detail.contains("strict_gates"), "{}", r.detail);
}

#[tokio::test]
async fn pumpswap_event_without_a_pool_account_is_pool_not_ready() {
    let mut w = World::new(base_config()).await;
    let mut ev = w.event(28);
    ev.protocol = LaunchProtocol::PumpSwap;
    ev.pool = Some(Pubkey::new_unique().to_string());
    ev.event_id = ev.compute_event_id();
    let o = w.sniper.consider_event(ev).await;
    let r = o.rejection.expect("rejected");
    assert!(
        matches!(
            r.reason,
            RejectReason::PoolNotReady | RejectReason::ExecutionUnavailable
        ),
        "{r}"
    );
    assert_eq!(w.node.sends.load(Ordering::SeqCst), 0);
    let _ = Utc::now();
}
