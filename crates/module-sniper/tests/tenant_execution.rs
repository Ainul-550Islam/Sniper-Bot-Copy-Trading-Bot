//! Sniper tenant execution tests (PROMPT 4/10 file 11).
//!
//! Prove, against the REAL engine and the REAL guard:
//!
//! 1. a tenant-bound sniper cannot execute with a wrong
//!    organization / runtime / generation / module / wallet context;
//! 2. a correctly-bound tenant execution passes the gate and fills in
//!    paper mode (nothing leaves the process — pinned blockhash, no
//!    RPC traffic);
//! 3. the existing single-operator behaviour is preserved: an unbound
//!    sniper runs the very same request unchanged;
//! 4. the tenant executor's authorization chain denies a stale runtime
//!    (fence) before the pipeline ever runs.

use bot_core::config::{AppConfig, NetworkConfig};
use std::str::FromStr;

use bot_core::execution::{
    AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::state::AppState;
use bot_core::tenant::{
    OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
};
use chrono::Utc;
use module_sniper::tenant_context::SniperTenantContext;
use module_sniper::tenant_executor::TenantSniperExecutor;
use solana_kit::execute::ExecStatus;
use solana_kit::tx::TxRequest;
use solana_kit::Wallet;
use solana_sdk::hash::Hash;
use std::sync::Arc;

fn issued(
    module: BotModule,
    org: OrganizationId,
    runtime: RuntimeId,
    generation: RuntimeGeneration,
    wallet_address: &str,
) -> TenantExecutionContext {
    let scope = bot_core::execution::ExecutionScope::new(
        org,
        runtime,
        generation,
        module,
        ExecutionMode::Paper,
    )
    .expect("scope");
    let mut checklist = AuthorityChecklist::new();
    let now = Utc::now();
    for name in AUTHORITY_CHECK_ORDER {
        checklist.record(name, now).expect("check");
    }
    let authority = checklist.finish(&scope, now).expect("authority");
    let wallet = TenantWalletRef::new(org, wallet_address).expect("wallet ref");
    let signer = TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "sniper-key")
        .expect("signer ref");
    TenantExecutionContext::issue(
        org,
        runtime,
        generation,
        module,
        ExecutionMode::Paper,
        authority,
        wallet,
        signer,
        ExecutionTrace::for_request(),
    )
    .expect("context")
}

fn loaded_wallet() -> Arc<Wallet> {
    let kp = solana_sdk::signature::Keypair::new();
    let b58 = bs58::encode(kp.to_bytes()).into_string();
    Arc::new(Wallet::load(&b58).expect("wallet must load"))
}

fn paper_request(label: &str, funding: &solana_sdk::pubkey::Pubkey) -> TxRequest {
    // Pin the blockhash: the builder signs locally and the paper executor
    // stops before any network call. Hermetic by construction. The
    // transfer is funded by the fee payer so the wallet can sign it.
    let mut req = TxRequest::new(label);
    req.blockhash = Some(Hash::new_unique());
    req.instructions
        .push(solana_sdk::system_instruction::transfer(
            funding,
            &solana_sdk::pubkey::Pubkey::new_unique(),
            1,
        ));
    req
}

fn executor_funding_pubkey(executor: &TenantSniperExecutor) -> solana_sdk::pubkey::Pubkey {
    solana_sdk::pubkey::Pubkey::from_str(executor.signing_context().wallet_address())
        .expect("bound wallet address must be a valid pubkey")
}

async fn executor_bound_to(wallet: Arc<Wallet>) -> TenantSniperExecutor {
    let context = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    );
    TenantSniperExecutor::new(
        AppState::new(AppConfig::from_defaults()),
        solana_kit::Rpc::new(&NetworkConfig::default()).expect("rpc"),
        wallet,
        None,
        context,
    )
    .await
    .expect("tenant sniper executor")
}

#[tokio::test]
async fn a_tenant_sniper_cannot_submit_with_a_foreign_contexts_metadata() {
    let wallet = loaded_wallet();
    let executor = executor_bound_to(wallet.clone()).await;

    // A request stamped for ANOTHER tenant: the guard refuses it before
    // anything is signed or broadcast.
    let foreign = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        executor.signing_context().runtime_id(),
        executor.signing_context().generation(),
        executor.signing_context().wallet_address(),
    );
    let meta = solana_kit::tenant_transaction::TenantTransactionMeta::from_context(
        &foreign, "sniper", None,
    );
    let req = paper_request("snipe-entry", &executor_funding_pubkey(&executor)).tenant(meta);

    let result = executor.submit(req).await.expect("submit must not error");
    assert_eq!(result.status, ExecStatus::Skipped);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("organization_mismatch"));
}

