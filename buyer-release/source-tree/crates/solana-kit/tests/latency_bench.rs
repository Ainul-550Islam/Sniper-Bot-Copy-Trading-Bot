//! Latency work evidence — BUILD PLAN §5.
//!
//! Two kinds of tests live here:
//!
//! 1. **Offline, always-run** proofs that the latency machinery behaves:
//!    the warm account cache serves reads with the network dead, and the
//!    broadcast fan-out wins on a healthy fallback while the primary rejects.
//!    These use a dead port / a mock JSON-RPC HTTP server, never the internet.
//!
//! 2. **Gated benchmarks** (`E2E_NETWORK=1`, same convention as
//!    `devnet_e2e.rs`): p50/p95 round-trip latency for the hot-path RPC calls
//!    (`getSlot`, `getLatestBlockhash`, `simulateTransaction`) and — with
//!    `E2E_LIVE=1` — the transaction landing rate through the real executor.
//!    Gated tests print a report; they only assert that the calls succeed and
//!    (live) that transactions land, never a wall-clock bound, so CI stays
//!    deterministic and slow networks do not create flakes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use solana_sdk::account::Account;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_system_interface::instruction as system_instruction;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use solana_kit::consts::TOKEN_PROGRAM;
use solana_kit::execute::{BroadcastMode, ExecPolicy, ExecStatus, Executor};
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;
use solana_kit::tx::TxRequest;

