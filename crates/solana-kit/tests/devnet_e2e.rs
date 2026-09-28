//! Devnet end-to-end tests — BUILD PLAN §3 "prove it works" evidence.
//!
//! These hit the REAL `https://api.devnet.solana.com` (or the cluster named
//! by `E2E_URL`, e.g. a local `solana-test-validator`) and are therefore
//! **gated**: they only run when `E2E_NETWORK=1` is set (and the careful-live
//! broadcast test additionally needs `E2E_LIVE=1`). CI never sets these, so
//! the default suite stays offline and deterministic.
//!
//! What they prove on a live cluster:
//! * the RPC client stack (retry wrapper, blockhash cache) against real nodes;
//! * the executor's paper path builds and signs a real transaction with a
//!   real recent blockhash (never broadcast);
//! * the simulate path gets a clean `simulateTransaction` verdict;
//! * (E2E_LIVE) the full build→broadcast→confirm loop lands a valueless
//!   self-transfer on devnet — the exact code path live trading uses, with
//!   an ephemeral key and no real funds at risk.

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_system_interface::instruction as system_instruction;

use solana_kit::execute::{BroadcastMode, ExecPolicy, ExecStatus, Executor};
use solana_kit::rpc::Rpc;
use solana_kit::tokens::Wallet;
use solana_kit::tx::TxRequest;

const DEVNET: &str = "https://api.devnet.solana.com";

fn gated(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// RPC under test. Defaults to public devnet; `E2E_URL` overrides it (e.g. a
/// local `solana-test-validator`, whose faucet is not rate limited — public
/// devnet faucets often are, and the simulate/live tests skip gracefully then).
fn devnet_rpc() -> Rpc {
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

fn policy(mode: bot_core::models::ExecutionMode) -> ExecPolicy {
    ExecPolicy {
        mode,
        broadcast: BroadcastMode::Rpc,
        simulate_first: true,
        abort_on_simulation_failure: true,
        confirm_timeout: Duration::from_secs(60),
        confirm_poll_interval: Duration::from_secs(1),
        max_attempts: 2,
        jito_url: None,
        min_priority_fee_micro_lamports: 0,
        fanout: false,
    }
}

fn self_transfer(wallet: &Wallet) -> TxRequest {
    TxRequest {
        // A 0-lamport transfer to ourselves: exercises build+sign with a real
        // blockhash and is inert on-chain.
        instructions: vec![system_instruction::transfer(
            &wallet.pubkey,
            &wallet.pubkey,
            0,
        )],
        label: "devnet-e2e-self-transfer".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn devnet_rpc_basics() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP devnet_rpc_basics: set E2E_NETWORK=1 to run");
        return;
    }
    let rpc = devnet_rpc();

    let version = rpc.get_version().await.expect("getVersion must work");
    assert!(!version.is_empty(), "version string: {version}");

    assert_eq!(rpc.health().await.expect("getHealth"), "ok");

    let slot_a = rpc.get_slot().await.expect("getSlot");
    let slot_b = rpc.get_slot().await.expect("getSlot");
    assert!(slot_b >= slot_a, "slots advance: {slot_a} -> {slot_b}");

    let blockhash = rpc
        .latest_blockhash(true)
        .await
        .expect("getLatestBlockhash")
        .blockhash;
    assert!(
        rpc.is_blockhash_valid(&blockhash)
            .await
            .expect("isBlockhashValid"),
        "a fresh blockhash must validate"
    );

    // A brand-new pubkey has no funds.
    // NOTE: `Pubkey::new_unique()` is deterministic (a counter over a fixed
    // test seed) and its first value is a well-known key that actually holds
    // devnet SOL — an unknown account here must be a genuinely random one.
    let fresh = Keypair::new().pubkey();
    assert_eq!(rpc.get_balance(&fresh).await.expect("getBalance"), 0);
    assert!(!rpc.account_exists(&fresh).await.expect("getAccount"));
}

#[tokio::test]
async fn executor_paper_builds_and_signs_a_real_devnet_tx() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP executor_paper: set E2E_NETWORK=1 to run");
        return;
    }
    let rpc = devnet_rpc();
    let wallet = Arc::new(Wallet::generate());
    // The ephemeral wallet holds 0 lamports, so a devnet simulation would
    // legitimately fail on fees and (with abort_on_simulation_failure) stop
    // the run before the paper branch. This test targets build+sign, not
    // simulation — the funded simulation path has its own test below.
    let mut paper_policy = policy(bot_core::models::ExecutionMode::Paper);
    paper_policy.simulate_first = false;
    let executor = Executor::new(rpc, Arc::clone(&wallet), paper_policy);

    let result = executor
        .run(self_transfer(&wallet))
        .await
        .expect("paper execution must not fail");

    assert!(matches!(result.status, ExecStatus::PaperFilled));
    assert!(result.paper, "paper flag set");
    assert!(!result.signature.is_empty(), "tx was really signed");
    assert!(result.tx_size > 0);
    assert!(result.error.is_none());
    // Nothing was broadcast: send/confirm timings stay unset in paper mode.
    assert!(result.send_ms.is_none() && result.confirm_ms.is_none());
}

