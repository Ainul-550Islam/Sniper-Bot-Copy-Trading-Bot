//! Two-replica fault injection over a REAL PostgreSQL deployment shape.
//!
//! Everything else in this repository proves one replica's half of a rule.
//! This test builds the multi-replica arrangement the deployment claims and
//! injects the two faults that actually happen:
//!
//! 1. **Duplicate broadcast attempt.** Two independent processes (two
//!    `DbIntentSink`s with different owner ids, each with its own
//!    deployment-scoped connection pool) race the SAME logical trade. Exactly
//!    one may poll its money-moving future; the loser must see a refusal, not
//!    a sent trade.
//! 2. **Crash between send and link.** Owner A reserves, its "process dies"
//!    (the sink is dropped without link/abandon), the lease expires, and
//!    owner B must then (a) be allowed to take over — the trade is not
//!    stuck forever — and (b) fence owner A's stale epoch out of every
//!    mutation.
//!
//! Both faults are injected in-process against disposable PostgreSQL with
//! real RLS roles. This is **not** an OS-level kill of a live replica, and it
//! is not live-chain evidence: the "broadcast" is an in-process future that
//! records that it was polled.
//!
//! Requires `POSTGRES_URL` (or `SNIPER_RESERVATION_TEST_PG`) pointing at a
//! disposable database whose login can create roles.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bot_core::config::DatabaseConfig;
use bot_core::db::repo::{BroadcastReservationRepo, IntentRecord};
use bot_core::db::Database;
use bot_core::recovery::{with_reserved_intent_disposition, IntentDisposition, IntentSink};
use bot_core::BotResult;
use chrono::Utc;
use sniper_suite::recon::DbIntentSink;
use sqlx::postgres::PgPoolOptions;