fn gated(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn rpc_at(url: String, fallbacks: Vec<String>) -> Rpc {
    Rpc::with_urls(
        url,
        String::new(),
        fallbacks,
        CommitmentConfig::confirmed(),
        1,
        Duration::from_secs(10),
    )
    .expect("rpc builds")
}

fn account(lamports: u64, owner: Pubkey) -> Account {
    Account {
        lamports,
        data: Vec::new(),
        owner,
        executable: false,
        rent_epoch: 0,
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

// ---------------------------------------------------------------------------
// 1. Offline proofs
// ---------------------------------------------------------------------------

/// With a warm cache the hot-path account reads never touch the network:
/// the RPC points at a dead port, yet cached lookups succeed while uncached
/// ones fail — that failure is the proof no round trip was skipped silently.
#[tokio::test]
async fn warm_cache_serves_hot_path_reads_offline() {
    let rpc = rpc_at("http://127.0.0.1:9".into(), Vec::new())
        .with_account_cache(Duration::from_secs(30), 64);

    let mint = Pubkey::new_unique();
    let user_ata = Pubkey::new_unique();
    rpc.account_cache()
        .insert(mint, account(1_461_600, *TOKEN_PROGRAM))
        .await;
    rpc.account_cache()
        .insert(user_ata, account(2_039_280, *TOKEN_PROGRAM))
        .await;

    // Sanity: the network really is dead (get_balance surfaces transport
    // errors; get_account maps them to "not found").
    assert!(
        rpc.get_balance(&mint).await.is_err(),
        "an uncached balance read must fail against the dead port"
    );

    // Cached reads served locally.
    let hit = rpc
        .get_account_cached(&mint)
        .await
        .expect("cached read must not touch the network")
        .expect("entry is warm");
    assert_eq!(hit.lamports, 1_461_600);
    assert_eq!(hit.owner, *TOKEN_PROGRAM);

    // pump.rs uses this for the user's ATA existence check.
    assert!(rpc.account_exists_cached(&user_ata).await.expect("cached"));

    // pump.rs uses this to resolve the mint's token program.
    assert_eq!(
        rpc.token_program_of(&mint).await.expect("cached owner"),
        *TOKEN_PROGRAM
    );

    // An unknown key is a miss that falls through to the (dead) network and
    // comes back empty — no phantom cache hit.
    let cold = Pubkey::new_unique();
    assert!(rpc
        .get_account_cached(&cold)
        .await
        .expect("get_account maps the transport error to not-found")
        .is_none());

    let (hits, _, _) = rpc.account_cache().stats();
    assert!(hits >= 3, "the three warm reads were counted as hits");
}

/// Broadcast fan-out: the primary rejects every `sendTransaction`, the
/// fallback accepts — the executor must return the fallback's signature
/// without any retry delay between endpoints (they race). Confirmation then
/// times out against the mock, which is fine: this test targets the
/// broadcast phase.
#[tokio::test]
async fn fanout_wins_on_the_healthy_fallback_when_primary_rejects() {
    // Mock A (primary): blockhash + status OK, sendTransaction rejected.
    let primary = spawn_mock_rpc(MockRole::RejectingSender).await;
    // Mock B (fallback): everything OK, sendTransaction returns a fixed sig.
    let fallback = spawn_mock_rpc(MockRole::AcceptingSender).await;

    let accepted_sig = Signature::from([9u8; 64]).to_string();

    let rpc = rpc_at(primary, vec![fallback.clone()]);
    let wallet = Arc::new(Wallet::generate());
    let policy = ExecPolicy {
        mode: bot_core::models::ExecutionMode::Live,
        broadcast: BroadcastMode::Rpc,
        simulate_first: false,
        abort_on_simulation_failure: false,
        confirm_timeout: Duration::from_secs(2),
        confirm_poll_interval: Duration::from_millis(400),
        max_attempts: 1,
        jito_url: None,
        min_priority_fee_micro_lamports: 0,
        fanout: true,
    };
    let executor = Executor::new(rpc, Arc::clone(&wallet), policy);

    let reg = bot_core::obs::metrics::global();
    let before = reg
        .counter(
            "bot_broadcast_fanout_total",
            "Fan-out broadcasts by outcome.",
            &[("outcome", "accepted")],
        )
        .get();

    let req = TxRequest {
        instructions: vec![system_instruction::transfer(
            &wallet.pubkey,
            &wallet.pubkey,
            0,
        )],
        label: "fanout-mock".into(),
        ..Default::default()
    };
    let result = executor
        .run(req)
        .await
        .expect("run must not hard-fail; the broadcast phase has a healthy endpoint");

    // solana-client verifies the endpoint echoes the transaction's own
    // signature; that the *fan-out* path won via mock B (mock A rejects
    // every send) is proven by the meter below and by reaching Sent at all.
    assert!(
        !result.signature.is_empty() && result.signature.parse::<Signature>().is_ok(),
        "a real signature came back: {}",
        result.signature
    );
    let _ = accepted_sig;
    assert!(
        matches!(result.status, ExecStatus::Sent),
        "broadcast succeeded, mock never confirms: status={:?} error={:?}",
        result.status,
        result.error
    );
    assert!(result.send_ms.is_some(), "send latency was measured");

    let after = reg
        .counter(
            "bot_broadcast_fanout_total",
            "Fan-out broadcasts by outcome.",
            &[("outcome", "accepted")],
        )
        .get();
    assert!(after > before, "the fan-out acceptance was metered");
}

/// Minimal JSON-RPC HTTP mock: enough methods for the executor's
/// build → broadcast → confirm-poll loop.
#[derive(Clone, Copy)]
enum MockRole {
    RejectingSender,
    AcceptingSender,
}

async fn spawn_mock_rpc(role: MockRole) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                // Read the headers, then exactly Content-Length body bytes —
                // requests (especially sendTransaction) can be split across
                // TCP segments, and a short read would drop the connection.
                let mut filled = 0usize;
                let body_start = loop {
                    let n = match tokio::time::timeout(
                        Duration::from_secs(5),
                        sock.read(&mut buf[filled..]),
                    )
                    .await
                    {
                        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return,
                        Ok(Ok(n)) => n,
                    };
                    filled += n;
                    if let Some(pos) = find_subslice(&buf[..filled], b"\r\n\r\n") {
                        break pos + 4;
                    }
                    if filled >= buf.len() {
                        return;
                    }
                };
                let content_length = String::from_utf8_lossy(&buf[..body_start])
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        (k.eq_ignore_ascii_case("content-length"))
                            .then(|| v.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while filled < body_start + content_length {
                    let n = match tokio::time::timeout(
                        Duration::from_secs(5),
                        sock.read(&mut buf[filled..]),
                    )
                    .await
                    {
                        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return,
                        Ok(Ok(n)) => n,
                    };
                    filled += n;
                }
                let text = String::from_utf8_lossy(&buf[body_start..filled]).to_string();
                let req: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let id = req["id"].clone();
                let method = req["method"].as_str().unwrap_or_default();
                let result = match (role, method) {
                    (_, "getLatestBlockhash") => json!({
                        "context": {"slot": 1},
                        "value": {
                            "blockhash": Hash::default().to_string(),
                            "lastValidBlockHeight": u64::MAX / 2,
                        }
                    }),
                    (_, "isBlockhashValid") => json!({
                        "context": {"slot": 1},
                        "value": true,
                    }),
                    (_, "getSignatureStatuses") => json!({
                        "context": {"slot": 1},
                        "value": [null],
                    }),
                    (MockRole::AcceptingSender, "sendTransaction") => {
                        // solana-client verifies the response signature
                        // against the submitted transaction, so echo the
                        // real one: it is the first 64 bytes of the wire tx.
                        let b64 = req["params"][0].as_str().unwrap_or_default();
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .unwrap_or_default();
                        // Wire layout: compact-u16 signature count, then the
                        // 64-byte signatures, then the message.
                        let mut off = 0usize;
                        let mut count = 0usize;
                        let mut shift = 0u32;
                        while off < bytes.len() {
                            let b = bytes[off];
                            off += 1;
                            count |= ((b & 0x7f) as usize) << shift;
                            if b & 0x80 == 0 {
                                break;
                            }
                            shift += 7;
                        }
                        if count >= 1 && bytes.len() >= off + 64 {
                            match Signature::try_from(&bytes[off..off + 64]) {
                                Ok(sig) => json!(sig.to_string()),
                                Err(_) => json!(Value::Null),
                            }
                        } else {
                            json!(Value::Null)
                        }
                    }
                    _ => {
                        let err = json!({
                            "code": -32003,
                            "message": "Transaction signature verification failure",
                        });
                        let resp = json!({"jsonrpc": "2.0", "error": err, "id": id}).to_string();
                        write_http(&mut sock, &resp).await;
                        return;
                    }
                };
                let resp = json!({"jsonrpc": "2.0", "result": result, "id": id}).to_string();
                write_http(&mut sock, &resp).await;
            });
        }
    });
    format!("http://{addr}")
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn write_http(sock: &mut tokio::net::TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

// ---------------------------------------------------------------------------
// 2. Gated live benchmarks
// ---------------------------------------------------------------------------

fn bench_rpc() -> Rpc {
    let url =
        std::env::var("E2E_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    rpc_at(url, Vec::new())
}

/// p50/p95/max for the hot-path RPC calls, measured sequentially so the
/// numbers reflect single-shot latency, not concurrency.
#[tokio::test]
async fn hot_path_rpc_latency_percentiles() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP hot_path_rpc_latency_percentiles: set E2E_NETWORK=1 to run");
        return;
    }
    let rpc = bench_rpc();
    const N: usize = 30;

    let mut slot_ms = Vec::with_capacity(N);
    let mut bh_ms = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        rpc.get_slot().await.expect("getSlot");
        slot_ms.push(t.elapsed().as_millis() as u64);

        let t = Instant::now();
        rpc.latest_blockhash(true)
            .await
            .expect("getLatestBlockhash");
        bh_ms.push(t.elapsed().as_millis() as u64);
    }

    for (label, ms) in [("getSlot", slot_ms), ("getLatestBlockhash", bh_ms)] {
        let mut sorted = ms.clone();
        sorted.sort_unstable();
        eprintln!(
            "LATENCY {label}: n={} p50={}ms p95={}ms max={}ms",
            sorted.len(),
            percentile(&sorted, 0.50),
            percentile(&sorted, 0.95),
            sorted.last().copied().unwrap_or(0),
        );
    }
}

