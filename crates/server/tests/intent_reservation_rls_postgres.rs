//! Production-sink durability under a REAL RLS deployment configuration.
//!
//! The other reservation test (`intent_broadcast_reservation_postgres.rs`)
//! uses a non-RLS `Database`, so the deployment-scoped journal cannot resolve
//! and every attempt fails closed. This test instead builds the production
//! shape: a migrated database with the `bot_app`/`bot_system` roles, an app
//! pool that `SET ROLE bot_app` and a `bot_system` pool that bypasses RLS, and
//! then proves against the real `DbIntentSink` that
//!
//! 1. the winner's pre-broadcast reservation is a SUCCESSFUL durable write
//!    (row visible to an independent connection, status `linked`),
//! 2. a second attempt for the same logical trade is refused and the
//!    money-moving future is not polled,
//! 3. the tenant policy really is enforced for the table (a foreign-tenant
//!    row is invisible to the deployment tenant).
//!
//! Requires `POSTGRES_URL` pointing at a disposable database whose login can
//! createdb/create role; the test provisions `sniper_app`/`sniper_system`
//! login roles itself. Repository/disposable-service proof: not multi-process,
//! crash-injection or live-chain evidence.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bot_core::config::DatabaseConfig;
use bot_core::db::repo::IntentRecord;
use bot_core::db::Database;
use bot_core::recovery::{with_reserved_intent_disposition, IntentDisposition, IntentSink};
use bot_core::BotResult;
use chrono::Utc;
use sniper_suite::recon::DbIntentSink;
use sqlx::postgres::PgPoolOptions;

/// Disposable database URL for this test (`SNIPER_RESERVATION_TEST_PG` is an
/// explicit alias for the same non-production database).
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

fn base_url() -> Option<String> {
    env_url()
}

fn skip(name: &str) {
    eprintln!("NOT_RUN: {name} — No disposable PostgreSQL given");
}

/// A disposable schema/roles environment: app login (`sniper_app`, member of
/// `bot_app`, NOBYPASSRLS) and system login (`sniper_system`, member of
/// `bot_system`, BYPASSRLS).
struct RlsEnv {
    app_url: String,
    system_url: String,
    migration_url: String,
}

async fn prepare_rls_env(base: &str) -> Option<RlsEnv> {
    let admin = match PgPoolOptions::new().max_connections(2).connect(base).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("RLS env: admin connect failed: {e}");
            return None;
        }
    };
    // Roles are cluster-wide: create-or-reset so a rerun is deterministic.
    for (role, rls_attr) in [
        ("sniper_app", "NOBYPASSRLS"),
        ("sniper_system", "BYPASSRLS"),
    ] {
        let ddl = format!(
            "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') \
             THEN CREATE ROLE {role} LOGIN PASSWORD 'sniper' {rls_attr}; END IF; END $$;"
        );
        if let Err(e) = sqlx::query(&ddl).execute(&admin).await {
            eprintln!("RLS env: create role {role} failed: {e}");
            return None;
        }
        // Deterministic password for the disposable login roles (the roles are
        // created or reset here, never reused from a shared deployment).
        if let Err(e) = sqlx::query(&format!("ALTER ROLE {role} PASSWORD 'sniper'"))
            .execute(&admin)
            .await
        {
            eprintln!("RLS env: password reset for {role} failed: {e}");
            return None;
        }
        if let Err(e) = sqlx::query(&format!("GRANT {role} TO CURRENT_USER"))
            .execute(&admin)
            .await
        {
            eprintln!("RLS env: grant {role} failed: {e}");
            return None;
        }
    }
    // Fresh schema state for the app pool: migrations run as the migration
    // login, which is the same superuser login the test was given.
    let rewrite = |user: &str| -> Option<String> {
        let (scheme, rest) = base.split_once("://")?;
        let (_creds, host) = rest.split_once('@')?;
        Some(format!("{scheme}://{user}:sniper@{host}"))
    };
    let app_url = match rewrite("sniper_app") {
        Some(u) => u,
        None => {
            eprintln!("RLS env: cannot rewrite base url for the app role");
            return None;
        }
    };
    let system_url = match rewrite("sniper_system") {
        Some(u) => u,
        None => {
            eprintln!("RLS env: cannot rewrite base url for the system role");
            return None;
        }
    };
    Some(RlsEnv {
        app_url,
        system_url,
        migration_url: base.to_string(),
    })
}