fn env_url() -> Option<String> {
    for name in ["SNIPER_RESERVATION_TEST_PG", "POSTGRES_URL"] {
        if let Ok(v) = std::env::var(name) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

struct RlsEnv {
    app_url: String,
    system_url: String,
    migration_url: String,
}

async fn prepare_rls_env(base: &str) -> Option<RlsEnv> {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(base)
        .await
        .ok()?;
    for (role, rls_attr) in [
        ("sniper_app", "NOBYPASSRLS"),
        ("sniper_system", "BYPASSRLS"),
    ] {
        let ddl = format!(
            "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') \
             THEN CREATE ROLE {role} LOGIN PASSWORD 'sniper' {rls_attr}; END IF; END $$;"
        );
        sqlx::query(&ddl).execute(&admin).await.ok()?;
        sqlx::query(&format!("ALTER ROLE {role} PASSWORD 'sniper'"))
            .execute(&admin)
            .await
            .ok()?;
        sqlx::query(&format!("GRANT {role} TO CURRENT_USER"))
            .execute(&admin)
            .await
            .ok()?;
    }
    let rewrite = |user: &str| -> Option<String> {
        let (scheme, rest) = base.split_once("://")?;
        let (_creds, host) = rest.split_once('@')?;
        Some(format!("{scheme}://{user}:sniper@{host}"))
    };
    Some(RlsEnv {
        app_url: rewrite("sniper_app")?,
        system_url: rewrite("sniper_system")?,
        migration_url: base.to_string(),
    })
}

/// One replica: its own env vars, its own pools, its own owner id.
fn replica(env: &RlsEnv, suffix: &str) -> DatabaseConfig {
    std::env::set_var(format!("SNIPER_FAULT_APP_URL_{suffix}"), &env.app_url);
    std::env::set_var(format!("SNIPER_FAULT_SYSTEM_URL_{suffix}"), &env.system_url);
    std::env::set_var(
        format!("SNIPER_FAULT_MIGRATION_URL_{suffix}"),
        &env.migration_url,
    );
    DatabaseConfig {
        enabled: true,
        rls_enabled: true,
        auto_migrate: true,
        required: true,
        url_env: format!("SNIPER_FAULT_APP_URL_{suffix}"),
        system_url_env: format!("SNIPER_FAULT_SYSTEM_URL_{suffix}"),
        migration_url_env: format!("SNIPER_FAULT_MIGRATION_URL_{suffix}"),
        app_login_role: "sniper_app".into(),
        system_login_role: "sniper_system".into(),
        ..Default::default()
    }
}

async fn db_for(cfg: &DatabaseConfig) -> Arc<Database> {
    let url = std::env::var(&cfg.url_env).expect("app url");
    Arc::new(
        Database::connect(cfg, &url)
            .await
            .expect("replica database must connect and migrate"),
    )
}

fn rec(intent_id: &str, trade_key: &str) -> IntentRecord {
    IntentRecord {
        intent_id: intent_id.to_string(),
        module: "fault-injection".into(),
        symbol: trade_key.to_string(),
        wallet: "W".into(),
        side: "buy".into(),
        qty: "1".into(),
        status: "pending".into(),
        signature: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn two_replicas_never_both_broadcast_and_a_crashed_owner_fences_out() {
    let Some(base) = env_url() else {
        eprintln!("NOT_RUN: two_replicas_never_both_broadcast_and_a_crashed_owner_fences_out");
        return;
    };
    let Some(env) = prepare_rls_env(&base).await else {
        eprintln!("NOT_RUN: could not prepare RLS roles");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let cfg_a = replica(&env, &format!("a{}", &suffix[..6]));
    let cfg_b = replica(&env, &format!("b{}", &suffix[..6]));
    let db_a = db_for(&cfg_a).await;
    let db_b = db_for(&cfg_b).await;

    let sink_a: Arc<dyn IntentSink> = Arc::new(DbIntentSink::with_owner(
        db_a.clone(),
        "replica-A".to_string(),
        Duration::from_secs(600),
    ));
    // Replica B uses a SHORT lease so the "crash" below is observable quickly.
    let sink_b: Arc<dyn IntentSink> = Arc::new(DbIntentSink::with_owner(
        db_b.clone(),
        "replica-B".to_string(),
        Duration::from_secs(1),
    ));

    let trade = format!("copy:wallet:fault-{}", &suffix[..8]);
    let sends_a = Arc::new(AtomicUsize::new(0));
    let sends_b = Arc::new(AtomicUsize::new(0));

    // ---- Fault 1: both replicas attempt the same logical trade ------------
    let a_sends = sends_a.clone();
    let (out_a, out_b) = tokio::join!(
        with_reserved_intent_disposition(
            Some(&sink_a),
            &trade,
            rec(&format!("{trade}#a"), &trade),
            async move {
                a_sends.fetch_add(1, Ordering::SeqCst);
                Ok("SIG-A".to_string())
            },
            |sig: &String| IntentDisposition::Link(sig.clone()),
        ),
        with_reserved_intent_disposition(
            Some(&sink_b),
            &trade,
            rec(&format!("{trade}#b"), &trade),
            {
                let b_sends = sends_b.clone();
                async move {
                    b_sends.fetch_add(1, Ordering::SeqCst);
                    Ok("SIG-B".to_string())
                }
            },
            |sig: &String| IntentDisposition::Link(sig.clone()),
        )
    );
    let winner_is_a = out_a.is_ok();
    assert_ne!(
        winner_is_a,
        out_b.is_ok(),
        "exactly one replica may broadcast: a={out_a:?} b={out_b:?}"
    );
    assert_eq!(
        sends_a.load(Ordering::SeqCst) + sends_b.load(Ordering::SeqCst),
        1,
        "only the winning replica's future may be polled"
    );
    let loser = if winner_is_a { &out_b } else { &out_a };
    let err = loser.as_ref().expect_err("the loser must be refused");
    assert!(
        err.to_string().contains("reserved"),
        "the refusal must name the durable reservation, got {err}"
    );

    // The durable row belongs to the winner and is linked. RLS requires the
    // read to be tenant-scoped, exactly like the production sweep.
    let org = bot_core::db::deployment_org::deployment_org_uuid(&db_a)
        .await
        .expect("deployment org");
    let repo_a = BroadcastReservationRepo::new(db_a.clone());
    let (owner, _epoch, status) =
        bot_core::tenant::rls_context::scope_tenant_context(org, repo_a.get(&trade))
            .await
            .unwrap()
            .expect("row");
    assert_eq!(status, "linked");
    assert_eq!(
        owner,
        if winner_is_a {
            "replica-A"
        } else {
            "replica-B"
        }
    );

    // ---- Fault 2a: a crashed owner's claim is not stuck forever -----------
    let crash_trade = format!("copy:wallet:crash-{}", &suffix[..8]);
    let crashed_sends = Arc::new(AtomicUsize::new(0));
    let crashed = crashed_sends.clone();
    // The reservation is taken, then the "process dies" before the broadcast
    // future resolves: no disposition is ever reported, so the claim stays
    // `reserved` until its lease lapses. (The future is abandoned, not
    // awaited — exactly what a killed process does.)
    let crashed_future = with_reserved_intent_disposition(
        Some(&sink_b),
        &crash_trade,
        rec(&format!("{crash_trade}#b"), &crash_trade),
        async move {
            crashed.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<BotResult<()>>().await
        },
        |_: &()| IntentDisposition::Link("never".into()),
    );
    // The killed process: the future is dropped before it ever resolves, so
    // the claim it holds is never linked or abandoned.
    // Do not race a fixed sleep against the sink's work: `reserve` does two
    // database round-trips before the broadcast future is ever polled, and under
    // the load of a full workspace sweep 30 ms is not enough — this test failed
    // in the sweep and passed on every isolated run. Wait for the actual
    // condition (bounded), which is what the assertion is really about.
    // `select!` is what POLLS the future; pinning alone would deadlock. The loop
    // keeps driving it until the sink has reserved and the closure ran.
    tokio::pin!(crashed_future);
    let polled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                _ = &mut crashed_future => {
                    panic!("the crashed attempt must never resolve")
                }
                _ = tokio::time::sleep(Duration::from_millis(5)) => {
                    if crashed_sends.load(Ordering::SeqCst) >= 1 {
                        return;
                    }
                }
            }
        }
    })
    .await;
    assert!(
        polled.is_ok(),
        "the crashed attempt's broadcast future was never polled within 10 s"
    );
    assert_eq!(
        crashed_sends.load(Ordering::SeqCst),
        1,
        "the crashed attempt's broadcast future was polled once"
    );
    // Now abandon it the way a killed process does: drop it unresolved. (Ending the
    // test's hold on the pinned future is the drop; an explicit `drop(...)` on a
    // `Pin<&mut _>` would move nothing and clippy flags it.)
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    // The lapsed claim is visible to the operator sweep.
    let lapsed = bot_core::tenant::rls_context::scope_tenant_context(org, repo_a.ambiguous(50))
        .await
        .unwrap();
    let row = lapsed
        .iter()
        .find(|r| r.trade_key == crash_trade)
        .expect("the crashed owner's claim must be visible to the sweep");
    assert_eq!(row.status, "reserved");
    assert!(row.lease_lapsed);
    assert_eq!(row.owner_id, "replica-B");

    // ---- Fault 2b: another replica takes over, the stale one is fenced ----
    let takeover_sends = Arc::new(AtomicUsize::new(0));
    let takeover = takeover_sends.clone();
    let taken: BotResult<String> = with_reserved_intent_disposition(
        Some(&sink_a),
        &crash_trade,
        rec(&format!("{crash_trade}#a"), &crash_trade),
        async move {
            takeover.fetch_add(1, Ordering::SeqCst);
            Ok("SIG-TAKEOVER".to_string())
        },
        |sig: &String| IntentDisposition::Link(sig.clone()),
    )
    .await;
    assert_eq!(
        taken.expect("a lapsed claim must be takeable"),
        "SIG-TAKEOVER"
    );
    assert_eq!(takeover_sends.load(Ordering::SeqCst), 1);
    let (owner, epoch, status) =
        bot_core::tenant::rls_context::scope_tenant_context(org, repo_a.get(&crash_trade))
            .await
            .unwrap()
            .unwrap();
    assert_eq!((owner.as_str(), status.as_str()), ("replica-A", "linked"));
    assert!(
        epoch >= 2,
        "a takeover must bump the fencing epoch: {epoch}"
    );

    // The previous owner cannot mutate the claim it lost.
    assert!(!bot_core::tenant::rls_context::scope_tenant_context(
        org,
        repo_a.complete(&crash_trade, "SIG-STALE", "replica-B", 1)
    )
    .await
    .unwrap());
    assert!(!bot_core::tenant::rls_context::scope_tenant_context(
        org,
        repo_a.abandon(&crash_trade, "replica-B", 1)
    )
    .await
    .unwrap());

    // And no third attempt may broadcast the now-settled trade.
    let third_sends = Arc::new(AtomicUsize::new(0));
    let third = third_sends.clone();
    let refused: BotResult<()> = with_reserved_intent_disposition(
        Some(&sink_b),
        &crash_trade,
        rec(&format!("{crash_trade}#c"), &crash_trade),
        async move {
            third.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        |_| IntentDisposition::Retain,
    )
    .await;
    assert!(refused.is_err(), "a settled trade must stay closed");
    assert_eq!(third_sends.load(Ordering::SeqCst), 0);
}
