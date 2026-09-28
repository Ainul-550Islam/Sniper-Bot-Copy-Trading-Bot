//! End-to-end test of the Geyser `transactionSubscribe` copy feed (BUILD
//! PLAN §5) against a mock Yellowstone-compatible websocket.
//!
//! Drives the real production path: `CopyFeed::spawn` with
//! `copy.feed = "transaction_subscribe"` + `network.geyser_ws_url` connects,
//! subscribes with the tracked wallets in `accountInclude`, parses the pushed
//! `transactionNotification` (full base64 tx + meta), decodes the swap
//! through the same pipeline as polling, and emits a [`WalletTrade`].
//! Offline and deterministic.
//!
//! TASK 3 regression guard: the feed's `mark_signature_seen` is fetch
//! suppression only. The emitted trade must still be *fresh* for the
//! pipeline's one authoritative dedup (`AppState::mark_copy_event_seen`,
//! `copy_event` namespace) — before TASK 3 the run loop re-marked the `sig`
//! namespace and every Geyser / polled event was dropped as a duplicate.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::hash::Hash;
use solana_sdk::message::v0;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransaction, TransactionBinaryEncoding, UiTransactionStatusMeta,
    UiTransactionTokenBalance,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use bot_core::config::{AppConfig, Config, CopyWallet};
use bot_core::models::{PositionSide, Venue};
use bot_core::state::AppState;
use solana_kit::consts::{PUMP_PROGRAM_ID, WSOL_MINT};
use solana_kit::rpc::Rpc;

use module_copy::event::{EventSource, LeaderTradeEvent};
use module_copy::event_dedup::{self, DedupOutcome};
use module_copy::feeds::CopyFeed;

const SERVER_SUB_ID: u64 = 777;

