//! Two-replica MODULE-LAYER integration test (Prompt 3 §X extension,
//! REMAINING-GAP 8 closure): the same whale trade is delivered to two
//! independent `CopyBot` instances that share the authoritative Postgres
//! claim store — exactly ONE replica passes the claim gate and proceeds
//! toward execution; the loser skips deterministically before anything
//! money-moving (§F/§G/§H).
//!
//! How the election is observed without a live cluster: both replicas run
//! paper mode with seeded balances (no RPC needed up to the claim) against a
//! local gated HTTP endpoint. It holds the winner's curve lookup open while
//! the competitor attempts its claim, so only a true claim loser can finish
//! before the controlled pre-broadcast failure releases the winner.
//!
//! GATED: skipped unless `POSTGRES_URL` points at a real database. Run with
//! `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use bot_core::config::{AppConfig, Config, CopyWallet, DatabaseConfig};
use bot_core::db::claims::PostgresClaimStore;
use bot_core::db::Database;
use bot_core::models::{PositionSide, Venue, WalletTrade};
use bot_core::ownership::{ClaimStatus, ClaimStore, OwnershipRegistry};
use module_copy::CopyBot;
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;

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
    // competing replica has attempted its durable claim.
    raw.network.rpc_url = rpc_url.to_string();
    raw.network.rpc_url_fallbacks = Vec::new();
    raw.network.max_retries = 0;

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
        Duration::from_secs(30),
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

#[tokio::test]
async fn two_replicas_one_whale_trade_one_execution() {
    let tag = run_id();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local gated RPC endpoint");
    let rpc_url = format!("http://{}", listener.local_addr().expect("RPC address"));
    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let (release_rpc_tx, mut release_rpc_rx) = oneshot::channel();
    let rpc_server = tokio::spawn(async move {
        // Keep the first request blocked until the competing replica has
        // returned from its claim attempt. Any second connection is recorded
        // and also held, making a missing claim gate fail deterministically.
        let (first, _) = listener.accept().await.expect("first RPC request");
        let mut sockets = vec![first];
        let _ = request_seen_tx.send(());
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    if let Ok((socket, _)) = accepted {
                        sockets.push(socket);
                    }
                }
                _ = &mut release_rpc_rx => break,
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
        eprintln!("POSTGRES_URL not set — skipping two-replica mirror test");
        return;
    };

    // A valid, unique mint so the winner's Pubkey parse succeeds and the
    // claim id is collision-free across runs. NOTE: `Pubkey::new_unique()`
    // is a per-process counter (deterministic across runs) — derive the mint
    // from the run tag instead, or reruns collide with their own stale claim
    // rows in the shared database.
    let mint = {
        // splitmix64 over the tag's bytes -> 32 deterministic-per-run bytes.
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
    };
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

    // Move each independent bot into a task. The first connection proves
    // that one bot passed the claim and is blocked in the curve lookup. The
    // other task must then return successfully as the claim loser, before the
    // controlled HTTP failure releases the winner.
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
    let shared_trade = Arc::new(trade);
    let mut tasks = JoinSet::new();
    let trade_a = shared_trade.clone();
    tasks.spawn(async move { ("A", bot_a.mirror_trade(&trade_a, &cfg_a).await) });
    tasks.spawn(async move { ("B", bot_b.mirror_trade(&shared_trade, &cfg_b).await) });

    tokio::time::timeout(Duration::from_secs(10), request_seen_rx)
        .await
        .expect("one bot must reach the gated RPC endpoint before timeout")
        .expect("RPC gate notification");
    let (first_name, first_result) =
        tokio::time::timeout(Duration::from_secs(10), tasks.join_next())
            .await
            .expect("the competing replica must finish while the winner is held")
            .expect("one mirror task must finish")
            .expect("join the first mirror task");
    assert!(
        first_result.is_ok(),
        "the replica without the claim should skip cleanly before RPC: {first_name}={first_result:?}"
    );

    release_rpc_tx
        .send(())
        .expect("release controlled pre-broadcast RPC failure");
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
        "only the claim holder may issue a curve lookup"
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

    // The shared claim row names the winner; the pre-broadcast failure
    // released it cleanly (determinate outcome → re-acquirable, §M).
    let store = PostgresClaimStore::new(db);
    let rec = store
        .get(&format!("copy:whale:{mint}"))
        .await
        .expect("claim store reachable")
        .expect("claim row must exist");
    assert_eq!(&rec.owner_id, winner, "the winner owns the claim");
    assert_eq!(rec.status, ClaimStatus::Released);
    assert_eq!(rec.epoch, 1, "no takeover happened");

    // The full lineage is auditable: acquired (by the winner) + released.
    let events = store
        .events(&format!("copy:whale:{mint}"))
        .await
        .expect("events readable");
    let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(kinds, vec!["acquired", "released"]);
    assert_eq!(events[0].owner_id, *winner);

    // The loser never produced a claim-generation of its own (it was
    // rejected atomically) and left no position behind.
    assert_eq!(events.iter().filter(|e| e.owner_id == *loser).count(), 0);
}
