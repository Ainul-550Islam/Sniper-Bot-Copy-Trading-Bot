//! Integration test for the dev-sell producer (PART 03 K2).
//!
//! `crates/module-sniper/src/risk_intel/dev_sell.rs` turns a creator-wallet
//! `logsSubscribe` notification into a `DevSellObservation` — but only after
//! the transaction has been FETCHED and verified. These tests drive that
//! verification against a local JSON-RPC endpoint that answers
//! `getTransaction` with a real, base64 bincode-serialized legacy transaction,
//! and assert the parsed observations.
//!
//! # What each case proves
//! * a creator-signed sell of a watched mint → exactly one observation, with
//!   the mint, the seller and the feed timestamp the node reported;
//! * the same sell signed by somebody else → NO observation (one sender: the
//!   creator wallet, never "whoever the node happened to mention");
//! * a sell whose venue `user` is not the creator → NO observation;
//! * a failed transaction → NO observation;
//! * an unreachable/refusing node → NO observation, no panic.
//!
//! The mock is local to this file on purpose: `tests/common`'s mock answers an
//! unknown method with `result: null`, which would make "the RPC refused" and
//! "the producer never called" look identical — the exact pair this module has
//! to keep apart. The transaction encoder is duplicated from
//! `first_slot_producer.rs` rather than shared because the two producers ask
//! for different RPC methods and one helper module used by two mock shapes
//! would hide which one a case exercises.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use bot_core::config::AdvancedExitConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::CompiledInstruction;
use solana_sdk::message::{Message, MessageHeader};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::Transaction;

use module_sniper::risk_intel::dev_sell;
use solana_kit::rpc::Rpc;

/// The slot the fixture transaction claims to live in.
const SLOT: u64 = 300_000_456;
/// The block time the fixture reports, in seconds.
const BLOCK_TIME: i64 = 1_760_000_000;
/// The account position the pump.fun sell layout names `user`; the base mint is
/// four before it (`mint` at 2), which the producer looks up by name.
const USER_SLOT: usize = 6;

/// A pump.fun sell of `mint` by `user`, paid for and signed by `signer`.
///
/// The three roles are separate on purpose: the producer must attribute a sell
/// to the creator only when the creator is BOTH the signer and the venue's
/// trader.
fn creator_sell(mint: &Pubkey, user: &Pubkey, signer: &Pubkey) -> Vec<u8> {
    let mut data = solana_kit::consts::PUMP_DISC_SELL.to_vec();
    data.extend_from_slice(&500u64.to_le_bytes());
    data.extend_from_slice(&500u64.to_le_bytes());
    data.push(0);
    encode_legacy(mint, user, signer, &data)
}

/// Serialize a legacy transaction that calls the pump.fun program with an
/// account list laid out exactly as `default_sell_layout` expects: the mint at
/// instruction account 2 and the trader at instruction account 6.
///
/// `bincode` writes the same bytes as `Transaction::serialize` (the signature
/// list is a `short_vec` under serde), so this is what a node really sends.
fn encode_legacy(mint: &Pubkey, user: &Pubkey, signer: &Pubkey, data: &[u8]) -> Vec<u8> {
    let account_keys = vec![
        *signer,                              // 0: fee payer, the only required signature
        Pubkey::new_from_array([11; 32]),     // 1 -> instruction account 0
        Pubkey::new_from_array([12; 32]),     // 2 -> instruction account 1
        *mint,                                // 3 -> instruction account 2 (`mint`)
        Pubkey::new_from_array([14; 32]),     // 4 -> instruction account 3
        Pubkey::new_from_array([15; 32]),     // 5 -> instruction account 4
        Pubkey::new_from_array([16; 32]),     // 6 -> instruction account 5
        *user,                                // 7 -> instruction account 6 (`user`)
        *solana_kit::consts::PUMP_PROGRAM_ID, // 8: the program itself
    ];
    debug_assert_eq!(account_keys[3], *mint);
    debug_assert_eq!(account_keys[7], *user);
    let message = Message {
        header: MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: (account_keys.len() - 2) as u8,
        },
        recent_blockhash: Hash::default(),
        account_keys: account_keys.clone(),
        instructions: vec![CompiledInstruction {
            program_id_index: (account_keys.len() - 1) as u8,
            accounts: (1..=USER_SLOT as u8 + 1).collect(),
            data: data.to_vec(),
        }],
    };
    let tx = Transaction {
        signatures: vec![Default::default()],
        message,
    };
    bincode::serialize(&tx).expect("a legacy transaction serializes")
}

