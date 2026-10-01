//! PROMPT 4/10 — tenant module factory against REAL PostgreSQL.
//!
//! Proves the server-plane wiring end to end at the persistence
//! boundary: a factory-built sniper/copy engine holds the tenant's own
//! context, the repository sinks write ONLY for the tenant they were
//! built for, replays are idempotent, and another tenant — or a sink
//! handed another tenant's outcome — can never produce or read a
//! foreign row.
//!
//! GATED exactly like the PROMPT 3 suites: every test prints NOT_RUN
//! and returns early unless `POSTGRES_URL` points at a real database.
//! Run with `--test-threads=1`.

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::execution::{
    AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::state::AppState;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId, SignerProvider};
use bot_core::trading_repository::copy::{
    model::{TenantCopyEvent, TenantLeader},
    TenantCopyEventRepo, TenantCopyRead, TenantCopyWrite,
};
use bot_core::trading_repository::executions::read::TenantExecutionRead;
use bot_core::trading_repository::executions::write::TenantExecutionWrite;
use bot_core::trading_repository::query_scope::TradingQueryScope;
use bot_core::trading_repository::write_scope::{TenantWriteScope, WriteOrigin};
use chrono::Utc;
use module_copy::event::{CopyOutcome, CopyStage, LeaderTradeEvent};
use module_copy::tenant_context::CopyTenantContext;
use module_copy::tenant_executor::TenantCopySink;
use module_sniper::tenant_executor::TenantExecutionSink;
use solana_kit::execute::{ExecStatus, ExecutionResult};
use solana_kit::tenant_transaction::TenantTransaction;

use sniper_suite::module_runtime::tenant_module_factory::{
    RepoCopySink, RepoExecutionSink, TenantModuleFactory,
};

/// The gated connection (None ⇒ suite reports NOT_RUN).
async fn setup() -> Option<Arc<Database>> {
    let url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    Some(Arc::new(db))
}

/// Unique namespace per test run (unique keys never collide between
/// CI jobs or repeated runs).
fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

/// Create a real organization row and return its typed id.
async fn org(db: &Database, slug: &str) -> OrganizationId {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO organizations (id, slug, name, status)
           VALUES ($1, $2, $3, 'active')"#,
    )
    .bind(id)
    .bind(slug)
    .bind(format!("Org {slug}"))
    .execute(db.pool())
    .await
    .expect("insert organization");
    OrganizationId::from(id)
}

/// Issue a REAL paper execution context bound to `address`.
fn issued(
    organization: OrganizationId,
    module: BotModule,
    address: &str,
) -> TenantExecutionContext {
    let runtime = RuntimeId::new();
    let generation = RuntimeGeneration::first();
    let scope = bot_core::execution::ExecutionScope::new(
        organization,
        runtime,
        generation,
        module,
        ExecutionMode::Paper,
    )
    .unwrap();
    let mut checklist = AuthorityChecklist::new();
    let now = Utc::now();
    for name in AUTHORITY_CHECK_ORDER {
        checklist.record(name, now).unwrap();
    }
    let authority = checklist.finish(&scope, now).unwrap();
    let wallet = bot_core::tenant::TenantWalletRef::new(organization, address).unwrap();
    let signer =
        bot_core::tenant::TenantSignerRef::new(organization, SignerProvider::Local, "pg-key")
            .unwrap();
    TenantExecutionContext::issue(
        organization,
        runtime,
        generation,
        module,
        ExecutionMode::Paper,
        authority,
        wallet,
        signer,
        ExecutionTrace::for_request(),
    )
    .unwrap()
}

/// A real keypair: its inline-base58 Wallet AND its address must come
/// from the SAME key so the guard's wallet binding matches.
fn loaded_wallet_with_address() -> (solana_kit::tokens::Wallet, String) {
    use solana_sdk::signer::Signer;
    let kp = solana_sdk::signature::Keypair::new();
    let address = kp.pubkey().to_string();
    let b58 = bs58::encode(kp.to_bytes()).into_string();
    (
        solana_kit::tokens::Wallet::load(&b58).expect("wallet"),
        address,
    )
}

/// A submitted (non-paper) result carrying a real-shaped signature.
fn submitted_result(signature: &str) -> ExecutionResult {
    ExecutionResult {
        signature: signature.to_string(),
        status: ExecStatus::Sent,
        paper: false,
        ..ExecutionResult::empty("pg-test", "intent-pg", false)
    }
}

