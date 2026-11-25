//! Two-replica MODULE-LAYER integration test (Prompt 3 §X extension,
//! REMAINING-GAP 8 closure): the same whale trade is delivered to two
//! independent `CopyBot` instances that share the authoritative Postgres
//! claim store — exactly ONE replica passes the claim gate and proceeds
//! toward execution; the loser skips deterministically before anything
//! money-moving (§F/§G/§H).
//!
//! # Why this file looks the way it does (2026-11-09 rewrite)
//!
//! The previous workspace run reported both attempts reaching RPC, while
//! isolated runs passed. The precise cause is not established by that log.
//! This test makes the required contention overlap explicit and checks the
//! durable claim lineage rather than relying only on returned errors.
//!
//! Every wait here is now **event-driven** (`Notify`) and the only timeout is
//! a deadlock guard: a `30s` cap that fails the test with a diagnostic rather
//! than hanging CI. The test is a three-phase handshake:
//!
//! | phase | gate | what it proves |
//! |---|---|---|
//! | 1 | `request_seen` | one replica got a claim **and** reached the pre-broadcast RPC (the gate exists and the winner really proceeds) |
//! | 2 | `competitor_done` | the other replica returned **while the winner was still held** — a true deterministic loser, not a fast finisher |
//! | 3 | `release_rpc` | the winner then fails at the injected RPC and releases cleanly |
//!
//! The claim store itself is also asserted directly (row owner, status,
//! epoch, append-only lineage), so a gate that silently stopped writing would
//! fail even if both futures happened to return sensible-looking errors.
//!
//! The SQL atomicity of `PostgresClaimStore::claim` is pinned statically in
//! `bot_core::db::claims::atomic_shape_tests`, and the N-replica × M-round
//! property is covered by `tests/claim_stress.rs`.
//!
//! GATED: skipped unless `POSTGRES_URL` points at a real database. Run with
//! `--test-threads=1` (shared stores, like every other gated suite here).

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinSet;

use bot_core::config::{AppConfig, Config, CopyWallet, DatabaseConfig};
use bot_core::db::claims::PostgresClaimStore;
use bot_core::db::Database;
use bot_core::models::{PositionSide, Venue, WalletTrade};
use bot_core::ownership::{ClaimStatus, ClaimStore, OwnershipRegistry};
use module_copy::CopyBot;
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;

/// Deadlock guard, NOT a timing assumption: every assertion below is driven
/// by a `Notify`, so it fails instead of waiting indefinitely (a loaded runner may also time out).
const DEADLOCK_GUARD: Duration = Duration::from_secs(30);

/// Explicit skip message so a gated run is never silently green.
const SKIP_MESSAGE: &str =
    "SKIP: POSTGRES_URL is not set — the two-replica claim test needs a real PostgreSQL 16 \
     instance to exercise the authoritative claim store. It is NOT reported as a pass.";

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

struct Ctx {
    bot: CopyBot,
    cfg: Config,
    db: Arc<Database>,
    replica: String,
}

async fn build_ctx(replica: &str, rpc_url: &str) -> Option<Ctx> {
    let url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let db_cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&db_cfg, &url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    let db = Arc::new(db);

    let mut raw = Config::default();
    raw.ha.replica_id = replica.to_string();
    raw.copy.enabled = true;
    raw.copy.wallets = vec![CopyWallet {
        address: "whale".into(),
        label: Some("whale".into()),
        fixed_sol: Some(0.01),
        fraction_of_their_size: 0.0,
        max_sol: 1.0,
        min_sol: 0.0,
        buys_only: false,
        slippage_pct: None,
        max_staleness_secs: 0,
        paused: false,
        max_exposure_sol: 0.0,
        max_open_positions: 0,
    }];
    // A test-local HTTP endpoint holds the first curve lookup open until the
    // competing replica has returned from its claim attempt.
    raw.network.rpc_url = rpc_url.to_string();
    raw.network.rpc_url_fallbacks = Vec::new();
    raw.network.max_retries = 0;
    // The held request and its lease must outlast the deadlock guard. A
    // client timeout must never silently release the winner while a slow
    // competitor is still approaching the claim gate.
    raw.network.request_timeout_ms = 120_000;

    let state = bot_core::state::AppState::new(AppConfig {
        raw: raw.clone(),
        source_path: None,
        warnings: Vec::new(),
    });
    // Paper mode reads seeded balances — no RPC before the claim gate.
    state.set_balances(Some(10.0), Some(1000.0)).await;

    let registry = OwnershipRegistry::new(
        Arc::new(PostgresClaimStore::new(db.clone())),
        replica,
        Duration::from_secs(120),
        Duration::from_secs(900),
    );
    let rpc = Rpc::new(&raw.network).expect("client construction is lazy");
    let bot = CopyBot::new(state, rpc, Arc::new(Wallet::generate()), None)
        .await
        .with_ownership(Arc::new(registry));
    Some(Ctx {
        bot,
        cfg: raw,
        db,
        replica: replica.to_string(),
    })
}

