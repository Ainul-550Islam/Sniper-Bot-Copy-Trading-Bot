//! Two-context distributed integration test (Prompt 3 §X).
//!
//! Builds TWO independent "replica contexts" — each with its own Postgres
//! pool, its own Redis connection, its own [`AppState`] and its own
//! [`OwnershipRegistry`] — sharing the SAME Postgres and Redis servers, and
//! proves the multi-replica invariants end to end:
//!
//! * one logical execution → exactly one active owner (Postgres AND Redis
//!   claim stores, raced concurrently);
//! * kill-switch / module-flag mutations on replica A converge onto replica
//!   B through the shared `runtime_flags` store (and B's money path is
//!   actually blocked while the kill is engaged);
//! * the position book written by replica A converges onto replica B via
//!   `list_open` + `merge_positions` (book sync), so B's exit claims use the
//!   SAME logical position identity.
//!
//! GATED: skipped unless BOTH `POSTGRES_URL` and `REDIS_URL` point at real
//! servers. Run with `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

use bot_core::config::{AppConfig, Config, DatabaseConfig, RedisConfig};
use bot_core::db::claims::{PostgresClaimStore, PostgresFlags};
use bot_core::db::repo::PositionRepo;
use bot_core::db::Database;
use bot_core::models::{ExecutionMode, Position, TradeSource, Venue};
use bot_core::ownership::{
    ClaimOutcome, OwnershipRegistry, RuntimeFlagsReader, RuntimeFlagsWriter,
};
use bot_core::redis_kv::RedisKv;
use bot_core::redis_ownership::RedisClaimStore;
use bot_core::state::Shared;

struct ReplicaCtx {
    #[allow(dead_code)] // identity kept for debugging output
    id: String,
    db: Arc<Database>,
    #[allow(dead_code)]
    kv: RedisKv,
    state: Shared,
    registry: OwnershipRegistry,
    flags: Arc<PostgresFlags>,
}

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

async fn build_ctx(id: &str) -> Option<ReplicaCtx> {
    let pg_url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let redis_url = std::env::var("REDIS_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())?;

    let db_cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&db_cfg, &pg_url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    let db = Arc::new(db);

    let redis_cfg = RedisConfig {
        enabled: true,
        connect_timeout_ms: 2_000,
        operation_timeout_ms: 1_000,
        ..Default::default()
    };
    let kv = RedisKv::connect(&redis_cfg, &redis_url)
        .await
        .expect("configured redis must connect");

    let mut cfg = Config::default();
    cfg.ha.replica_id = id.to_string();
    let state = bot_core::state::AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    });

    let registry = OwnershipRegistry::new(
        Arc::new(PostgresClaimStore::new(db.clone())),
        state.replica_id(),
        Duration::from_secs(30),
        Duration::from_secs(900),
    );
    let flags = Arc::new(PostgresFlags::new(db.clone()));
    state.attach_flags_writer(flags.clone());

    Some(ReplicaCtx {
        id: id.to_string(),
        db,
        kv,
        state,
        registry,
        flags,
    })
}

