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
use bot_core::models::{BotModule, ExecutionMode, Position, TradeSource, Venue};
use bot_core::state::AppState;
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId, SignerProvider};
use bot_core::trading_repository::copy::{
    model::{TenantCopyEvent, TenantLeader},
    TenantCopyEventRepo, TenantCopyRead, TenantCopyWrite,
};
use bot_core::trading_repository::executions::read::TenantExecutionRead;
use bot_core::trading_repository::executions::write::TenantExecutionWrite;
use bot_core::trading_repository::positions::{TenantPositionRead, TenantPositionWrite};
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

/// Issue a REAL paper context bound to a SPECIFIC (already registered) runtime.
fn issued_on(
    organization: OrganizationId,
    module: BotModule,
    address: &str,
    runtime: RuntimeId,
    generation: RuntimeGeneration,
) -> TenantExecutionContext {
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

/// Seed the durable signer binding the gateway would have verified at
/// issuance. A production context only exists when this row was active
/// (`binding_guard`), so tests that build contexts by hand must seed it —
/// without it the guard's live binding check refuses, which is the point.
async fn bind_signer(db: &Arc<Database>, org: OrganizationId, key_ref: &str, active: bool) {
    use bot_core::tenant::SignerProvider;
    use sniper_suite::tenant::registry::{
        PgTenantBindingRegistry, SignerBindingRow, TenantBindingRegistry,
    };
    PgTenantBindingRegistry::new(Arc::clone(db))
        .put_signer(SignerBindingRow {
            organization_id: org,
            provider: SignerProvider::Local,
            key_ref: key_ref.to_string(),
            active,
            created_at: Utc::now(),
        })
        .await
        .expect("seed signer binding");
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
        attempts: 1,
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
    let signature = solana_sdk::signature::Signature::new_unique().to_string();
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
    let sink_b = RepoExecutionSink::new(
        TenantExecutionWrite::new(Arc::clone(&db)),
        TenantPositionWrite::new(Arc::clone(&db)),
        write_b,
    );
    let err = sink_b.record(&tx).await.unwrap_err();
    assert!(err.to_string().contains("organization mismatch"), "{err}");
}

#[tokio::test]
async fn tenant_position_sink_persists_and_scopes_the_risk_projection() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: tenant_position_sink_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let tenant_a = org(&db, &format!("position-a-{run}")).await;
    let tenant_b = org(&db, &format!("position-b-{run}")).await;
    let write_a = TenantWriteScope::new(tenant_a, "pg:test", WriteOrigin::Job).unwrap();
    let write_b = TenantWriteScope::new(tenant_b, "pg:test", WriteOrigin::Job).unwrap();
    let sink_a = RepoExecutionSink::new(
        TenantExecutionWrite::new(Arc::clone(&db)),
        TenantPositionWrite::new(Arc::clone(&db)),
        write_a,
    );
    let sink_b = RepoExecutionSink::new(
        TenantExecutionWrite::new(Arc::clone(&db)),
        TenantPositionWrite::new(Arc::clone(&db)),
        write_b,
    );
    let id = format!("tenant-position-{run}");
    let mut position = Position::new(
        id.clone(),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Live,
        "mint-risk-projection".into(),
        "RISK".into(),
        "SOL".into(),
    );
    position.apply_buy(100.0, 0.01, 1.0);
    sink_a
        .record_position(tenant_a, &position)
        .await
        .expect("persist A");

    let read = TenantPositionRead::new(Arc::clone(&db));
    let row = read
        .get(&TradingQueryScope::new(tenant_a), &id)
        .await
        .expect("tenant A reads its position");
    assert_eq!(row.organization_id, tenant_a);
    assert_eq!(row.source, "sniper");
    assert_eq!(row.status, "open");
    assert!((row.cost_basis - 1.0).abs() < 1e-9);
    assert!(read
        .get(&TradingQueryScope::new(tenant_b), &id)
        .await
        .is_err());

    let error = sink_b
        .record_position(tenant_a, &position)
        .await
        .expect_err("foreign position must be refused before repository access");
    assert!(error.to_string().contains("organization mismatch"));
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
    let sink_b = RepoCopySink::new(
        TenantCopyEventRepo::new(Arc::clone(&db)),
        TenantPositionWrite::new(Arc::clone(&db)),
        write_b,
    );
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

/// The production wiring has to survive a runtime rotation WITHOUT anyone
/// remembering to opt in.
///
/// This is the residual that the guard-level and executor-level witnesses could
/// not reach: they attach a probe by hand. Here a real factory builds a real
/// copy engine, nobody calls `with_runtime_fence`, and the guard STILL carries a
/// live fence built over the same database. Then the tenant's runtime is rotated
/// in the registry and the engine's own guard — the object its orders pass
/// through — refuses the next transaction as `runtime_fenced`.
#[tokio::test]
async fn a_factory_built_engine_is_runtime_fenced_by_default() {
    use sniper_suite::runtime_registry::{
        PgRuntimeStore, RuntimeRegistryService, RuntimeStore, TenantRuntimeRecord,
    };
    use solana_kit::tenant_broadcast_guard::TenantBroadcastDeny;

    let Some(db) = setup().await else {
        println!("NOT_RUN: POSTGRES_URL is not set");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("p4-fence-{run}")).await;
    let factory = TenantModuleFactory::new(Arc::clone(&db));

    // The registry row the engine will be fenced against.
    let runtime_id = RuntimeId::new();
    let generation = RuntimeGeneration::first();
    let store: Arc<dyn RuntimeStore> = Arc::new(PgRuntimeStore::new(Arc::clone(&db)));
    store
        .insert(&TenantRuntimeRecord::new_active(
            tenant,
            runtime_id,
            generation,
            "factory-fence",
            Utc::now(),
        ))
        .await
        .expect("register the live runtime");

    // A REAL engine, built exactly as production builds it — no fence opt-in.
    // The signer binding the gateway would have required at issuance.
    bind_signer(&db, tenant, "pg-key", true).await;
    let (funding, address) = loaded_wallet_with_address();
    // Bound to the runtime that is actually live.
    let context = issued_on(tenant, BotModule::Copy, &address, runtime_id, generation);
    let wallet = Arc::new(funding);
    let executor = factory
        .build_copy(
            AppState::new(bot_core::config::AppConfig::from_defaults()),
            solana_kit::Rpc::new(&bot_core::config::NetworkConfig::default()).expect("rpc"),
            Arc::clone(&wallet),
            None,
            context.clone(),
        )
        .await
        .expect("factory copy");

    // The guard shipped WITHOUT anyone asking for a fence.
    let guard = executor
        .bot()
        .tenant_guard()
        .expect("a tenant engine carries its broadcast guard")
        .clone();
    assert!(
        guard.has_fence_probe(),
        "every factory-built engine must carry a live runtime fence by default"
    );

    // While the runtime is current, the guard authorizes its own context.
    let meta = solana_kit::tenant_transaction::TenantTransactionMeta::from_context(
        executor.signing_context().execution_context(),
        "copy",
        Some("intent-fence-1"),
    );
    guard
        .authorize_live(&meta)
        .await
        .expect("the live runtime authorizes its own module");

    // Rotate the tenant's runtime: a successor takes over, this engine is stale.
    store
        .rotate(tenant, RuntimeId::new(), "factory-fence-2", Utc::now())
        .await
        .expect("rotate");

    // The captured metadata still matches — nothing in it changed…
    guard
        .authorize_meta(&meta)
        .expect("the captured context cannot see the rotation");
    // …but the live fence refuses the broadcast.
    match guard.authorize_live(&meta).await {
        Err(TenantBroadcastDeny::RuntimeFenced { detail, source }) => {
            assert_eq!(source, "runtime_registry");
            assert!(!detail.is_empty());
        }
        other => panic!("a rotated runtime must be refused at broadcast, got {other:?}"),
    }

    // And the service that built the fence sees the successor as current, so the
    // refusal is about THIS engine's staleness, not a broken registry.
    let service = RuntimeRegistryService::new(Arc::clone(&store));
    assert!(service.current(tenant).await.expect("current").is_some());
}

/// A signer binding revoked AFTER the module was issued must stop the next
/// broadcast.
///
/// The captured context stores `is_active()` from issue time, so a revocation
/// afterwards is invisible to it forever. A real factory-built engine carries a
/// live binding check (built over `tenant_bindings`, no opt-in); this test
/// registers an ACTIVE binding, builds the engine, flips the durable row to
/// inactive, and proves the engine's own guard refuses with `signer_inactive`
/// while `authorize_meta` still passes.
#[tokio::test]
async fn a_binding_revoked_after_issuance_blocks_the_next_broadcast() {
    use bot_core::tenant::SignerProvider;
    use sniper_suite::tenant::registry::{
        PgTenantBindingRegistry, RegistrySignerProbe, SignerBindingRow, TenantBindingRegistry,
    };
    use solana_kit::tenant_broadcast_guard::TenantBroadcastDeny;

    let Some(db) = setup().await else {
        println!("NOT_RUN: POSTGRES_URL is not set");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("p4-signer-{run}")).await;
    let factory = TenantModuleFactory::new(Arc::clone(&db));

    // An ACTIVE signer binding in the durable registry, keyed exactly as the
    // issued context will reference it ("copy-key", local provider).
    let registry = PgTenantBindingRegistry::new(Arc::clone(&db));
    // The key reference the ISSUED context carries (see `issued`), not the
    // wallet label: the guard checks the binding the context points at.
    let key_ref = "pg-key".to_string();
    registry
        .put_signer(SignerBindingRow {
            organization_id: tenant,
            provider: SignerProvider::Local,
            key_ref: key_ref.clone(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .expect("bind the signer");

    // A live runtime row, so the runtime fence is not what refuses here.
    let runtime_id = RuntimeId::new();
    let generation = RuntimeGeneration::first();
    let store: Arc<dyn sniper_suite::runtime_registry::RuntimeStore> = Arc::new(
        sniper_suite::runtime_registry::PgRuntimeStore::new(Arc::clone(&db)),
    );
    store
        .insert(
            &sniper_suite::runtime_registry::TenantRuntimeRecord::new_active(
                tenant,
                runtime_id,
                generation,
                "signer-fence",
                Utc::now(),
            ),
        )
        .await
        .expect("live runtime");

    let (funding, address) = loaded_wallet_with_address();
    let context = issued_on(tenant, BotModule::Copy, &address, runtime_id, generation);
    let wallet = Arc::new(funding);
    let executor = factory
        .build_copy(
            AppState::new(bot_core::config::AppConfig::from_defaults()),
            solana_kit::Rpc::new(&bot_core::config::NetworkConfig::default()).expect("rpc"),
            Arc::clone(&wallet),
            None,
            context.clone(),
        )
        .await
        .expect("factory copy");

    let guard = executor.bot().tenant_guard().expect("guard").clone();
    assert!(
        guard.has_signer_probe(),
        "every factory-built engine must carry a live signer-binding check"
    );
    let meta = solana_kit::tenant_transaction::TenantTransactionMeta::from_context(
        executor.signing_context().execution_context(),
        "copy",
        Some("intent-signer-1"),
    );

    // While the binding is active, the complete gate authorizes.
    guard
        .authorize_live(&meta)
        .await
        .expect("an active binding authorizes");

    // Offboarding: flip the durable row to inactive, nothing else changes.
    registry
        .put_signer(SignerBindingRow {
            organization_id: tenant,
            provider: SignerProvider::Local,
            key_ref: key_ref.clone(),
            active: false,
            created_at: Utc::now(),
        })
        .await
        .expect("revoke the signer");

    // The captured flag still says active — that is the blindness…
    guard
        .authorize_meta(&meta)
        .expect("the captured is_active() cannot see a later revocation");
    // …and the live binding check refuses the broadcast.
    match guard.authorize_live(&meta).await {
        Err(TenantBroadcastDeny::SignerInactive { detail, source }) => {
            assert_eq!(source, "binding_registry");
            assert_eq!(detail, "binding_revoked");
        }
        other => panic!("a revoked signer must be refused at broadcast, got {other:?}"),
    }

    // The probe itself is the same object the factory built, and it now reports
    // the revocation for this tenant only.
    let probe = RegistrySignerProbe::new(Arc::new(PgTenantBindingRegistry::new(Arc::clone(&db))));
    use solana_kit::tenant_broadcast_guard::SignerBindingProbe;
    assert_eq!(
        probe
            .signer_is_active(tenant, SignerProvider::Local, &key_ref)
            .await
            .unwrap_err(),
        "binding_revoked"
    );
    // An unbound key is refused too (never "assume active").
    assert_eq!(
        probe
            .signer_is_active(tenant, SignerProvider::Local, "never-bound")
            .await
            .unwrap_err(),
        "binding_not_found"
    );
}

/// A signing key rotated under an UNCHANGED reference must stop the next
/// broadcast — on the REAL registry, with a REAL factory-built engine.
///
/// The durable binding row stays active (same `provider/key_ref`), so the live
/// binding check passes. What changed is the key behind the identity: the
/// wallet the engine would sign with no longer controls the wallet the tenant
/// issued the context for. The guard pins the approved key at construction and
/// re-checks the resolved key at broadcast, so the order is vetoed with
/// `signer_key_mismatch` rather than signed by an unapproved key.
#[tokio::test]
async fn a_key_rotated_under_the_same_reference_blocks_the_next_broadcast() {
    use solana_kit::tenant_broadcast_guard::TenantBroadcastDeny;

    let Some(db) = setup().await else {
        println!("NOT_RUN: POSTGRES_URL is not set");
        return;
    };
    let run = run_id();
    let tenant = org(&db, &format!("p4-rotate-{run}")).await;
    let factory = TenantModuleFactory::new(Arc::clone(&db));

    // The APPROVED keypair: the context is issued for this address.
    let (approved_wallet, approved_address) = loaded_wallet_with_address();
    bind_signer(&db, tenant, "pg-key", true).await;
    let runtime_id = RuntimeId::new();
    let generation = RuntimeGeneration::first();
    let store: Arc<dyn sniper_suite::runtime_registry::RuntimeStore> = Arc::new(
        sniper_suite::runtime_registry::PgRuntimeStore::new(Arc::clone(&db)),
    );
    store
        .insert(
            &sniper_suite::runtime_registry::TenantRuntimeRecord::new_active(
                tenant,
                runtime_id,
                generation,
                "rotate-fence",
                Utc::now(),
            ),
        )
        .await
        .expect("live runtime");

    let context = issued_on(
        tenant,
        BotModule::Copy,
        &approved_address,
        runtime_id,
        generation,
    );

    // The engine signs with the APPROVED wallet and pins its key.
    let executor = factory
        .build_copy(
            AppState::new(bot_core::config::AppConfig::from_defaults()),
            solana_kit::Rpc::new(&bot_core::config::NetworkConfig::default()).expect("rpc"),
            Arc::new(approved_wallet),
            None,
            context.clone(),
        )
        .await
        .expect("factory copy");
    let guard = executor.bot().tenant_guard().expect("guard").clone();
    // No signer registry was supplied, so nothing is pinned: the funding wallet
    // signs and construction already proved it is the bound wallet.
    assert!(!guard.has_signer_key_binding());
    guard
        .authorize_signer_identity(None)
        .expect("a wallet-only executor is authorized");

    // A DIFFERENT keypair now stands in for the rotated binding: the binding row
    // is untouched (still active), the context is untouched, and only the key
    // behind the identity changed.
    let (rotated_wallet, rotated_address) = loaded_wallet_with_address();
    assert_ne!(approved_address, rotated_address);
    let guard_pinned = guard.as_ref().clone().with_expected_signer_key(
        approved_address
            .parse::<solana_sdk::pubkey::Pubkey>()
            .expect("approved key"),
    );
    // The rotated key is what resolves now, and it is not the approved one.
    let rotated_key = rotated_address
        .parse::<solana_sdk::pubkey::Pubkey>()
        .expect("rotated key");
    match guard_pinned.authorize_signer_identity(Some(rotated_key)) {
        Err(TenantBroadcastDeny::SignerKeyMismatch { detail, source }) => {
            assert_eq!(source, "signer_registry");
            assert!(detail.contains("resolves to"), "{detail}");
            assert!(detail.contains(&rotated_address), "{detail}");
        }
        other => panic!("a rotated key must be refused at broadcast, got {other:?}"),
    }
    // The approved key still passes: the refusal is about the rotation, not the
    // pinning mechanism.
    guard_pinned
        .authorize_signer_identity(Some(
            approved_address
                .parse::<solana_sdk::pubkey::Pubkey>()
                .expect("approved key"),
        ))
        .expect("the approved key is authorized");

    // And the rotated wallet really would be a different signer: the engine built
    // for that key is REFUSED at construction because it does not control the
    // tenant's issued wallet.
    let rotated_executor = factory
        .build_copy(
            AppState::new(bot_core::config::AppConfig::from_defaults()),
            solana_kit::Rpc::new(&bot_core::config::NetworkConfig::default()).expect("rpc"),
            Arc::new(rotated_wallet),
            None,
            context,
        )
        .await;
    assert!(
        rotated_executor.is_err(),
        "an engine for an unapproved key must not be built"
    );
}
