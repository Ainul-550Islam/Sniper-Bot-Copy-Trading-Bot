//! Shared harness for the sniper integration tests.
//!
//! * a scripted JSON-RPC **mock node** (accounts, blockhashes, simulation,
//!   send behaviour, confirmation) so the REAL `Sniper` — pipeline, risk
//!   engine, hardened executor, ledger — runs end to end offline;
//! * byte-accurate **pump.fun account encoders** (Global, BondingCurve, SPL
//!   Mint) built from the same offsets the parsers use;
//! * **event builders** for well-formed launch events.
//!
//! Everything here is test-only code; nothing in `src/` depends on it.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use bot_core::config::{AppConfig, Config};
use bot_core::models::{ExecutionMode, LaunchFeed, TokenLaunch};
use bot_core::state::{AppState, Shared};
use solana_kit::consts::*;
use solana_kit::rpc::Rpc;
use solana_kit::tokens::{Wallet, SPL_MINT_LEN};

use module_sniper::event::LaunchEvent;
use module_sniper::Sniper;

pub const SOL: u64 = 1_000_000_000;

// ---------------------------------------------------------------------------
// Mock JSON-RPC node
// ---------------------------------------------------------------------------

/// How the mock answers `sendTransaction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendBehaviour {
    /// Accept and return the signature.
    Accept,
    /// Definite JSON-RPC rejection (the node answered "no").
    Reject,
    /// Close the socket without answering (transport ambiguity).
    Drop,
}

#[derive(Debug, Clone)]
pub struct MockAccount {
    pub owner: Pubkey,
    pub lamports: u64,
    pub data: Vec<u8>,
}

/// Scripted node state. Every knob is atomic/mutex so a test can flip it
/// while the sniper runs.
pub struct MockNode {
    accounts: Mutex<HashMap<String, MockAccount>>,
    /// Lamport balances for `getBalance` (defaults to 10 SOL for any key).
    balances: Mutex<HashMap<String, u64>>,
    pub default_balance: AtomicU64,
    blockhashes: Mutex<std::collections::VecDeque<(Hash, u64)>>,
    pub block_height: AtomicU64,
    pub slot: AtomicU64,
    simulate_error: Mutex<Option<String>>,
    send: Mutex<SendBehaviour>,
    /// `getTransaction` reports the last sent transaction as landed.
    pub confirm: AtomicBool,
    /// Report the landed transaction as failed on chain (`meta.err`).
    pub landed_failed: AtomicBool,
    /// Extra latency added to every response.
    pub latency_ms: AtomicU64,
    /// When set, every request is answered with a JSON-RPC error.
    pub fail_all: AtomicBool,
    pub sends: AtomicUsize,
    pub simulations: AtomicUsize,
    pub requests: AtomicUsize,
    methods: Mutex<Vec<String>>,
    last_sent_b64: Mutex<Option<String>>,
    last_sent_sig: Mutex<Option<String>>,
}

impl Default for MockNode {
    fn default() -> Self {
        MockNode {
            accounts: Mutex::new(HashMap::new()),
            balances: Mutex::new(HashMap::new()),
            default_balance: AtomicU64::new(10 * SOL),
            blockhashes: Mutex::new(std::collections::VecDeque::new()),
            block_height: AtomicU64::new(1_000),
            slot: AtomicU64::new(500),
            simulate_error: Mutex::new(None),
            send: Mutex::new(SendBehaviour::Accept),
            confirm: AtomicBool::new(true),
            landed_failed: AtomicBool::new(false),
            latency_ms: AtomicU64::new(0),
            fail_all: AtomicBool::new(false),
            sends: AtomicUsize::new(0),
            simulations: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            methods: Mutex::new(Vec::new()),
            last_sent_b64: Mutex::new(None),
            last_sent_sig: Mutex::new(None),
        }
    }
}

impl MockNode {
    pub fn set_account(&self, key: Pubkey, owner: Pubkey, data: Vec<u8>) {
        self.accounts.lock().unwrap().insert(
            key.to_string(),
            MockAccount {
                owner,
                lamports: 2_039_280,
                data,
            },
        );
    }

    pub fn remove_account(&self, key: &Pubkey) {
        self.accounts.lock().unwrap().remove(&key.to_string());
    }

