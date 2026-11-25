//! Claim-store stress property (P0): N replicas × M rounds against a REAL
//! PostgreSQL claim store.
//!
//! The single-round module test (`two_replica_mirror.rs`) proves the pipeline
//! elects one executor once. This file attacks the store directly, at the
//! layer where a lost update would be invisible from above:
//!
//! * **exactly one winner per event** — `EXACTLY_ONE_WINNER` rounds, each
//!   contested by N replicas released by the same barrier;
//! * **no simultaneous second claim** — winner count and acquisition lineage
//!   are checked in the store-level test; a separate test below also counts
//!   mock sendTransaction calls using independent in-process replica states;
//! * **lease recovery after simulated crash** — after an owner stops renewing
//!   and releasing, a competing replica must
//!   NOT be able to acquire before the lease lapses (`NO_LEAK`), and MUST be
//!   able to after it lapses (takeover, epoch+1, `previous_owner` recorded).
//!   A separate subprocess test exercises abrupt exit and a fresh process.
//!   Neither establishes multi-process live broadcast or reconciliation safety.
//!
//! GATED: skipped unless `POSTGRES_URL` points at a real database. Run with
//! `--test-threads=1`. Every claim id is namespaced by process id + nanosecond
//! stamp, so reruns against a shared database cannot collide with old rows.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Barrier;

use bot_core::config::DatabaseConfig;
use bot_core::db::claims::PostgresClaimStore;
use bot_core::db::Database;
use bot_core::ownership::{ClaimDecision, ClaimRequest, ClaimStatus, ClaimStore, StoreContext};

/// Replicas contending for the same execution id in each round.
const REPLICAS: usize = 8;
/// Rounds of the one-winner property.
const ROUNDS: usize = 25;
/// Rounds of the crash/restart (no-leak) property.
const CRASH_ROUNDS: usize = 10;

const SKIP_MESSAGE: &str =
    "SKIP: POSTGRES_URL is not set — the claim stress property needs a real PostgreSQL 16 \
     instance. It is NOT reported as a pass.";

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

async fn store() -> Option<(PostgresClaimStore, Arc<Database>)> {
    let url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    let db = Arc::new(db);
    Some((PostgresClaimStore::new(db.clone()), db))
}

fn request(execution_id: &str, module: &str) -> ClaimRequest {
    ClaimRequest {
        execution_id: execution_id.to_string(),
        kind: "entry".into(),
        module: module.into(),
        strategy: "stress".into(),
        symbol: "STRESS".into(),
    }
}

fn ctx(owner: String, lease: Duration, grace: Duration) -> StoreContext {
    StoreContext {
        owner_id: owner,
        lease,
        handoff_grace: grace,
    }
}

/// Extracts the winner, or `None` for a deterministic loser.
fn winner_of(decision: ClaimDecision) -> Option<bot_core::ownership::ExecutionClaim> {
    match decision {
        ClaimDecision::Acquired(claim) => Some(claim),
        ClaimDecision::Rejected { .. } => None,
    }
}

#[tokio::test]
async fn different_copy_exit_paths_contend_on_one_position_claim() {
    let Some((store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let store = Arc::new(store);
    let position = format!("position-{}", run_id());
    let execution_id = module_copy::intent::exit_claim_id(&position);
    let paths = ["mirror_exit", "recon_exit", "stop_loss", "take_profit"];
    let barrier = Arc::new(Barrier::new(paths.len()));
    let mut tasks = Vec::new();
    for path in paths {
        let store = store.clone();
        let barrier = barrier.clone();
        let execution_id = execution_id.clone();
        tasks.push(tokio::spawn(async move {
            let req = ClaimRequest {
                execution_id,
                kind: "exit".into(),
                module: "copy".into(),
                strategy: path.into(),
                symbol: "mint".into(),
            };
            let ctx = ctx(
                format!("{path}-{}", run_id()),
                Duration::from_secs(30),
                Duration::from_secs(900),
            );
            barrier.wait().await;
            store.claim(&req, &ctx).await.expect("postgres claim")
        }));
    }
    let mut winners = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), ClaimDecision::Acquired(_)) {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exit strategies must contend on the position, not rule"
    );
}

