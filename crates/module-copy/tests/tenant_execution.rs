//! Tenant execution boundary for module-copy (PROMPT 4/10 §C).
//!
//! Most tests are hermetic and run in paper mode. The explicitly gated
//! shared Redis witness requires REDIS_URL and never sends a transaction.

use std::sync::Arc;

use bot_core::config::{AppConfig, Config, NetworkConfig};
use bot_core::execution::{
    AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
};
use bot_core::models::{BotModule, ExecutionMode, PositionSide, Venue, WalletTrade};
use bot_core::state::AppState;
use bot_core::tenant::{
    OrganizationId, RuntimeGeneration, RuntimeId, SignerProvider, TenantSignerRef, TenantWalletRef,
};
use chrono::Utc;
use module_copy::tenant_context::CopyTenantContext;
use module_copy::tenant_executor::{TenantCopyExecutor, TenantCopySink};
use module_copy::CopyBot;
use solana_kit::tokens::Wallet;

/// Load a REAL wallet from a generated keypair (inline base58) — the
/// same production load path, no fixtures on disk.
fn loaded_wallet() -> Wallet {
    let kp = solana_sdk::signature::Keypair::new();
    let b58 = bs58::encode(kp.to_bytes()).into_string();
    Wallet::load(&b58).expect("wallet must load from a fresh keypair")
}

/// Issue a REAL tenant execution context (module = Copy, paper) bound
/// to `wallet`'s address. Every unwrap in this path must succeed: the
/// authority checklist is fully satisfied and the identity is sound.
fn issued_for_wallet(
    org: OrganizationId,
    runtime: RuntimeId,
    generation: RuntimeGeneration,
    address: &str,
) -> TenantExecutionContext {
    let scope = bot_core::execution::ExecutionScope::new(
        org,
        runtime,
        generation,
        BotModule::Copy,
        ExecutionMode::Paper,
    )
    .unwrap();
    let mut checklist = AuthorityChecklist::new();
    let now = Utc::now();
    for name in AUTHORITY_CHECK_ORDER {
        checklist.record(name, now).unwrap();
    }
    let authority = checklist.finish(&scope, now).unwrap();
    let wallet = TenantWalletRef::new(org, address).unwrap();
    let signer = TenantSignerRef::new(org, SignerProvider::Local, "copy-key").unwrap();
    TenantExecutionContext::issue(
        org,
        runtime,
        generation,
        BotModule::Copy,
        ExecutionMode::Paper,
        authority,
        wallet,
        signer,
        ExecutionTrace::for_request(),
    )
    .unwrap()
}

/// Build a tenant copy executor whose funding wallet is freshly
/// generated and bound into the issued context (they always match).
async fn executor_bound_to(
    org: OrganizationId,
    runtime: RuntimeId,
) -> (TenantCopyExecutor, String) {
    let wallet = Arc::new(loaded_wallet());
    let address = wallet.pubkey.to_string();
    let executor = TenantCopyExecutor::new(
        AppState::new(AppConfig::from_defaults()),
        solana_kit::Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        wallet,
        None,
        issued_for_wallet(org, runtime, RuntimeGeneration::first(), &address),
    )
    .await
    .expect("tenant copy executor must construct with a matching wallet");
    (executor, address)
}

fn leader_trade(leader: &str, signature: &str) -> WalletTrade {
    WalletTrade {
        wallet: leader.to_string(),
        signature: signature.to_string(),
        slot: 1,
        block_time: None,
        side: PositionSide::Long,
        mint: "11111111111111111111111111111111".to_string(),
        symbol: None,
        token_amount: 1.0,
        sol_amount: 0.001,
        venue: Venue::PumpFun,
        fee_sol: 0.0,
        discriminator: None,
        observed_at: Utc::now(),
    }
}