fn rls_config(env: &RlsEnv, suffix: &str) -> DatabaseConfig {
    // The production `connect` path reads URLs from env vars; unique names per
    // test process keep parallel test binaries from colliding.
    std::env::set_var(format!("SNIPER_RESV_APP_URL_{suffix}"), &env.app_url);
    std::env::set_var(format!("SNIPER_RESV_SYSTEM_URL_{suffix}"), &env.system_url);
    std::env::set_var(
        format!("SNIPER_RESV_MIGRATION_URL_{suffix}"),
        &env.migration_url,
    );
    DatabaseConfig {
        enabled: true,
        rls_enabled: true,
        auto_migrate: true,
        required: true,
        url_env: format!("SNIPER_RESV_APP_URL_{suffix}"),
        system_url_env: format!("SNIPER_RESV_SYSTEM_URL_{suffix}"),
        migration_url_env: format!("SNIPER_RESV_MIGRATION_URL_{suffix}"),
        app_login_role: "sniper_app".into(),
        system_login_role: "sniper_system".into(),
        ..Default::default()
    }
}

fn rec(intent_id: &str, trade_key: &str) -> IntentRecord {
    IntentRecord {
        intent_id: intent_id.to_string(),
        module: "reservation-test".into(),
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
async fn production_rls_sink_reserves_durably_and_refuses_a_second_attempt() {
    let Some(base) = base_url() else {
        return skip("production_rls_sink_reserves_durably_and_refuses_a_second_attempt");
    };
    let Some(env) = prepare_rls_env(&base).await else {
        return skip("production_rls_sink_reserves_durably_and_refuses_a_second_attempt");
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let cfg = rls_config(&env, &suffix);
    let app_url = std::env::var(&cfg.url_env).expect("app url");
    let db = Arc::new(
        Database::connect(&cfg, &app_url)
            .await
            .expect("RLS database must connect and migrate"),
    );
    let sink: Arc<dyn IntentSink> = Arc::new(DbIntentSink::with_owner(
        db.clone(),
        "replica-under-test".to_string(),
        Duration::from_secs(600),
    ));
    let key = format!("copy:wallet:rls-{}", &suffix[..8]);

    // ---- 1. the winner's reservation is a real durable write --------------
    let sends = Arc::new(AtomicUsize::new(0));
    let counter = sends.clone();
    let out: BotResult<String> = with_reserved_intent_disposition(
        Some(&sink),
        &key,
        rec(&format!("{key}#1"), &key),
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok("SIG-rls-1".to_string())
        },
        |sig: &String| IntentDisposition::Link(sig.clone()),
    )
    .await;
    assert_eq!(
        out.expect("the first attempt must be reserved and broadcast"),
        "SIG-rls-1"
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    // Independent privileged readback: the row exists and is terminal-linked.
    let system = PgPoolOptions::new()
        .max_connections(1)
        .connect(&env.system_url)
        .await
        .expect("system pool");
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT trade_key, status, claim_epoch FROM intent_broadcast_reservations \
         WHERE trade_key = $1",
    )
    .bind(&key)
    .fetch_optional(&system)
    .await
    .expect("readback query");
    let (trade_key, status, _epoch) = row.expect("the durable reservation row must exist");
    assert_eq!(trade_key, key);
    assert_eq!(status, "linked", "a linked trade must be terminal");

    // ---- 2. a second attempt for the same trade must not broadcast --------
    let second_sends = Arc::new(AtomicUsize::new(0));
    let counter = second_sends.clone();
    let second: BotResult<()> = with_reserved_intent_disposition(
        Some(&sink),
        &key,
        rec(&format!("{key}#2"), &key),
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        |_| IntentDisposition::Retain,
    )
    .await;
    let err = second.expect_err("a settled trade must refuse a second attempt");
    assert!(
        err.to_string().contains("reserved"),
        "the refusal must name the reservation, got {err}"
    );
    assert_eq!(
        second_sends.load(Ordering::SeqCst),
        0,
        "the duplicate attempt must never poll the broadcast future"
    );

    // ---- 3. the tenant policy is enforced on the new table ----------------
    let foreign = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'foreign tenant')")
        .bind(foreign)
        .bind(format!("foreign-{}", &suffix[..8]))
        .execute(&system)
        .await
        .expect("seed foreign tenant");
    sqlx::query(
        "INSERT INTO intent_broadcast_reservations \
             (organization_id, trade_key, owner_id, lease_until) \
         VALUES ($1, $2, 'foreign-owner', now() + interval '600 seconds')",
    )
    .bind(foreign)
    .bind(format!("{key}-foreign"))
    .execute(&system)
    .await
    .expect("seed foreign reservation");
    let visible: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM intent_broadcast_reservations WHERE trade_key = $1",
    )
    .bind(format!("{key}-foreign"))
    .fetch_one(&system)
    .await
    .expect("count query");
    assert_eq!(visible, 1, "the privileged connection must see the row");
    // The deployment-scoped app pool never sees another tenant's row.
    let leaked = bot_core::db::repo::BroadcastReservationRepo::new(db.clone())
        .get(&format!("{key}-foreign"))
        .await
        .expect("scoped read");
    assert!(
        leaked.is_none(),
        "a foreign tenant's reservation must be invisible to this deployment"
    );
}

