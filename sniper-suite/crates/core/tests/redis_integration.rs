//! Redis integration tests (BUILD PLAN §4-iii).
//!
//! GATED: skipped unless `REDIS_URL` points at a real Redis (CI provides a
//! service container; locally: `docker run -p 6379:6379 redis:7` then
//! `REDIS_URL=redis://127.0.0.1:6379 cargo test -p bot-core --test
//!  redis_integration -- --test-threads=1`).

use std::time::Duration;

use bot_core::config::RedisConfig;
use bot_core::dedup::{DedupBackend, DedupStore};
use bot_core::redis_kv::RedisKv;

fn url() -> Option<String> {
    std::env::var("REDIS_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

fn cfg() -> RedisConfig {
    RedisConfig {
        enabled: true,
        connect_timeout_ms: 2_000,
        operation_timeout_ms: 1_000,
        ..Default::default()
    }
}

async fn setup() -> Option<RedisKv> {
    let Some(url) = url() else {
        eprintln!("REDIS_URL not set — skipping redis integration tests");
        return None;
    };
    Some(
        RedisKv::connect(&cfg(), &url)
            .await
            .expect("configured redis must connect"),
    )
}

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

#[tokio::test]
async fn set_nx_ttl_is_first_arrival_once() {
    let Some(kv) = setup().await else { return };
    let key = format!("test:nx:{}", run_id());
    assert!(kv
        .set_nx_ttl(&key, "1", Duration::from_secs(30))
        .await
        .expect("set"));
    assert!(!kv
        .set_nx_ttl(&key, "1", Duration::from_secs(30))
        .await
        .expect("set2"));
    assert_eq!(kv.get(&key).await.expect("get").as_deref(), Some("1"));
    let ttl = kv.ttl_secs(&key).await.expect("ttl");
    assert!((1..=30).contains(&ttl), "ttl within window: {ttl}");
    kv.del(&key).await.expect("del");
    assert!(kv.get(&key).await.expect("get2").is_none());
}

#[tokio::test]
async fn incr_expire_counts_and_sets_ttl() {
    let Some(kv) = setup().await else { return };
    let key = format!("test:incr:{}", run_id());
    for expected in 1..=3u64 {
        let n = kv
            .incr_expire(&key, Duration::from_secs(30))
            .await
            .expect("incr");
        assert_eq!(n, expected);
    }
    let ttl = kv.ttl_secs(&key).await.expect("ttl");
    assert!(ttl > 0, "expiry set: {ttl}");
    kv.del(&key).await.expect("del");
}

#[tokio::test]
async fn locks_acquire_release_and_are_token_guarded() {
    let Some(kv) = setup().await else { return };
    let key = format!("test:lock:{}", run_id());
    assert!(kv
        .acquire_lock(&key, "token-a", Duration::from_secs(30))
        .await
        .expect("acquire"));
    assert!(
        !kv.acquire_lock(&key, "token-b", Duration::from_secs(30))
            .await
            .expect("acquire2"),
        "held lock cannot be taken"
    );
    // Wrong token cannot release.
    assert!(!kv.release_lock(&key, "token-b").await.expect("rel-b"));
    // Right token can.
    assert!(kv.release_lock(&key, "token-a").await.expect("rel-a"));
    assert!(
        kv.acquire_lock(&key, "token-b", Duration::from_secs(30))
            .await
            .expect("acquire3"),
        "free again"
    );
    kv.del(&key).await.expect("cleanup");
}

#[tokio::test]
async fn dedup_facade_over_redis_matches_memory_semantics() {
    let Some(kv) = setup().await else { return };
    let run = run_id();
    let store = DedupStore::new(
        DedupBackend::Redis,
        Some(kv.clone()),
        None,
        Duration::from_secs(30),
        1024,
    );
    assert_eq!(store.backend(), DedupBackend::Redis);
    assert!(store.mark("itest", &run).await, "first arrival");
    assert!(!store.mark("itest", &run).await, "duplicate");

    // A NEW facade (simulating a restart with an empty L1) still sees the
    // key — this is the whole point of the durable L2.
    let restarted = DedupStore::new(
        DedupBackend::Redis,
        Some(kv.clone()),
        None,
        Duration::from_secs(30),
        1024,
    );
    assert!(
        !restarted.mark("itest", &run).await,
        "restart does NOT reprocess (L2 remembered)"
    );

    restarted.forget("itest", &run).await;
    assert!(
        restarted.mark("itest", &run).await,
        "after forget the key is new again in every layer"
    );
    assert!(
        !store.mark("itest", &run).await,
        "forget cleared the first facade's L1 too (shared L2)"
    );
    kv.del(&format!("dedup:itest:{run}"))
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn open_helper_connects_from_env() {
    // Exercises bot_core::redis_kv::open with the real URL so the startup
    // path (env resolution + connect + ping) is covered, not just connect().
    let Some(url) = url() else {
        eprintln!("REDIS_URL not set — skipping");
        return;
    };
    std::env::set_var("TEST_REDIS_URL_OPEN", &url);
    let mut c = cfg();
    c.url_env = "TEST_REDIS_URL_OPEN".into();
    let opened = bot_core::redis_kv::open(&c).await.expect("open");
    assert!(opened.is_some());
    opened.unwrap().ping().await.expect("ping");
    std::env::remove_var("TEST_REDIS_URL_OPEN");
}

// ---------------------------------------------------------------------------
// Prompt 3 (§D/§U/§W): distributed execution ownership on Redis — Lua CAS
// over `own:claim:{execution_id}` hashes. Used as the claim store only when
// Postgres is absent; the semantics must match the authoritative store.
// ---------------------------------------------------------------------------

use bot_core::ownership::{
    ClaimOutcome, ClaimStatus, OwnershipRegistry, RuntimeFlagsReader, RuntimeFlagsWriter,
};
use bot_core::redis_ownership::{RedisClaimStore, RedisFlags};

fn redis_registry(kv: &RedisKv, replica: &str, lease: Duration) -> OwnershipRegistry {
    OwnershipRegistry::new(
        std::sync::Arc::new(RedisClaimStore::new(kv.clone())),
        replica,
        lease,
        Duration::from_secs(900),
    )
}

/// Two replicas over the SAME Redis: exactly one owner; loser sees the
/// holder (§G). Redis TIME drives all lease math (clock-skew safe).
#[tokio::test]
async fn redis_claim_two_replicas_one_owner() {
    let Some(kv) = setup().await else { return };
    let id = format!("snipe:{}", run_id());
    let a = redis_registry(&kv, "rep-A", Duration::from_secs(30));
    let b = redis_registry(&kv, "rep-B", Duration::from_secs(30));

    match a.claim(&id, "entry", "sniper", "pump", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => {
            assert_eq!(g.epoch(), 1);
            g.fence().await.expect("owner fences");
        }
        _ => panic!("A must acquire"),
    }
    match b.claim(&id, "entry", "sniper", "pump", "S").await.unwrap() {
        ClaimOutcome::OwnedByOther {
            owner_id, status, ..
        } => {
            assert_eq!(owner_id, "rep-A");
            assert_eq!(status, ClaimStatus::Claimed);
        }
        _ => panic!("B must be rejected"),
    }
    // Introspection reads the hash back.
    let rec = a.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.owner_id, "rep-A");
    assert_eq!(rec.kind, "entry");
}

/// Lease expiry → takeover (epoch 2, previous owner recorded); the stale
/// owner is fenced on every continuation (§E/§J).
#[tokio::test]
async fn redis_claim_expiry_takeover_and_fencing() {
    let Some(kv) = setup().await else { return };
    let id = format!("exit:{}", run_id());
    let a = redis_registry(&kv, "rep-A", Duration::from_millis(1100));
    let b = redis_registry(&kv, "rep-B", Duration::from_secs(30));

    let mut ga = match a.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert!(ga.fence().await.is_err(), "expired owner must be fenced");
    assert!(!ga.renew().await.unwrap());

    let mut gb = match b.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!("B must take over"),
    };
    assert_eq!(gb.epoch(), 2);
    assert_eq!(gb.claim().previous_owner.as_deref(), Some("rep-A"));
    assert!(ga.fence().await.is_err());
    assert!(!ga.release().await, "stale owner cannot release");
    assert!(gb.fence().await.is_ok());
    assert!(gb.release().await);
}

/// Release/handoff semantics match Postgres: released is re-acquirable,
/// handed_off blocks during the grace window (§I/§M).
#[tokio::test]
async fn redis_claim_release_and_handoff_grace() {
    let Some(kv) = setup().await else { return };
    let a = OwnershipRegistry::new(
        std::sync::Arc::new(RedisClaimStore::new(kv.clone())),
        "rep-A",
        Duration::from_secs(30),
        Duration::from_millis(1100),
    );
    let b = OwnershipRegistry::new(
        std::sync::Arc::new(RedisClaimStore::new(kv.clone())),
        "rep-B",
        Duration::from_secs(30),
        Duration::from_millis(1100),
    );

    let id = format!("exit:r:{}", run_id());
    let mut g = match a.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    assert!(g.release().await);
    assert!(matches!(
        b.claim(&id, "exit", "sniper", "sl", "S").await.unwrap(),
        ClaimOutcome::Owned(_)
    ));

    let id2 = format!("exit:h:{}", run_id());
    let mut g2 = match a.claim(&id2, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    assert!(g2.hand_off().await);
    match b.claim(&id2, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::OwnedByOther { status, .. } => assert_eq!(status, ClaimStatus::HandedOff),
        _ => panic!("grace must block"),
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(matches!(
        b.claim(&id2, "exit", "sniper", "sl", "S").await.unwrap(),
        ClaimOutcome::Owned(_)
    ));
}

/// Renewal extends the Redis lease past its original expiry (§N).
#[tokio::test]
async fn redis_claim_renew_extends_lease() {
    let Some(kv) = setup().await else { return };
    let id = format!("snipe:renew:{}", run_id());
    let a = redis_registry(&kv, "rep-A", Duration::from_secs(2));
    let b = redis_registry(&kv, "rep-B", Duration::from_secs(2));

    let mut g = match a.claim(&id, "entry", "sniper", "pump", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(g.renew().await.unwrap());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    g.fence().await.expect("renewed lease still fences");
    assert!(matches!(
        b.claim(&id, "entry", "sniper", "pump", "S").await.unwrap(),
        ClaimOutcome::OwnedByOther { .. }
    ));
    assert!(g.release().await);
}

/// Runtime flags over Redis hashes (§Q fallback store).
#[tokio::test]
async fn redis_runtime_flags_roundtrip() {
    let Some(kv) = setup().await else { return };
    let flags = RedisFlags::new(kv.clone());
    let tag = format!("test-{}", run_id());

    RuntimeFlagsWriter::write(&flags, "kill_switch", true, &tag, "rep-A").await;
    RuntimeFlagsWriter::write(&flags, "module:copy", false, &tag, "rep-A").await;

    let rows = RuntimeFlagsReader::read_all(&flags).await.unwrap();
    let kill = rows
        .iter()
        .find(|r| r.flag == "kill_switch")
        .expect("kill row");
    assert!(kill.enabled);
    assert_eq!(kill.updated_by, "rep-A");
    assert!(rows.iter().any(|r| r.flag == "module:copy" && !r.enabled));

    RuntimeFlagsWriter::write(&flags, "kill_switch", false, "released", "rep-B").await;
    let rows = RuntimeFlagsReader::read_all(&flags).await.unwrap();
    let kill = rows.iter().find(|r| r.flag == "kill_switch").unwrap();
    assert!(!kill.enabled);
    assert_eq!(kill.updated_by, "rep-B");

    // Cleanup (flags have no TTL by design).
    kv.del("own:flag:kill_switch").await.unwrap();
    kv.del("own:flag:module:copy").await.unwrap();
}