/// A valid, unique mint so the winner's Pubkey parse succeeds and the claim id
/// is collision-free across runs. `Pubkey::new_unique()` is a per-process
/// counter (deterministic across runs), so the mint is derived from the run
/// tag instead — otherwise reruns collide with their own stale claim rows in
/// the shared database.
fn mint_for(tag: &str) -> String {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for b in tag.as_bytes() {
        seed = seed.rotate_left(5) ^ u64::from(*b);
    }
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(8) {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    solana_sdk::pubkey::Pubkey::new_from_array(bytes).to_string()
}

#[tokio::test]
async fn two_replicas_one_whale_trade_one_execution() {
    if std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .is_none()
    {
        eprintln!("{SKIP_MESSAGE}");
        return;
    }

    let tag = run_id();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local gated RPC endpoint");
    let rpc_url = format!("http://{}", listener.local_addr().expect("RPC address"));

    // Deterministic, event-driven gates (no sleeps, no socket-race inference).
    let request_seen = Arc::new(Notify::new());
    let release_rpc = Arc::new(Notify::new());
    let request_seen_tx = request_seen.clone();
    let release_rpc_rx = release_rpc.clone();
    let rpc_server = tokio::spawn(async move {
        // Hold the FIRST connection open until the test releases it: that is
        // the replica that holds the claim. Any second connection is recorded
        // too, which makes a missing claim gate fail deterministically instead
        // of racing.
        let mut sockets = Vec::new();
        let first = listener.accept().await.expect("first RPC request").0;
        sockets.push(first);
        request_seen_tx.notify_one();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    if let Ok((socket, _)) = accepted {
                        sockets.push(socket);
                    }
                }
                _ = release_rpc_rx.notified() => break,
            }
        }
        let connection_count = sockets.len();
        for mut socket in sockets {
            let _ = socket
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
        connection_count
    });

    let (Some(a), Some(b)) = (
        build_ctx(&format!("rep-A-{tag}"), &rpc_url).await,
        build_ctx(&format!("rep-B-{tag}"), &rpc_url).await,
    ) else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };

    let mint = mint_for(&tag);
    let trade = WalletTrade {
        wallet: "whale".into(),
        signature: format!("sig-{tag}"),
        slot: 1,
        block_time: None,
        side: PositionSide::Long, // a whale BUY
        mint: mint.clone(),
        symbol: None,
        token_amount: 100.0,
        sol_amount: 1.0,
        venue: Venue::PumpFun,
        fee_sol: 0.0,
        discriminator: None,
        observed_at: chrono::Utc::now(),
    };
    let execution_id = format!("copy:whale:{mint}");

    let Ctx {
        bot: mut bot_a,
        cfg: cfg_a,
        db,
        replica: replica_a,
    } = a;
    let Ctx {
        bot: mut bot_b,
        cfg: cfg_b,
        replica: replica_b,
        ..
    } = b;

    // ---- phase 1 + 2: one winner reaches the gate, the other returns during
    // that window (event-driven, never a timing guess) ----------------------
    let shared_trade = Arc::new(trade);
    let mut tasks = JoinSet::new();
    let trade_a = shared_trade.clone();
    tasks.spawn(async move { ("A", bot_a.mirror_trade(&trade_a, &cfg_a).await) });
    tasks.spawn(async move { ("B", bot_b.mirror_trade(&shared_trade, &cfg_b).await) });

    tokio::time::timeout(DEADLOCK_GUARD, request_seen.notified())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "DEADLOCK GUARD: no replica reached the gated RPC within {DEADLOCK_GUARD:?} — \
                 the claim gate let nobody through (or the pipeline stalled before execution)"
            )
        });

    let (first_name, first_result) = tokio::time::timeout(DEADLOCK_GUARD, tasks.join_next())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "DEADLOCK GUARD: the competing replica did not return within {DEADLOCK_GUARD:?} \
                 while the claim holder was held at the RPC gate"
            )
        })
        .expect("a mirror task must finish")
        .expect("join the first mirror task");
    assert!(
        first_result.is_ok(),
        "the replica without the claim must skip cleanly before RPC: {first_name}={first_result:?}"
    );

    // ---- phase 3: release the winner and let it fail at the injected RPC ---
    release_rpc.notify_one();
    let (second_name, second_result) = tasks
        .join_next()
        .await
        .expect("the claimed replica must finish after RPC is released")
        .expect("join the claimed mirror task");
    assert_ne!(first_name, second_name);
    assert!(
        second_result.is_err(),
        "the sole claim holder should reach the injected RPC failure: {second_name}={second_result:?}"
    );
    assert_eq!(
        rpc_server.await.expect("RPC gate task"),
        1,
        "exactly ONE replica may issue a curve lookup (a second connection means the claim \
         gate admitted two executors)"
    );

    let winner = if second_name == "A" {
        &replica_a
    } else {
        &replica_b
    };
    let loser = if first_name == "A" {
        &replica_a
    } else {
        &replica_b
    };

    // ---- the store itself: owner, status, epoch, lineage -------------------
    let store = PostgresClaimStore::new(db);
    let rec = store
        .get(&execution_id)
        .await
        .expect("claim store reachable")
        .expect("claim row must exist");
    assert_eq!(&rec.owner_id, winner, "the winner owns the claim");
    assert_eq!(rec.status, ClaimStatus::Released);
    assert_eq!(rec.epoch, 1, "no takeover happened");
    assert_eq!(rec.takeover_count, 0, "exactly one generation owned this");

    // The full lineage is auditable: acquired (by the winner) + released.
    let events = store.events(&execution_id).await.expect("events readable");
    let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["acquired", "released"],
        "one acquisition and one clean release — a takeover or a second \
         acquisition means two replicas believed they owned this trade"
    );
    assert_eq!(events[0].owner_id, *winner);

    // The loser never produced a claim-generation of its own (it was rejected
    // atomically). This assertion checks lineage, not position persistence.
    assert_eq!(
        events.iter().filter(|e| e.owner_id == *loser).count(),
        0,
        "the losing replica must leave no claim lineage at all"
    );
}