async fn two_contexts() -> Option<(ReplicaCtx, ReplicaCtx)> {
    if std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .is_none()
        || std::env::var("REDIS_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
    {
        eprintln!("POSTGRES_URL/REDIS_URL not both set — skipping distributed integration tests");
        return None;
    }
    let tag = run_id();
    let a = build_ctx(&format!("ctx-A-{tag}")).await?;
    let b = build_ctx(&format!("ctx-B-{tag}")).await?;
    Some((a, b))
}

/// §X/§D: concurrent claims on the authoritative Postgres store from two
/// fully independent contexts → exactly one active owner.
#[tokio::test]
async fn two_contexts_pg_claim_race_single_owner() {
    let Some((a, b)) = two_contexts().await else {
        return;
    };
    let id = format!("snipe:{}", run_id());
    let (ra, rb) = tokio::join!(
        a.registry.claim(&id, "entry", "sniper", "pump", "S"),
        b.registry.claim(&id, "entry", "sniper", "pump", "S")
    );
    let owned = matches!(ra.unwrap(), ClaimOutcome::Owned(_)) as u8
        + matches!(rb.unwrap(), ClaimOutcome::Owned(_)) as u8;
    assert_eq!(owned, 1, "exactly one context may own the execution");
}

/// §X/§D: the same race on the Redis claim store (separate connections,
/// Lua-atomic) → exactly one active owner.
#[tokio::test]
async fn two_contexts_redis_claim_race_single_owner() {
    let Some((a, b)) = two_contexts().await else {
        return;
    };
    let ra = OwnershipRegistry::new(
        Arc::new(RedisClaimStore::new(a.kv.clone())),
        a.state.replica_id(),
        Duration::from_secs(30),
        Duration::from_secs(900),
    );
    let rb = OwnershipRegistry::new(
        Arc::new(RedisClaimStore::new(b.kv.clone())),
        b.state.replica_id(),
        Duration::from_secs(30),
        Duration::from_secs(900),
    );
    let id = format!("snipe:redis:{}", run_id());
    let (oa, ob) = tokio::join!(
        ra.claim(&id, "entry", "sniper", "pump", "S"),
        rb.claim(&id, "entry", "sniper", "pump", "S")
    );
    let owned = matches!(oa.unwrap(), ClaimOutcome::Owned(_)) as u8
        + matches!(ob.unwrap(), ClaimOutcome::Owned(_)) as u8;
    assert_eq!(owned, 1);
}

/// §X/§Q: replica A engages the kill switch → the shared runtime-flag store
/// → replica B's sync converges and B's money path is blocked. Releasing on
/// A converges B back (B never touched the flag locally, so the shared row
/// is always current for B).
#[tokio::test]
async fn two_contexts_kill_switch_propagates() {
    let Some((a, b)) = two_contexts().await else {
        return;
    };
    assert!(!a.state.kill_switch() && !b.state.kill_switch());

    a.state.set_kill_switch(true, "distributed test").await;
    let rows = RuntimeFlagsReader::read_all(&*b.flags).await.unwrap();
    let report = b.state.apply_flag_sync(&rows).await;
    assert!(report.kill_changed, "B must converge onto A's kill");
    assert!(b.state.kill_switch());
    assert!(
        b.state.may_broadcast().await.is_err(),
        "B's broadcast gate must be closed while the kill is engaged"
    );

    // Module flags ride the same channel.
    a.state
        .set_enabled(bot_core::models::BotModule::Sniper, true)
        .await;
    let rows = RuntimeFlagsReader::read_all(&*b.flags).await.unwrap();
    b.state.apply_flag_sync(&rows).await;
    assert!(
        b.state
            .is_enabled(bot_core::models::BotModule::Sniper)
            .await
    );

    // Release on A → B converges back.
    a.state
        .set_kill_switch(false, "distributed test over")
        .await;
    let rows = RuntimeFlagsReader::read_all(&*b.flags).await.unwrap();
    let report = b.state.apply_flag_sync(&rows).await;
    assert!(report.kill_changed);
    assert!(!b.state.kill_switch());

    // Leave the shared table in a clean state for other runs.
    RuntimeFlagsWriter::write(&*a.flags, "kill_switch", false, "cleanup", "test").await;
}

/// §X/§Q: the position written into the shared DB by replica A converges
/// onto replica B's book (book sync), and both contexts then compete for the
/// SAME exit-claim identity derived from that position id.
#[tokio::test]
async fn two_contexts_book_sync_and_exit_claim_identity() {
    let Some((a, b)) = two_contexts().await else {
        return;
    };
    let pos_id = format!("p-{}", run_id());
    let mint = format!("MINT{}", run_id());

    let mut pos = Position::new(
        pos_id.clone(),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Paper,
        mint.clone(),
        mint.clone(),
        "SOL".to_string(),
    );
    pos.qty = 5.0;
    pos.avg_entry = 1.0;
    pos.last_mark = 1.1;

    // Replica A persists the position (its normal durable write path).
    PositionRepo::new(a.db.clone())
        .upsert(&pos)
        .await
        .expect("A upserts");

    // Replica B runs one book-sync pass: list_open + merge.
    let rows = PositionRepo::new(b.db.clone())
        .list_open()
        .await
        .expect("B lists open positions");
    let (inserted, _) = b.state.merge_positions(rows).await;
    assert!(inserted >= 1);
    let local = b
        .state
        .position(&pos_id)
        .await
        .expect("position converged onto B's book");
    assert_eq!(local.qty, 5.0);

    // Both contexts now derive the SAME logical exit identity from the
    // converged position id — and exactly one may execute the exit.
    let exit_id = format!("exit:{pos_id}:stop_loss");
    let (ra, rb) = tokio::join!(
        a.registry
            .claim(&exit_id, "exit", "sniper", "stop_loss", &mint),
        b.registry
            .claim(&exit_id, "exit", "sniper", "stop_loss", &mint)
    );
    let owned = matches!(ra.unwrap(), ClaimOutcome::Owned(_)) as u8
        + matches!(rb.unwrap(), ClaimOutcome::Owned(_)) as u8;
    assert_eq!(owned, 1, "exactly one replica may sell the position");

    // Cleanup: close the position row so later runs see a tidy book.
    pos.status = bot_core::models::PositionStatus::Closed;
    pos.qty = 0.0;
    pos.closed_at = Some(chrono::Utc::now());
    let _ = PositionRepo::new(a.db.clone()).upsert(&pos).await;
}