#[tokio::test]
async fn n_replicas_same_event_exactly_one_winner() {
    let Some((store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let store = Arc::new(store);
    let tag = run_id();
    let lease = Duration::from_secs(30);
    let grace = Duration::from_secs(900);

    for round in 0..ROUNDS {
        let execution_id = format!("stress:one-winner:{tag}:{round}");
        // A barrier (not a sleep): every replica is parked at the same line
        // and released together; actual database scheduling may still serialize.
        let barrier = Arc::new(Barrier::new(REPLICAS));
        let mut tasks = Vec::with_capacity(REPLICAS);
        for replica in 0..REPLICAS {
            let store = store.clone();
            let barrier = barrier.clone();
            let execution_id = execution_id.clone();
            let owner = format!("rep-{replica}-{tag}");
            let ctx = ctx(owner, lease, grace);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let decision = store.claim(&request(&execution_id, "copy"), &ctx).await;
                (ctx.owner_id.clone(), decision)
            }));
        }

        let mut winners = Vec::new();
        let mut losers = Vec::new();
        for task in tasks {
            let (owner, decision) = task.await.expect("claim task must not panic");
            match winner_of(decision.expect("the claim store must be reachable")) {
                Some(claim) => {
                    assert_eq!(
                        claim.owner_id, owner,
                        "round {round}: the claim must be acquired by its own replica"
                    );
                    assert_eq!(
                        claim.epoch, 1,
                        "round {round}: first generation must be epoch 1"
                    );
                    assert_eq!(
                        claim.takeover_count, 0,
                        "round {round}: a takeover in a fresh round means a stale row leaked"
                    );
                    assert!(
                        claim.previous_owner.is_none(),
                        "round {round}: a fresh claim has no previous owner"
                    );
                    winners.push(claim);
                }
                None => losers.push(owner),
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "round {round}: exactly one replica may own the claim, got {} winners \
             (losers: {:?}) — more than one winner is a DOUBLE-MIRROR",
            winners.len(),
            losers
        );
        assert_eq!(losers.len(), REPLICAS - 1);

        // Zero double-mirror, observed from the durable side: the lineage of
        // this execution id holds exactly ONE acquisition. A second
        // `acquired`/`takeover` row would mean a second replica got a claim
        // generation of the same trade.
        let events = store
            .events(&execution_id)
            .await
            .expect("claim lineage must be readable");
        let acquisitions = events
            .iter()
            .filter(|e| e.event == "acquired" || e.event == "takeover")
            .count();
        assert_eq!(
            acquisitions, 1,
            "round {round}: exactly one acquisition may be journaled, events={events:?}"
        );
        assert_eq!(
            events.len(),
            1,
            "round {round}: one row — the winner's claim"
        );

        // The row names the winner and is still actively held (no release).
        let rec = store
            .get(&execution_id)
            .await
            .expect("claim store reachable")
            .expect("claim row must exist");
        assert_eq!(rec.owner_id, winners[0].owner_id);
        assert_eq!(rec.status, ClaimStatus::Claimed);
    }
}

