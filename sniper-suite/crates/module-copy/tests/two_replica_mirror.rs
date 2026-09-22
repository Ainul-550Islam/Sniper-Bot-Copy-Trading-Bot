//! Two-replica MODULE-LAYER integration test (Prompt 3 §X extension,
//! REMAINING-GAP 8 closure): the same whale trade is delivered to two
//! independent `CopyBot` instances that share the authoritative Postgres
//! claim store — exactly ONE replica passes the claim gate and proceeds
//! toward execution; the loser skips deterministically before anything
//! money-moving (§F/§G/§H).
//!
//! How the election is observed without a live cluster: both replicas run
//! paper mode with seeded balances (no RPC needed up to the claim) against a
//! DEAD rpc port. The winner therefore errors at the bonding-curve load —
//! which proves it got past the claim — while the loser returns `Ok(())`
//! having done nothing. The shared claim row then names the winner, in
//! `released` state (the pre-broadcast failure released it cleanly: nothing
//! moved, so a redelivered event could proceed — §M).
//!
//! GATED: skipped unless `POSTGRES_URL` points at a real database. Run with
//! `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

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

async fn build_ctx(replica: &str) -> Option<Ctx> {
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
    // Dead port with no retries/fallbacks: the winner's curve load fails
    // fast and deterministically once it is past the claim gate.
    raw.network.rpc_url = "http://127.0.0.1:9".into();
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
    let (Some(mut a), Some(mut b)) = (
        build_ctx(&format!("rep-A-{tag}")).await,
        build_ctx(&format!("rep-B-{tag}")).await,
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

    // Deliver the SAME trade to both replicas concurrently — the multi-feed
    // scenario §H describes (each replica has its own feed connection).
    let (ra, rb) = tokio::join!(
        a.bot.mirror_trade(&trade, &a.cfg),
        b.bot.mirror_trade(&trade, &b.cfg)
    );

    // Exactly one replica proceeded past the claim gate: the winner errors
    // at the bonding-curve load (dead port), the loser skips with Ok(()).
    let (a_err, b_err) = (ra.is_err(), rb.is_err());
    assert_ne!(
        a_err, b_err,
        "exactly one replica may pass the claim gate: A={ra:?} B={rb:?}"
    );
    let winner = if a_err { &a.replica } else { &b.replica };
    let loser = if a_err { &b.replica } else { &a.replica };

    // The shared claim row names the winner; the pre-broadcast failure
    // released it cleanly (determinate outcome → re-acquirable, §M).
    let store = PostgresClaimStore::new(a.db.clone());
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
