//! End-to-end launch detection against a mock PumpPortal websocket.
//!
//! Drives the real production path: `LaunchDetector::spawn` reads the sniper
//! config, opens the feed, subscribes, and maps wire messages into
//! [`TokenLaunch`] values on the module's decision channel — including the
//! module heartbeat side effect. Offline and deterministic.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use bot_core::config::{AppConfig, Config};
use bot_core::models::{BotModule, LaunchFeed};
use bot_core::state::AppState;
use solana_kit::rpc::Rpc;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;

use module_sniper::LaunchDetector;

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
async fn launch_detector_maps_pumpportal_new_token_to_token_launch() {
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let signature =
        "4uQeVj5tqViQh7oWW2qNRjDpGtBaDEhNcVfUv5x9rESpMn8Z2GvqQUfzKb1yWd1HsNqRt9xKcLevbnpWKd8nH1Jq";
    let wire = json!({
        "signature": signature,
        "mint": mint.to_string(),
        "traderPublicKey": creator.to_string(),
        "txType": "create",
        "initialBuy": 2.0,
        "marketCapSol": 31.5,
        "name": "E2E Token",
        "symbol": "E2E",
        "uri": "https://mock.example/e2e.json",
        "pool": "" // empty => must default to "bonding-curve"
    })
    .to_string();

    let (addr, received) = spawn_mock(vec![wire]).await;

    // Real config path: only the feed URL points at the mock, and the second
    // (logsSubscribe) feed is switched off so no stray connections are made.
    let mut cfg = Config::default();
    cfg.sniper.use_pumpportal = true;
    cfg.sniper.pumpportal_ws_url = format!("ws://{addr}/");
    cfg.sniper.use_log_subscription = false;
    let state = AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let mut launches = LaunchDetector::spawn(state.clone(), offline_rpc())
        .await
        .expect("detector must start with the pumpportal feed enabled");

    let launch = tokio::time::timeout(Duration::from_secs(15), launches.recv())
        .await
        .expect("timed out waiting for a launch")
        .expect("launch channel closed");

    // The subscription went out over the wire.
    let frames = received.lock().unwrap().clone();
    assert_eq!(frames, vec![json!({"method": "subscribeNewToken"})]);

    // Field mapping (launch_from_pumpportal) — the contract the entry engine
    // relies on.
    assert_eq!(launch.mint, mint.to_string());
    assert_eq!(launch.name, "E2E Token");
    assert_eq!(launch.symbol, "E2E");
    assert_eq!(launch.uri.as_deref(), Some("https://mock.example/e2e.json"));
    assert_eq!(launch.creator, creator.to_string());
    assert_eq!(launch.pool, "bonding-curve", "empty pool must default");
    assert_eq!(launch.initial_buy_sol, 2.0);
    assert_eq!(launch.market_cap_sol, 31.5);
    assert_eq!(launch.signature.as_deref(), Some(signature));
    assert_eq!(launch.tx_type.as_deref(), Some("create"));
    assert!(matches!(launch.feed, LaunchFeed::PumpPortal));
    assert!(launch.socials.is_none());

    // Side effect: the detector heartbeated the sniper module, which is what
    // the readiness probe consumes.
    let status = state.module_status(BotModule::Sniper).await;
    assert!(
        status.last_heartbeat.is_some(),
        "detect must heartbeat the module on every launch"
    );
}