#[tokio::test]
async fn crashed_owner_leaks_nothing_and_expiry_allows_exactly_one_takeover() {
    let Some((store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let store = Arc::new(store);
    let tag = run_id();
    // Short lease so the "after expiry" half is a real wall-clock lapse, not a
    // fake clock: the store's expiry is Postgres `now()`, which a test cannot
    // advance. 1 s is the store's documented minimum resolution.
    let lease = Duration::from_millis(1000);
    let grace = Duration::from_secs(60);

    for round in 0..CRASH_ROUNDS {
        let execution_id = format!("stress:crash:{tag}:{round}");

        // 1. One owner acquires, then CRASHES: it never renews, never releases.
        let dead = ctx(format!("dead-{round}-{tag}"), lease, grace);
        let acquired = winner_of(
            store
                .claim(&request(&execution_id, "copy"), &dead)
                .await
                .expect("claim store reachable"),
        )
        .expect("the first claim on a fresh id must be acquired");
        assert_eq!(acquired.owner_id, dead.owner_id);

        // 2. A restarting replica must NOT be able to take it over while the
        //    dead owner's lease is still running (that would be a leak: two
        //    executors, one trade).
        let fresh = ctx(format!("restart-{round}-{tag}"), lease, grace);
        let blocked = store
            .claim(&request(&execution_id, "copy"), &fresh)
            .await
            .expect("claim store reachable");
        match blocked {
            ClaimDecision::Rejected {
                owner_id, status, ..
            } => {
                assert_eq!(owner_id, dead.owner_id, "the dead owner still holds it");
                assert_eq!(status, ClaimStatus::Claimed);
            }
            ClaimDecision::Acquired(claim) => panic!(
                "LEAKED CLAIM: replica {} acquired {execution_id} while the crashed owner's \
                 lease was still live (epoch {})",
                claim.owner_id, claim.epoch
            ),
        }

        // 3. After the lease lapses exactly one replica wins the takeover, the
        //    epoch advances and the crashed owner is recorded — no row is
        //    deleted, so the audit trail survives the crash.
        tokio::time::sleep(lease + Duration::from_millis(250)).await;
        let barrier = Arc::new(Barrier::new(REPLICAS));
        let mut tasks = Vec::with_capacity(REPLICAS);
        for replica in 0..REPLICAS {
            let store = store.clone();
            let barrier = barrier.clone();
            let execution_id = execution_id.clone();
            let c = ctx(format!("takeover-{replica}-{round}-{tag}"), lease, grace);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                store.claim(&request(&execution_id, "copy"), &c).await
            }));
        }
        let mut winners = Vec::new();
        for task in tasks {
            if let Some(claim) = winner_of(
                task.await
                    .expect("claim task must not panic")
                    .expect("store reachable"),
            ) {
                winners.push(claim);
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "round {round}: after expiry exactly one takeover may win, got {}",
            winners.len()
        );
        let takeover = &winners[0];
        assert_eq!(
            takeover.epoch, 2,
            "round {round}: the takeover is generation 2"
        );
        assert_eq!(
            takeover.takeover_count, 1,
            "round {round}: exactly one expiry-driven takeover"
        );
        assert_eq!(
            takeover.previous_owner.as_deref(),
            Some(dead.owner_id.as_str()),
            "round {round}: the crashed owner must be recorded on the new generation"
        );
        assert_eq!(takeover.status, ClaimStatus::Claimed);
    }
}

