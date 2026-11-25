//! Real PostgreSQL duplicate readback regression. Does not test fee settlement.
use bot_core::accounting::{
    fill_event, AccountingFinding, Applied, BookPosition, Entry, EventSide, GlobalLedger,
    LedgerStore, StoredEvent,
};
use bot_core::config::DatabaseConfig;
use bot_core::db::{accounting::AccountingRepo, Database};
use bot_core::events::EventBus;
use bot_core::models::{BotModule, ExecutionMode, Venue};
use std::sync::Arc;

struct PgStore(Arc<Database>);
#[async_trait::async_trait]
impl LedgerStore for PgStore {
    async fn record_event(&self, e: &StoredEvent, p: &Entry) -> Option<bool> {
        AccountingRepo::new(self.0.clone())
            .record_event(e, p)
            .await
            .ok()
    }
    async fn load_events(&self) -> Option<Vec<StoredEvent>> {
        AccountingRepo::new(self.0.clone()).load_events().await.ok()
    }
    async fn load_event(&self, id: &str) -> Option<Option<StoredEvent>> {
        AccountingRepo::new(self.0.clone())
            .load_event(id)
            .await
            .ok()
    }
    async fn upsert_position(&self, p: &BookPosition) -> bool {
        AccountingRepo::new(self.0.clone())
            .upsert_position(p)
            .await
            .is_ok()
    }
    async fn append_finding(&self, f: &AccountingFinding) -> bool {
        AccountingRepo::new(self.0.clone())
            .append_finding(f)
            .await
            .is_ok()
    }
    async fn recent_findings(&self, n: usize) -> Option<Vec<AccountingFinding>> {
        AccountingRepo::new(self.0.clone())
            .recent_findings(n as i64)
            .await
            .ok()
    }
}

#[tokio::test]
async fn concurrent_replicas_compare_the_committed_payload() {
    let Ok(url) = std::env::var("POSTGRES_URL") else {
        eprintln!("NOT_RUN: accounting_retry_postgres requires POSTGRES_URL");
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
    let store = Arc::new(PgStore(db.clone()));
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