#[tokio::test]
async fn a_tenant_copy_executor_denies_foreign_contexts() {
    let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
    let (executor, address) = executor_bound_to(org, runtime).await;

    // Same identity: authorized.
    assert!(executor
        .authorize(&issued_for_wallet(
            org,
            runtime,
            RuntimeGeneration::first(),
            &address
        ))
        .is_ok());

    // Foreign organization.
    let err = executor
        .authorize(&issued_for_wallet(
            OrganizationId::new(),
            runtime,
            RuntimeGeneration::first(),
            &address,
        ))
        .unwrap_err();
    assert_eq!(err.as_str(), "organization_mismatch");

    // Stale generation (rotated runtime).
    let err = executor
        .authorize(&issued_for_wallet(
            org,
            runtime,
            RuntimeGeneration::first().next().unwrap(),
            &address,
        ))
        .unwrap_err();
    assert_eq!(err.as_str(), "generation_mismatch");

    // Wrong wallet.
    let other = loaded_wallet().pubkey.to_string();
    let err = executor
        .authorize(&issued_for_wallet(
            org,
            runtime,
            RuntimeGeneration::first(),
            &other,
        ))
        .unwrap_err();
    assert_eq!(err.as_str(), "wallet_mismatch");

    // The runtime fence denies rotated identities too.
    assert!(executor
        .verify_runtime(org, runtime, RuntimeGeneration::first().next().unwrap())
        .is_err());
}

#[tokio::test]
async fn a_wallet_bound_to_another_tenant_cannot_drive_the_copy_executor() {
    let wallet = loaded_wallet();
    let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
    // The guard binds the CONTEXT's wallet; constructing the executor
    // with a DIFFERENT funding wallet must fail at build time.
    let context_wallet = loaded_wallet();
    let err = TenantCopyExecutor::new(
        AppState::new(AppConfig::from_defaults()),
        solana_kit::Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::new(wallet),
        None,
        issued_for_wallet(
            org,
            runtime,
            RuntimeGeneration::first(),
            &context_wallet.pubkey.to_string(),
        ),
    )
    .await
    .err()
    .expect("mismatched funding wallet must be refused");
    // The guard refuses at construction: the funding wallet is not the
    // wallet bound in the context.
    assert!(
        err.to_string().contains("wallet_mismatch") || err.to_string().contains("does not match"),
        "{err}"
    );
}

#[tokio::test]
async fn the_same_leader_trade_is_deduplicated_tenant_locally() {
    let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
    let (mut executor, _address) = executor_bound_to(org, runtime).await;

    let cfg = Config::default();
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    let trade = leader_trade(leader, "sig-tenant-a-1");

    // First sight: the pipeline RUNS (it rejects the leader as not
    // followed, which is expected with an empty registry) — the point
    // is that the tenant saw the event.
    let first = executor.process_trade(&trade, "test", &cfg).await;
    assert_ne!(
        first.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT"),
        "the first sight must not be a tenant duplicate"
    );

    // Second sight of the SAME external trade: tenant-local duplicate,
    // refused before the pipeline runs.
    let second = executor.process_trade(&trade, "test", &cfg).await;
    let rejection = second.rejection.expect("duplicate must carry a rejection");
    assert_eq!(rejection.reason.as_str(), "DUPLICATE_EVENT");
    assert!(rejection.detail.contains(org.to_string().as_str()));

    // A DIFFERENT trade from the same leader is not a duplicate.
    let third = executor
        .process_trade(&leader_trade(leader, "sig-tenant-a-2"), "test", &cfg)
        .await;
    assert_ne!(
        third.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT")
    );
}

#[derive(Default)]
struct RecordingSink(std::sync::Mutex<Vec<module_copy::event::CopyOutcome>>);

#[async_trait::async_trait]
impl TenantCopySink for RecordingSink {
    async fn record_outcome(
        &self,
        _context: &CopyTenantContext,
        _event: &module_copy::event::LeaderTradeEvent,
        outcome: &module_copy::event::CopyOutcome,
    ) -> bot_core::error::BotResult<()> {
        self.0.lock().unwrap().push(outcome.clone());
        Ok(())
    }
}

#[tokio::test]
async fn tenant_dedup_distinguishes_same_signature_mint_and_side() {
    let (mut executor, _) = executor_bound_to(OrganizationId::new(), RuntimeId::new()).await;
    let cfg = Config::default();
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    let first = leader_trade(leader, "same-signature");
    let mut other_mint = first.clone();
    other_mint.mint = solana_sdk::pubkey::Pubkey::new_unique().to_string();
    let mut other_side = first.clone();
    other_side.side = PositionSide::Short;
    for trade in [&first, &other_mint, &other_side] {
        let result = executor.process_trade(trade, "test", &cfg).await;
        assert_ne!(
            result.rejection.as_ref().map(|r| r.reason.as_str()),
            Some("DUPLICATE_EVENT")
        );
    }
    assert_eq!(executor.tenant_state().dedup_len(), 3);
    let duplicate = executor.process_trade(&other_mint, "test", &cfg).await;
    assert_eq!(
        duplicate.rejection.unwrap().reason.as_str(),
        "DUPLICATE_EVENT"
    );
}