    pub fn set_balance(&self, key: Pubkey, lamports: u64) {
        self.balances
            .lock()
            .unwrap()
            .insert(key.to_string(), lamports);
    }

    pub fn set_simulate_error(&self, err: Option<&str>) {
        *self.simulate_error.lock().unwrap() = err.map(|s| s.to_string());
    }

    pub fn set_send(&self, behaviour: SendBehaviour) {
        *self.send.lock().unwrap() = behaviour;
    }

    pub fn send_behaviour(&self) -> SendBehaviour {
        *self.send.lock().unwrap()
    }

    /// Queue a blockhash/last-valid-block-height pair (front to back; the
    /// last one repeats forever).
    pub fn push_blockhash(&self, hash: Hash, last_valid_block_height: u64) {
        self.blockhashes
            .lock()
            .unwrap()
            .push_back((hash, last_valid_block_height));
    }

    pub fn methods(&self) -> Vec<String> {
        self.methods.lock().unwrap().clone()
    }

    pub fn last_sent_signature(&self) -> Option<String> {
        self.last_sent_sig.lock().unwrap().clone()
    }

    fn account_json(&self, key: &str) -> Value {
        match self.accounts.lock().unwrap().get(key) {
            Some(acc) => json!({
                "data": [base64::engine::general_purpose::STANDARD.encode(&acc.data), "base64"],
                "executable": false,
                "lamports": acc.lamports,
                "owner": acc.owner.to_string(),
                "rentEpoch": 0,
                "space": acc.data.len(),
            }),
            None => Value::Null,
        }
    }

    fn ctx(&self) -> Value {
        json!({"slot": self.slot.load(Ordering::SeqCst)})
    }

    /// Answer one request. `None` = drop the connection.
    fn respond(&self, req: &Value) -> Option<Value> {
        let method = req["method"].as_str().unwrap_or_default().to_string();
        let id = req["id"].clone();
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.methods.lock().unwrap().push(method.clone());
        if self.fail_all.load(Ordering::SeqCst) {
            return Some(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32005, "message": "node is unhealthy (mock fail_all)"}}),
            );
        }
        let result: Value = match method.as_str() {
            "getHealth" => json!("ok"),
            "getVersion" => json!({"solana-core": "2.3.13", "feature-set": 1}),
            "getSlot" => json!(self.slot.load(Ordering::SeqCst)),
            "getBlockHeight" => json!(self.block_height.load(Ordering::SeqCst)),
            "getBalance" => {
                let key = req["params"][0].as_str().unwrap_or_default();
                let v = self
                    .balances
                    .lock()
                    .unwrap()
                    .get(key)
                    .copied()
                    .unwrap_or(self.default_balance.load(Ordering::SeqCst));
                json!({"context": self.ctx(), "value": v})
            }
            "getAccountInfo" => {
                let key = req["params"][0].as_str().unwrap_or_default();
                json!({"context": self.ctx(), "value": self.account_json(key)})
            }
            "getMultipleAccounts" => {
                let keys: Vec<String> = req["params"][0]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|k| k.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let values: Vec<Value> = keys.iter().map(|k| self.account_json(k)).collect();
                json!({"context": self.ctx(), "value": values})
            }
            "getLatestBlockhash" => {
                let mut q = self.blockhashes.lock().unwrap();
                let (hash, lvbh) = if q.len() > 1 {
                    q.pop_front().unwrap()
                } else {
                    q.front()
                        .copied()
                        .unwrap_or((Hash::new_unique(), u64::MAX / 2))
                };
                json!({"context": self.ctx(), "value": {"blockhash": hash.to_string(), "lastValidBlockHeight": lvbh}})
            }
            "isBlockhashValid" => json!({"context": self.ctx(), "value": true}),
            "getRecentPrioritizationFees" => json!([]),
            "simulateTransaction" => {
                self.simulations.fetch_add(1, Ordering::SeqCst);
                match self.simulate_error.lock().unwrap().clone() {
                    Some(err) => json!({"context": self.ctx(), "value": {
                        "err": {"InstructionError": [0, {"Custom": 6001}]},
                        "logs": ["Program log: Instruction: Buy", format!("Program log: {err}")],
                        "unitsConsumed": 1200
                    }}),
                    None => {
                        json!({"context": self.ctx(), "value": {"err": null, "logs": ["Program log: ok"], "unitsConsumed": 1200}})
                    }
                }
            }
            "sendTransaction" => {
                self.sends.fetch_add(1, Ordering::SeqCst);
                let b64 = req["params"][0].as_str().unwrap_or_default().to_string();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .unwrap_or_default();
                // Wire format: compact-u16 signature count (1 byte for one
                // signer) followed by the 64-byte signature.
                let sig = bytes
                    .get(1..65)
                    .map(bs58::encode)
                    .map(|e| e.into_string())
                    .unwrap_or_default();
                *self.last_sent_b64.lock().unwrap() = Some(b64);
                *self.last_sent_sig.lock().unwrap() = Some(sig.clone());
                match self.send_behaviour() {
                    SendBehaviour::Accept => json!(sig),
                    SendBehaviour::Reject => {
                        return Some(
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32002, "message": "Transaction simulation failed: Blockhash not found"}}),
                        );
                    }
                    SendBehaviour::Drop => return None,
                }
            }
            "getTransaction" => {
                if self.confirm.load(Ordering::SeqCst) {
                    match self.last_sent_b64.lock().unwrap().clone() {
                        Some(b64) => {
                            let failed = self.landed_failed.load(Ordering::SeqCst);
                            json!({
                                "slot": self.slot.load(Ordering::SeqCst),
                                "blockTime": null,
                                "transaction": [b64, "base64"],
                                "meta": {
                                    "err": if failed { json!({"InstructionError": [0, {"Custom": 1}]}) } else { Value::Null },
                                    "status": if failed { json!({"Err": {"InstructionError": [0, {"Custom": 1}]}}) } else { json!({"Ok": null}) },
                                    "fee": 5000,
                                    "preBalances": [], "postBalances": [],
                                    "innerInstructions": [], "logMessages": ["Program log: landed"],
                                    "preTokenBalances": [], "postTokenBalances": [], "rewards": []
                                }
                            })
                        }
                        None => Value::Null,
                    }
                } else {
                    Value::Null
                }
            }
            "getSignatureStatuses" => json!({"context": self.ctx(), "value": [null]}),
            _ => Value::Null,
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }
}

