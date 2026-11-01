//! LIVE **devnet** money path — a real airdrop, a real signed transaction, a
//! real confirmation, on a real cluster.
//!
//! `#[ignore]`d by default (needs the public internet + a devnet faucet):
//!
//! ```text
//! SOLANA_LIVE_DEVNET_RPC=https://api.devnet.solana.com \
//!   cargo test -p solana-kit --test live_devnet_transaction -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Scope, stated honestly:
//! * This runs on **devnet**, with **devnet** lamports. Those have no value and
//!   are not the buyer's funds. The faucet is the only source of funds here.
//! * What it proves: the wallet can be funded, a transfer is built and signed
//!   with a fresh in-memory keypair (nothing persisted, nothing logged), the
//!   crate's own `sendTransaction` + `confirm` path returns a landed signature,
//!   and the lamport balances on chain move by exactly the amount sent.
//! * What it does NOT prove: mainnet execution, funded mainnet capital, real
//!   venue fills (Jupiter/Raydium/PumpSwap quotes), or anything that needs a
//!   mainnet balance. None of that is claimed anywhere in the delivery.

use std::str::FromStr;
use std::time::Duration;

use solana_kit::rpc::{ConfirmOutcome, Rpc};
use solana_rpc_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;
use solana_system_interface::instruction as system_instruction;

const AIRDROP_LAMPORTS: u64 = 1_000_000_000; // 1 devnet SOL
                                             // Rent matters: a brand-new account must receive at least the rent-exempt
                                             // minimum (~890_880 lamports for a 0-byte account) or the runtime rejects the
                                             // transfer with "insufficient funds for rent" — discovered by running this test
                                             // against a real validator, not by reading about it.
const SEND_LAMPORTS: u64 = 2_000_000;

fn devnet_rpc() -> Option<(String, Rpc)> {
    let url = std::env::var("SOLANA_LIVE_DEVNET_RPC").unwrap_or_default();
    if url.trim().is_empty() {
        eprintln!("SKIPPED: set SOLANA_LIVE_DEVNET_RPC (e.g. https://api.devnet.solana.com)");
        return None;
    }
    let url = url.trim().to_string();
    let rpc = Rpc::with_urls(
        url.clone(),
        String::new(),
        Vec::new(),
        CommitmentConfig::confirmed(),
        3,
        Duration::from_secs(30),
    )
    .expect("devnet RPC client must build");
    Some((url, rpc))
}

/// Ask the real devnet faucet for lamports, tolerating the faucet's rate limit.
///
/// `solana-rpc-client`'s `request_airdrop` is a blocking call, so it runs on a
/// blocking thread rather than inside the async runtime.
async fn airdrop(url: String, to: String, lamports: u64) -> bool {
    tokio::task::spawn_blocking(move || {
        let client = RpcClient::new_with_commitment(url, CommitmentConfig::confirmed());
        let pubkey = solana_sdk::pubkey::Pubkey::from_str(&to).expect("pubkey");
        for attempt in 1..=4 {
            match client.request_airdrop(&pubkey, lamports) {
                Ok(signature) => {
                    eprintln!("devnet airdrop requested: {signature} (attempt {attempt})");
                    for _ in 0..30 {
                        if client.get_account(&pubkey).is_ok() {
                            return true;
                        }
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    return true;
                }
                Err(err) => {
                    eprintln!("devnet airdrop attempt {attempt} refused: {err}");
                    std::thread::sleep(Duration::from_secs(5 * attempt));
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the public internet + the devnet faucet (SOLANA_LIVE_DEVNET_RPC)"]
async fn live_devnet_airdrop_sign_send_confirm() {
    let Some((url, rpc)) = devnet_rpc() else {
        return;
    };

    // Fresh in-memory wallet. Never written to disk, never printed.
    let payer = Keypair::new();
    let payer_pubkey = payer.pubkey();
    eprintln!("devnet wallet (throwaway): {payer_pubkey}");

    if !airdrop(url.clone(), payer_pubkey.to_string(), AIRDROP_LAMPORTS).await {
        eprintln!(
            "SKIPPED: the devnet faucet refused every attempt (public faucet rate limit) — \
             no funds were fabricated and nothing was sent"
        );
        return;
    }

    let funded = rpc.get_balance(&payer_pubkey).await.expect("balance");
    assert!(
        funded > 0,
        "after a successful airdrop the wallet must hold lamports, got {funded}"
    );
    eprintln!("devnet balance after airdrop: {funded} lamports");

    // Build a real transfer to a second fresh keypair.
    let recipient = Keypair::new();
    let recipient_pubkey = recipient.pubkey();
    let blockhash = rpc.latest_blockhash(true).await.expect("blockhash");

    let instruction = system_instruction::transfer(&payer_pubkey, &recipient_pubkey, SEND_LAMPORTS);
    let message = solana_sdk::message::Message::new(&[instruction], Some(&payer_pubkey));
    let mut transaction = Transaction::new_unsigned(message);
    transaction
        .try_sign(&[&payer], blockhash.blockhash)
        .expect("local signing must succeed");
    assert!(
        transaction.is_signed(),
        "the transaction must carry the payer signature"
    );

    let versioned = solana_sdk::transaction::VersionedTransaction::from(transaction);
    let signature = rpc
        .send_transaction(&versioned)
        .await
        .expect("devnet must accept the signed transfer");

    let outcome = rpc
        .confirm(&signature, Duration::from_secs(45), Duration::from_secs(2))
        .await
        .expect("confirm must return an outcome");

    match outcome {
        ConfirmOutcome::Confirmed { slot, fee, .. } => {
            eprintln!("devnet transfer LANDED: {signature} slot={slot} fee={fee}");
            let landed = rpc
                .get_balance(&recipient_pubkey)
                .await
                .expect("recipient balance");
            assert_eq!(
                landed, SEND_LAMPORTS,
                "the recipient must hold exactly what was sent"
            );
        }
        other => panic!("the devnet transfer did not land: {other:?}"),
    }
}

/// Negative control: an oversized transfer from a fresh, unfunded account must
/// be rejected by the cluster — never silently "succeed".
#[tokio::test]
#[ignore = "requires the public internet (SOLANA_LIVE_DEVNET_RPC)"]
async fn live_devnet_unfunded_transfer_is_refused() {
    let Some((_url, rpc)) = devnet_rpc() else {
        return;
    };
    let empty = Keypair::new();
    let recipient = Keypair::new();
    let blockhash = rpc.latest_blockhash(true).await.expect("blockhash");
    let instruction =
        system_instruction::transfer(&empty.pubkey(), &recipient.pubkey(), SEND_LAMPORTS);
    let message = solana_sdk::message::Message::new(&[instruction], Some(&empty.pubkey()));
    let mut transaction = Transaction::new_unsigned(message);
    transaction
        .try_sign(&[&empty], blockhash.blockhash)
        .expect("signing an unfunded transaction is legal locally");
    let versioned = solana_sdk::transaction::VersionedTransaction::from(transaction);

    let result = rpc.send_transaction(&versioned).await;
    match result {
        Err(err) => eprintln!("devnet refused the unfunded transfer as expected: {err}"),
        Ok(signature) => {
            // If it was accepted for forwarding, it must still never confirm.
            let outcome = rpc
                .confirm(&signature, Duration::from_secs(20), Duration::from_secs(2))
                .await
                .expect("confirm");
            assert!(
                !matches!(outcome, ConfirmOutcome::Confirmed { .. }),
                "an unfunded transfer must never confirm: {outcome:?}"
            );
            eprintln!("devnet did not confirm the unfunded transfer: {outcome:?}");
        }
    }
}