#[tokio::test]
async fn tenant_duplicate_does_not_overwrite_first_sink_outcome() {
    let (executor, _) = executor_bound_to(OrganizationId::new(), RuntimeId::new()).await;
    let sink = Arc::new(RecordingSink::default());
    let mut executor = executor.with_copy_sink(sink.clone());
    let cfg = Config::default();
    let trade = leader_trade("9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM", "same-sig");
    let first = executor.process_trade(&trade, "test", &cfg).await;
    let duplicate = executor.process_trade(&trade, "test", &cfg).await;
    assert_eq!(
        duplicate.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT")
    );
    let records = sink.0.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event_id, first.event_id);
    assert_eq!(records[0].reason_label(), first.reason_label());
}

#[tokio::test]
async fn tenant_unavailable_shared_dedup_retries_without_journaling() {
    let (executor, _) = executor_bound_to(OrganizationId::new(), RuntimeId::new()).await;
    let sink = Arc::new(RecordingSink::default());
    let mut executor = executor.with_copy_sink(sink.clone());
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    let mut cfg = Config::default();
    cfg.copy.wallets = vec![bot_core::config::CopyWallet {
        address: leader.into(),
        ..Default::default()
    }];
    cfg.ha.mode = "active_active".into();
    cfg.execution.mode = ExecutionMode::Simulate;
    executor.bot_mut().sync_leaders(&cfg).await;
    let trade = leader_trade(leader, "unavailable-signature");
    for _ in 0..2 {
        let result = executor.process_trade(&trade, "test", &cfg).await;
        assert_eq!(
            result.rejection.as_ref().map(|r| r.reason.as_str()),
            Some("DEDUP_UNAVAILABLE")
        );
        assert_eq!(executor.tenant_state().dedup_len(), 0);
    }
    assert!(
        sink.0.lock().unwrap().is_empty(),
        "unknown outcomes must not reach tenant event upsert"
    );
}

#[tokio::test]
async fn two_tenants_seeing_the_same_leader_trade_both_process_it() {
    let (org_a, runtime_a) = (OrganizationId::new(), RuntimeId::new());
    let (org_b, runtime_b) = (OrganizationId::new(), RuntimeId::new());
    let (mut a, _a_addr) = executor_bound_to(org_a, runtime_a).await;
    let (mut b, _b_addr) = executor_bound_to(org_b, runtime_b).await;

    let cfg = Config::default();
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    let trade = leader_trade(leader, "sig-shared-1");

    // The SAME external event is new for BOTH tenants: neither tenant's
    // dedup state suppressed the other.
    let out_a = a.process_trade(&trade, "test", &cfg).await;
    let out_b = b.process_trade(&trade, "test", &cfg).await;
    assert_ne!(
        out_a.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT")
    );
    assert_ne!(
        out_b.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT")
    );

    // And each tenant suppresses its OWN replay.
    assert_eq!(
        a.process_trade(&trade, "test", &cfg)
            .await
            .rejection
            .expect("replay must be rejected")
            .reason
            .as_str(),
        "DUPLICATE_EVENT"
    );
    assert_eq!(
        b.process_trade(&trade, "test", &cfg)
            .await
            .rejection
            .expect("replay must be rejected")
            .reason
            .as_str(),
        "DUPLICATE_EVENT"
    );
}

/// A test sink acknowledges a position projection without polling or sleeping.
struct PositionWitnessSink(
    std::sync::Mutex<Option<tokio::sync::oneshot::Sender<(OrganizationId, String)>>>,
);

#[async_trait::async_trait]
impl TenantCopySink for PositionWitnessSink {
    async fn record_outcome(
        &self,
        _context: &CopyTenantContext,
        _event: &module_copy::event::LeaderTradeEvent,
        _outcome: &module_copy::event::CopyOutcome,
    ) -> bot_core::error::BotResult<()> {
        Ok(())
    }

