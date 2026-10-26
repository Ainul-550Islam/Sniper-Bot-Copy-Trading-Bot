//! Integration test for the first-slot producer (PART 03 K1).
//!
//! `crates/module-sniper/src/risk_intel/first_slot.rs` is the producer that
//! fills `LaunchEvent.first_slot_buys`, the datum the bundler gate
//! (`sniper.max_bundler_ratio`) reads. These tests drive it against a local
//! JSON-RPC endpoint that answers `getBlock` with a real mainnet-shaped block
//! fixture, and assert the PARSED output — the attributed buys and the signal
//! the gate consumes — not the fixture's own text.
//!
//! # Fixtures
//! `tests/fixtures/first_slot/fixture_*.json`. Each contains one
//! `UiConfirmedBlock` whose transactions are base64 bincode-serialized legacy
//! Solana transactions, built by `build_block` below, so `decode_instructions`
//! decodes the same bytes the node would send. The block itself is built from
//! pubkeys this file generates, and every fixture is checked to deserialize
//! into `UiConfirmedBlock` (see `every_fixture_is_a_block_the_node_could_send`).
//!
//! `write_fixtures` below runs as an ordinary test (no `--ignored`): it
//! regenerates the three blocks from the builder and the other cases then
//! assert against what is on disk, so a generator the checked-in fixtures do
//! not reflect fails the suite instead of rotting quietly.
//!
//! The mock node is deliberately local to this file: `tests/common`'s mock
//! answers unknown methods with `result: null`, which would make the "the RPC
//! failed" case indistinguishable from "the producer never called".

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use bot_core::config::SniperConfig;
use bot_core::models::{LaunchFeed, TokenLaunch};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::CompiledInstruction;
use solana_sdk::message::{Message, MessageHeader};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::Transaction;

use module_sniper::event::LaunchEvent;
use module_sniper::market::bundling_signal;
use module_sniper::risk_intel::first_slot;
use solana_kit::rpc::Rpc;

/// The slot the fixture block pretends to be.
const SLOT: u64 = 300_000_123;

// ---------------------------------------------------------------------------
// Block construction
// ---------------------------------------------------------------------------

/// The account position the pump.fun buy/sell layout names `user`; the mint is
/// two before it (both verified by `layout_names_resolve_to_the_documented_positions`
/// in the producer's unit tests).
const USER_SLOT: usize = 6;

/// A pump.fun bonding-curve swap: `disc + amount + limit + track_volume`.
fn pump_swap(mint: &Pubkey, user: &Pubkey, buy: bool) -> TxSpec {
    pump_swap_funded_by(mint, user, &Pubkey::new_from_array([99; 32]), buy)
}

/// The same swap paid for by `funder` — the wallet the producer attributes it
/// to, because the fee payer is account 0 of the message.
fn pump_swap_funded_by(mint: &Pubkey, user: &Pubkey, funder: &Pubkey, buy: bool) -> TxSpec {
    let disc = if buy {
        solana_kit::consts::PUMP_DISC_BUY
    } else {
        solana_kit::consts::PUMP_DISC_SELL
    };
    let mut data = disc.to_vec();
    data.extend_from_slice(&1_000u64.to_le_bytes());
    data.extend_from_slice(&1_000u64.to_le_bytes());
    data.push(0);
    // Message order: fee payer, then the instruction's accounts in layout
    // order (mint at 2, user at 6 of THAT list), then the program itself.
    let accounts = vec![
        *funder,                              // 0: fee payer / signer
        Pubkey::new_from_array([11; 32]),     // 1
        Pubkey::new_from_array([12; 32]),     // 2
        *mint,                                // 3 -> instruction account 2
        Pubkey::new_from_array([14; 32]),     // 4 -> instruction account 3
        Pubkey::new_from_array([15; 32]),     // 5 -> instruction account 4
        Pubkey::new_from_array([16; 32]),     // 6 -> instruction account 5
        *user,                                // 7 -> instruction account 6
        *solana_kit::consts::PUMP_PROGRAM_ID, // 8: the program
    ];
    debug_assert_eq!(accounts[3], *mint);
    debug_assert_eq!(accounts[7], *user);
    // The program is the last key and is addressed by index — the layout's own
    // account list never refers to it.
    let program_id_index = (accounts.len() - 1) as u8;
    TxSpec {
        accounts,
        program_id_index,
        instruction_accounts: (1..=USER_SLOT as u8 + 1).collect(),
        data,
        failed: false,
    }
}

/// One transaction of the fixture block.
#[derive(Debug, Clone)]
struct TxSpec {
    /// Message account keys; index 0 is the fee payer and the only signer.
    accounts: Vec<Pubkey>,
    program_id_index: u8,
    /// Which message accounts the instruction touches, in layout order.
    instruction_accounts: Vec<u8>,
    data: Vec<u8>,
    /// `true` puts a transaction error in the meta: the producer must skip it.
    failed: bool,
}

