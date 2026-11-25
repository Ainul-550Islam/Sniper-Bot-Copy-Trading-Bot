//! Journal-conflict repair against a REAL PostgreSQL ledger journal.
//!
//! The in-memory suite proves the repair semantics (journal wins, unbacked
//! pending rows are discarded, an unadjudicable pending row refuses the
//! repair). This test proves the same code path re-reads a real durable
//! journal, rebuilds the book from it, is idempotent, and leaves the ledger
//! able to book again afterwards.
//!
//! Ignored by default: requires an isolated migrated database via
//! `POSTGRES_URL` (and `REQUIRE_POSTGRES=1` to fail instead of skipping).

use std::sync::Arc;

use bot_core::accounting::{fill_event, Applied, EventSide, GlobalLedger, LedgerStore};
use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::events::EventBus;
use bot_core::models::{BotModule, ExecutionMode, Venue};
use sniper_suite::accounting::DbLedgerStore;

fn fill(id: &str, qty: f64, quote: f64) -> bot_core::accounting::AccountingEvent {
    fill_event(
        BotModule::Copy,
        Venue::PumpFun,
        "w",
        "copy",
        "MINT",
        "SOL",
        EventSide::Buy,
        qty,
        quote / qty,
        quote,
        0.0,
        ExecutionMode::Paper,
        id,
        None,
        Some(format!("p-{id}")),
        chrono::Utc::now(),
        "",
    )
}

/// Runs whenever `POSTGRES_URL` points at a migrated database (gap #31); it is
/// no longer parked behind `#[ignore]`, where a failing repair path went
/// unnoticed. Missing service ⇒ NOT_RUN, fatal under `REQUIRE_POSTGRES=1`.
///
/// The repair refuses to rebuild from a journal that does not cover the
/// deployment's positions, and that probe is deployment-wide — it reads whatever
/// the database holds. Sharing a database with the rest of the suite therefore
/// made this test's verdict depend on which other tests ran first. It now builds
/// its own throwaway database (unless `POSTGRES_REPAIR_URL` names one) so the
/// result depends only on this test.
#[tokio::test]
async fn repair_rebuilds_the_book_from_a_real_durable_journal() {
    let Some(server_url) = postgres_or_skip("ledger_conflict_repair_postgres") else {
        return;
    };
    let (url, cleanup) = match std::env::var("POSTGRES_REPAIR_URL") {
        Ok(explicit) if !explicit.trim().is_empty() => (explicit, None),
        _ => create_scratch_database(&server_url).await,
    };
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &url)
            .await
            .expect("postgres connect"),
    );
    db.migrate().await.expect("migrations");
    // The repair refuses to rebuild the book from a journal that does not cover
    // positions the deployment already has — a closed position with no ledger
    // backing means the journal is not the whole story. That probe is
    // deployment-wide and reads whatever rows the database holds, so state the
    // precondition here with a diagnostic that names it. Without this, the
    // refusal surfaces as a bare `reason: "durable journal unavailable"` and
    // looks like a broken connection.
    assert!(
        bot_core::db::accounting::AccountingRepo::new(db.clone())
            .position_ledger_coverage_complete()
            .await
            .expect("ledger coverage probe"),
        "this test needs a database whose positions are all backed by the ledger journal, \
         and POSTGRES_REPAIR_URL points at one that already carries closed positions without \
         ledger rows, so the repair correctly refuses. Unset POSTGRES_REPAIR_URL to get the \
         throwaway database this test normally provisions for itself."
    );
    let store: Arc<dyn LedgerStore> = Arc::new(DbLedgerStore::new(db.clone()));
    let run = uuid::Uuid::new_v4().simple().to_string();

    // One replica books two events into the durable journal.
    let writer = Arc::new(GlobalLedger::new(EventBus::new(32), "writer"));
    writer.attach_store(store.clone()).await;
    for (n, (qty, quote)) in [(100.0, 1.0), (40.0, 0.5)].iter().enumerate() {
        let event = fill(&format!("pg-repair-{run}-{n}"), *qty, *quote);
        assert!(
            writer.submit(event).await.is_new(),
            "the durable journal must accept the event"
        );
    }
    assert_eq!(writer.pending_count().await, 0, "journal is healthy");

    // A second replica starts empty and repairs from the journal only.
    let reader = Arc::new(GlobalLedger::new(EventBus::new(32), "reader"));
    reader.attach_store(store.clone()).await;
    assert!(reader.is_empty().await);
    let report = reader.reconcile_from_journal().await;
    assert!(report.repaired, "{report:?}");
    assert_eq!(report.claimed_conflict, None, "nothing was conflicting");
    assert_eq!(report.discarded_pending_events, 0);
    assert!(
        report.replayed_events >= 2,
        "the durable rows must be replayed, got {report:?}"
    );
    assert_eq!(reader.journal_conflict().await, None);
    assert_eq!(reader.len().await, writer.len().await);
    assert_eq!(reader.book().await.cash(), writer.book().await.cash());
    assert_eq!(
        reader.book().await.open_count(),
        writer.book().await.open_count()
    );

    // Idempotent: a second repair changes nothing.
    let again = reader.reconcile_from_journal().await;
    assert!(again.repaired);
    assert_eq!(again.replayed_events, report.replayed_events);
    assert_eq!(reader.len().await, report.replayed_events);

    // And the repaired replica can book again.
    let next = fill(&format!("pg-repair-{run}-after"), 10.0, 1.0);
    assert!(matches!(reader.submit(next).await, Applied::New(_)));

    drop_scratch_database(cleanup).await;
}

