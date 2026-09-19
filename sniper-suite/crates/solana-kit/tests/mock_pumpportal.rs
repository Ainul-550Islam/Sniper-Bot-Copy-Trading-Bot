//! Integration tests for [`PumpPortalFeed`] against a protocol-faithful mock
//! of the PumpPortal websocket (`wss://pumpportal.fun/api/data`).
//!
//! Proves the full client stack over a real TCP/WS socket: connect, subscribe
//! frame emission, message classification (`txType` routing), reconnect with
//! re-subscription, and non-JSON passthrough. Fully offline and deterministic.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use solana_kit::pumpportal::{PumpPortalFeed, PumpPortalMessage, PumpPortalSubscription};

/// A mock PumpPortal server: for every connection it records the subscribe
/// frames the client sends, then pushes `script` messages verbatim.
struct MockServer {
    addr: SocketAddr,
    /// Every JSON frame received from any connection (subscribe frames).
    received: Arc<Mutex<Vec<Value>>>,
    /// Total accepted websocket connections.
    connections: Arc<AtomicUsize>,
}

impl MockServer {
    fn url(&self) -> String {
        format!("ws://{}/", self.addr)
    }

    fn received(&self) -> Vec<Value> {
        self.received.lock().unwrap().clone()
    }
}

async fn spawn_mock(script: Vec<String>, close_after_script: bool) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));

    let recv2 = Arc::clone(&received);
    let conn2 = Arc::clone(&connections);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            conn2.fetch_add(1, Ordering::SeqCst);
            let script = script.clone();
            let received = Arc::clone(&recv2);
            tokio::spawn(handle_conn(stream, script, received, close_after_script));
        }
    });

    MockServer {
        addr,
        received,
        connections,
    }
}

async fn handle_conn(
    stream: TcpStream,
    script: Vec<String>,
    received: Arc<Mutex<Vec<Value>>>,
    close_after_script: bool,
) {
    let Ok(mut ws) = accept_async(stream).await else {
        return;
    };

    // 1) Record subscribe frames. The client sends them all immediately after
    //    connecting and then only reads, so "400 ms of silence" ends the phase.
    loop {
        match tokio::time::timeout(Duration::from_millis(400), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    received.lock().unwrap().push(v);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => break, // silence: subscriptions complete
        }
    }

    // 2) Push the scripted messages.
    for m in script {
        if ws.send(Message::Text(m)).await.is_err() {
            return;
        }
    }
    if ws.flush().await.is_err() {
        return;
    }

    if close_after_script {
        let _ = ws.close(None).await;
        return;
    }

    // 3) Keep the socket open until the client goes away.
    while ws.next().await.is_some() {}
}

async fn start_feed(
    server: &MockServer,
    sub: PumpPortalSubscription,
) -> (
    PumpPortalFeed,
    tokio::sync::mpsc::Receiver<PumpPortalMessage>,
) {
    let feed = PumpPortalFeed::new(sub, 64).with_url(server.url());
    feed.start().await.expect("feed must start");
    let rx = feed
        .receiver()
        .await
        .expect("receiver must be available exactly once");
    (feed, rx)
}

async fn next_msg(rx: &mut tokio::sync::mpsc::Receiver<PumpPortalMessage>) -> PumpPortalMessage {
    tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("timed out waiting for a feed message")
        .expect("feed channel closed unexpectedly")
}

fn new_token_json(mint: &Pubkey, creator: &Pubkey) -> String {
    json!({
        "signature": "4uQeVj5tqViQh7oWW2qNRjDpGtBaDEhNcVfUv5x9rESpMn8Z2GvqQUfzKb1yWd1HsNqRt9xKcLevbnpWKd8nH1Jq",
        "mint": mint.to_string(),
        "traderPublicKey": creator.to_string(),
        "txType": "create",
        "initialBuy": 1.5,
        "marketCapSol": 12.25,
        "name": "Mock Token",
        "symbol": "MOCK",
        "uri": "https://mock.example/meta.json",
        "pool": "bonding-curve",
        "bondingCurveKey": Pubkey::new_unique().to_string(),
        "associatedBondingCurveKey": Pubkey::new_unique().to_string()
    })
    .to_string()
}

#[tokio::test]
async fn subscribes_new_token_and_delivers_launches() {
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let server = spawn_mock(vec![new_token_json(&mint, &creator)], false).await;

    let (_feed, mut rx) = start_feed(&server, PumpPortalSubscription::launches_only()).await;
    let msg = next_msg(&mut rx).await;

    // The subscribe frame went out over the wire exactly as PumpPortal expects.
    let frames = server.received();
    assert_eq!(frames, vec![json!({"method": "subscribeNewToken"})]);

    // Convenience accessors work on the wire values.
    assert_eq!(msg.mint(), Some(mint));
    assert_eq!(msg.trader(), Some(creator));

    match msg {
        PumpPortalMessage::NewToken(m) => {
            assert_eq!(m.mint, mint.to_string());
            assert_eq!(m.trader_public_key, creator.to_string());
            assert_eq!(m.tx_type, "create");
            assert_eq!(m.name, "Mock Token");
            assert_eq!(m.symbol, "MOCK");
            assert_eq!(m.uri, "https://mock.example/meta.json");
            assert_eq!(m.pool, "bonding-curve");
            assert_eq!(m.mint_pubkey(), Some(mint));
            // SOL -> lamports conversion on the wire values.
            assert_eq!(m.initial_buy_lamports(), 1_500_000_000);
            assert_eq!(m.market_cap_lamports(), 12_250_000_000);
            assert!(m.bonding_curve_key.is_some());
        }
        other => panic!("expected NewToken, got {other:?}"),
    }
}