#[tokio::test]
async fn released_claim_is_reacquirable_and_fenced_for_the_old_owner() {
    let Some((store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let tag = run_id();
    let execution_id = format!("stress:release:{tag}");
    let lease = Duration::from_secs(30);
    let grace = Duration::from_secs(900);

    let first = ctx(format!("first-{tag}"), lease, grace);
    let claim = winner_of(
        store
            .claim(&request(&execution_id, "copy"), &first)
            .await
            .expect("store reachable"),
    )
    .expect("fresh claim must be acquired");

    // The old owner is fenced the moment it releases: its verify/renew fail.
    assert!(
        store
            .verify(&execution_id, &first.owner_id, claim.epoch)
            .await
            .expect("verify reachable"),
        "the active owner passes fencing"
    );
    assert!(
        store
            .release(
                &execution_id,
                &first.owner_id,
                claim.epoch,
                ClaimStatus::Released
            )
            .await
            .expect("release reachable"),
        "the active owner may release"
    );
    assert!(
        !store
            .verify(&execution_id, &first.owner_id, claim.epoch)
            .await
            .expect("verify reachable"),
        "a released claim must not fence as owned"
    );
    assert!(
        !store
            .renew(&execution_id, &first.owner_id, claim.epoch)
            .await
            .expect("renew reachable"),
        "a released claim must not be renewable"
    );

    // Released is immediately re-acquirable — and the generation advances, so
    // a late message from the previous owner cannot pass any CAS.
    let second = ctx(format!("second-{tag}"), lease, grace);
    let next = winner_of(
        store
            .claim(&request(&execution_id, "copy"), &second)
            .await
            .expect("store reachable"),
    )
    .expect("a released claim must be re-acquirable");
    assert_eq!(next.epoch, claim.epoch + 1);
    assert_eq!(next.owner_id, second.owner_id);
    assert_eq!(
        next.takeover_count, 0,
        "re-acquisition after a clean release is not a takeover"
    );
    assert_eq!(
        next.previous_owner.as_deref(),
        Some(first.owner_id.as_str())
    );

    // Fencing is epoch-based: the old generation stays dead.
    assert!(
        !store
            .verify(&execution_id, &first.owner_id, claim.epoch)
            .await
            .expect("verify reachable"),
        "the previous owner's epoch must stay fenced"
    );
    assert!(store
        .verify(&execution_id, &second.owner_id, next.epoch)
        .await
        .expect("verify reachable"));

    let lineage = store.events(&execution_id).await.expect("lineage readable");
    // Only lifecycle transitions order the generations; the store also
    // appends diagnostic rows (`fenced`, `renew_rejected`) that must NOT be
    // read as ownership generations.
    let lifecycle: Vec<&str> = lineage
        .iter()
        .map(|e| e.event.as_str())
        .filter(|k| {
            matches!(
                *k,
                "acquired" | "released" | "reacquired" | "takeover" | "handed_off"
            )
        })
        .collect();
    assert_eq!(
        lifecycle,
        vec!["acquired", "released", "reacquired"],
        "the lineage must show the release and the re-acquisition in order"
    );
    // The refusals are recorded too — the audit trail explains WHY the old
    // owner lost, it does not merely omit it.
    let diagnostics: Vec<&str> = lineage
        .iter()
        .map(|e| e.event.as_str())
        .filter(|k| matches!(*k, "fenced" | "renew_rejected"))
        .collect();
    assert!(
        diagnostics.len() >= 2,
        "the release must leave fenced/renew_rejected evidence, got {diagnostics:?}"
    );
    assert_eq!(
        lineage
            .iter()
            .filter(|e| e.event == "acquired" || e.event == "takeover")
            .count(),
        1,
        "exactly one acquisition generation before the release"
    );
}

// Invoked as a subprocess by the test below; in ordinary test runs it is inert.
// Exit without unwinding after acquiring, so no graceful lease release runs.
#[tokio::test]
async fn claim_crash_worker() {
    let Ok(execution_id) = std::env::var("CLAIM_STRESS_CHILD_ID") else {
        return;
    };
    let owner = std::env::var("CLAIM_STRESS_CHILD_OWNER").expect("child owner");
    let (store, _) = store().await.expect("child requires POSTGRES_URL");
    let decision = store
        .claim(
            &request(&execution_id, "copy"),
            &ctx(owner, Duration::from_secs(10), Duration::from_secs(60)),
        )
        .await
        .expect("child claim");
    match decision {
        ClaimDecision::Acquired(c) => println!("CHILD_ACQUIRED:{}", c.epoch),
        ClaimDecision::Rejected { .. } => println!("CHILD_REJECTED"),
    }
    use std::io::Write;
    std::io::stdout().flush().expect("flush child result");
    std::process::exit(0);
}

fn run_claim_child(execution_id: &str, owner: &str) -> String {
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "claim_crash_worker", "--nocapture"])
        .env("CLAIM_STRESS_CHILD_ID", execution_id)
        .env("CLAIM_STRESS_CHILD_OWNER", owner)
        .output()
        .expect("start fresh replica process");
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("child output is UTF-8")
}

