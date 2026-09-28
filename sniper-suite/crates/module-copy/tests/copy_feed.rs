//! End-to-end copy-feed test against a mock PumpPortal websocket.
//!
//! Drives the real production path: `CopyFeed::spawn` reads the copy config,
//! subscribes to `subscribeAccountTrade` for the tracked wallets, and maps
//! wire trades into [`WalletTrade`] values (side + venue inference included).
//! Offline and deterministic.
//!
//! TASK 3: also proves the authoritative single-mark dedup contract for
//! PumpPortal deliveries — the feed does not pre-mark anything, the
//! pipeline's `copy_event` mark decides each trade exactly once, and a
//! buy and a sell of the same mint are distinct events.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use bot_core::config::{AppConfig, Config, CopyWallet};
use bot_core::models::{BotModule, PositionSide, Venue};
use bot_core::state::AppState;
use solana_kit::rpc::Rpc;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;

use module_copy::event::{EventSource, LeaderTradeEvent};
use module_copy::event_dedup::{self, DedupOutcome};
use module_copy::feeds::CopyFeed;

async fn spawn_mock(script: Vec<String>) -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recv2 = Arc::clone(&received);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let script = script.clone();
            let received = Arc::clone(&recv2);
            tokio::spawn(handle_conn(stream, script, received));
        }
    });
    (addr, received)
}

async fn handle_conn(stream: TcpStream, script: Vec<String>, received: Arc<Mutex<Vec<Value>>>) {
    let Ok(mut ws) = accept_async(stream).await else {
        return;
    };
    loop {
        match tokio::time::timeout(Duration::from_millis(400), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    received.lock().unwrap().push(v);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => break,
        }
    }
    for m in script {
        if ws.send(Message::Text(m)).await.is_err() {
            return;
        }
    }
    let _ = ws.flush().await;
    while ws.next().await.is_some() {}
}

fn offline_rpc() -> Rpc {
    Rpc::with_urls(
        "http://127.0.0.1:1".into(),
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        1,
        Duration::from_millis(100),
    )
    .expect("rpc builds offline")
}

#[tokio::test]
async fn copy_feed_subscribes_tracked_wallets_and_maps_trades() {
    let whale = Pubkey::new_unique();
    let mint = Pubkey::new_unique();

    // Two trades: a bonding-curve buy and a PumpSwap sell (venue inference).
    let buy = json!({
        "signature": "5hJq3vD9xEZLmPcT6f1WdS2YbU8nKgQr4aVoXtZiE7pCbN1yMkF3uLwHd9sRexT2qAzGvBoNcLpKjMi8YuH1xQ",
        "mint": mint.to_string(),
        "traderPublicKey": whale.to_string(),
        "txType": "buy",
        "tokenAmount": 50_000.0,
        "solAmount": 2.5,
        "pool": "bonding-curve",
        "timestamp": 1_700_000_000i64
    })
    .to_string();
    let sell = json!({
        "signature": "2Lm9nWzS2yQ8oXfU6vEaT3bJcKdR1pHzGiF4hNkVoXt5aWqNrT7zMeIsOtQu3yBgWfCpXoAeLjNk7ZuJ2wI9yR",
        "mint": mint.to_string(),
        "traderPublicKey": whale.to_string(),
        "txType": "sell",
        "tokenAmount": 10_000.0,
        "solAmount": 0.75,
        "pool": "pump-amm",
        "timestamp": 1_700_000_500i64
    })
    .to_string();

    let (addr, received) = spawn_mock(vec![buy, sell]).await;

    let mut cfg = Config::default();
    cfg.copy.feed = "pumpportal".into();
    cfg.copy.wallets = vec![CopyWallet {
        address: whale.to_string(),
        label: Some("test-whale".into()),
        fixed_sol: None,
        fraction_of_their_size: 0.05,
        max_sol: 1.0,
        min_sol: 0.0,
        buys_only: false,
        max_staleness_secs: 300,
        slippage_pct: Some(15.0),
        paused: false,
        max_exposure_sol: 0.0,
        max_open_positions: 0,
    }];
    // The copy feed reuses the sniper PumpPortal URL setting.
    cfg.sniper.pumpportal_ws_url = format!("ws://{addr}/");
    let state = AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let mut trades = CopyFeed::spawn(state.clone(), offline_rpc())
        .await
        .expect("copy feed must start with the pumpportal source");

    // First delivery proves connect + subscribe happened; only then are the
    // captured frames guaranteed to be complete (spawn returns before the
    // connection task has written anything).
    let t1 = tokio::time::timeout(Duration::from_secs(15), trades.recv())
        .await
        .expect("timed out waiting for trade 1")
        .expect("channel closed");

    // Subscription names the tracked wallet(s).
    let frames = received.lock().unwrap().clone();
    assert_eq!(frames.len(), 1, "exactly one subscribe frame: {frames:?}");
    assert_eq!(frames[0]["method"], "subscribeAccountTrade");
    assert_eq!(frames[0]["keys"], json!([whale.to_string()]));
    assert_eq!(t1.wallet, whale.to_string());
    assert_eq!(t1.mint, mint.to_string());
    assert!(matches!(t1.side, PositionSide::Long), "buy => Long");
    assert!(
        matches!(t1.venue, Venue::PumpFun),
        "bonding-curve => PumpFun"
    );
    assert_eq!(t1.token_amount, 50_000.0);
    assert_eq!(t1.sol_amount, 2.5);
    assert!(
        !t1.signature.is_empty() && t1.signature.chars().all(|c| c != ' '),
        "signature passes through verbatim"
    );
    assert!(t1.block_time.is_some(), "timestamp maps to block_time");

    let t2 = tokio::time::timeout(Duration::from_secs(15), trades.recv())
        .await
        .expect("timed out waiting for trade 2")
        .expect("channel closed");
    assert!(matches!(t2.side, PositionSide::Short), "sell => Short");
    assert!(matches!(t2.venue, Venue::PumpSwap), "pump-amm => PumpSwap");

    // Authoritative single-mark dedup: the PumpPortal feed pre-marks nothing
    // (`sig` namespace untouched), the pipeline's `copy_event` mark is fresh
    // exactly once per event, and the buy / sell are distinct events.
    assert!(
        state.mark_signature_seen(&t1.signature).await,
        "the pumpportal feed does not mark the sig namespace"
    );
    let e1 = LeaderTradeEvent::from_wallet_trade(&t1, EventSource::PumpPortal, 1);
    let e2 = LeaderTradeEvent::from_wallet_trade(&t2, EventSource::PumpPortal, 2);
    assert_ne!(e1.event_id, e2.event_id);
    assert_eq!(event_dedup::claim(&state, &e1).await, DedupOutcome::Fresh);
    assert_eq!(
        event_dedup::claim(&state, &e1).await,
        DedupOutcome::Duplicate
    );
    assert_eq!(event_dedup::claim(&state, &e2).await, DedupOutcome::Fresh);
    assert_eq!(state.seen_copy_event_count().await, 2);

    // Side effect: the feed heartbeats the copy module (readiness probe data).
    let status = state.module_status(BotModule::Copy).await;
    assert!(status.last_heartbeat.is_some());
}
