//! End-to-end launch detection through a mock Geyser `transactionSubscribe`
//! websocket (BUILD PLAN §5).
//!
//! Drives the real production path: `LaunchDetector::spawn` with
//! `sniper.use_transaction_subscribe` + `network.geyser_ws_url` connects,
//! subscribes with the pump program in `accountInclude`, and turns a pushed
//! `transactionNotification` (full base64 tx + meta with log messages) into a
//! [`TokenLaunch`] via the same event parser the logsSubscribe feed uses —
//! tagged `LaunchFeed::TransactionSubscribe` and carrying the push slot.
//! Offline and deterministic.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::hash::Hash;
use solana_sdk::message::{v0, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransaction, TransactionBinaryEncoding, UiTransactionStatusMeta,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use bot_core::config::{AppConfig, Config};
use bot_core::models::{BotModule, LaunchFeed};
use bot_core::state::AppState;
use solana_kit::consts::{EV_PUMP_CREATE, PUMP_PROGRAM_ID, TOKEN_PROGRAM, WSOL_MINT};
use solana_kit::rpc::Rpc;

use module_sniper::LaunchDetector;

const SERVER_SUB_ID: u64 = 4242;

/// Mock Geyser endpoint: answers `transactionSubscribe` with a subscription
/// id, then pushes the scripted notifications.
async fn spawn_mock_geyser(notifications: Vec<Value>) -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recv2 = Arc::clone(&received);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let notes = notifications.clone();
            let received = Arc::clone(&recv2);
            tokio::spawn(handle_conn(stream, notes, received));
        }
    });
    (addr, received)
}