#[tokio::test]
async fn actual_process_exit_and_restart_recovers_expired_lease() {
    let Some((store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let id = format!("stress:process-restart:{}", run_id());
    let first = run_claim_child(&id, "crashed-process");
    assert!(first.contains("CHILD_ACQUIRED:1"), "{first}");
    // A fresh process cannot take over the first process's unexpired lease.
    let blocked = run_claim_child(&id, "restarted-process");
    assert!(blocked.contains("CHILD_REJECTED"), "{blocked}");

    // Let the database lease expire naturally before starting the new owner.
    tokio::time::sleep(Duration::from_secs(11)).await;
    let restarted = run_claim_child(&id, "restarted-process");
    assert!(restarted.contains("CHILD_ACQUIRED:2"), "{restarted}");
    let row = store
        .get(&id)
        .await
        .expect("claim readable")
        .expect("claim exists");
    assert_eq!(row.previous_owner.as_deref(), Some("crashed-process"));
    assert_eq!(row.takeover_count, 1);
    assert!(!store
        .verify(&id, "crashed-process", 1)
        .await
        .expect("old owner fenced"));
    assert!(store
        .release(&id, "restarted-process", 2, ClaimStatus::Released)
        .await
        .expect("release"));
}

#[tokio::test]
async fn expired_owner_can_release_only_before_a_takeover() {
    let Some((store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let id = format!("stress:late-release:{}", run_id());
    let old = ctx(
        "old-owner".into(),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );
    let first = winner_of(
        store
            .claim(&request(&id, "copy"), &old)
            .await
            .expect("claim"),
    )
    .expect("fresh claim");
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(!store
        .verify(&id, &old.owner_id, first.epoch)
        .await
        .expect("verify"));
    assert!(
        store
            .release(&id, &old.owner_id, first.epoch, ClaimStatus::Released)
            .await
            .expect("release"),
        "release has no lease-expiry predicate if the epoch is unchanged"
    );

    let next = ctx(
        "new-owner".into(),
        Duration::from_secs(10),
        Duration::from_secs(60),
    );
    let second = winner_of(
        store
            .claim(&request(&id, "copy"), &next)
            .await
            .expect("claim"),
    )
    .expect("released row can be reacquired");
    assert!(
        !store
            .release(&id, &old.owner_id, first.epoch, ClaimStatus::Released)
            .await
            .expect("release"),
        "the old generation is fenced after reacquisition"
    );
    assert!(store
        .release(&id, &next.owner_id, second.epoch, ClaimStatus::Released)
        .await
        .expect("release"));
}

// End-to-end coverage with separate in-process replica states, a real Postgres
// dedup/ownership backend and the JSON-RPC mock venue. The execution ledger
// remains process-global in this harness, so this is not a multi-process proof.
mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_replicas_send_exactly_one_mock_transaction_per_event() {
    use bot_core::dedup::{DedupBackend, DedupStore};
    use bot_core::models::BotModule;
    use bot_core::ownership::OwnershipRegistry;
    use common::*;
    use module_copy::event::{CopyStage, EventSource, RejectReason};
    use module_copy::CopyBot;
    use solana_kit::tokens::Wallet;
    use solana_sdk::pubkey::Pubkey;
    use std::sync::atomic::Ordering;

    let Some((_store, db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    const PIPELINE_ROUNDS: usize = 5;
    let node = Arc::new(MockNode::default());
    let url = spawn_mock_node(node.clone()).await;
    let wallet = Arc::new(Wallet::generate());
    let tag = run_id();
    for round in 0..PIPELINE_ROUNDS {
        let mint = Pubkey::new_unique();
        install_pump_token(&node, mint, Pubkey::new_unique(), CurveSpec::fresh());
        let mut trade = leader_buy(LEADER, mint, (round + 1) as u8, 0.5);
        // IDs must remain unique across reruns against the same PostgreSQL.
        trade.signature = format!("stress-pipeline-{tag}-{round}");
        let event = event(&trade, EventSource::PumpPortal, 1);
        let barrier = Arc::new(Barrier::new(REPLICAS));
        let mut handles = Vec::new();
        for i in 0..REPLICAS {
            let mut cfg = live_copy_config();
            cfg.ha.mode = "active_active".into();
            cfg.ha.replica_id = format!("pipeline-{tag}-{round}-{i}");
            let state = state_with(cfg.clone());
            state.set_balances(Some(10.0), Some(1_000.0)).await;
            state.attach_dedup(DedupStore::new(
                DedupBackend::Postgres,
                None,
                Some(db.clone()),
                Duration::from_secs(3600),
                1000,
            ));
            let registry = OwnershipRegistry::new(
                Arc::new(PostgresClaimStore::new(db.clone())),
                cfg.ha.replica_id.clone(),
                Duration::from_secs(60),
                Duration::from_secs(900),
            );
            let mut bot = CopyBot::new(state.clone(), mock_rpc(&url), wallet.clone(), None)
                .await
                .with_ownership(Arc::new(registry));
            let barrier = barrier.clone();
            let event = event.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let outcome = bot.process_event(&event, &cfg).await;
                let positions = state.open_positions_for(BotModule::Copy).await.len();
                (outcome, positions)
            }));
        }
        let mut filled = 0;
        let mut duplicate = 0;
        let mut positions = 0;
        for handle in handles {
            let (outcome, count) = handle.await.expect("replica task");
            positions += count;
            if outcome.stage == CopyStage::Filled {
                filled += 1;
            } else if outcome.rejection.as_ref().map(|r| r.reason)
                == Some(RejectReason::DuplicateEvent)
            {
                duplicate += 1;
            } else {
                panic!("round {round}: unexpected outcome {outcome:?}");
            }
        }
        assert_eq!(filled, 1, "round {round}: exactly one filled mirror");
        assert_eq!(
            duplicate,
            REPLICAS - 1,
            "round {round}: remaining events deduplicated"
        );
        assert_eq!(positions, 1, "round {round}: exactly one booked position");
        assert_eq!(
            node.sends.load(Ordering::SeqCst),
            round + 1,
            "round {round}: exactly one mock sendTransaction per event"
        );
    }
}

/// Child entry point for the multi-process mock-venue test. In ordinary test
/// discovery it is inert; the parent supplies these test-only variables.
#[tokio::test]
async fn subprocess_broadcast_worker() {
    use bot_core::dedup::{DedupBackend, DedupStore};
    use bot_core::ownership::OwnershipRegistry;
    use module_copy::event::{CopyStage, EventSource, RejectReason};
    use module_copy::CopyBot;
    use solana_kit::tokens::Wallet;
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let Ok(mint) = std::env::var("CLAIM_PROCESS_MINT") else {
        return;
    };
    let mint = Pubkey::from_str(&mint).expect("child mint");
    let signature = std::env::var("CLAIM_PROCESS_SIGNATURE").expect("child signature");
    let url = std::env::var("CLAIM_PROCESS_RPC").expect("child mock URL");
    let gate = std::env::var("CLAIM_PROCESS_GATE").expect("child barrier address");
    let replica = std::env::var("CLAIM_PROCESS_REPLICA").expect("child replica");
    let wallet = Arc::new(
        Wallet::load(&std::env::var("CLAIM_PROCESS_WALLET").expect("test key"))
            .expect("test wallet"),
    );
    let (_store, db) = store().await.expect("PostgreSQL required for subprocess");
    let mut cfg = common::live_copy_config();
    cfg.ha.mode = "active_active".into();
    cfg.ha.replica_id = replica;
    let state = common::state_with(cfg.clone());
    state.set_balances(Some(10.0), Some(1_000.0)).await;
    state.attach_dedup(DedupStore::new(
        DedupBackend::Postgres,
        None,
        Some(db.clone()),
        Duration::from_secs(3600),
        1000,
    ));
    let ownership = OwnershipRegistry::new(
        Arc::new(PostgresClaimStore::new(db)),
        cfg.ha.replica_id.clone(),
        Duration::from_secs(60),
        Duration::from_secs(900),
    );
    let mut bot = CopyBot::new(state, common::mock_rpc(&url), wallet, None)
        .await
        .with_ownership(Arc::new(ownership));
    let mut trade = common::leader_buy(common::LEADER, mint, 1, 0.5);
    trade.signature = signature;
    let event = common::event(&trade, EventSource::PumpPortal, 1);
    // The TCP gate synchronizes independent processes, not just Rust tasks.
    let mut socket = tokio::net::TcpStream::connect(gate)
        .await
        .expect("connect process barrier");
    socket.write_all(b"R").await.expect("announce readiness");
    let mut release = [0u8; 1];
    socket
        .read_exact(&mut release)
        .await
        .expect("barrier release");
    let result = bot.process_event(&event, &cfg).await;
    if result.stage == CopyStage::Filled {
        println!("CHILD_PIPELINE_FILLED");
    } else if result.rejection.as_ref().map(|r| r.reason) == Some(RejectReason::DuplicateEvent) {
        println!("CHILD_PIPELINE_DUPLICATE");
    } else {
        panic!("unexpected child outcome: {result:?}");
    }
}

/// Four independently executed processes share PostgreSQL and a mock RPC node.
/// This verifies actual mock sends, not an in-process ledger or real-chain fills.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_processes_send_one_mock_transaction_per_event() {
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signature::Keypair;
    use std::sync::atomic::Ordering;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;

    let Some((_store, _db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    const PROCESS_REPLICAS: usize = 4;
    const PROCESS_ROUNDS: usize = 3;
    let node = Arc::new(common::MockNode::default());
    let url = common::spawn_mock_node(node.clone()).await;
    let wallet_spec = bs58::encode(Keypair::new().to_bytes()).into_string();
    let tag = run_id();
    for round in 0..PROCESS_ROUNDS {
        let mint = Pubkey::new_unique();
        common::install_pump_token(
            &node,
            mint,
            Pubkey::new_unique(),
            common::CurveSpec::fresh(),
        );
        let signature = format!("process-pipeline-{tag}-{round}");
        let gate = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("barrier bind");
        let address = gate.local_addr().expect("barrier address").to_string();
        let mut children = Vec::new();
        for i in 0..PROCESS_REPLICAS {
            let child = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", "subprocess_broadcast_worker", "--nocapture"])
                .env("CLAIM_PROCESS_MINT", mint.to_string())
                .env("CLAIM_PROCESS_SIGNATURE", &signature)
                .env("CLAIM_PROCESS_RPC", &url)
                .env("CLAIM_PROCESS_GATE", &address)
                .env(
                    "CLAIM_PROCESS_REPLICA",
                    format!("process-{tag}-{round}-{i}"),
                )
                .env("CLAIM_PROCESS_WALLET", &wallet_spec)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("spawn replica process");
            children.push(child);
        }
        // All children have initialized independently before any is released.
        let mut sockets = Vec::new();
        for _ in 0..PROCESS_REPLICAS {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(60), gate.accept())
                .await
                .expect("process readiness guard")
                .expect("accept child");
            let mut ready = [0u8; 1];
            tokio::time::timeout(Duration::from_secs(60), socket.read_exact(&mut ready))
                .await
                .expect("readiness guard")
                .expect("child ready");
            assert_eq!(ready, *b"R");
            sockets.push(socket);
        }
        for socket in &mut sockets {
            socket.write_all(b"G").await.expect("release child");
        }
        let mut filled = 0;
        let mut duplicates = 0;
        for child in children {
            let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
                .await
                .expect("child completion guard")
                .expect("child wait");
            assert!(
                output.status.success(),
                "child failure: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).expect("child stdout");
            if text.contains("CHILD_PIPELINE_FILLED") {
                filled += 1;
            } else if text.contains("CHILD_PIPELINE_DUPLICATE") {
                duplicates += 1;
            } else {
                panic!("child did not report an outcome: {text}");
            }
        }
        assert_eq!(filled, 1, "round {round}: exactly one filled replica");
        assert_eq!(
            duplicates,
            PROCESS_REPLICAS - 1,
            "round {round}: other replicas deduplicate"
        );
        assert_eq!(
            node.sends.load(Ordering::SeqCst),
            round + 1,
            "round {round}: one mock broadcast per event"
        );
    }
}

/// No live/simulated clustered replica may use a process-local verdict if
/// shared dedup is missing or unavailable, even with a healthy claim store.
#[tokio::test]
async fn clustered_copy_fails_closed_without_shared_dedup() {
    use bot_core::dedup::{DedupBackend, DedupStore};
    use bot_core::models::ExecutionMode;
    use bot_core::ownership::OwnershipRegistry;
    use module_copy::event::{CopyOutcome, CopyStage, EventSource, RejectReason};
    use module_copy::recovery::{CopyStore, MemoryCopyStore};
    use module_copy::CopyBot;
    use solana_kit::tokens::Wallet;
    use solana_sdk::pubkey::Pubkey;
    use std::sync::atomic::Ordering;

    let Some((_store, claim_db)) = store().await else {
        eprintln!("{SKIP_MESSAGE}");
        return;
    };
    let url = std::env::var("POSTGRES_URL").expect("configured PostgreSQL");
    let config = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let failed_db = Arc::new(Database::connect(&config, &url).await.expect("dedup pool"));
    failed_db.close().await;
    let node = Arc::new(common::MockNode::default());
    let rpc = common::spawn_mock_node(node.clone()).await;
    let tag = run_id();
    for mode in [ExecutionMode::Simulate, ExecutionMode::Live] {
        for backend in ["absent", "memory", "closed-postgres"] {
            let mut cfg = common::live_copy_config();
            cfg.ha.mode = "active_active".into();
            cfg.execution.mode = mode;
            cfg.ha.replica_id = format!("fail-closed-{tag}-{backend}-{}", mode.as_str());
            let state = common::state_with(cfg.clone());
            state.set_balances(Some(10.0), Some(1_000.0)).await;
            match backend {
                "memory" => state.attach_dedup(DedupStore::new(
                    DedupBackend::Memory,
                    None,
                    None,
                    Duration::from_secs(60),
                    1000,
                )),
                "closed-postgres" => state.attach_dedup(DedupStore::new(
                    DedupBackend::Postgres,
                    None,
                    Some(failed_db.clone()),
                    Duration::from_secs(60),
                    1000,
                )),
                _ => {}
            }
            let registry = OwnershipRegistry::new(
                Arc::new(PostgresClaimStore::new(claim_db.clone())),
                &cfg.ha.replica_id,
                Duration::from_secs(60),
                Duration::from_secs(900),
            );
            let journal = Arc::new(MemoryCopyStore::new());
            let mut bot = CopyBot::new(
                state.clone(),
                common::mock_rpc(&rpc),
                Arc::new(Wallet::generate()),
                None,
            )
            .await
            .with_copy_store(journal.clone())
            .with_ownership(Arc::new(registry));
            let mint = Pubkey::new_unique();
            let mut trade = common::leader_buy(common::LEADER, mint, 1, 0.5);
            trade.signature = format!("fail-closed-{tag}-{backend}-{}", mode.as_str());
            let event = common::event(&trade, EventSource::PumpPortal, 1);
            // A different replica may have already finished this event. Its
            // authoritative outcome must not be overwritten by our failure.
            let winner = CopyOutcome {
                event_id: event.event_id.clone(),
                stage: CopyStage::Filled,
                rejection: None,
                intent_id: Some("winner-intent".into()),
                position_id: Some("winner-position".into()),
                requested_sol: None,
                sized_sol: None,
                signature: None,
                total_ms: 0,
            };
            let winner_record = module_copy::mirror::journal_record(&event, &winner);
            assert!(journal.record_event(winner_record.clone()).await);
            let out = bot.process_event(&event, &cfg).await;
            assert_eq!(journal.event(&event.event_id), Some(winner_record));
            trade.signature.push_str("-new");
            let new_event = common::event(&trade, EventSource::PumpPortal, 2);
            let new_out = bot.process_event(&new_event, &cfg).await;
            assert_eq!(
                new_out.rejection.as_ref().map(|r| r.reason),
                Some(RejectReason::DedupUnavailable)
            );
            assert!(
                journal.event(&new_event.event_id).is_none(),
                "unknown decision must not be seeded from journal on recovery"
            );
            assert_eq!(out.stage, CopyStage::Failed, "{backend}, {mode:?}: {out:?}");
            assert_eq!(
                out.rejection.as_ref().map(|r| r.reason),
                Some(RejectReason::DedupUnavailable)
            );
            assert!(
                !state.copy_event_seen(&event.dedup_key()).await,
                "failed shared check must not poison the local dedup cache"
            );
        }
    }
    assert_eq!(
        node.sends.load(Ordering::SeqCst),
        0,
        "no broadcast during dedup failure"
    );
}

#[tokio::test]
async fn shared_redis_dedup_refuses_zero_ttl_and_serializes_first_arrival() {
    use bot_core::config::RedisConfig;
    use bot_core::dedup::{DedupBackend, DedupStore};
    use bot_core::redis_kv::RedisKv;
    let Ok(url) = std::env::var("REDIS_URL") else {
        eprintln!(
            "SKIP: REDIS_URL is not set — shared Redis dedup test requires a real Redis service"
        );
        return;
    };
    let redis = RedisKv::connect(&RedisConfig::default(), &url)
        .await
        .expect("Redis reachable");
    let zero = DedupStore::new(
        DedupBackend::Redis,
        Some(redis.clone()),
        None,
        Duration::ZERO,
        64,
    );
    assert!(zero
        .mark_shared("copy_event", "zero-ttl-test")
        .await
        .is_err());
    let a = DedupStore::new(
        DedupBackend::Redis,
        Some(redis.clone()),
        None,
        Duration::from_secs(60),
        64,
    );
    let b = DedupStore::new(
        DedupBackend::Redis,
        Some(redis),
        None,
        Duration::from_secs(60),
        64,
    );
    let key = format!("redis-shared-test-{}", run_id());
    let (x, y) = tokio::join!(
        a.mark_shared("copy_event", &key),
        b.mark_shared("copy_event", &key)
    );
    assert_ne!(
        x.expect("first backend call"),
        y.expect("second backend call")
    );
}