/// Serve `node` on a random loopback port; returns the HTTP URL.
pub async fn spawn_mock_node(node: Arc<MockNode>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let node = node.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1 << 16];
                let mut n = 0usize;
                loop {
                    let Ok(r) = sock.read(&mut buf[n..]).await else {
                        return;
                    };
                    if r == 0 {
                        break;
                    }
                    n += r;
                    if let Some(body) = complete_http_body(&buf[..n]) {
                        let req: Value = match serde_json::from_slice(body) {
                            Ok(v) => v,
                            Err(_) => return,
                        };
                        let latency = node.latency_ms.load(Ordering::SeqCst);
                        if latency > 0 {
                            tokio::time::sleep(Duration::from_millis(latency)).await;
                        }
                        let Some(resp) = node.respond(&req) else {
                            let _ = sock.shutdown().await;
                            return;
                        };
                        let body = resp.to_string();
                        let out = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = sock.write_all(out.as_bytes()).await;
                        let _ = sock.shutdown().await;
                        return;
                    }
                    if n == buf.len() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// The JSON body once the whole HTTP request has arrived.
fn complete_http_body(raw: &[u8]) -> Option<&[u8]> {
    let text = std::str::from_utf8(raw).ok()?;
    let split = text.find("\r\n\r\n")?;
    let headers = &text[..split];
    let body_start = split + 4;
    let len = headers
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if raw.len() >= body_start + len {
        Some(&raw[body_start..body_start + len])
    } else {
        None
    }
}

/// A TCP endpoint that accepts and immediately drops connections — the
/// transport-level ambiguity every failure-injection suite needs.
pub async fn spawn_drop_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let _ = listener.accept().await;
        }
    });
    addr
}

/// An `Rpc` pointed at `url` with no fallbacks and a short timeout.
pub fn mock_rpc(url: &str) -> Rpc {
    Rpc::with_urls(
        url.to_string(),
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        1,
        Duration::from_secs(2),
    )
    .expect("rpc builds")
}

/// An `Rpc` with a primary and one fallback provider.
pub fn mock_rpc_with_fallback(primary: &str, fallback: &str) -> Rpc {
    Rpc::with_urls(
        primary.to_string(),
        String::new(),
        vec![fallback.to_string()],
        CommitmentConfig::confirmed(),
        1,
        Duration::from_secs(2),
    )
    .expect("rpc builds")
}