/// A real-shaped leader trade event for the copy sink.
fn leader_event(leader: &str, event_id: &str) -> LeaderTradeEvent {
    LeaderTradeEvent {
        event_id: event_id.to_string(),
        leader: leader.to_string(),
        signature: format!("sig-{event_id}"),
        slot: 42,
        block_time: None,
        side: bot_core::models::PositionSide::Long,
        mint: "11111111111111111111111111111111".to_string(),
        symbol: None,
        venue: bot_core::models::Venue::PumpFun,
        token_amount: 1.0,
        sol_amount: 0.001,
        fee_sol: 0.0,
        discriminator: None,
        source: module_copy::event::EventSource::Manual,
        source_sequence: 1,
        observed_at: Utc::now(),
    }
}

fn mirrored_outcome(event_id: &str) -> CopyOutcome {
    CopyOutcome {
        event_id: event_id.to_string(),
        stage: CopyStage::Filled,
        rejection: None,
        intent_id: Some("intent-copy-pg".to_string()),
        position_id: Some("pos-1".to_string()),
        requested_sol: Some(0.001),
        sized_sol: Some(0.001),
        signature: Some("sig-mirror".to_string()),
        total_ms: 5,
    }
}

/// Seed the tenant's leader row so the copy-events foreign key shape
/// matches production (the sink records events for configured leaders).
async fn seed_leader(db: &Arc<Database>, organization: OrganizationId, address: &str) {
    let now = Utc::now();
    let rec = TenantLeader {
        organization_id: organization,
        address: address.to_string(),
        label: "pg-leader".to_string(),
        status: "active".to_string(),
        source: "config".to_string(),
        followed_at: now,
        status_since: now,
        events_seen: 0,
        mirrored: 0,
        rejected: 0,
        last_event_at: None,
        last_slot: None,
        updated_at: now,
    };
    TenantCopyWrite::new(Arc::clone(db))
        .upsert_leader(
            &TenantWriteScope::new(organization, "pg:test", WriteOrigin::Http).unwrap(),
            &rec,
        )
        .await
        .expect("seed leader");
}

#[tokio::test]
async fn sniper_sink_writes_only_the_bound_tenants_rows() {
    let Some(db) = setup().await else {
        println!("NOT_RUN: POSTGRES_URL is not set");
        return;
    };
    let run = run_id();
    let tenant_a = org(&db, &format!("p4-sniper-a-{run}")).await;
    let tenant_b = org(&db, &format!("p4-sniper-b-{run}")).await;
    let factory = TenantModuleFactory::new(Arc::clone(&db));

    // A REAL factory-built sniper engine for tenant A (paper, hermetic
    // rpc pool, generated wallet).
    let (funding, address) = loaded_wallet_with_address();
    let context = issued(tenant_a, BotModule::Sniper, &address);
    let wallet = Arc::new(funding);
    let executor = factory
        .build_sniper(
            AppState::new(bot_core::config::AppConfig::from_defaults()),
            solana_kit::Rpc::new(&bot_core::config::NetworkConfig::default()).expect("rpc"),
            Arc::clone(&wallet),
            None,
            context,
        )
        .await
        .expect("factory sniper");
    assert_eq!(executor.tenant_context().organization_id(), tenant_a);

    // The sink the factory attached records a submitted transaction
    // under tenant A's identity.
    let meta = solana_kit::tenant_transaction::TenantTransactionMeta::from_context(
        executor.signing_context().execution_context(),
        "sniper",
        Some("intent-pg-1"),
    );
    let signature = format!("sig-sniper-{run}");
    let tx = TenantTransaction::new(meta, &submitted_result(&signature));
    executor_sniper_record(&executor, &tx)
        .await
        .expect("record");

    // Tenant A reads its row back…
    let read = TenantExecutionRead::new(Arc::clone(&db));
    let scope_a = TradingQueryScope::new(tenant_a);
    let row = read
        .transaction(&scope_a, &signature)
        .await
        .expect("read")
        .expect("row must exist for tenant A");
    assert_eq!(row.organization_id, tenant_a);
    assert_eq!(row.status, "submitted");

    // …tenant B never sees it.
    let scope_b = TradingQueryScope::new(tenant_b);
    assert!(read
        .transaction(&scope_b, &signature)
        .await
        .expect("read")
        .is_none());

    // A sink built for tenant B refuses tenant A's outcome outright.
    let write_b = TenantWriteScope::new(tenant_b, "pg:test", WriteOrigin::Http).unwrap();
    let sink_b = RepoExecutionSink::new(TenantExecutionWrite::new(Arc::clone(&db)), write_b);
    let err = sink_b.record(&tx).await.unwrap_err();
    assert!(err.to_string().contains("organization mismatch"), "{err}");
}