/// The proven-non-send release is a real durable write through the production
/// sink: the claim becomes `abandoned` at once (the trade key is retryable)
/// instead of blocking every replica until a lease nobody will renew lapses.
#[tokio::test]
async fn a_proven_non_send_frees_the_durable_claim_through_the_production_sink() {
    let Some(base) = base_url() else {
        return skip("a_proven_non_send_frees_the_durable_claim_through_the_production_sink");
    };
    let Some(env) = prepare_rls_env(&base).await else {
        return skip("a_proven_non_send_frees_the_durable_claim_through_the_production_sink");
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let cfg = rls_config(&env, &suffix);
    let app_url = std::env::var(&cfg.url_env).expect("app url");
    let db = Arc::new(
        Database::connect(&cfg, &app_url)
            .await
            .expect("RLS database must connect and migrate"),
    );
    let sink: Arc<dyn IntentSink> = Arc::new(DbIntentSink::with_owner(
        db.clone(),
        "replica-release".to_string(),
        Duration::from_secs(600),
    ));
    let key = format!("copy:wallet:release-{}", &suffix[..8]);

    // A broadcast classified as "no signature" is a proven non-send.
    let out: BotResult<()> = with_reserved_intent_disposition(
        Some(&sink),
        &key,
        rec(&format!("{key}#1"), &key),
        async { Ok(()) },
        |_| IntentDisposition::Abandon,
    )
    .await;
    out.expect("the abandoned attempt must complete");

    let system = PgPoolOptions::new()
        .max_connections(1)
        .connect(&env.system_url)
        .await
        .expect("system pool");
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT status, claim_epoch FROM intent_broadcast_reservations WHERE trade_key = $1",
    )
    .bind(&key)
    .fetch_optional(&system)
    .await
    .expect("readback query");
    let (status, epoch) = row.expect("the durable reservation row must exist");
    assert_eq!(
        status, "abandoned",
        "a proven non-send must free the claim immediately"
    );

    // A later attempt may broadcast: the claim was released, not left to lapse.
    let sends = Arc::new(AtomicUsize::new(0));
    let counter = sends.clone();
    let retry: BotResult<String> = with_reserved_intent_disposition(
        Some(&sink),
        &key,
        rec(&format!("{key}#2"), &key),
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok("SIG-retry".to_string())
        },
        |sig: &String| IntentDisposition::Link(sig.clone()),
    )
    .await;
    assert_eq!(retry.expect("the retry must be allowed"), "SIG-retry");
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let row: (String, i64) = sqlx::query_as(
        "SELECT status, claim_epoch FROM intent_broadcast_reservations WHERE trade_key = $1",
    )
    .bind(&key)
    .fetch_one(&system)
    .await
    .expect("readback query");
    assert_eq!(row.0, "linked");
    assert_eq!(
        row.1,
        epoch + 1,
        "the retry must take the claim over with a bumped epoch"
    );
}
