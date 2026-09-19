//! Crash-recovery reconciliation end-to-end (Prompt 2 §W/§I/§F).
//!
//! GATED like `devnet_e2e`: runs only with `E2E_NETWORK=1` (and the
//! convergence test additionally `E2E_LIVE=1`). `E2E_URL` points at the
//! cluster — a local `solana-test-validator` in CI/manual runs, so no real
//! funds are ever at risk.
//!
//! What this proves with the REAL executor, REAL RPC and REAL chain state:
//!
//! 1. `crash_recovery_converges_to_chain_truth`:
//!    intent → persist → broadcast → confirm → **crash before the position/
//!    state update is durable** → restart → on-chain truth discovery →
//!    deterministic reconciliation → converged order state, EXACTLY ONE
//!    on-chain transfer, and an accidental retry of the same logical
//!    execution collapsing onto the terminal order (no double-spend).
//!
//! 2. `ambiguous_send_is_never_a_definite_failure`:
//!    broadcast into a transport black hole (case 1/§F: RPC timeout before
//!    an answer) → the executor reports `SendUnknown` WITH the signature,
//!    reconciliation against the real chain classifies it inconclusive
//!    (retry — never "failed"), and nothing lands or is resubmitted.

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_system_interface::instruction as system_instruction;

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::oms::{OrderDraft, OrderManager, OrderStatus};
use bot_core::reconciliation::{
    classify_execution, ExternalExecutionState, ExternalState, LocalExecutionState, ReconOutcome,
};

use solana_kit::execute::{BroadcastMode, ExecPolicy, ExecStatus, Executor};
use solana_kit::rpc::{ConfirmOutcome, Rpc};
use solana_kit::tokens::Wallet;
use solana_kit::tx::TxRequest;

const DEVNET: &str = "https://api.devnet.solana.com";