/// The `getTransaction` response for one transaction, as a node sends it.
fn confirmed_response(bytes: &[u8], meta_err: Option<&str>, block_time: Option<i64>) -> String {
    let balances: Vec<u64> = vec![0; 8];
    let meta = serde_json::json!({
        "err": meta_err.map(serde_json::Value::from).unwrap_or(serde_json::Value::Null),
        "status": if meta_err.is_some() { serde_json::json!({"Err": "AccountNotFound"}) } else { serde_json::json!({"Ok": null}) },
        "fee": 5_000,
        "preBalances": balances,
        "postBalances": balances,
    });
    // `EncodedConfirmedTransactionWithStatusMeta` FLATTENS its
    // `EncodedTransactionWithStatusMeta`, so `transaction` and `meta` are
    // siblings of `slot`/`blockTime` here — exactly as a node sends them.
    let result = serde_json::json!({
        "slot": SLOT,
        "blockTime": block_time,
        "transaction": [BASE64.encode(bytes), "base64"],
        "meta": meta,
    });
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"result":{}}}"#,
        serde_json::to_string(&result).expect("serializable")
    )
}

/// A one-request-one-response JSON-RPC endpoint serving one canned body.
fn serve(body: String, hits: Arc<AtomicU64>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    let addr = listener.local_addr().expect("addr");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let served = body;
    let seen = bodies.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(request) = read_request(&mut stream) else {
                continue;
            };
            hits.fetch_add(1, Ordering::SeqCst);
            seen.lock().expect("bodies").push(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                served.len(),
                served
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), bodies)
}

/// Read one HTTP request in full, so the client never sees a reset while it is
/// still writing its body.
fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 8 * 1024];
    let mut head = String::new();
    let body_end = loop {
        let read = stream.read(&mut buf).ok()?;
        if read == 0 {
            return if head.is_empty() { None } else { Some(head) };
        }
        head.push_str(&String::from_utf8_lossy(&buf[..read]));
        if let Some(end) = head.find("\r\n\r\n") {
            let length = head[..end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if !name.trim().eq_ignore_ascii_case("content-length") {
                        return None;
                    }
                    value.trim().parse::<usize>().ok()
                })
                .unwrap_or(0);
            break end + length;
        }
    };
    let mut request = head.clone();
    while request.len() < body_end {
        let read = stream.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        request.push_str(&String::from_utf8_lossy(&buf[..read]));
    }
    Some(request)
}

fn rpc_at(url: String) -> Rpc {
    Rpc::with_urls(
        url,
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        0,
        Duration::from_secs(5),
    )
    .expect("rpc")
}

// ---------------------------------------------------------------------------
// the positive case, and the negatives that make it mean something
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_signed_sell_by_the_creator_is_verified_from_the_transaction() {
    let mint = Pubkey::new_from_array([7; 32]);
    let creator = Pubkey::new_from_array([21; 32]);
    let bytes = creator_sell(&mint, &creator, &creator);
    let hits = Arc::new(AtomicU64::new(0));
    let (url, bodies) = serve(
        confirmed_response(&bytes, None, Some(BLOCK_TIME)),
        hits.clone(),
    );
    let rpc = rpc_at(url);
    let signature = Signature::from([3u8; 64]);

    let observations = dev_sell::verified_creator_sells(&rpc, &creator, &signature).await;

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "verified from the tx itself"
    );
    let asked = bodies.lock().expect("bodies").join("\n");
    assert!(
        asked.contains("getTransaction"),
        "the producer must read back the transaction: {asked}"
    );
    let [only] = observations.as_slice() else {
        panic!("exactly one observation, got {observations:?}");
    };
    assert_eq!(only.mint, mint);
    assert_eq!(only.seller, creator);
    assert_eq!(
        only.observed_at,
        chrono::DateTime::from_timestamp(BLOCK_TIME, 0).expect("a real timestamp"),
        "the observation carries the feed timestamp the node reported"
    );
}

#[tokio::test]
async fn the_feed_timestamp_is_the_read_time_when_the_node_has_no_block_time() {
    // An empty `blockTime` is a gap, not a licence to invent a date: the
    // producer falls back to when it read the transaction, which is what
    // `dev_sell_ttl_secs` then ages against.
    let mint = Pubkey::new_from_array([7; 32]);
    let creator = Pubkey::new_from_array([21; 32]);
    let bytes = creator_sell(&mint, &creator, &creator);
    let hits = Arc::new(AtomicU64::new(0));
    let (url, _bodies) = serve(confirmed_response(&bytes, None, None), hits.clone());
    let rpc = rpc_at(url);

    let observations =
        dev_sell::verified_creator_sells(&rpc, &creator, &Signature::from([3u8; 64])).await;

    assert_eq!(observations.len(), 1);
    let age = chrono::Utc::now() - observations[0].observed_at;
    assert!(
        age.num_seconds() < 60 && age.num_seconds() > -5,
        "the fallback is this process's clock, not a fabricated chain time: {age}"
    );
}