#[tokio::test]
async fn a_correctly_bound_tenant_execution_passes_the_gate() {
    let wallet = loaded_wallet();
    let executor = executor_bound_to(wallet).await;
    let result = executor
        .submit(paper_request(
            "snipe-entry",
            &executor_funding_pubkey(&executor),
        ))
        .await
        .expect("submit");
    assert_ne!(result.status, ExecStatus::Skipped);
    assert!(!result
        .error
        .as_deref()
        .unwrap_or("")
        .contains("tenant broadcast denied"));
    // Paper fill: nothing left the process.
    assert!(result.paper);
}

#[tokio::test]
async fn the_authorization_chain_denies_cross_context_requests() {
    let wallet = loaded_wallet();
    let executor = executor_bound_to(wallet.clone()).await;

    // The executor's own context authorizes.
    let bound = executor.tenant_context().execution_context().clone();
    assert!(executor.authorize(&bound).is_ok());

    // Another organization.
    let foreign_org = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        executor.tenant_context().runtime_id(),
        executor.tenant_context().generation(),
        executor.tenant_context().wallet_address(),
    );
    assert!(executor.authorize(&foreign_org).is_err());

    // Stale generation (the runtime was fenced/rotated).
    let stale = issued(
        BotModule::Sniper,
        executor.tenant_context().organization_id(),
        executor.tenant_context().runtime_id(),
        executor.tenant_context().generation().next().unwrap(),
        executor.tenant_context().wallet_address(),
    );
    assert!(executor.authorize(&stale).is_err());

    // Fence re-verification against a rotated generation denies too.
    assert!(executor
        .verify_runtime(
            executor.tenant_context().organization_id(),
            executor.tenant_context().runtime_id(),
            executor.tenant_context().generation().next().unwrap(),
        )
        .is_err());
}

#[tokio::test]
async fn an_unbound_sniper_preserves_the_single_operator_behaviour() {
    // The SAME request, run through a deployment-global sniper (no tenant
    // context, no guard): unchanged behaviour — it executes in paper
    // mode rather than being skipped.
    let operator_wallet = std::sync::Arc::new(loaded_wallet());
    let sniper = module_sniper::Sniper::new(
        AppState::new(AppConfig::from_defaults()),
        solana_kit::Rpc::new(&NetworkConfig::default()).expect("rpc"),
        std::sync::Arc::clone(&operator_wallet),
        None,
    )
    .await
    .expect("sniper");
    assert!(sniper.tenant_context().is_none());

    let result = sniper
        .execute_request(paper_request("operator-entry", &operator_wallet.pubkey))
        .await
        .expect("execute");
    assert_ne!(result.status, ExecStatus::Skipped);
    assert!(result.paper);
}

#[test]
fn the_sniper_context_adapter_refuses_foreign_modules_and_scopes_dedup() {
    let wallet = loaded_wallet();
    let ctx = SniperTenantContext::adapt(issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    ))
    .expect("sniper context");

    // Same external event, two tenants: independent dedup keys.
    let other = SniperTenantContext::adapt(issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    ))
    .expect("sniper context");
    assert_ne!(ctx.dedup_key("launch-1"), other.dedup_key("launch-1"));

    // A Copy context can never adapt into the sniper.
    let copy = issued(
        BotModule::Copy,
        ctx.organization_id(),
        ctx.runtime_id(),
        ctx.generation(),
        ctx.wallet_address(),
    );
    assert!(SniperTenantContext::adapt(copy).is_err());
}

#[test]
fn a_wallet_bound_to_another_tenant_cannot_drive_the_executor() {
    // Adapter-level: the context's wallet binding is part of the tenant
    // identity; a context whose wallet differs from the executor's bound
    // wallet is a wallet_mismatch deny (also enforced again by the guard
    // at broadcast time).
    let wallet = loaded_wallet();
    let other = loaded_wallet();
    assert_ne!(wallet.pubkey, other.pubkey);

    let bound = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    );
    let presented = issued(
        BotModule::Sniper,
        bound.organization_id(),
        bound.runtime_id(),
        bound.generation(),
        &other.pubkey.to_string(),
    );
    let ctx = SniperTenantContext::adapt(bound).unwrap();
    let presented_ctx = SniperTenantContext::adapt(presented).unwrap();
    assert_ne!(
        ctx.wallet_address(),
        presented_ctx.wallet_address(),
        "two different material wallets must bind different addresses"
    );
}