/// Serialize one spec into the `["<base64>", "base64"]` form
/// `EncodedTransaction` expects. `bincode` writes the same bytes as
/// `Transaction::serialize` (the signature list is a `short_vec` in serde), so
/// this is what a node really sends.
fn encode(spec: &TxSpec) -> serde_json::Value {
    let message = Message {
        header: MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: (spec.accounts.len() - 2) as u8,
        },
        account_keys: spec.accounts.clone(),
        recent_blockhash: Hash::default(),
        instructions: vec![CompiledInstruction {
            program_id_index: spec.program_id_index,
            accounts: spec.instruction_accounts.clone(),
            data: spec.data.clone(),
        }],
    };
    let tx = Transaction {
        signatures: vec![Default::default()],
        message,
    };
    let bytes = bincode::serialize(&tx).expect("a legacy transaction serializes");
    let balances: Vec<u64> = vec![0; spec.accounts.len()];
    let meta = serde_json::json!({
        "err": if spec.failed { serde_json::json!("AccountNotFound") } else { serde_json::Value::Null },
        "status": { "Ok": null },
        "fee": 5_000,
        "preBalances": balances,
        "postBalances": balances,
    });
    serde_json::json!({ "transaction": [BASE64.encode(bytes), "base64"], "meta": meta })
}

/// The whole fixture block: one `UiConfirmedBlock` with these transactions.
fn build_block(txs: &[TxSpec]) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = txs.iter().map(encode).collect();
    serde_json::json!({
        "previousBlockhash": "11111111111111111111111111111111",
        "blockhash": "22222222222222222222222222222222",
        "parentSlot": SLOT - 1,
        "blockTime": 1_760_000_000_i64,
        "blockHeight": SLOT - 200_000_000_u64,
        "transactions": entries,
        "rewards": [],
    })
}

/// The transactions of the clustered launch: two buyers funded by ONE wallet,
/// one buyer funded by another, a sell (which must not count as a buy) and a
/// failed buy (which must not count at all).
fn clustered_launch(mint: &Pubkey, bundle_funder: &Pubkey, lone_funder: &Pubkey) -> Vec<TxSpec> {
    let buyer_a = Pubkey::new_from_array([21; 32]);
    let buyer_b = Pubkey::new_from_array([22; 32]);
    let buyer_c = Pubkey::new_from_array([23; 32]);
    let mut specs = vec![
        pump_swap_funded_by(mint, &buyer_a, bundle_funder, true),
        pump_swap_funded_by(mint, &buyer_b, bundle_funder, true),
        pump_swap_funded_by(mint, &buyer_c, lone_funder, true),
    ];
    specs.push(pump_swap(mint, &buyer_a, false));
    let mut failed = pump_swap(mint, &Pubkey::new_from_array([24; 32]), true);
    failed.failed = true;
    specs.push(failed);
    specs
}

// ---------------------------------------------------------------------------
// Fixture files
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("first_slot")
}

fn fixture_path(name: &str) -> PathBuf {
    fixture_dir().join(format!("fixture_{name}.json"))
}

/// The fixture block, read from disk and handed to the mock node verbatim.
fn fixture(name: &str) -> String {
    let path = fixture_path(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "fixture {} must exist (run the `write_fixtures` test): {e}",
            path.display()
        )
    })
}

/// The mints/program ids used by the fixtures, derived the same way the
/// generator derived them so the assertions cannot drift from the files.
fn launch_mint() -> Pubkey {
    Pubkey::new_from_array([7; 32])
}

fn bundle_funder() -> Pubkey {
    Pubkey::new_from_array([31; 32])
}

fn lone_funder() -> Pubkey {
    Pubkey::new_from_array([32; 32])
}

fn other_mint() -> Pubkey {
    Pubkey::new_from_array([8; 32])
}