/// An `Rpc` whose endpoint is unreachable (nothing listens on the port).
pub fn offline_rpc() -> Rpc {
    Rpc::with_urls(
        "http://127.0.0.1:9".to_string(),
        "ws://127.0.0.1:9".to_string(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        0,
        Duration::from_millis(300),
    )
    .expect("rpc builds")
}

// ---------------------------------------------------------------------------
// Account encoders
// ---------------------------------------------------------------------------

fn put_u64(d: &mut [u8], off: usize, v: u64) {
    d[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn put_pk(d: &mut [u8], off: usize, v: &Pubkey) {
    d[off..off + 32].copy_from_slice(&v.to_bytes());
}

/// The pump.fun `Global` account with canonical defaults and a 1% fee.
pub fn pump_global_bytes(fee_recipient: Pubkey) -> Vec<u8> {
    let mut d = vec![0u8; GLOBAL_OFF_IS_HOLDER_REWARD_ENABLED + 1];
    d[..8].copy_from_slice(&PUMP_ACC_DISC_GLOBAL);
    d[GLOBAL_OFF_INITIALIZED] = 1;
    put_pk(&mut d, GLOBAL_OFF_AUTHORITY, &Pubkey::new_unique());
    put_pk(&mut d, GLOBAL_OFF_FEE_RECIPIENT, &fee_recipient);
    put_u64(
        &mut d,
        GLOBAL_OFF_INITIAL_VIRTUAL_TOKEN_RESERVES,
        bot_core::maths::PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES,
    );
    put_u64(
        &mut d,
        GLOBAL_OFF_INITIAL_VIRTUAL_SOL_RESERVES,
        bot_core::maths::PUMP_INITIAL_VIRTUAL_SOL_RESERVES,
    );
    put_u64(
        &mut d,
        GLOBAL_OFF_INITIAL_REAL_TOKEN_RESERVES,
        bot_core::maths::PUMP_INITIAL_REAL_TOKEN_RESERVES,
    );
    put_u64(
        &mut d,
        GLOBAL_OFF_TOKEN_TOTAL_SUPPLY,
        bot_core::maths::PUMP_TOKEN_TOTAL_SUPPLY,
    );
    put_u64(&mut d, GLOBAL_OFF_FEE_BASIS_POINTS, 100);
    put_pk(&mut d, GLOBAL_OFF_WITHDRAW_AUTHORITY, &Pubkey::new_unique());
    d[GLOBAL_OFF_ENABLE_MIGRATE] = 1;
    for i in 0..7 {
        put_pk(&mut d, GLOBAL_OFF_FEE_RECIPIENTS + 32 * i, &fee_recipient);
    }
    d
}

/// Shape of a bonding curve for the encoder.
#[derive(Debug, Clone, Copy)]
pub struct CurveSpec {
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
    pub real_sol: u64,
    pub real_tokens: u64,
    pub complete: bool,
}

impl CurveSpec {
    /// A fresh curve right after creation plus a small first buy.
    pub fn fresh() -> Self {
        CurveSpec {
            virtual_sol: 30 * SOL + SOL / 2,
            virtual_tokens: 1_055_000_000_000_000,
            real_sol: SOL / 2,
            real_tokens: 775_100_000_000_000,
            complete: false,
        }
    }
}

/// A v1 (81-byte) bonding curve account.
pub fn bonding_curve_bytes(creator: Pubkey, spec: CurveSpec) -> Vec<u8> {
    let mut d = vec![0u8; BONDING_CURVE_MIN_LEN];
    d[..8].copy_from_slice(&PUMP_ACC_DISC_BONDING_CURVE);
    put_u64(&mut d, BC_OFF_VIRTUAL_TOKEN_RESERVES, spec.virtual_tokens);
    put_u64(&mut d, BC_OFF_VIRTUAL_SOL_RESERVES, spec.virtual_sol);
    put_u64(&mut d, BC_OFF_REAL_TOKEN_RESERVES, spec.real_tokens);
    put_u64(&mut d, BC_OFF_REAL_SOL_RESERVES, spec.real_sol);
    put_u64(
        &mut d,
        BC_OFF_TOKEN_TOTAL_SUPPLY,
        bot_core::maths::PUMP_TOKEN_TOTAL_SUPPLY,
    );
    d[BC_OFF_COMPLETE] = spec.complete as u8;
    put_pk(&mut d, BC_OFF_CREATOR, &creator);
    d
}

/// An SPL mint account (82 bytes).
pub fn mint_bytes(
    mint_authority: Option<Pubkey>,
    freeze_authority: Option<Pubkey>,
    supply: u64,
    decimals: u8,
) -> Vec<u8> {
    let mut d = vec![0u8; SPL_MINT_LEN];
    if let Some(a) = mint_authority {
        d[0..4].copy_from_slice(&1u32.to_le_bytes());
        d[4..36].copy_from_slice(&a.to_bytes());
    }
    put_u64(&mut d, 36, supply);
    d[44] = decimals;
    d[45] = 1;
    if let Some(f) = freeze_authority {
        d[46..50].copy_from_slice(&1u32.to_le_bytes());
        d[50..82].copy_from_slice(&f.to_bytes());
    }
    d
}

/// Install a complete, tradable pump.fun world for `mint` on the node:
/// Global, bonding curve and the mint (authorities revoked).
pub fn install_pump_token(node: &MockNode, mint: Pubkey, creator: Pubkey, spec: CurveSpec) {
    node.set_account(
        *PUMP_GLOBAL,
        *PUMP_PROGRAM_ID,
        pump_global_bytes(Pubkey::new_unique()),
    );
    node.set_account(
        solana_kit::pump::bonding_curve_pda(&mint),
        *PUMP_PROGRAM_ID,
        bonding_curve_bytes(creator, spec),
    );
    node.set_account(
        mint,
        *TOKEN_PROGRAM,
        mint_bytes(None, None, bot_core::maths::PUMP_TOKEN_TOTAL_SUPPLY, 6),
    );
}

// ---------------------------------------------------------------------------
// Events, state, sniper
// ---------------------------------------------------------------------------

/// A base58 signature of exactly 64 bytes, distinct per `tag`.
pub fn signature(tag: u8) -> String {
    let mut bytes = [0u8; 64];
    bytes[63] = tag.max(1);
    bytes[0] = tag;
    bs58::encode(bytes).into_string()
}

pub fn token_launch(
    mint: Pubkey,
    creator: Pubkey,
    sig: &str,
    observed_at: DateTime<Utc>,
) -> TokenLaunch {
    TokenLaunch {
        mint: mint.to_string(),
        name: "Harness Token".into(),
        symbol: "HRN".into(),
        uri: None,
        creator: creator.to_string(),
        pool: "bonding-curve".into(),
        initial_buy_sol: 0.5,
        market_cap_sol: 30.5,
        market_cap_usd: None,
        total_supply: Some(1_000_000_000.0),
        slot: Some(500),
        signature: Some(sig.to_string()),
        tx_type: Some("create".into()),
        observed_at,
        feed: LaunchFeed::PumpPortal,
        socials: None,
    }
}

/// A well-formed pump.fun launch event observed "now".
pub fn pump_event(mint: Pubkey, creator: Pubkey, sig_tag: u8) -> LaunchEvent {
    let launch = token_launch(mint, creator, &signature(sig_tag), Utc::now());
    LaunchEvent::from_token_launch(launch, sig_tag as u64, format!("raw-{sig_tag}"))
}

/// Base config for pipeline tests: sniper on, paper mode, no feeds, no
/// layout file, screening thresholds off, fixed fees.
pub fn base_config() -> Config {
    let mut cfg = Config::default();
    cfg.sniper.enabled = true;
    cfg.sniper.use_pumpportal = false;
    cfg.sniper.use_log_subscription = false;
    cfg.sniper.use_transaction_subscribe = false;
    cfg.sniper.pump_layout_file = String::new();
    cfg.sniper.pump_learn_account_layout = false;
    cfg.sniper.buy_sol = 0.05;
    cfg.sniper.slippage_pct = 15.0;
    cfg.sniper.max_launch_age_secs = 120;
    cfg.sniper.max_entry_latency_ms = 0;
    cfg.risk.min_creator_buy_sol = 0.0;
    cfg.risk.min_socials = 0;
    cfg.risk.max_launch_market_cap_sol = 0.0;
    cfg.risk.min_sol_reserve = 0.01;
    cfg.risk.sniper_max_pending_executions = 0;
    cfg.risk.sniper_failed_entry_cooldown_secs = 0;
    cfg.execution.mode = ExecutionMode::Paper;
    cfg.execution.allow_live_trading = false;
    cfg.execution.confirm_timeout_ms = 1_500;
    cfg.execution.confirm_poll_ms = 100;
    cfg.execution.send_retries = 1;
    cfg.execution.fee_mode = "fixed".into();
    cfg
}

pub fn live_config() -> Config {
    let mut cfg = base_config();
    cfg.execution.mode = ExecutionMode::Live;
    cfg.execution.allow_live_trading = true;
    cfg
}

pub fn state_with(cfg: Config) -> Shared {
    AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    })
}

/// A sniper wired to `rpc` with a fresh wallet.
pub async fn sniper(state: Shared, rpc: Rpc) -> (Sniper, Arc<Wallet>) {
    let wallet = Arc::new(Wallet::generate());
    let s = Sniper::new(state, rpc, wallet.clone(), None)
        .await
        .expect("sniper builds");
    (s, wallet)
}

/// A fully wired offline world: mock node with a tradable pump token, a
/// sniper pointed at it, and the event for that token.
pub struct World {
    pub node: Arc<MockNode>,
    pub url: String,
    pub state: Shared,
    pub sniper: Sniper,
    pub wallet: Arc<Wallet>,
    pub mint: Pubkey,
    pub creator: Pubkey,
}

impl World {
    pub async fn new(cfg: Config) -> Self {
        let node = Arc::new(MockNode::default());
        let url = spawn_mock_node(node.clone()).await;
        let mint = Pubkey::new_unique();
        let creator = Pubkey::new_unique();
        install_pump_token(&node, mint, creator, CurveSpec::fresh());
        let state = state_with(cfg);
        let (sniper, wallet) = sniper(state.clone(), mock_rpc(&url)).await;
        World {
            node,
            url,
            state,
            sniper,
            wallet,
            mint,
            creator,
        }
    }

    pub fn event(&self, sig_tag: u8) -> LaunchEvent {
        pump_event(self.mint, self.creator, sig_tag)
    }
}

// ---------------------------------------------------------------------------
// Log-line builders (pump `Create` events as a node would print them)
// ---------------------------------------------------------------------------

/// Anchor-encode a pump `Create` event body (field order = `decode_create`).
pub fn create_event_payload(
    name: &str,
    symbol: &str,
    uri: &str,
    mint: Pubkey,
    creator: Pubkey,
    timestamp: i64,
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
    b.extend_from_slice(solana_kit::pump::bonding_curve_pda(&mint).as_ref()); // bonding curve
    b.extend_from_slice(creator.as_ref()); // user (the creator opens the curve)
    b.extend_from_slice(creator.as_ref()); // creator
    b.extend_from_slice(&timestamp.to_le_bytes()); // timestamp
    b.extend_from_slice(&1_073_000_000_000u64.to_le_bytes()); // virtual token reserves
    b.extend_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual sol reserves
    b.extend_from_slice(&793_100_000_000u64.to_le_bytes()); // real token reserves
    b.extend_from_slice(&1_000_000_000_000u64.to_le_bytes()); // total supply
    b.extend_from_slice(TOKEN_PROGRAM.as_ref()); // token program
    b.push(0); // is_mayhem_mode
    b.push(0); // is_cashback_enabled
    b.extend_from_slice(WSOL_MINT.as_ref()); // quote mint
    b.extend_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual quote reserves
    b.extend_from_slice(&100u64.to_le_bytes()); // creator fee bps
    b.push(0); // is_holder_reward
    b
}

/// The log lines of a pump.fun `create` transaction.
pub fn pump_create_logs(mint: Pubkey, creator: Pubkey) -> Vec<String> {
    let payload = create_event_payload(
        "Feed Token",
        "FEED",
        "https://mock.example/feed.json",
        mint,
        creator,
        Utc::now().timestamp(),
    );
    vec![
        format!("Program {} invoke [1]", *PUMP_PROGRAM_ID),
        "Program log: Instruction: Create".to_string(),
        format!(
            "Program data: {}",
            base64::engine::general_purpose::STANDARD.encode(payload)
        ),
        format!("Program {} success", *PUMP_PROGRAM_ID),
    ]
}
