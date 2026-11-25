//! Production server adapter duplicate readback against real PostgreSQL.
//! Requires an isolated POSTGRES_URL and explicit `--ignored`; does not test settlement.
use bot_core::accounting::{fill_event, Applied, EventSide, GlobalLedger};
use bot_core::config::DatabaseConfig;
use bot_core::db::{accounting::AccountingRepo, Database};
use bot_core::events::EventBus;
use bot_core::models::{BotModule, ExecutionMode, Venue};
use sniper_suite::accounting::DbLedgerStore;
use std::sync::Arc;

/// Runs whenever `POSTGRES_URL` points at a migrated database (gap #31).
#[tokio::test]
async fn production_adapter_concurrent_replicas_compare_committed_payload() {
    let Some(url) = postgres_or_skip("accounting_production_retry_postgres") else {
        return;
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
    let store = Arc::new(DbLedgerStore::new(db.clone()));
    let id = format!("retry-pg-{}", uuid::Uuid::new_v4());
    let event = fill_event(
        BotModule::Copy,
        Venue::PumpFun,
        &id,
        "copy",
        "mint",
        "SOL",
        EventSide::Buy,
        10.0,
        0.1,
        1.0,
        0.0,
        ExecutionMode::Paper,
        &id,
        None,
        Some(id.clone()),
        chrono::Utc::now(),
        "",
    );
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let l = Arc::new(GlobalLedger::new(EventBus::new(16), format!("replica-{i}")));
        l.attach_store(store.clone()).await;
        let barrier = barrier.clone();
        let mut e = event.clone();
        if i >= 4 {
            e.quote_amount = 2.0;
        }
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            l.submit(e).await
        }));
    }
    let (mut inserted, mut duplicate, mut conflict) = (0, 0, 0);
    for t in tasks {
        match t.await.unwrap() {
            Applied::New(_) => inserted += 1,
            Applied::Duplicate => duplicate += 1,
            Applied::Rejected(_) => conflict += 1,
        }
    }
    assert_eq!((inserted, duplicate, conflict), (1, 3, 4));
    let repo = AccountingRepo::new(db);
    let stored = repo.load_event(&event.event_id()).await.unwrap().unwrap();
    assert!(stored.event.quote_amount == 1.0 || stored.event.quote_amount == 2.0);
}

/// No service ⇒ NOT_RUN (fatal under `REQUIRE_POSTGRES=1`), never a silent pass.
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
