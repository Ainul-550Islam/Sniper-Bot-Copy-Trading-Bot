//! Regression for a non-authoritative clustered dedup failure racing a
//! different replica's completed event journal write. Requires POSTGRES_URL.
mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bot_core::config::DatabaseConfig;
use bot_core::db::claims::PostgresClaimStore;
use bot_core::db::copy::{
    CopyEventRecord, CopyLinkRecord, CopyRepo, LeaderEventRecord, LeaderRecord,
};
use bot_core::db::Database;
use bot_core::models::ExecutionMode;
use bot_core::ownership::OwnershipRegistry;
use chrono::{DateTime, Utc};
use module_copy::event::{CopyOutcome, CopyStage, EventSource, RejectReason};
use module_copy::recovery::{CopyStore, MemoryCopyStore};
use module_copy::CopyBot;
use solana_kit::tokens::Wallet;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::Barrier;

// The real DB event journal with in-memory leader/link methods. This tests
// CopyBot's decision not to call record_event against the real upsert; it is
// not the production server adapter or an RLS authorization test.
struct PgEventStore {
    repo: CopyRepo,
    other: MemoryCopyStore,
}

#[async_trait]
impl CopyStore for PgEventStore {
    async fn upsert_leader(&self, rec: LeaderRecord) -> bool {
        self.other.upsert_leader(rec).await
    }
    async fn append_leader_event(&self, rec: LeaderEventRecord) -> bool {
        self.other.append_leader_event(rec).await
    }
    async fn load_leaders(&self) -> Option<Vec<LeaderRecord>> {
        self.other.load_leaders().await
    }
    async fn record_event(&self, rec: CopyEventRecord) -> bool {
        self.repo.record_event(&rec).await.is_ok()
    }
    async fn events_since(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Option<Vec<CopyEventRecord>> {
        self.repo.events_since(since, limit as i64).await.ok()
    }
    async fn upsert_link(&self, rec: CopyLinkRecord) -> bool {
        self.other.upsert_link(rec).await
    }
    async fn open_links(&self) -> Option<Vec<CopyLinkRecord>> {
        self.other.open_links().await
    }
    async fn close_link(
        &self,
        id: &str,
        status: &str,
        exit: Option<&str>,
        note: Option<&str>,
    ) -> bool {
        self.other.close_link(id, status, exit, note).await
    }
}

#[tokio::test]
async fn unavailable_dedup_cannot_overwrite_concurrent_postgres_winner() {
    let Some(url) = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|x| !x.trim().is_empty())
    else {
        eprintln!("SKIP: POSTGRES_URL is not set — real PostgreSQL journal regression not run");
        return;
    };
    let dbcfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&dbcfg, &url)
            .await
            .expect("connect PostgreSQL"),
    );
    db.migrate().await.expect("apply migrations");
    // A separate pool stands in for the winning replica's database client.
    let winner_db = Arc::new(Database::connect(&dbcfg, &url).await.expect("winner pool"));
    let winner_repo = CopyRepo::new(winner_db);
    let journal = Arc::new(PgEventStore {
        repo: CopyRepo::new(db.clone()),
        other: MemoryCopyStore::new(),
    });
    let node = Arc::new(common::MockNode::default());
    let url = common::spawn_mock_node(node.clone()).await;
    let mut cfg = common::live_copy_config();
    cfg.ha.mode = "active_active".into();
    cfg.ha.replica_id = format!("journal-regression-{}", std::process::id());
    cfg.execution.mode = ExecutionMode::Live;
    let state = common::state_with(cfg.clone());
    state.set_balances(Some(10.0), Some(1000.0)).await;
    // No dedup facade is attached: the losing bot must fail closed.
    let ownership = OwnershipRegistry::new(
        Arc::new(PostgresClaimStore::new(db)),
        &cfg.ha.replica_id,
        Duration::from_secs(60),
        Duration::from_secs(900),
    );
    let mut bot = CopyBot::new(
        state,
        common::mock_rpc(&url),
        Arc::new(Wallet::generate()),
        None,
    )
    .await
    .with_copy_store(journal.clone())
    .with_ownership(Arc::new(ownership));
    let tag = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let mut trade = common::leader_buy(common::LEADER, Pubkey::new_unique(), 1, 0.5);
    trade.signature = format!("journal-race-{tag}");
    let event = common::event(&trade, EventSource::PumpPortal, 1);
    let winner = CopyOutcome {
        event_id: event.event_id.clone(),
        stage: CopyStage::Filled,
        rejection: None,
        intent_id: Some("winning-intent".into()),
        position_id: Some("winning-position".into()),
        requested_sol: None,
        sized_sol: None,
        signature: None,
        total_ms: 0,
    };
    let winner_record = module_copy::mirror::journal_record(&event, &winner);
    let gate = Arc::new(Barrier::new(2));
    let writer_gate = gate.clone();
    let writer = async {
        writer_gate.wait().await;
        winner_repo
            .record_event(&winner_record)
            .await
            .expect("winning event insert");
    };
    let loser = async {
        gate.wait().await;
        bot.process_event(&event, &cfg).await
    };
    let (_, outcome) = tokio::join!(writer, loser);
    assert_eq!(
        outcome.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::DedupUnavailable)
    );
    let recorded = winner_repo
        .get_event(&event.event_id)
        .await
        .expect("read journal")
        .expect("winner exists");
    assert_eq!(recorded.stage, "FILLED");
    assert_eq!(recorded.reject_reason, None);
    assert_eq!(recorded.intent_id.as_deref(), Some("winning-intent"));
    assert_eq!(recorded.position_id.as_deref(), Some("winning-position"));
    // A later unavailable attempt cannot overwrite a previously committed win.
    let later = bot.process_event(&event, &cfg).await;
    assert_eq!(
        later.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::DedupUnavailable)
    );
    assert_eq!(
        winner_repo
            .get_event(&event.event_id)
            .await
            .expect("read winner")
            .expect("winner remains")
            .stage,
        "FILLED"
    );
    trade.signature.push_str("-fresh");
    let fresh = common::event(&trade, EventSource::PumpPortal, 2);
    let result = bot.process_event(&fresh, &cfg).await;
    assert_eq!(
        result.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::DedupUnavailable)
    );
    assert!(winner_repo
        .get_event(&fresh.event_id)
        .await
        .expect("read fresh")
        .is_none());
    assert_eq!(node.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
}