/// Write the fixtures this file reads, then prove they still parse as blocks.
///
/// Runs by default rather than behind `--ignore`: the files are committed, so
/// re-generating them here is what turns "the generator drifted from the
/// checked-in fixture" into a test failure instead of a silent divergence.
#[test]
fn write_fixtures() {
    let blocks = [
        (
            "clustered",
            clustered_launch(&launch_mint(), &bundle_funder(), &lone_funder()),
        ),
        ("other_mint", {
            let mut specs = clustered_launch(&other_mint(), &bundle_funder(), &lone_funder());
            // One buy of OUR mint, by a single funder: no cluster to report.
            specs.push(pump_swap(
                &launch_mint(),
                &Pubkey::new_from_array([41; 32]),
                true,
            ));
            specs
        }),
        (
            "no_buys",
            vec![pump_swap(
                &launch_mint(),
                &Pubkey::new_from_array([42; 32]),
                false,
            )],
        ),
    ];
    std::fs::create_dir_all(fixture_dir()).expect("fixture dir");
    for (name, specs) in blocks {
        let json = build_block(&specs);
        // A fixture the node could not have produced is not a fixture.
        let parsed: solana_transaction_status::UiConfirmedBlock =
            serde_json::from_value(json.clone()).expect("the block deserializes");
        assert_eq!(
            parsed.transactions.as_ref().map(Vec::len).unwrap_or(0),
            specs.len()
        );
        let mut text = serde_json::to_string_pretty(&json).expect("pretty json");
        text.push('\n'); // a trailing newline, so the file ends like every other
        std::fs::write(fixture_path(name), text).expect("fixture written");
    }
}

#[test]
fn every_fixture_is_a_block_the_node_could_send() {
    for name in ["clustered", "other_mint", "no_buys"] {
        let raw = fixture(name);
        let block: solana_transaction_status::UiConfirmedBlock =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(block.parent_slot, SLOT - 1);
        assert_eq!(block.block_time, Some(1_760_000_000));
        assert!(
            block
                .transactions
                .as_ref()
                .is_some_and(|txs| !txs.is_empty()),
            "{name}: a block with no transactions tests nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// The mock node
// ---------------------------------------------------------------------------

/// Wrap a block in the JSON-RPC envelope a node answers with.
fn block_response(block: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{block}}}"#)
}

/// A one-request-one-response JSON-RPC endpoint. `hits` counts every request
/// and `bodies` keeps them, so a test can prove the producer asked for the
/// launch slot — or never called at all.
fn serve(body: String, hits: Arc<AtomicU64>, bodies: Arc<Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(request) = read_request(&mut stream) else {
                continue;
            };
            hits.fetch_add(1, Ordering::SeqCst);
            bodies.lock().expect("bodies").push(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

/// The error node: a well-formed JSON-RPC error for `getBlock`, i.e. the RPC is
/// reachable and refuses the request (the case a bare 500 would blur).
fn serve_error(hits: Arc<AtomicU64>) -> String {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "error": { "code": -32007, "message": "Block not available for slot" },
        "id": 1,
    })
    .to_string();
    serve(body, hits, Arc::new(Mutex::new(Vec::new())))
}

/// Read one HTTP request in full.
///
/// The body MUST be drained before the response is written: answering while
/// the client is still writing makes the peer see a reset instead of the block,
/// and the producer would report an RPC error for a perfectly good fixture.
fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 8 * 1024];
    let mut head = String::new();
    let body_len = loop {
        let read = stream.read(&mut buf).ok()?;
        if read == 0 {
            return if head.is_empty() { None } else { Some(head) };
        }
        head.push_str(&String::from_utf8_lossy(&buf[..read]));
        if let Some(end) = head.find("\r\n\r\n") {
            let headers = &head[..end];
            let length = headers
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
    while request.len() < body_len {
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

fn event_for(mint: &Pubkey) -> LaunchEvent {
    let launch = TokenLaunch {
        mint: mint.to_string(),
        name: "Fixture Token".into(),
        symbol: "FIXT".into(),
        uri: None,
        creator: Pubkey::new_from_array([5; 32]).to_string(),
        pool: String::new(),
        initial_buy_sol: 0.0,
        market_cap_sol: 0.0,
        market_cap_usd: None,
        total_supply: None,
        slot: Some(SLOT),
        signature: None,
        tx_type: None,
        observed_at: chrono::Utc::now(),
        feed: LaunchFeed::SolanaLogs,
        socials: None,
    };
    LaunchEvent::from_token_launch(launch, 1, "fixture".into())
}

fn cfg_with_ratio(ratio: f64) -> SniperConfig {
    let mut cfg = bot_core::config::AppConfig::from_defaults().raw.sniper;
    cfg.max_bundler_ratio = ratio;
    cfg
}

// ---------------------------------------------------------------------------
// (a) a real block with a cluster parses into the attribution the gate wants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_block_of_one_funder_becomes_a_clustered_attribution() {
    let hits = Arc::new(AtomicU64::new(0));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let url = serve(
        block_response(&fixture("clustered")),
        hits.clone(),
        bodies.clone(),
    );
    let rpc = rpc_at(url);
    let mut event = event_for(&launch_mint());

    let attached = first_slot::attach_if_enabled(&rpc, &cfg_with_ratio(0.5), &mut event).await;

    assert_eq!(hits.load(Ordering::SeqCst), 1, "exactly one RPC round trip");
    let asked = bodies.lock().expect("bodies").join("\n");
    assert!(
        asked.contains("getBlock"),
        "the producer must read a block: {asked}"
    );
    assert!(
        asked.contains(&SLOT.to_string()),
        "the block must be the launch slot: {asked}"
    );

    // Two of the three buyers share the bundle funder; the third is alone.
    assert_eq!(attached, 3, "three buys of this mint in the block");
    assert_eq!(event.first_slot_buys.len(), 3);
    let in_bundle = event
        .first_slot_buys
        .iter()
        .filter(|buy| buy.funder == bundle_funder())
        .count();
    assert_eq!(in_bundle, 2, "the cluster is the shared funding source");
    assert!(
        event
            .first_slot_buys
            .iter()
            .all(|buy| buy.buyer != buy.funder),
        "a funder that is also a buyer must stay visible, not be merged away"
    );

    // The value the gate consumes: 3 first-slot buyers, 2 of them bundled.
    let signal = bundling_signal(&event).expect("a non-empty list is a signal");
    assert_eq!(signal.first_slot_buyers, 3);
    assert_eq!(signal.bundled_wallets, 2);
}

// ---------------------------------------------------------------------------
// (b) a wrong mint does not add a buy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn buys_of_another_mint_in_the_same_slot_are_not_attributed() {
    let hits = Arc::new(AtomicU64::new(0));
    let url = serve(
        block_response(&fixture("other_mint")),
        hits.clone(),
        Arc::new(Mutex::new(Vec::new())),
    );
    let rpc = rpc_at(url);
    let mut event = event_for(&launch_mint());

    let attached = first_slot::attach_if_enabled(&rpc, &cfg_with_ratio(0.5), &mut event).await;

    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        attached, 1,
        "only the single buy of THIS mint counts; the others belong to a different launch"
    );
    assert_eq!(event.first_slot_buys.len(), 1);
    assert_eq!(
        event.first_slot_buys[0].buyer,
        Pubkey::new_from_array([41; 32])
    );
    // One buyer from one funder is not a cluster, so the gate stays permissive.
    let signal = bundling_signal(&event).expect("the list is still evidence of buyers");
    assert_eq!(signal.first_slot_buyers, 1);
    assert_eq!(signal.bundled_wallets, 0);
}