/// Record through the EXECUTOR's own attached sink (the exact object
/// the factory installed — not a locally rebuilt one).
async fn executor_sniper_record(
    executor: &module_sniper::tenant_executor::TenantSniperExecutor,
    tx: &TenantTransaction,
) -> bot_core::error::BotResult<()> {
    executor.record_execution(tx).await
}

#[tokio::test]
async fn copy_sink_writes_only_the_bound_tenants_rows_and_is_idempotent() {
    let Some(db) = setup().await else {
        println!("NOT_RUN: POSTGRES_URL is not set");
        return;
    };
    let run = run_id();
    let tenant_a = org(&db, &format!("p4-copy-a-{run}")).await;
    let tenant_b = org(&db, &format!("p4-copy-b-{run}")).await;
    let factory = TenantModuleFactory::new(Arc::clone(&db));

    let (funding, address) = loaded_wallet_with_address();
    let context = issued(tenant_a, BotModule::Copy, &address);
    let wallet = Arc::new(funding);
    let executor = factory
        .build_copy(
            AppState::new(bot_core::config::AppConfig::from_defaults()),
            solana_kit::Rpc::new(&bot_core::config::NetworkConfig::default()).expect("rpc"),
            Arc::clone(&wallet),
            None,
            context,
        )
        .await
        .expect("factory copy");
    assert_eq!(executor.tenant_context().organization_id(), tenant_a);

    let leader = format!("leader-{run}");
    seed_leader(&db, tenant_a, &leader).await;

    // Record one mirrored outcome through the executor's own sink.
    let event_id = format!("evt-{run}");
    let event = leader_event(&leader, &event_id);
    let outcome = mirrored_outcome(&event_id);
    executor_copy_record(&executor, &event, &outcome)
        .await
        .expect("record");

    // The row exists for tenant A only, and a replay is idempotent.
    let events = TenantCopyEventRepo::new(Arc::clone(&db));
    let write_a = TenantWriteScope::new(tenant_a, "pg:test", WriteOrigin::Http).unwrap();
    let rec = TenantCopyEvent {
        organization_id: tenant_a,
        event_id: event_id.clone(),
        leader: leader.clone(),
        signature: event.signature.clone(),
        slot: 42,
        mint: event.mint.clone(),
        side: "buy".to_string(),
        venue: event.venue.to_string(),
        token_amount: 1.0,
        sol_amount: 0.001,
        source: "manual".to_string(),
        source_sequence: 1,
        event_at: None,
        observed_at: Utc::now(),
        stage: "FILLED".to_string(),
        reject_reason: None,
        detail: None,
        intent_id: Some("intent-copy-pg".to_string()),
        position_id: Some("pos-1".to_string()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let fresh = events.record(&write_a, &rec).await.expect("replay");
    assert!(!fresh, "replaying the same event must not double-count");

    let read = TenantCopyRead::new(Arc::clone(&db));
    let scope_a = TradingQueryScope::new(tenant_a);
    let scope_b = TradingQueryScope::new(tenant_b);
    let rows_a = read
        .events_for_leader(&scope_a, &leader, 10)
        .await
        .expect("read A");
    assert_eq!(rows_a.len(), 1, "exactly one row for tenant A");
    assert_eq!(rows_a[0].event_id, event_id);
    let rows_b = read
        .events_for_leader(&scope_b, &leader, 10)
        .await
        .expect("read B");
    assert!(rows_b.is_empty(), "tenant B must not see tenant A's event");

    // A sink built for tenant B refuses tenant A's context outright.
    let write_b = TenantWriteScope::new(tenant_b, "pg:test", WriteOrigin::Http).unwrap();
    let sink_b = RepoCopySink::new(TenantCopyEventRepo::new(Arc::clone(&db)), write_b);
    let foreign_ctx =
        CopyTenantContext::adapt(issued(tenant_a, BotModule::Copy, &address)).expect("adapt");
    let err = sink_b
        .record_outcome(&foreign_ctx, &event, &outcome)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("organization mismatch"), "{err}");
}

/// Record through the EXECUTOR's own attached sink (the exact object
/// the factory installed).
async fn executor_copy_record(
    executor: &module_copy::tenant_executor::TenantCopyExecutor,
    event: &LeaderTradeEvent,
    outcome: &CopyOutcome,
) -> bot_core::error::BotResult<()> {
    executor.record_outcome(event, outcome).await
}