async fn handle_conn(stream: TcpStream, notes: Vec<Value>, received: Arc<Mutex<Vec<Value>>>) {
    let Ok(mut ws) = accept_async(stream).await else {
        return;
    };
    let mut pushed = false;
    loop {
        match tokio::time::timeout(Duration::from_millis(250), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    let is_sub = v["method"] == "transactionSubscribe";
                    received.lock().unwrap().push(v.clone());
                    if is_sub && !pushed {
                        pushed = true;
                        let id = v["id"].clone();
                        let ack = json!({"jsonrpc": "2.0", "id": id, "result": SERVER_SUB_ID});
                        if ws.send(Message::Text(ack.to_string())).await.is_err() {
                            return;
                        }
                        for n in notes.clone() {
                            let frame = json!({
                                "jsonrpc": "2.0",
                                "method": "transactionNotification",
                                "params": {"subscription": SERVER_SUB_ID, "result": n},
                            });
                            if ws.send(Message::Text(frame.to_string())).await.is_err() {
                                return;
                            }
                        }
                        let _ = ws.flush().await;
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => {
                if pushed {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
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

/// A real signed v0 transaction touching the pump program, serialised the way
/// a base64 Geyser subscription delivers it.
fn wire_tx_json(payer: &Pubkey, programs: &[Pubkey], keypair: &Keypair) -> Value {
    let ixs: Vec<solana_sdk::instruction::Instruction> = programs
        .iter()
        .map(|p| solana_sdk::instruction::Instruction {
            program_id: *p,
            accounts: vec![solana_sdk::instruction::AccountMeta::new(*payer, true)],
            data: vec![1],
        })
        .collect();
    let msg = v0::Message::try_compile(payer, &ixs, &[], Hash::default()).unwrap();
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[keypair]).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx).unwrap());
    let encoded = EncodedTransaction::Binary(b64, TransactionBinaryEncoding::Base64);
    serde_json::to_value(&encoded).unwrap()
}

/// Anchor-encode a pump `Create` event body (field order = `decode_create`).
fn create_event_payload(
    name: &str,
    symbol: &str,
    uri: &str,
    mint: Pubkey,
    creator: Pubkey,
) -> Vec<u8> {
    fn string(out: &mut Vec<u8>, v: &str) {
        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        out.extend_from_slice(v.as_bytes());
    }
    let mut b = Vec::new();
    b.extend_from_slice(&EV_PUMP_CREATE);
    string(&mut b, name);
    string(&mut b, symbol);
    string(&mut b, uri);
    b.extend_from_slice(mint.as_ref()); // mint
    b.extend_from_slice(Pubkey::new_unique().as_ref()); // bonding curve
    b.extend_from_slice(creator.as_ref()); // user (the creator opens the curve)
    b.extend_from_slice(creator.as_ref()); // creator
    b.extend_from_slice(&1_750_000_000i64.to_le_bytes()); // timestamp
    b.extend_from_slice(&1_073_000_000_000u64.to_le_bytes()); // virtual token reserves
    b.extend_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual sol reserves
    b.extend_from_slice(&793_100_000_000u64.to_le_bytes()); // real token reserves
    b.extend_from_slice(&1_000_000_000_000u64.to_le_bytes()); // total supply
    b.extend_from_slice(TOKEN_PROGRAM.as_ref()); // token program
    b.push(0); // is_mayhem_mode
    b.push(0); // is_cashback_enabled
               // Multi-quote upgrade fields.
    b.extend_from_slice(WSOL_MINT.as_ref()); // quote mint
    b.extend_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual quote reserves
    b.extend_from_slice(&100u64.to_le_bytes()); // creator fee bps
    b.push(0); // is_holder_reward
    b
}

fn meta_with_logs(logs: Vec<String>) -> UiTransactionStatusMeta {
    UiTransactionStatusMeta {
        err: None,
        status: Ok(()),
        fee: 5000,
        pre_balances: vec![],
        post_balances: vec![],
        inner_instructions: OptionSerializer::Skip,
        log_messages: OptionSerializer::Some(logs),
        pre_token_balances: OptionSerializer::Skip,
        post_token_balances: OptionSerializer::Skip,
        rewards: OptionSerializer::Skip,
        loaded_addresses: OptionSerializer::Skip,
        return_data: OptionSerializer::Skip,
        compute_units_consumed: OptionSerializer::Skip,
        cost_units: OptionSerializer::Skip,
    }
}

#[tokio::test]
async fn geyser_transaction_subscribe_yields_a_token_launch() {
    let creator = Keypair::new();
    let mint = Pubkey::new_unique();

    let payload = create_event_payload(
        "Geyser Token",
        "GEYSER",
        "https://mock.example/geyser.json",
        mint,
        creator.pubkey(),
    );
    let data_log = format!(
        "Program data: {}",
        base64::engine::general_purpose::STANDARD.encode(&payload)
    );
    let logs = vec![
        format!("Program {} invoke [1]", *PUMP_PROGRAM_ID),
        "Program log: Instruction: Create".to_string(),
        data_log,
        format!("Program {} success", *PUMP_PROGRAM_ID),
    ];

    let sig = "3geyser3geyser3geyser3geyser3geyser3geyser3geyser3geyser3geyser3geyser3geyser3gey";
    let note = json!({
        "signature": sig,
        "slot": 310_000_999u64,
        "blockTime": 1_750_000_000i64,
        "transaction": wire_tx_json(&creator.pubkey(), &[*PUMP_PROGRAM_ID], &creator),
        "meta": serde_json::to_value(meta_with_logs(logs.clone())).unwrap(),
    });

    // A failed create must be skipped even though its logs contain the event.
    let mut failed = note.clone();
    failed["signature"] =
        json!("2skipped2skipped2skipped2skipped2skipped2skipped2skipped2skipped2skippe");
    failed["meta"]["err"] = json!({"InstructionError": [0, {"Custom": 6000}]});

    let (addr, received) = spawn_mock_geyser(vec![failed, note]).await;

    let mut cfg = Config::default();
    cfg.sniper.use_pumpportal = false;
    cfg.sniper.use_log_subscription = false;
    cfg.sniper.use_transaction_subscribe = true;
    cfg.network.geyser_ws_url = Some(format!("ws://{addr}/"));
    let state = AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let mut launches = LaunchDetector::spawn(state.clone(), offline_rpc())
        .await
        .expect("detector must start with the geyser feed enabled");

    let launch = tokio::time::timeout(Duration::from_secs(15), launches.recv())
        .await
        .expect("timed out waiting for the pushed launch")
        .expect("launch channel closed");

    // Decoded from the pushed Create event.
    assert_eq!(launch.mint, mint.to_string());
    assert_eq!(launch.name, "Geyser Token");
    assert_eq!(launch.symbol, "GEYSER");
    assert_eq!(
        launch.uri.as_deref(),
        Some("https://mock.example/geyser.json")
    );
    assert_eq!(launch.creator, creator.pubkey().to_string());
    assert_eq!(launch.pool, "bonding-curve");
    assert_eq!(launch.tx_type.as_deref(), Some("create"));
    assert_eq!(launch.signature.as_deref(), Some(sig));
    // Geyser-specific contract: the push feed is identified and slotted.
    assert!(matches!(launch.feed, LaunchFeed::TransactionSubscribe));
    assert_eq!(launch.slot, Some(310_000_999));
    // 30 SOL virtual over 1.073T virtual tokens, scaled to the 1B real supply.
    assert!(
        launch.market_cap_sol > 0.0,
        "mcap must be derived from reserves"
    );
    assert_eq!(launch.total_supply, Some(1_000_000.0));
    assert!(launch.socials.is_none());

    // The failed notification must not produce a second launch.
    assert!(
        tokio::time::timeout(Duration::from_millis(400), launches.recv())
            .await
            .is_err(),
        "failed creates must be skipped"
    );

    // The subscription filtered on the pump program.
    let frames = received.lock().unwrap().clone();
    let sub = frames
        .iter()
        .find(|f| f["method"] == "transactionSubscribe")
        .expect("a transactionSubscribe frame was sent");
    assert_eq!(
        sub["params"][0]["accountInclude"],
        json!([PUMP_PROGRAM_ID.to_string()])
    );

    // Side effect: the detector heartbeated the sniper module.
    let status = state.module_status(BotModule::Sniper).await;
    assert!(status.last_heartbeat.is_some());
}

#[tokio::test]
async fn geyser_feed_requires_a_url_and_does_not_start_without_one() {
    // use_transaction_subscribe without geyser_ws_url and with every other
    // feed disabled: spawn must fail loudly rather than pretend to run.
    let mut cfg = Config::default();
    cfg.sniper.use_pumpportal = false;
    cfg.sniper.use_log_subscription = false;
    cfg.sniper.use_transaction_subscribe = true;
    cfg.network.geyser_ws_url = None;
    let state = AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let err = LaunchDetector::spawn(state, offline_rpc())
        .await
        .expect_err("no startable feed must be a config error");
    assert!(
        err.to_string().contains("use_transaction_subscribe"),
        "the error must name the misconfigured feed: {err}"
    );
}