    async fn record_position(
        &self,
        context: &CopyTenantContext,
        position: &bot_core::models::Position,
    ) -> bot_core::error::BotResult<()> {
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send((context.organization_id(), position.id.clone()));
        }
        Ok(())
    }
}

/// Witness current unsafe behavior, not an isolation guarantee. Position has
/// no tenant identity, so sharing AppState causes each tenant's startup pump
/// to attribute the same pre-existing copy position to its own context.
#[tokio::test]
async fn shared_app_state_currently_projects_one_position_into_two_tenant_sinks() {
    use bot_core::models::{Position, TradeSource};
    let shared = AppState::new(AppConfig::from_defaults());
    let position = Position::new(
        "shared-position".into(),
        TradeSource::Copy,
        Venue::PumpFun,
        ExecutionMode::Paper,
        "mint".into(),
        "mint".into(),
        "SOL".into(),
    );
    shared.upsert_position(position).await;
    let org_a = OrganizationId::new();
    let org_b = OrganizationId::new();
    let wallet_a = Arc::new(loaded_wallet());
    let wallet_b = Arc::new(loaded_wallet());
    let (tx_a, rx_a) = tokio::sync::oneshot::channel();
    let (tx_b, rx_b) = tokio::sync::oneshot::channel();
    let _a = TenantCopyExecutor::new(
        shared.clone(),
        solana_kit::Rpc::new(&NetworkConfig::default()).unwrap(),
        wallet_a.clone(),
        None,
        issued_for_wallet(
            org_a,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            &wallet_a.pubkey.to_string(),
        ),
    )
    .await
    .unwrap()
    .with_copy_sink(Arc::new(PositionWitnessSink(std::sync::Mutex::new(Some(
        tx_a,
    )))));
    let _b = TenantCopyExecutor::new(
        shared,
        solana_kit::Rpc::new(&NetworkConfig::default()).unwrap(),
        wallet_b.clone(),
        None,
        issued_for_wallet(
            org_b,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            &wallet_b.pubkey.to_string(),
        ),
    )
    .await
    .unwrap()
    .with_copy_sink(Arc::new(PositionWitnessSink(std::sync::Mutex::new(Some(
        tx_b,
    )))));
    let a = tokio::time::timeout(std::time::Duration::from_secs(5), rx_a)
        .await
        .expect("A projection stalled")
        .expect("A sink closed");
    let b = tokio::time::timeout(std::time::Duration::from_secs(5), rx_b)
        .await
        .expect("B projection stalled")
        .expect("B sink closed");
    assert_eq!(a, (org_a, "shared-position".to_string()));
    assert_eq!(b, (org_b, "shared-position".to_string()));
}

