//! Tenant execution boundary for module-copy (PROMPT 4/10 §C).
//!
//! Hermetic by construction: no network, no database, no clocks. The
//! engine runs in paper mode with a pinned blockhash; every deny is
//! asserted on the machine-readable reason label.

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
use module_copy::tenant_executor::TenantCopyExecutor;
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