/// Airdrop devnet SOL to the ephemeral wallet. Returns false when the faucet
/// refuses (rate limits happen on public devnet) — callers skip instead of
/// failing, because the airdrop is infrastructure, not the code under test.
async fn try_fund(rpc: &Rpc, wallet: &Wallet) -> bool {
    for _ in 0..3 {
        if rpc
            .raw()
            .request_airdrop(&wallet.pubkey, 100_000_000) // 0.1 SOL
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

#[tokio::test]
async fn executor_simulate_gets_a_clean_devnet_verdict() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP executor_simulate: set E2E_NETWORK=1 to run");
        return;
    }
    let rpc = devnet_rpc();
    let wallet = Arc::new(Wallet::generate());
    if !try_fund(&rpc, &wallet).await {
        eprintln!(
            "SKIP: devnet faucet refused the airdrop (rate limit); simulation needs fee funds"
        );
        return;
    }

    let executor = Executor::new(
        rpc,
        Arc::clone(&wallet),
        policy(bot_core::models::ExecutionMode::Simulate),
    );
    let sim = executor
        .simulate_request(&self_transfer(&wallet))
        .await
        .expect("simulateTransaction must return");
    assert!(
        sim.error.is_none(),
        "clean tx must simulate cleanly: {:?}",
        sim.error
    );
    assert!(sim.units_consumed > 0, "CU metering ran");
}

#[tokio::test]
async fn executor_live_lands_a_valueless_devnet_self_transfer() {
    if !gated("E2E_NETWORK") || !gated("E2E_LIVE") {
        eprintln!("SKIP executor_live: set E2E_NETWORK=1 E2E_LIVE=1 to run (broadcasts a 0-value self-transfer on devnet)");
        return;
    }
    let rpc = devnet_rpc();
    let wallet = Arc::new(Wallet::generate());
    if !try_fund(&rpc, &wallet).await {
        eprintln!("SKIP: devnet faucet refused the airdrop (rate limit)");
        return;
    }

    let executor = Executor::new(
        rpc.clone(),
        Arc::clone(&wallet),
        policy(bot_core::models::ExecutionMode::Live),
    );
    let result = executor
        .run(self_transfer(&wallet))
        .await
        .expect("live execution must not fail");

    assert!(
        matches!(result.status, ExecStatus::Confirmed),
        "expected Confirmed, got {:?} (error: {:?})",
        result.status,
        result.error
    );
    assert!(!result.paper);
    assert!(result.send_ms.is_some() && result.confirm_ms.is_some());

    // The signature really exists on chain.
    let sig: solana_sdk::signature::Signature = result
        .signature
        .parse()
        .expect("result carries a real signature");
    let status = rpc
        .raw()
        .get_signature_status(&sig)
        .await
        .expect("getSignatureStatuses");
    assert!(status.is_some(), "signature is known to the cluster");
    assert!(
        status.unwrap().is_ok(),
        "transaction executed without error"
    );
}