/// Mock Geyser endpoint: answers `transactionSubscribe` with a subscription
/// id, then pushes the scripted notifications, and records every client
/// frame for assertions.
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
                        // Confirm the subscription (the client waits on this).
                        let id = v["id"].clone();
                        let ack = json!({"jsonrpc": "2.0", "id": id, "result": SERVER_SUB_ID});
                        if ws.send(Message::Text(ack.to_string())).await.is_err() {
                            return;
                        }
                        // Then push every scripted notification.
                        eprintln!(
                            "MOCK: acked subscribe, pushing {} notifications",
                            notes.len()
                        );
                        for n in notes.clone() {
                            let frame = json!({
                                "jsonrpc": "2.0",
                                "method": "transactionNotification",
                                "params": {"subscription": SERVER_SUB_ID, "result": n},
                            });
                            if ws.send(Message::Text(frame.to_string())).await.is_err() {
                                eprintln!("MOCK: send failed");
                                return;
                            }
                        }
                        let _ = ws.flush().await;
                        eprintln!("MOCK: notifications flushed");
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => {
                if pushed {
                    // Stay open so the client keeps its subscription; just
                    // stop waiting on reads once everything was delivered.
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

/// A real signed v0 transaction, serialised the way a base64 Geyser
/// subscription delivers it (`EncodedTransaction::Binary` on the wire).
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

fn balance(
    index: u8,
    mint: Pubkey,
    amount: u64,
    decimals: u8,
    owner: Pubkey,
) -> UiTransactionTokenBalance {
    serde_json::from_value(json!({
        "accountIndex": index,
        "mint": mint.to_string(),
        "uiTokenAmount": {
            "uiAmount": null,
            "decimals": decimals,
            "amount": amount.to_string(),
            "uiAmountString": ""
        },
        "owner": owner.to_string()
    }))
    .unwrap()
}

fn swap_meta(
    pre: Vec<UiTransactionTokenBalance>,
    post: Vec<UiTransactionTokenBalance>,
) -> UiTransactionStatusMeta {
    UiTransactionStatusMeta {
        err: None,
        status: Ok(()),
        fee: 5000,
        pre_balances: vec![],
        post_balances: vec![],
        inner_instructions: OptionSerializer::Skip,
        log_messages: OptionSerializer::Skip,
        pre_token_balances: OptionSerializer::Some(pre),
        post_token_balances: OptionSerializer::Some(post),
        rewards: OptionSerializer::Skip,
        loaded_addresses: OptionSerializer::Skip,
        return_data: OptionSerializer::Skip,
        compute_units_consumed: OptionSerializer::Skip,
        cost_units: OptionSerializer::Skip,
    }
}

#[tokio::test]
async fn geyser_feed_pushes_a_whale_buy_into_the_copy_channel() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let whale = Keypair::new();
    let mint = Pubkey::new_unique();

    // The notification exactly as a Yellowstone-style endpoint delivers it:
    // full base64 transaction + meta with the whale's token balances.
    let pre = vec![
        balance(1, *WSOL_MINT, 5_000_000_000, 9, whale.pubkey()),
        balance(2, mint, 0, 6, whale.pubkey()),
    ];
    let post = vec![
        balance(1, *WSOL_MINT, 3_500_000_000, 9, whale.pubkey()),
        balance(2, mint, 25_000_000, 6, whale.pubkey()),
    ];
    let sig = "4uQeVj5tqViQh7oWWkqRuGQgD6Yx1TtE6GR7eVYpump11111111111111111111111111111111";
    let note = json!({
        "signature": sig,
        "slot": 310_000_123u64,
        "blockTime": 1_750_000_000i64,
        "transaction": wire_tx_json(&whale.pubkey(), &[*PUMP_PROGRAM_ID, Pubkey::new_unique()], &whale),
        "meta": serde_json::to_value(swap_meta(pre, post)).unwrap(),
    });
    // A failed transaction must be skipped silently.
    let mut failed = note.clone();
    failed["signature"] =
        json!("5failed5failed5failed5failed5failed5failed5failed5failed5failed5fa");
    failed["meta"]["err"] = json!({"InstructionError": [0, {"Custom": 1}]});

    let (addr, received) = spawn_mock_geyser(vec![failed, note]).await;

    let mut cfg = Config::default();
    cfg.copy.feed = "transaction_subscribe".into();
    cfg.copy.enabled = true;
    cfg.copy.wallets = vec![CopyWallet {
        address: whale.pubkey().to_string(),
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
    cfg.network.geyser_ws_url = Some(format!("ws://{addr}/"));
    let state = AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let mut trades = CopyFeed::spawn(state.clone(), offline_rpc())
        .await
        .expect("geyser copy feed must start");

    // The push arrives without any polling round trip.
    let t = tokio::time::timeout(Duration::from_secs(15), trades.recv())
        .await
        .unwrap_or_else(|e| {
            panic!(
                "timed out waiting for the pushed trade: {e}; frames seen by mock: {:?}",
                received.lock().unwrap()
            )
        })
        .expect("channel closed");

    assert_eq!(t.wallet, whale.pubkey().to_string());
    assert_eq!(t.signature, sig);
    assert_eq!(t.slot, 310_000_123);
    assert!(matches!(t.side, PositionSide::Long), "buy => Long");
    assert!(matches!(t.venue, Venue::PumpFun), "pump program => PumpFun");
    assert_eq!(t.mint, mint.to_string());
    assert_eq!(t.token_amount, 25.0);
    assert!((t.sol_amount - 1.5).abs() < 1e-9, "1.5 SOL spent");
    assert!(t.block_time.is_some());

    // The feed marked the signature for fetch suppression (`sig` namespace)…
    assert!(
        !state.mark_signature_seen(sig).await,
        "the geyser feed marks each decoded signature once"
    );
    // …which must NOT make the pipeline drop the event: the authoritative
    // dedup lives in the `copy_event` namespace and sees the event as fresh
    // exactly once, then as a duplicate.
    let event = LeaderTradeEvent::from_wallet_trade(&t, EventSource::TransactionSubscribe, 1);
    assert!(!state.copy_event_seen(&event.dedup_key()).await);
    assert_eq!(
        event_dedup::claim(&state, &event).await,
        DedupOutcome::Fresh,
        "a geyser-delivered trade is not a duplicate on first processing"
    );
    assert_eq!(
        event_dedup::claim(&state, &event).await,
        DedupOutcome::Duplicate,
        "…and is decided exactly once"
    );
    assert_eq!(state.seen_copy_event_count().await, 1);

    // Nothing else may be emitted (the failed tx is skipped).
    assert!(
        tokio::time::timeout(Duration::from_millis(400), trades.recv())
            .await
            .is_err(),
        "failed transactions and duplicates must not reach the channel"
    );

    // The subscription named the tracked wallet in accountInclude.
    let frames = received.lock().unwrap().clone();
    let sub = frames
        .iter()
        .find(|f| f["method"] == "transactionSubscribe")
        .expect("a transactionSubscribe frame was sent");
    assert_eq!(
        sub["params"][0]["accountInclude"],
        json!([whale.pubkey().to_string()])
    );
    assert_eq!(sub["params"][1]["encoding"], "base64");
    assert_eq!(sub["params"][1]["transactionDetails"], "full");
}

#[tokio::test]
async fn geyser_feed_falls_back_to_polling_without_a_geyser_url() {
    // No geyser_ws_url configured: spawn must still succeed (poll fallback)
    // and stay quiet — the offline RPC has no signatures to serve.
    let whale = Keypair::new();
    let mut cfg = Config::default();
    cfg.copy.feed = "transaction_subscribe".into();
    cfg.copy.enabled = true;
    cfg.copy.wallets = vec![CopyWallet {
        address: whale.pubkey().to_string(),
        label: None,
        fixed_sol: None,
        fraction_of_their_size: 0.05,
        max_sol: 1.0,
        min_sol: 0.0,
        buys_only: false,
        max_staleness_secs: 300,
        slippage_pct: None,
        paused: false,
        max_exposure_sol: 0.0,
        max_open_positions: 0,
    }];
    cfg.network.geyser_ws_url = None;
    let state = AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let mut trades = CopyFeed::spawn(state, offline_rpc())
        .await
        .expect("feed must fall back to logs_poll instead of failing");
    assert!(
        tokio::time::timeout(Duration::from_millis(600), trades.recv())
            .await
            .is_err(),
        "the offline poll fallback emits nothing"
    );
}