/// Simulation round-trip latency. Uses an unfunded wallet, so the verdict is
/// an expected fee error — the point is the transport cost of
/// `simulateTransaction` on the hot path, and the executor's simulate helper
/// is exercised end to end.
#[tokio::test]
async fn simulate_round_trip_latency() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP simulate_round_trip_latency: set E2E_NETWORK=1 to run");
        return;
    }
    let rpc = bench_rpc();
    let wallet = Arc::new(Wallet::generate());
    let executor = Executor::new(
        rpc,
        Arc::clone(&wallet),
        ExecPolicy {
            mode: bot_core::models::ExecutionMode::Simulate,
            simulate_first: false,
            abort_on_simulation_failure: false,
            ..Default::default()
        },
    );
    let req = TxRequest {
        instructions: vec![system_instruction::transfer(
            &wallet.pubkey,
            &wallet.pubkey,
            0,
        )],
        label: "latency-bench-simulate".into(),
        ..Default::default()
    };

    const N: usize = 10;
    let mut ms = Vec::with_capacity(N);
    let mut verdicts = 0;
    for _ in 0..N {
        let t = Instant::now();
        let sim = executor
            .simulate_request(&req)
            .await
            .expect("simulateTransaction must return a verdict");
        ms.push(t.elapsed().as_millis() as u64);
        if sim.error.is_none() {
            verdicts += 1;
        }
    }
    ms.sort_unstable();
    eprintln!(
        "LATENCY simulateTransaction: n={} p50={}ms p95={}ms max={}ms clean_verdicts={}/{} (unfunded wallet: fee errors expected)",
        ms.len(),
        percentile(&ms, 0.50),
        percentile(&ms, 0.95),
        ms.last().copied().unwrap_or(0),
        verdicts,
        N,
    );
}