fn gated(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn real_rpc() -> Rpc {
    let url = std::env::var("E2E_URL").unwrap_or_else(|_| DEVNET.to_string());
    Rpc::with_urls(
        url,
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        2,
        Duration::from_secs(30),
    )
    .expect("rpc builds")
}

fn policy(mode: ExecutionMode, confirm_timeout: Duration) -> ExecPolicy {
    ExecPolicy {
        mode,
        broadcast: BroadcastMode::Rpc,
        simulate_first: false,
        abort_on_simulation_failure: true,
        confirm_timeout,
        confirm_poll_interval: Duration::from_secs(1),
        max_attempts: 1,
        jito_url: None,
        min_priority_fee_micro_lamports: 0,
        fanout: false,
    }
}

/// Airdrop 0.1 SOL; false when the faucet refuses (skip, don't fail —
/// the faucet is infrastructure, not the code under test).
async fn try_fund(rpc: &Rpc, wallet: &Wallet) -> bool {
    for _ in 0..3 {
        if rpc
            .raw()
            .request_airdrop(&wallet.pubkey, 100_000_000)
            .await
            .is_ok()
        {
            for _ in 0..30 {
                if rpc.get_balance(&wallet.pubkey).await.unwrap_or(0) > 0 {
                    return true;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

fn intent(key: &str) -> OrderDraft {
    OrderDraft {
        idempotency_key: key.to_string(),
        module: BotModule::Sniper,
        side: "buy".into(),
        symbol: "RECON-E2E".into(),
        venue: "system".into(),
        mode: ExecutionMode::Live,
        qty: 1_000_000.0,
        price: None,
        meta: serde_json::json!({ "source": "recon_crash_e2e" }),
    }
}

#[tokio::test]
async fn crash_recovery_converges_to_chain_truth() {
    if !gated("E2E_NETWORK") || !gated("E2E_LIVE") {
        eprintln!(
            "SKIP crash_recovery: set E2E_NETWORK=1 E2E_LIVE=1 to run \
             (broadcasts one 1000-lamport transfer with an ephemeral key)"
        );
        return;
    }
    let rpc = real_rpc();
    let wallet = Arc::new(Wallet::generate());
    if !try_fund(&rpc, &wallet).await {
        eprintln!("SKIP crash_recovery: faucet refused the airdrop");
        return;
    }
    let recipient = Keypair::new().pubkey();
    let intent_key = format!(
        "recon-crash-e2e-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );

    // ---- Phase 1: local intent → execution --------------------------------
    // The OMS is the process-memory state that a crash destroys; the intent
    // key + signature are what durable storage (DB/journal) would hand back
    // on restart.
    let mgr = OrderManager::new(None, 64);
    let order = mgr.create(intent(&intent_key)).await.expect("create");
    mgr.transition(&order.id, OrderStatus::Submitted, Some("send"))
        .await
        .expect("submitted");

    let executor = Executor::new(
        rpc.clone(),
        Arc::clone(&wallet),
        policy(ExecutionMode::Live, Duration::from_secs(60)),
    );
    let req = TxRequest {
        instructions: vec![system_instruction::transfer(
            &wallet.pubkey,
            &recipient,
            1_000_000, // ≥ rent-exempt minimum for a 0-data account
        )],
        label: "recon-crash-e2e".into(),
        ..Default::default()
    };
    let result = executor.run(req).await.expect("execution returns");
    assert!(
        matches!(result.status, ExecStatus::Confirmed),
        "transfer must confirm on the test validator, got {:?} ({:?})",
        result.status,
        result.error
    );
    let signature = result.signature.clone();
    assert!(!signature.is_empty());

    // ---- Phase 2: CRASH before the post-execution state update ------------
    // The order never reached Filled; the in-memory OMS is gone entirely.
    drop(mgr);
    // "Restart": fresh OMS, durable facts = intent key + signature only.
    let restarted = OrderManager::new(None, 64);
    let recovered = restarted
        .create(intent(&intent_key))
        .await
        .expect("re-create from persisted intent");
    assert_ne!(
        recovered.status,
        OrderStatus::Filled,
        "restarted memory must NOT assume success"
    );
    restarted
        .transition(&recovered.id, OrderStatus::Submitted, Some("recovered"))
        .await
        .expect("submitted");
    restarted
        .attach_external(&recovered.id, None, Some(signature.clone()))
        .await
        .expect("attach signature");
    // recover_from_db semantics: non-terminal after restart → Unknown.
    restarted
        .transition(&recovered.id, OrderStatus::Unknown, Some("restart"))
        .await
        .expect("unknown");

    // ---- Phase 3: on-chain truth discovery → deterministic reconciliation --
    let probe = rpc
        .confirm(
            &signature.parse().expect("sig parses"),
            Duration::from_secs(30),
            Duration::from_secs(1),
        )
        .await
        .expect("confirm returns");
    let external = match probe {
        ConfirmOutcome::Confirmed { .. } => {
            ExternalState::Observed(ExternalExecutionState::Succeeded)
        }
        ConfirmOutcome::Failed { .. } => {
            ExternalState::Observed(ExternalExecutionState::FailedOnExternal)
        }
        ConfirmOutcome::Timeout => ExternalState::Observed(ExternalExecutionState::Pending),
    };
    let outcome = classify_execution(LocalExecutionState::SubmittedUnconfirmed, external.clone());
    assert_eq!(
        outcome,
        ReconOutcome::InSync,
        "chain truth must resolve the ambiguity"
    );
    if let ExternalState::Observed(ExternalExecutionState::Succeeded) = external {
        restarted
            .transition(
                &recovered.id,
                OrderStatus::Filled,
                Some("reconciled from chain"),
            )
            .await
            .expect("filled");
    } else {
        panic!("transfer was confirmed in phase 1; chain must still say so");
    }
    let final_order = restarted.get(&recovered.id).await.expect("order");
    assert_eq!(final_order.status, OrderStatus::Filled);
    assert_eq!(final_order.signature.as_deref(), Some(signature.as_str()));

    // ---- Phase 4: convergence — exactly one execution, retry collapses ----
    let balance = rpc.get_balance(&recipient).await.expect("balance");
    assert_eq!(
        balance, 1_000_000,
        "exactly ONE transfer landed (converged state)"
    );

    // Case 8 (§F): an accidental retry of the same logical execution after
    // the ambiguous window collapses onto the existing terminal order — the
    // guard every module checks before broadcasting.
    let retry = restarted
        .create(intent(&intent_key))
        .await
        .expect("retry create");
    assert_eq!(retry.id, recovered.id, "same intent → same order");
    assert!(
        retry.status.is_terminal(),
        "terminal orders must never re-execute"
    );
    let balance_after = rpc.get_balance(&recipient).await.expect("balance2");
    assert_eq!(balance_after, 1_000_000, "no second transfer");
}

#[tokio::test]
async fn ambiguous_send_is_never_a_definite_failure() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP ambiguous_send: set E2E_NETWORK=1 to run");
        return;
    }
    let rpc = real_rpc();
    let wallet = Arc::new(Wallet::generate());
    if !try_fund(&rpc, &wallet).await {
        eprintln!("SKIP ambiguous_send: faucet refused the airdrop");
        return;
    }
    let start_balance = rpc.get_balance(&wallet.pubkey).await.expect("bal");

    // A transport black hole: accepts TCP, never answers (case 1/3 in §F —
    // RPC timeout / connection lost after the request left the process).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let black_hole = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let _ = listener.accept().await; // drop immediately
        }
    });
    let broken_rpc = Rpc::with_urls(
        black_hole,
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        1,
        Duration::from_secs(2),
    )
    .expect("rpc builds");

    // Real recent blockhash so the tx is genuinely valid and broadcastable —
    // only the SEND path is broken.
    let blockhash = rpc
        .latest_blockhash(true)
        .await
        .expect("blockhash")
        .blockhash;
    let executor = Executor::new(
        broken_rpc,
        Arc::clone(&wallet),
        policy(ExecutionMode::Live, Duration::from_secs(3)),
    );
    let req = TxRequest {
        instructions: vec![system_instruction::transfer(
            &wallet.pubkey,
            &wallet.pubkey,
            0,
        )],
        label: "recon-ambiguous-e2e".into(),
        blockhash: Some(blockhash),
        ..Default::default()
    };
    let result = executor.run(req).await.expect("run returns Ok");

    // The outcome must be AMBIGUOUS, carrying the signature — never a
    // definite failure, and never silently dropped.
    assert_eq!(
        result.status,
        ExecStatus::SendUnknown,
        "transport black hole must produce SendUnknown (error: {:?})",
        result.error
    );
    assert!(!result.signature.is_empty(), "signature survives ambiguity");
    assert!(result.succeeded(), "rides the claim/recon path");

    // Reconcile against the REAL chain: the tx never reached a leader, but
    // the honest verdict within the confirmation horizon is "unknown —
    // retry", never "failed" and never "confirmed".
    let sig: solana_sdk::signature::Signature = result.signature.parse().expect("sig");
    let probe = rpc
        .confirm(&sig, Duration::from_secs(10), Duration::from_secs(2))
        .await
        .expect("confirm returns");
    let external = match probe {
        ConfirmOutcome::Confirmed { .. } => {
            ExternalState::Observed(ExternalExecutionState::Succeeded)
        }
        ConfirmOutcome::Failed { .. } => {
            ExternalState::Observed(ExternalExecutionState::FailedOnExternal)
        }
        ConfirmOutcome::Timeout => ExternalState::Observed(ExternalExecutionState::Pending),
    };
    let outcome = classify_execution(LocalExecutionState::SubmittedUnconfirmed, external);
    assert!(
        matches!(
            outcome,
            ReconOutcome::UnknownExecution
                | ReconOutcome::MissingTransaction
                | ReconOutcome::InSync
        ),
        "unexpected classification: {outcome:?}"
    );
    assert!(
        outcome.is_inconclusive() || outcome == ReconOutcome::InSync,
        "a never-broadcast tx must not be classified divergent: {outcome:?}"
    );

    // No funds moved, no fee burned: nothing landed and nothing was
    // resubmitted (max_attempts = 1 with an ambiguous outcome never retries).
    let end_balance = rpc.get_balance(&wallet.pubkey).await.expect("bal2");
    assert_eq!(start_balance, end_balance, "no lamports left the wallet");
}