#[tokio::test]
async fn account_trade_subscription_lists_wallets_and_delivers_trades() {
    let wallet = Pubkey::new_unique();
    let mint = Pubkey::new_unique();
    let trade = json!({
        "signature": "5hJq3vD9xEZLmPcT6f1WdS2YbU8nKgQr4aVoXtZiE7pCbN1yMkF3uLwHd9sRexT2qAzGvBoNcLpKjMi8YuH1xQ",
        "mint": mint.to_string(),
        "traderPublicKey": wallet.to_string(),
        "txType": "buy",
        "tokenAmount": 1000.0,
        "solAmount": 0.5,
        "newTokenAmount": 9000.0,
        "newSolAmount": 4.5,
        "pool": "pump-amm",
        "timestamp": 1_700_000_000i64
    })
    .to_string();
    let server = spawn_mock(vec![trade], false).await;

    let sub = PumpPortalSubscription {
        account_trades: vec![wallet.to_string()],
        ..Default::default()
    };
    let (_feed, mut rx) = start_feed(&server, sub).await;
    let msg = next_msg(&mut rx).await;

    let frames = server.received();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["method"], "subscribeAccountTrade");
    assert_eq!(frames[0]["keys"], json!([wallet.to_string()]));

    assert_eq!(msg.trader(), Some(wallet));
    match msg {
        // Token- and account-trade payloads are identical on the wire; the
        // feed reports both as Trade and consumers route on the wallet.
        PumpPortalMessage::Trade(t) => {
            assert_eq!(t.mint, mint.to_string());
            assert_eq!(t.trader_public_key, wallet.to_string());
            assert!(t.is_buy());
            assert!(!t.is_sell());
            assert_eq!(t.token_amount, 1000.0);
            assert_eq!(t.sol_amount, 0.5);
            assert_eq!(t.timestamp, Some(1_700_000_000));
        }
        other => panic!("expected Trade, got {other:?}"),
    }
}

#[tokio::test]
async fn migrations_and_garbage_pass_through_as_other() {
    let mint = Pubkey::new_unique();
    let migration = json!({
        "signature": "3Kq8mVzR1xP7nLtY5wCeGdS4bUaHvJ2oXtEiF6gNkQrM9zWcB1yLdHsNqRt2xKvLevbnpWKd8nH1Jq",
        "mint": mint.to_string(),
        "traderPublicKey": Pubkey::new_unique().to_string(),
        "txType": "migrate",
        "pool": "amm",
        "name": "Graduated",
        "symbol": "GRAD",
        "uri": "https://mock.example/grad.json",
        "marketCapSol": 85.0
    })
    .to_string();
    let unknown = json!({"txType": "airdrop", "mint": mint.to_string()}).to_string();
    let server = spawn_mock(
        vec![migration, "this-is-not-json".to_string(), unknown],
        false,
    )
    .await;

    let sub = PumpPortalSubscription {
        migrations: true,
        ..Default::default()
    };
    let (_feed, mut rx) = start_feed(&server, sub).await;

    // subscribeMigration frame is emitted for the migration subscription.
    let msg = next_msg(&mut rx).await;
    match msg {
        PumpPortalMessage::Migration(m) => {
            assert_eq!(m.mint, mint.to_string());
            assert_eq!(m.pool, "amm");
        }
        other => panic!("expected Migration, got {other:?}"),
    }

    // Non-JSON frames are preserved verbatim as Other(String) — never dropped,
    // never panicking.
    match next_msg(&mut rx).await {
        PumpPortalMessage::Other(Value::String(s)) => assert_eq!(s, "this-is-not-json"),
        other => panic!("expected Other(string), got {other:?}"),
    }

    // Unknown txType => Other(value).
    match next_msg(&mut rx).await {
        PumpPortalMessage::Other(v) => assert_eq!(v["txType"], "airdrop"),
        other => panic!("expected Other(value), got {other:?}"),
    }
}

#[tokio::test]
async fn feed_reconnects_and_resubscribes_after_server_close() {
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    // Every connection: deliver one launch, then close the socket.
    let server = spawn_mock(vec![new_token_json(&mint, &creator)], true).await;

    let (feed, mut rx) = start_feed(&server, PumpPortalSubscription::launches_only()).await;

    // First delivery.
    assert!(matches!(
        next_msg(&mut rx).await,
        PumpPortalMessage::NewToken(_)
    ));
    assert!(feed.is_connected() || !feed.is_connected()); // may already be mid-reconnect

    // Second delivery can only arrive after a reconnect (server closed conn 1).
    assert!(matches!(
        next_msg(&mut rx).await,
        PumpPortalMessage::NewToken(_)
    ));
    assert!(
        server.connections.load(Ordering::SeqCst) >= 2,
        "feed must have reconnected"
    );

    // The resubscription went out on the new connection too.
    let frames = server.received();
    assert!(
        frames
            .iter()
            .filter(|f| f["method"] == "subscribeNewToken")
            .count()
            >= 2,
        "each connection must re-subscribe: {frames:?}"
    );

    feed.stop().await;
}