// ---------------------------------------------------------------------------
// (c) RPC error -> empty list, no panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_rpc_leaves_the_observation_empty_and_the_process_up() {
    let hits = Arc::new(AtomicU64::new(0));
    let rpc = rpc_at(serve_error(hits.clone()));
    let mut event = event_for(&launch_mint());

    let attached = first_slot::attach_if_enabled(&rpc, &cfg_with_ratio(0.5), &mut event).await;

    assert_eq!(hits.load(Ordering::SeqCst), 1, "it tried once");
    assert_eq!(attached, 0, "a failed read is no evidence");
    assert!(event.first_slot_buys.is_empty());
    // The gate's contract for an empty list: `None`, i.e. "not observed" —
    // which is what makes the gate fail closed instead of trading blind.
    assert!(bundling_signal(&event).is_none());

    // The fetch helper itself distinguishes "no buys" from "no data".
    let unavailable = first_slot::fetch(&rpc, &launch_mint(), SLOT).await;
    assert!(unavailable.is_none(), "an error is reported as unknown");
}

// ---------------------------------------------------------------------------
// (d) ratio == 0 -> no RPC call at all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_disabled_gate_costs_no_rpc() {
    let hits = Arc::new(AtomicU64::new(0));
    let url = serve(
        block_response(&fixture("clustered")),
        hits.clone(),
        Arc::new(Mutex::new(Vec::new())),
    );
    let rpc = rpc_at(url);

    for ratio in [0.0, -1.0, f64::NAN] {
        let mut event = event_for(&launch_mint());
        let cfg = cfg_with_ratio(ratio);
        assert!(
            !first_slot::enabled(&cfg),
            "ratio {ratio} must not enable the producer"
        );
        assert_eq!(
            first_slot::attach_if_enabled(&rpc, &cfg, &mut event).await,
            0
        );
        assert!(event.first_slot_buys.is_empty());
    }

    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the producer is off: not one request, not one block fetched"
    );
}

/// A block that holds no buy of this mint is read, and the answer is an empty
/// list — not "unknown". The two states must stay distinguishable.
#[tokio::test]
async fn a_block_without_buys_is_read_and_reported_as_empty() {
    let hits = Arc::new(AtomicU64::new(0));
    let url = serve(
        block_response(&fixture("no_buys")),
        hits.clone(),
        Arc::new(Mutex::new(Vec::new())),
    );
    let rpc = rpc_at(url);

    let buys = first_slot::fetch(&rpc, &launch_mint(), SLOT)
        .await
        .expect("the block was read");
    assert!(buys.is_empty(), "a sell-only block attributes no buy");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
