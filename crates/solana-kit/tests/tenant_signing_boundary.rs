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

/// Gap: "tenant runtime freshness at broadcast" — end-to-end at the executor.
///
/// A rotation happens AFTER the module was built: the executor and its guard
/// still hold the captured context, and every metadata rule still matches. The
/// live fence probe is the only thing that can refuse the broadcast, and a
/// refusal must surface as a policy veto — never as a submitted transaction.
#[tokio::test]
async fn a_rotation_after_build_blocks_the_next_broadcast() {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Debug)]
    struct ToggleProbe {
        current: AtomicBool,
    }

    #[async_trait::async_trait]
    impl solana_kit::tenant_broadcast_guard::RuntimeFenceProbe for ToggleProbe {
        async fn fence_is_current(
            &self,
            _organization_id: OrganizationId,
            _runtime_id: RuntimeId,
            _generation: RuntimeGeneration,
        ) -> Result<(), String> {
            if self.current.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err("superseded: tenant runtime rotated to another instance".into())
            }
        }
        fn probe_name(&self) -> &'static str {
            "toggle"
        }
    }

    let wallet = loaded_wallet();
    let ctx = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &wallet.pubkey.to_string(),
    );
    let probe = Arc::new(ToggleProbe {
        current: AtomicBool::new(true),
    });
    let guard = TenantBroadcastGuard::new(ctx.clone(), wallet.pubkey)
        .expect("guard")
        .with_fence_probe(probe.clone());
    let executor = Executor::new(
        Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::new(wallet),
        paper_policy(),
    )
    .with_tenant_guard(Arc::new(guard))
    .expect("guard wallet matches executor wallet");

    // While the runtime is current the paper execution proceeds.
    let ok = executor
        .run(tenant_request(&ctx, "fence-live"))
        .await
        .expect("paper run");
    assert_ne!(
        ok.failure,
        Some(bot_core::execution::FailureClass::PolicyVeto),
        "a current runtime must not be vetoed: {ok:?}"
    );

    // The runtime is rotated; the very next broadcast is refused.
    probe.current.store(false, Ordering::SeqCst);
    let vetoed = executor
        .run(tenant_request(&ctx, "fence-rotated"))
        .await
        .expect("paper run");
    assert_eq!(
        vetoed.failure,
        Some(bot_core::execution::FailureClass::PolicyVeto),
        "a rotated runtime must be refused at broadcast time: {vetoed:?}"
    );
    let detail = vetoed.error.clone().unwrap_or_default();
    assert!(
        detail.contains("runtime_fenced"),
        "the veto must name the fence, got: {detail}"
    );
    assert!(
        vetoed.signature.is_empty() && vetoed.broadcast_signature().is_none(),
        "a vetoed broadcast leaves no signature behind, got {:?}",
        vetoed.signature
    );
    assert!(!vetoed.paper, "a veto is not a paper execution result");
}

/// A signing key rotated under an UNCHANGED reference must stop the next
/// broadcast — end to end, through the executor's own run path.
///
/// `SwitchSigner` stands in for a custody rotation: the identity string never
/// changes, but the key it resolves to does. The guard pins the approved key at
/// construction and re-resolves at broadcast, so the order is vetoed with
/// `signer_key_mismatch` instead of being signed by an unapproved key.
#[tokio::test]
async fn a_key_rotated_under_the_same_reference_blocks_the_next_broadcast() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use solana_kit::signer::{SignerRegistry, TransactionSigner};

    #[derive(Debug)]
    struct SwitchSigner {
        rotated: AtomicBool,
        approved: solana_sdk::pubkey::Pubkey,
        replacement: solana_sdk::pubkey::Pubkey,
    }

    #[async_trait::async_trait]
    impl TransactionSigner for SwitchSigner {
        fn pubkey(&self) -> solana_sdk::pubkey::Pubkey {
            if self.rotated.load(Ordering::SeqCst) {
                self.replacement
            } else {
                self.approved
            }
        }

        async fn sign_message(
            &self,
            _message: &[u8],
        ) -> Result<solana_sdk::signature::Signature, bot_core::error::SignerError> {
            Ok(solana_sdk::signature::Signature::new_unique())
        }
    }

    // The wallet the context is issued for IS the approved signer key.
    let wallet = loaded_wallet();
    let approved = wallet.pubkey;
    let signer = Arc::new(SwitchSigner {
        rotated: AtomicBool::new(false),
        approved,
        replacement: solana_sdk::pubkey::Pubkey::new_unique(),
    });
    let mut registry = SignerRegistry::new();
    // The key reference the shared `issued(...)` helper binds.
    let identity = "tenant-key";
    registry
        .register(identity, signer.clone())
        .expect("register signer");

    let ctx = issued(
        BotModule::Sniper,
        OrganizationId::new(),
        RuntimeId::new(),
        RuntimeGeneration::first(),
        &approved.to_string(),
    );
    assert_eq!(ctx.signer_identity(), identity);

    // Guard pinned to what the registry resolves NOW (the approved key).
    let guard = TenantBroadcastGuard::new(ctx.clone(), approved)
        .expect("guard")
        .with_expected_signer_key(approved);
    let executor = Executor::new(
        Rpc::new(&NetworkConfig::default()).expect("rpc pool"),
        Arc::new(wallet),
        paper_policy(),
    )
    .with_signer_registry(Arc::new(registry))
    .with_tenant_guard(Arc::new(guard))
    .expect("guard wallet matches executor wallet");

    // While the reference still resolves to the approved key: normal paper run.
    let ok = executor
        .run(tenant_request(&ctx, "snipe-entry"))
        .await
        .expect("run must not error");
    assert!(
        !ok.error
            .as_deref()
            .unwrap_or_default()
            .contains("signer_key_mismatch"),
        "the approved key must pass: {:?}",
        ok.error
    );

    // Custody rotation: same identity, different key.
    signer.rotated.store(true, Ordering::SeqCst);

    let vetoed = executor
        .run(tenant_request(&ctx, "snipe-entry"))
        .await
        .expect("run must not error");
    assert_eq!(vetoed.status, ExecStatus::Skipped);
    assert!(!vetoed.succeeded());
    assert!(!vetoed.paper, "a veto is not a paper fill");
    assert!(vetoed.signature.is_empty());
    assert_eq!(
        vetoed.failure,
        Some(bot_core::execution::FailureClass::PolicyVeto)
    );
    let detail = vetoed.error.clone().unwrap_or_default();
    assert!(
        detail.contains("signer_key_mismatch"),
        "the veto must name the rotated key, got {detail}"
    );
    assert!(
        detail.contains("resolves to"),
        "the veto must show what resolved, got {detail}"
    );
}