/// Witness current cross-tenant collision with independent AppStates and a
/// real Redis backend. This is a known defect, not expected safe behavior.
#[tokio::test]
async fn a_shared_redis_dedup_does_not_collide_across_tenants() {
    use bot_core::config::RedisConfig;
    use bot_core::dedup::{DedupBackend, DedupStore};
    use bot_core::redis_kv::RedisKv;

    let Ok(url) = std::env::var("REDIS_URL") else {
        assert_ne!(
            std::env::var("REQUIRE_SERVICES").as_deref(),
            Ok("1"),
            "REDIS_URL is required"
        );
        eprintln!("SKIP: REDIS_URL is not set — real Redis tenant collision witness");
        return;
    };
    let redis_a = RedisKv::connect(&RedisConfig::default(), &url)
        .await
        .expect("Redis A");
    let redis_b = RedisKv::connect(&RedisConfig::default(), &url)
        .await
        .expect("Redis B");
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    for mode in [ExecutionMode::Paper, ExecutionMode::Simulate] {
        let mut app_cfg = AppConfig::from_defaults();
        app_cfg.raw.copy.wallets = vec![bot_core::config::CopyWallet {
            address: leader.into(),
            ..Default::default()
        }];
        app_cfg.raw.execution.mode = mode;
        app_cfg.raw.ha.mode = "active_active".into();
        let cfg = app_cfg.raw.clone();
        let a_state = AppState::new(app_cfg.clone());
        let b_state = AppState::new(app_cfg);
        a_state.attach_dedup(DedupStore::new(
            DedupBackend::Redis,
            Some(redis_a.clone()),
            None,
            std::time::Duration::from_secs(60),
            1000,
        ));
        b_state.attach_dedup(DedupStore::new(
            DedupBackend::Redis,
            Some(redis_b.clone()),
            None,
            std::time::Duration::from_secs(60),
            1000,
        ));
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        let wallet_a = Arc::new(loaded_wallet());
        let wallet_b = Arc::new(loaded_wallet());
        let mut a = TenantCopyExecutor::new(
            a_state,
            solana_kit::Rpc::new(&NetworkConfig::default()).unwrap(),
            wallet_a.clone(),
            None,
            issued_for_wallet(
                org_a,
                RuntimeId::new(),
                RuntimeGeneration::first(),
                &wallet_a.pubkey.to_string(),
            ),
        )
        .await
        .unwrap();
        let mut b = TenantCopyExecutor::new(
            b_state,
            solana_kit::Rpc::new(&NetworkConfig::default()).unwrap(),
            wallet_b.clone(),
            None,
            issued_for_wallet(
                org_b,
                RuntimeId::new(),
                RuntimeGeneration::first(),
                &wallet_b.pubkey.to_string(),
            ),
        )
        .await
        .unwrap();
        let trade = leader_trade(leader, &format!("redis-tenant-witness-{org_a}-{org_b}"));
        let first = a.process_trade(&trade, "test", &cfg).await;
        assert_ne!(
            first.rejection.as_ref().map(|r| r.reason.as_str()),
            Some("DUPLICATE_EVENT"),
            "mode {mode:?}"
        );
        // Same trade, SECOND tenant, SAME Redis facade: its own namespace.
        let second = b.process_trade(&trade, "test", &cfg).await;
        assert_ne!(
            second.rejection.as_ref().map(|r| r.reason.as_str()),
            Some("DUPLICATE_EVENT"),
            "tenant B must mirror the same trade independently in mode {mode:?}: {:?}",
            second.rejection
        );
        // A tenant's own repeat through the shared Redis is still deduped.
        let repeat = b.process_trade(&trade, "test", &cfg).await;
        let rejection = repeat.rejection.expect("tenant B's own repeat is rejected");
        assert_eq!(
            rejection.reason.as_str(),
            "DUPLICATE_EVENT",
            "mode {mode:?}"
        );
        // A tenant's own repeat is caught by the tenant-local prefilter first
        // (the pipeline never sees it, so the shared Redis is never asked);
        // either way the duplicate is scoped to THIS organization and never
        // to the bare operator key.
        assert!(
            rejection
                .detail
                .contains("tenant-local duplicate for organization")
                || rejection
                    .detail
                    .contains("already decided (dedup key tenant:"),
            "unexpected detail: {}",
            rejection.detail
        );
        assert!(
            !rejection.detail.contains("(dedup key copy:"),
            "the bare operator key must not appear"
        );
        assert_eq!(a.tenant_state().dedup_len(), 1);
        assert_eq!(b.tenant_state().dedup_len(), 1);
    }
}