/// A throwaway, freshly migrated database on the same server as `server_url`.
///
/// Returns the URL to use and the admin URL + database name needed to drop it.
async fn create_scratch_database(server_url: &str) -> (String, Option<(String, String)>) {
    let admin = admin_url(server_url);
    let name = format!("sniper_repair_{}", uuid::Uuid::new_v4().simple());
    let pool = sqlx::PgPool::connect(&admin)
        .await
        .expect("connect to the maintenance database to create a scratch database");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&pool)
        .await
        .expect("create the scratch database");
    pool.close().await;
    (with_database(server_url, &name), Some((admin, name)))
}

/// Best effort: a leaked scratch database is noise, not a failed test.
async fn drop_scratch_database(cleanup: Option<(String, String)>) {
    let Some((admin, name)) = cleanup else {
        return;
    };
    let Ok(pool) = sqlx::PgPool::connect(&admin).await else {
        eprintln!("NOTE: scratch database {name} was not dropped (no admin connection)");
        return;
    };
    if sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
        .execute(&pool)
        .await
        .is_err()
    {
        eprintln!("NOTE: scratch database {name} was not dropped (drop failed)");
    }
    pool.close().await;
}

/// `postgres://…/<db>` → `postgres://…/postgres`.
fn admin_url(url: &str) -> String {
    with_database(url, "postgres")
}

/// `postgres://…/<db>` → `postgres://…/<name>`, dropping any query string.
fn with_database(url: &str, name: &str) -> String {
    let base = url.split('?').next().unwrap_or(url);
    match base.rfind('/') {
        Some(cut) => format!("{}/{name}", &base[..cut]),
        None => format!("{base}/{name}"),
    }
}

#[test]
fn scratch_database_urls_are_built_from_the_configured_server() {
    let server = "postgres://sniper:sniper@127.0.0.1:55432/sniper_gate";
    assert_eq!(
        admin_url(server),
        "postgres://sniper:sniper@127.0.0.1:55432/postgres"
    );
    assert_eq!(
        with_database(server, "scratch"),
        "postgres://sniper:sniper@127.0.0.1:55432/scratch"
    );
    assert_eq!(
        with_database("postgres://u:p@h:1/db?sslmode=disable", "scratch"),
        "postgres://u:p@h:1/scratch"
    );
}

/// Same contract as every other database-backed suite: no service ⇒ NOT_RUN
/// (fatal under `REQUIRE_POSTGRES=1`), so absence is never a pass.
fn postgres_or_skip(what: &str) -> Option<String> {
    match std::env::var("POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("NOT_RUN: {what} — POSTGRES_URL missing");
            assert!(
                std::env::var("REQUIRE_POSTGRES").as_deref() != Ok("1"),
                "{what} requires POSTGRES_URL and REQUIRE_POSTGRES=1 is set"
            );
            None
        }
    }
}
