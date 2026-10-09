//! Tenant signing boundary tests (PROMPT 4/10 file 29).
//!
//! These prove the fail-closed rule end-to-end at the executor level: a
//! tenant-guarded [`Executor`] REFUSES to sign or broadcast when the
//! organization, runtime, generation, module or wallet does not match its
//! bound context — and that the refusal happens BEFORE any broadcast
//! (paper mode + a pinned blockhash, so nothing ever leaves the process
//! and no RPC endpoint is contacted by these tests).
//!
//! They also pin the deployment-global invariant: an executor WITHOUT a
//! guard runs the same request unchanged (the pre-tenant behaviour).

use std::str::FromStr;

use bot_core::config::NetworkConfig;
use bot_core::execution::{
    AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{
    ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
};
use chrono::Utc;
use solana_kit::execute::{ExecPolicy, ExecStatus};
use solana_kit::tenant_broadcast_guard::TenantBroadcastGuard;
use solana_kit::tenant_signing_context::TenantSigningContext;
use solana_kit::tenant_transaction::TenantTransactionMeta;
use solana_kit::tx::TxRequest;
use solana_kit::{Executor, Rpc, Wallet};
use solana_sdk::hash::Hash;
use std::sync::Arc;

/// Issue a real core execution context through the full authority path.
fn issued(
    module: BotModule,
    org: OrganizationId,
    runtime: RuntimeId,
    generation: RuntimeGeneration,
    wallet_address: &str,
) -> TenantSigningContext {
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
    let signer = TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "tenant-key")
        .expect("signer ref");
    TenantSigningContext::new(
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
        .expect("context"),
    )
}

fn loaded_wallet() -> Wallet {
    let kp = solana_sdk::signature::Keypair::new();
    let b58 = bs58::encode(kp.to_bytes()).into_string();
    Wallet::load(&b58).expect("wallet must load")
}

fn paper_policy() -> ExecPolicy {
    ExecPolicy {
        mode: ExecutionMode::Paper,
        simulate_first: false,
        ..Default::default()
    }
}

/// A paper-mode executor bound to one tenant (guard attached). The SAME
/// wallet funds the executor and is bound to the context.
fn tenant_executor(module: BotModule) -> (Executor, TenantSigningContext) {
    let wallet = loaded_wallet();
    let ctx = issued(
        module,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    );
    let guard = TenantBroadcastGuard::new(ctx.clone(), wallet.pubkey).expect("guard");
    let executor = Executor::new(
        Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::new(wallet),
        paper_policy(),
    )
    .with_tenant_guard(Arc::new(guard))
    .expect("guard wallet matches executor wallet");
    (executor, ctx)
}

/// A tenant-bound request with a pinned blockhash: hermetic — the builder
/// signs locally and the paper executor stops before any network call.
fn tenant_request(ctx: &TenantSigningContext, label: &str) -> TxRequest {
    let meta = TenantTransactionMeta::from_context(ctx.execution_context(), label, None);
    let mut req = TxRequest::new(label).tenant(meta);
    req.blockhash = Some(Hash::new_unique());
    req
}

#[tokio::test]
async fn wrong_organization_is_denied_before_broadcast() {
    let (executor, ctx) = tenant_executor(BotModule::Sniper);

    // A DIFFERENT tenant's context, same shape otherwise.
    let foreign = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        ctx.runtime_id(),
        ctx.generation(),
        ctx.wallet_address(),
    );
    let req = TxRequest::new("snipe-entry").tenant(TenantTransactionMeta::from_context(
        foreign.execution_context(),
        "sniper",
        None,
    ));

    let result = executor.run(req).await.expect("run must not error");
    assert_eq!(result.status, ExecStatus::Skipped);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("organization_mismatch"));
    assert!(!result.succeeded());
}

#[tokio::test]
async fn stale_generation_is_denied_before_broadcast() {
    let (executor, ctx) = tenant_executor(BotModule::Sniper);

    let rotated = issued(
        BotModule::Sniper,
        ctx.organization_id(),
        ctx.runtime_id(),
        ctx.generation().next().unwrap(),
        ctx.wallet_address(),
    );
    let req = TxRequest::new("snipe-entry").tenant(TenantTransactionMeta::from_context(
        rotated.execution_context(),
        "sniper",
        None,
    ));

    let result = executor.run(req).await.expect("run");
    assert_eq!(result.status, ExecStatus::Skipped);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("generation_mismatch"));
}