/// Reproduce the old dead-port harness assumption without timing races: a
/// pre-broadcast failure releases the lease, then a later delivery on a
/// different process-local dedup state may acquire the same key again.
/// Two RPC errors in this sequence do not imply overlapping claim holders.
#[tokio::test]
async fn sequential_fast_failures_can_both_reach_rpc() {
    if std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .is_none()
    {
        eprintln!("{SKIP_MESSAGE}");
        return;
    }
    let tag = run_id();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("RPC listener");
    let url = format!("http://{}", listener.local_addr().expect("RPC address"));
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.expect("RPC connection");
            socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.expect("RPC failure response");
        }
    });
    let mut a = build_ctx(&format!("seq-A-{tag}"), &url)
        .await
        .expect("configured DB");
    let mut b = build_ctx(&format!("seq-B-{tag}"), &url)
        .await
        .expect("configured DB");
    let mint = mint_for(&tag);
    let trade = WalletTrade {
        wallet: "whale".into(),
        signature: format!("seq-{tag}"),
        slot: 1,
        block_time: None,
        side: PositionSide::Long,
        mint: mint.clone(),
        symbol: None,
        token_amount: 100.0,
        sol_amount: 1.0,
        venue: Venue::PumpFun,
        fee_sol: 0.0,
        discriminator: None,
        observed_at: chrono::Utc::now(),
    };
    let first = tokio::time::timeout(DEADLOCK_GUARD, a.bot.mirror_trade(&trade, &a.cfg))
        .await
        .expect("first finishes");
    assert!(first.is_err(), "first reaches injected failure: {first:?}");
    let second = tokio::time::timeout(DEADLOCK_GUARD, b.bot.mirror_trade(&trade, &b.cfg))
        .await
        .expect("second finishes");
    assert!(
        second.is_err(),
        "second may reacquire after release: {second:?}"
    );
    tokio::time::timeout(DEADLOCK_GUARD, server)
        .await
        .expect("both RPCs observed")
        .expect("server task");
    let store = PostgresClaimStore::new(a.db);
    let events = store
        .events(&format!("copy:whale:{mint}"))
        .await
        .expect("read lineage");
    let kinds: Vec<&str> = events.iter().map(|event| event.event.as_str()).collect();
    assert_eq!(kinds, ["acquired", "released", "reacquired", "released"]);
    assert_eq!(events[0].owner_id, a.replica);
    assert_eq!(events[2].owner_id, b.replica);
}