/// The same leader trade reaching two tenants that share one in-memory
/// `AppState` must be TWO independent mirrors. Before the tenant namespace
/// fix the second tenant was rejected as `DUPLICATE_EVENT` by the shared
/// pipeline even though its tenant-local prefilter had accepted it (the
/// witness recorded that defect); now each tenant claims its own key
/// (`tenant:{org}:copy:…`) and both proceed.
#[tokio::test]
async fn shared_app_state_no_longer_collides_across_tenants() {
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    let mut app_cfg = AppConfig::from_defaults();
    app_cfg.raw.copy.wallets = vec![bot_core::config::CopyWallet {
        address: leader.into(),
        ..Default::default()
    }];
    let cfg = app_cfg.raw.clone();
    let shared = AppState::new(app_cfg);
    let wallet_a = Arc::new(loaded_wallet());
    let wallet_b = Arc::new(loaded_wallet());
    let mut a = TenantCopyExecutor::new(
        shared.clone(),
        solana_kit::Rpc::new(&NetworkConfig::default()).unwrap(),
        wallet_a.clone(),
        None,
        issued_for_wallet(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            &wallet_a.pubkey.to_string(),
        ),
    )
    .await
    .unwrap();
    let mut b = TenantCopyExecutor::new(
        shared,
        solana_kit::Rpc::new(&NetworkConfig::default()).unwrap(),
        wallet_b.clone(),
        None,
        issued_for_wallet(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            &wallet_b.pubkey.to_string(),
        ),
    )
    .await
    .unwrap();
    let trade = leader_trade(leader, "shared-state-isolation");
    let first = a.process_trade(&trade, "test", &cfg).await;
    assert_ne!(
        first.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT"),
        "tenant A must process the trade"
    );
    let second = b.process_trade(&trade, "test", &cfg).await;
    assert_ne!(
        second.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT"),
        "tenant B mirrors the same leader trade independently: {:?}",
        second.rejection
    );
    // Each tenant still dedups its OWN repeat of the same trade.
    let again = a.process_trade(&trade, "test", &cfg).await;
    assert_eq!(
        again.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("DUPLICATE_EVENT"),
        "a tenant's own repeat is still a duplicate"
    );
    assert_eq!(a.tenant_state().dedup_len(), 1);
    assert_eq!(b.tenant_state().dedup_len(), 1);
}

#[tokio::test]
async fn an_unbound_copy_bot_preserves_the_operator_behaviour() {
    // No tenant context: the bot must construct and run exactly as
    // before (verified by construction + the absence of tenant state).
    let mut bot = CopyBot::new(
        AppState::new(AppConfig::from_defaults()),
        solana_kit::Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::new(loaded_wallet()),
        None,
    )
    .await;
    assert!(bot.tenant_context().is_none());
    // The shared pipeline still processes events (hermetic paper path).
    let cfg = Config::default();
    let trade = leader_trade(
        "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM",
        "sig-operator-1",
    );
    let event = bot.event_from_trade(&trade, "test");
    let outcome = bot.process_event(&event, &cfg).await;
    // An unknown leader is refused by the pipeline — NOT by a tenant
    // guard (no guard exists here).
    let reason = outcome
        .rejection
        .as_ref()
        .map(|r| r.reason.as_str())
        .unwrap_or("none");
    assert_ne!(reason, "DUPLICATE_EVENT");
    assert!(outcome.rejection.is_some(), "unknown leader is refused");
}

#[tokio::test]
async fn the_context_adapter_scopes_leader_and_event_keys_per_tenant() {
    let (org_a, runtime_a) = (OrganizationId::new(), RuntimeId::new());
    let (org_b, runtime_b) = (OrganizationId::new(), RuntimeId::new());
    let address = loaded_wallet().pubkey.to_string();
    let a = CopyTenantContext::adapt(issued_for_wallet(
        org_a,
        runtime_a,
        RuntimeGeneration::first(),
        &address,
    ))
    .unwrap();
    let b = CopyTenantContext::adapt(issued_for_wallet(
        org_b,
        runtime_b,
        RuntimeGeneration::first(),
        &address,
    ))
    .unwrap();
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    assert_ne!(a.leader_key(leader), b.leader_key(leader));
    assert_ne!(a.event_key(leader, "sig-1"), b.event_key(leader, "sig-1"));
    // Copy is paper by default unless issued live.
    assert!(!a.is_live());
    assert_eq!(a.signer_identity(), "copy-key");
}

#[tokio::test]
async fn tenant_process_trade_uses_current_leader_pause_and_unfollow_config() {
    let (mut executor, _) = executor_bound_to(OrganizationId::new(), RuntimeId::new()).await;
    let leader = "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM";
    let mut cfg = Config::default();
    cfg.copy.wallets = vec![bot_core::config::CopyWallet {
        address: leader.into(),
        paused: true,
        ..Default::default()
    }];
    let first = executor
        .process_trade(&leader_trade(leader, "config-follow"), "test", &cfg)
        .await;
    assert_eq!(
        first.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("LEADER_PAUSED")
    );
    cfg.copy.wallets.clear();
    let second = executor
        .process_trade(&leader_trade(leader, "config-unfollow"), "test", &cfg)
        .await;
    assert_eq!(
        second.rejection.as_ref().map(|r| r.reason.as_str()),
        Some("LEADER_REMOVED")
    );
}