#[tokio::test]
async fn foreign_runtime_is_denied_before_broadcast() {
    let (executor, ctx) = tenant_executor(BotModule::Sniper);

    let foreign = issued(
        BotModule::Sniper,
        ctx.organization_id(),
        RuntimeId::new(),
        ctx.generation(),
        ctx.wallet_address(),
    );
    let req = TxRequest::new("snipe-entry").tenant(TenantTransactionMeta::from_context(
        foreign.execution_context(),
        "sniper",
        None,
    ));

    let result = executor.run(req).await.expect("run");
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("runtime_mismatch"));
}

#[tokio::test]
async fn wrong_module_is_denied_before_broadcast() {
    let (executor, ctx) = tenant_executor(BotModule::Sniper);

    let copy_ctx = issued(
        BotModule::Copy,
        ctx.organization_id(),
        ctx.runtime_id(),
        ctx.generation(),
        ctx.wallet_address(),
    );
    let req = TxRequest::new("copy-mirror").tenant(TenantTransactionMeta::from_context(
        copy_ctx.execution_context(),
        "copy",
        None,
    ));

    let result = executor.run(req).await.expect("run");
    assert!(result.error.as_deref().unwrap().contains("module_mismatch"));
}

#[tokio::test]
async fn missing_metadata_is_denied_before_broadcast() {
    let (executor, _ctx) = tenant_executor(BotModule::Sniper);

    let result = executor
        .run(TxRequest::new("anonymous-entry"))
        .await
        .expect("run");
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("missing_tenant_meta"));
}

#[tokio::test]
async fn matching_metadata_passes_the_gate_and_fills_in_paper_mode() {
    let (executor, ctx) = tenant_executor(BotModule::Sniper);

    let funding = solana_sdk::pubkey::Pubkey::from_str(ctx.wallet_address()).expect("bound pubkey");
    let mut req = tenant_request(&ctx, "snipe-entry");
    req.instructions
        .push(solana_sdk::system_instruction::transfer(
            &funding,
            &solana_sdk::pubkey::Pubkey::new_unique(),
            1,
        ));

    let result = executor.run(req).await.expect("run");
    assert_ne!(result.status, ExecStatus::Skipped);
    assert!(!result
        .error
        .as_deref()
        .unwrap_or("")
        .contains("tenant broadcast denied"));
}

#[tokio::test]
async fn unguarded_executor_keeps_the_deployment_global_behaviour() {
    // ONE wallet funds the executor and the transfer: the message's fee
    // payer is the wallet, so the source of the transfer must be too.
    let wallet = Arc::new(loaded_wallet());
    let executor = Executor::new(
        Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::clone(&wallet),
        paper_policy(),
    );
    assert!(executor.tenant_guard().is_none());

    // An anonymous (operator/deployment-global) request runs unchanged —
    // the pre-tenant behaviour is preserved byte-for-byte.
    let mut req = TxRequest::new("operator-entry");
    req.blockhash = Some(Hash::new_unique());
    req.instructions
        .push(solana_sdk::system_instruction::transfer(
            &wallet.pubkey,
            &solana_sdk::pubkey::Pubkey::new_unique(),
            1,
        ));
    let result = executor.run(req).await.expect("run");
    assert_ne!(result.status, ExecStatus::Skipped);
}

#[tokio::test]
async fn attaching_a_guard_for_another_wallet_fails_at_construction() {
    let wallet = loaded_wallet();
    let ctx = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    );
    let guard = Arc::new(TenantBroadcastGuard::new(ctx, wallet.pubkey).expect("guard"));

    // A DIFFERENT wallet funds this executor: the attach must fail loudly.
    let err = Executor::new(
        Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::new(loaded_wallet()),
        paper_policy(),
    )
    .with_tenant_guard(guard)
    .err()
    .expect("mismatched guard wallet must be refused");
    assert!(err
        .to_string()
        .contains("does not match the executor wallet"));
}

#[test]
fn guard_exposes_its_bound_identity_for_observability() {
    let wallet = loaded_wallet();
    let org = OrganizationId::new();
    let runtime = RuntimeId::new();
    let generation = RuntimeGeneration::first();
    let ctx = issued(
        BotModule::Copy,
        org,
        runtime,
        generation,
        &wallet.pubkey.to_string(),
    );
    let guard = TenantBroadcastGuard::new(ctx, wallet.pubkey).expect("guard");
    assert_eq!(guard.organization_id(), org);
    assert_eq!(guard.runtime_id(), runtime);
    assert_eq!(guard.generation(), generation);
    assert_eq!(guard.context().module(), ModuleKind::Copy);
}