#[tokio::test]
async fn a_sell_signed_by_another_wallet_is_not_attributed_to_the_creator() {
    // The single-sender rule: the creator must be a required SIGNER of the
    // transaction that sold. A stranger's sell that happens to mention the
    // creator in its account list proves nothing.
    let mint = Pubkey::new_from_array([7; 32]);
    let creator = Pubkey::new_from_array([21; 32]);
    let stranger = Pubkey::new_from_array([22; 32]);
    // The creator sells, but the STRANGER pays for and signs the transaction.
    let bytes = creator_sell(&mint, &creator, &stranger);
    let hits = Arc::new(AtomicU64::new(0));
    let (url, _bodies) = serve(
        confirmed_response(&bytes, None, Some(BLOCK_TIME)),
        hits.clone(),
    );
    let rpc = rpc_at(url);

    let observations =
        dev_sell::verified_creator_sells(&rpc, &creator, &Signature::from([3u8; 64])).await;
    assert!(
        observations.is_empty(),
        "a creator who did not sign cannot be credited with the sell: {observations:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "it looked, and refused to conclude"
    );

    // Positive control, so the assertion above cannot pass by being vacuous:
    // the same shape, with the stranger as signer AND trader, IS verified.
    let stranger_sold = creator_sell(&mint, &stranger, &stranger);
    let control_hits = Arc::new(AtomicU64::new(0));
    let (control_url, _bodies) = serve(
        confirmed_response(&stranger_sold, None, Some(BLOCK_TIME)),
        control_hits.clone(),
    );
    let control = rpc_at(control_url);
    let attributed =
        dev_sell::verified_creator_sells(&control, &stranger, &Signature::from([3u8; 64])).await;
    assert_eq!(
        attributed.len(),
        1,
        "a creator's own verified sell is reported"
    );
    assert_eq!(attributed[0].seller, stranger);
    assert_eq!(attributed[0].mint, mint);
    assert_eq!(
        dev_sell::verified_creator_sells(&control, &creator, &Signature::from([3u8; 64]))
            .await
            .len(),
        0,
        "and never to a wallet that only shares the block"
    );
}

#[tokio::test]
async fn a_sell_the_creator_did_not_make_is_not_attributed_to_the_creator() {
    // The creator signs the transaction (it is their transaction) but the
    // venue's `user` account — the wallet the swap is attributed to — is
    // somebody else. Nothing this creator sold, so no signal.
    let mint = Pubkey::new_from_array([7; 32]);
    let creator = Pubkey::new_from_array([21; 32]);
    let other = Pubkey::new_from_array([23; 32]);
    let bytes = creator_sell(&mint, &other, &creator);
    let hits = Arc::new(AtomicU64::new(0));
    let (url, _bodies) = serve(
        confirmed_response(&bytes, None, Some(BLOCK_TIME)),
        hits.clone(),
    );
    let rpc = rpc_at(url);

    let observations =
        dev_sell::verified_creator_sells(&rpc, &creator, &Signature::from([3u8; 64])).await;
    assert!(
        observations.is_empty(),
        "the sell belongs to {other}, not to the creator"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_failed_transaction_never_produces_a_dev_sell() {
    let mint = Pubkey::new_from_array([7; 32]);
    let creator = Pubkey::new_from_array([21; 32]);
    let bytes = creator_sell(&mint, &creator, &creator);
    let hits = Arc::new(AtomicU64::new(0));
    let (url, _bodies) = serve(
        confirmed_response(&bytes, Some("AccountNotFound"), Some(BLOCK_TIME)),
        hits.clone(),
    );
    let rpc = rpc_at(url);

    let observations =
        dev_sell::verified_creator_sells(&rpc, &creator, &Signature::from([3u8; 64])).await;
    assert!(
        observations.is_empty(),
        "a transaction that failed sold nothing"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_unreachable_node_produces_no_observation() {
    // Port 1 is closed: the read fails, and the module must report "unknown"
    // by returning nothing — never a signal, never a panic, never a hang
    // beyond its own timeout.
    let rpc = Rpc::with_urls(
        "http://127.0.0.1:1".to_string(),
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        0,
        Duration::from_millis(200),
    )
    .expect("rpc");
    let started = std::time::Instant::now();
    let observations = dev_sell::verified_creator_sells(
        &rpc,
        &Pubkey::new_from_array([21; 32]),
        &Signature::from([3u8; 64]),
    )
    .await;
    assert!(observations.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "verification is bounded by the client timeout"
    );
}

// ---------------------------------------------------------------------------
// the switch: off means the watcher is never built
// ---------------------------------------------------------------------------

#[test]
fn a_deployment_without_the_advanced_exit_block_has_no_dev_sell_producer() {
    // `spawn` reads the same field the exit policy engine is built from, so
    // there is no configuration in which the watcher runs with no consumer.
    let defaults = bot_core::config::AppConfig::from_defaults();
    assert!(defaults.raw.sniper.advanced_exit.is_none());
    assert!(!dev_sell::enabled(&defaults.raw));
    assert_eq!(dev_sell::ttl_secs(&defaults.raw), 0);

    let with_ttl = |ttl: u64| {
        let mut app = bot_core::config::AppConfig::from_defaults();
        app.raw.sniper.advanced_exit = Some(AdvancedExitConfig {
            ladder: Vec::new(),
            break_even: None,
            dev_sell_ttl_secs: ttl,
        });
        app.raw
    };
    assert!(!dev_sell::enabled(&with_ttl(0)), "an explicit 0 is off");
    assert!(dev_sell::enabled(&with_ttl(30)));
}