/// Landing rate through the real executor: half the sends race the fan-out
/// code path (single endpoint here, unless `E2E_FALLBACK_URL` provides a
/// second), half use the sequential path. Reports confirmed/total.
#[tokio::test]
async fn landing_rate_sequential_vs_fanout() {
    if !gated("E2E_NETWORK") || !gated("E2E_LIVE") {
        eprintln!(
            "SKIP landing_rate: set E2E_NETWORK=1 E2E_LIVE=1 to run (broadcasts valueless self-transfers)"
        );
        return;
    }
    let url =
        std::env::var("E2E_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let fallbacks: Vec<String> = std::env::var("E2E_FALLBACK_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .map(|u| vec![u])
        .unwrap_or_default();
    let rpc = rpc_at(url, fallbacks);

    let wallet = Arc::new(Wallet::generate());
    if !try_fund(&rpc, &wallet).await {
        eprintln!("SKIP: faucet refused the airdrop (rate limit) — landing rate needs fee funds");
        return;
    }

    let make_policy = |fanout: bool| ExecPolicy {
        mode: bot_core::models::ExecutionMode::Live,
        broadcast: BroadcastMode::Rpc,
        simulate_first: false,
        abort_on_simulation_failure: false,
        confirm_timeout: Duration::from_secs(60),
        confirm_poll_interval: Duration::from_secs(1),
        max_attempts: 2,
        jito_url: None,
        min_priority_fee_micro_lamports: 0,
        fanout,
    };

    let mut landed = (0u32, 0u32); // (sequential, fanout)
    let mut sent_ms = (Vec::new(), Vec::new());
    for fanout in [false, true] {
        for i in 0..3 {
            let executor = Executor::new(rpc.clone(), Arc::clone(&wallet), make_policy(fanout));
            let req = TxRequest {
                instructions: vec![system_instruction::transfer(
                    &wallet.pubkey,
                    &wallet.pubkey,
                    0,
                )],
                label: format!(
                    "landing-rate-{}-{}",
                    if fanout { "fanout" } else { "seq" },
                    i
                ),
                ..Default::default()
            };
            let t = Instant::now();
            match executor.run(req).await {
                Ok(r) if matches!(r.status, ExecStatus::Confirmed) => {
                    if fanout {
                        landed.1 += 1;
                    } else {
                        landed.0 += 1;
                    }
                }
                Ok(r) => eprintln!(
                    "LANDING {}: status={:?} error={:?}",
                    if fanout { "fanout" } else { "seq" },
                    r.status,
                    r.error
                ),
                Err(e) => eprintln!(
                    "LANDING {}: run failed: {e}",
                    if fanout { "fanout" } else { "seq" }
                ),
            }
            let ms = t.elapsed().as_millis() as u64;
            if fanout {
                sent_ms.1.push(ms);
            } else {
                sent_ms.0.push(ms);
            }
            eprintln!("LANDING sample fanout={fanout} wall={ms}ms");
        }
    }

    eprintln!(
        "LANDING RATE: sequential {}/3 confirmed, fanout {}/3 confirmed (build→broadcast→confirm wall times above)",
        landed.0, landed.1
    );
    assert!(
        landed.0 + landed.1 >= 5,
        "at least 5 of 6 valueless self-transfers must land on a healthy cluster"
    );
}

/// Airdrop helper (same contract as `devnet_e2e.rs`): false = skip, not fail.
async fn try_fund(rpc: &Rpc, wallet: &Wallet) -> bool {
    for _ in 0..3 {
        if rpc
            .raw()
            .request_airdrop(&wallet.pubkey, 100_000_000)
            .await
            .is_ok()
        {
            for _ in 0..30 {
                if rpc.get_balance(&wallet.pubkey).await.unwrap_or(0) > 0 {
                    return true;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}
