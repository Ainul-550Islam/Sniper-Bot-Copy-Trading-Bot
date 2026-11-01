//! LIVE read-only Solana **mainnet-beta** integration through this crate's own
//! RPC client — not a mock, not a recorded fixture.
//!
//! `#[ignore]`d by default because it needs the public internet and a
//! mainnet-beta RPC endpoint. Run it explicitly:
//!
//! ```text
//! SOLANA_LIVE_RPC=https://api.mainnet-beta.solana.com \
//!   cargo test -p solana-kit --test live_mainnet_readonly -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Everything asserted here is read-only: `getVersion`, `getSlot`,
//! `getBlockHeight`, `getLatestBlockhash`, `isBlockhashValid`, `getBalance`,
//! `getAccount`, `getTokenLargestAccounts`, `getRecentPrioritizationFees`, and
//! the crate's own SPL-mint decoder. No keypair is loaded, no transaction is
//! signed and nothing is sent — there is no wallet and no funds in this
//! environment, and this file never pretends otherwise.
//!
//! One method is deliberately absent: `getTokenLargestAccounts` is refused
//! outright on the anonymous mainnet-beta endpoint (`code 429, "Too many
//! requests for a specific RPC call"`, verified with six backed-off attempts),
//! so it needs a keyed provider rather than a retry. Nothing here fakes it.
//!
//! The anonymous public endpoints throttle hard (HTTP 429), so every call goes
//! through [`retrying`], which backs off on 429 and fails loudly on anything
//! else. A 429 retry is a real retry against the real cluster — never a
//! substituted value.

use std::fmt::Display;
use std::time::Duration;

use solana_kit::rpc::Rpc;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;

/// USDC mint on mainnet-beta — a long-lived, well-known token mint.
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// SPL Token program (not Token-2022) — owns the USDC mint.
const SPL_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

fn live_rpc() -> Option<Rpc> {
    let url = std::env::var("SOLANA_LIVE_RPC").unwrap_or_default();
    if url.trim().is_empty() {
        eprintln!(
            "SKIPPED: set SOLANA_LIVE_RPC (e.g. https://api.mainnet-beta.solana.com) to run this test"
        );
        return None;
    }
    // Optional: only used when the operator supplies endpoints that actually
    // serve the cluster (the usual free public ones are gated now).
    let fallbacks: Vec<String> = std::env::var("SOLANA_LIVE_RPC_FALLBACKS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let rpc = Rpc::with_urls(
        url.trim().to_string(),
        String::new(),
        fallbacks,
        CommitmentConfig::confirmed(),
        2,
        Duration::from_secs(20),
    )
    .expect("live RPC client must build from a valid URL");
    Some(rpc)
}

/// Run one RPC call, backing off while the endpoint rate-limits us.
async fn retrying<T, E: Display, F, Fut>(what: &str, mut call: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt = 0u32;
    loop {
        match call().await {
            Ok(value) => return value,
            Err(err) => {
                let message = err.to_string();
                let throttled = message.contains("429") || message.contains("too many requests");
                attempt += 1;
                assert!(
                    throttled,
                    "{what} failed against the live cluster after {attempt} attempt(s): {message}"
                );
                assert!(
                    attempt < 6,
                    "{what} was still rate-limited after {attempt} attempts: {message}"
                );
                let wait = 8 * attempt as u64;
                eprintln!("{what}: endpoint throttled us, retrying in {wait}s");
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        }
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_secs(2)).await;
}

#[tokio::test]
#[ignore = "requires the public internet + a mainnet-beta RPC endpoint (SOLANA_LIVE_RPC)"]
async fn live_mainnet_health_version_and_slot() {
    let Some(rpc) = live_rpc() else { return };

    let version = retrying("getVersion", || rpc.get_version()).await;
    // Mainnet reports a bare semver (e.g. "4.3.2"); other clusters prefix it
    // with "solana-core:". Either form must parse as a real version.
    let numeric = version
        .trim_start_matches("solana-core:")
        .trim()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .chars()
        .all(|c| c.is_ascii_digit());
    assert!(
        numeric && version.contains('.'),
        "unexpected live version string: {version}"
    );
    settle().await;

    let health = retrying("getHealth", || rpc.health()).await;
    assert_eq!(health, "ok", "live cluster health: {health}");
    settle().await;

    let slot = retrying("getSlot", || rpc.get_slot()).await;
    settle().await;
    let height = retrying("getBlockHeight", || rpc.get_block_height()).await;
    // Mainnet is far past these floors; anything lower means we are not
    // talking to the real cluster.
    assert!(slot > 300_000_000, "live slot looked wrong: {slot}");
    assert!(height > 0, "live block height looked wrong: {height}");
    eprintln!("LIVE mainnet slot={slot} block_height={height} version={version}");
}

#[tokio::test]
#[ignore = "requires the public internet + a mainnet-beta RPC endpoint (SOLANA_LIVE_RPC)"]
async fn live_mainnet_blockhash_is_real_and_valid() {
    let Some(rpc) = live_rpc() else { return };
    let cached = retrying("getLatestBlockhash", || rpc.latest_blockhash(false)).await;
    assert!(cached.is_fresh(Duration::from_secs(60)));
    settle().await;
    assert!(
        retrying("isBlockhashValid", || rpc
            .is_blockhash_valid(&cached.blockhash))
        .await,
        "a fresh mainnet blockhash must be reported valid by the cluster"
    );
    eprintln!(
        "LIVE mainnet blockhash={} last_valid_block_height={}",
        cached.blockhash, cached.last_valid_block_height
    );
}

#[tokio::test]
#[ignore = "requires the public internet + a mainnet-beta RPC endpoint (SOLANA_LIVE_RPC)"]
async fn live_mainnet_reads_the_real_usdc_mint() {
    let Some(rpc) = live_rpc() else { return };
    let mint: Pubkey = USDC_MINT.parse().expect("USDC mint address");

    // The crate's own SPL-mint decoder, run against real account data.
    let decimals = retrying("tokenDecimals", || rpc.token_decimals(&mint)).await;
    assert_eq!(decimals, 6, "USDC really has 6 decimals on mainnet");
    settle().await;

    let owner = retrying("tokenProgramOf", || rpc.token_program_of(&mint)).await;
    assert_eq!(owner.to_string(), SPL_TOKEN_PROGRAM);
    settle().await;

    let data = retrying("getAccountData", || rpc.get_account_data(&mint)).await;
    assert_eq!(data.len(), 82, "SPL mint account data is 82 bytes");
    settle().await;

    // A pubkey with no account must come back as None — no invention.
    //
    // Deliberately NOT `Pubkey::new_unique()`: that generator walks a
    // low-entropy counter sequence and real accounts exist at those addresses
    // on mainnet (people have sent SOL to them from tests), so it is not a
    // reliable "absent" probe — verified live during this run. A sha256-derived
    // address is high-entropy and has never held an account.
    let absent = Pubkey::new_from_array(
        solana_sdk::hash::hash(b"sniper-suite/live-mainnet-readonly/absent-probe-v1").to_bytes(),
    );
    assert!(
        retrying("getAccountInfo(absent)", || rpc.get_account(&absent))
            .await
            .is_none(),
        "an account that does not exist must not be reported (probe {absent})"
    );
    eprintln!("LIVE mainnet USDC mint decimals={decimals} owner={owner} data_len=82");
}

#[tokio::test]
#[ignore = "requires the public internet + a mainnet-beta RPC endpoint (SOLANA_LIVE_RPC)"]
async fn live_mainnet_balances_and_fees_are_real_numbers() {
    let Some(rpc) = live_rpc() else { return };
    let mint: Pubkey = USDC_MINT.parse().expect("mint");

    // Real lamports for a real account: the USDC mint carries its
    // rent-exempt reserve on mainnet.
    let balance = retrying("getBalance", || rpc.get_balance(&mint)).await;
    assert!(
        balance > 0,
        "an existing rent-exempt account must hold lamports, got {balance}"
    );
    settle().await;

    let fees = retrying("getRecentPrioritizationFees", || {
        rpc.recent_prioritization_fees(&[])
    })
    .await;
    assert!(!fees.is_empty(), "mainnet must report fee samples");
    settle().await;

    // Same numbers through a second, independent read path: the account's own
    // lamport field must agree with getBalance.
    let account = retrying("getAccountInfo", || rpc.get_account(&mint))
        .await
        .expect("the USDC mint must exist on mainnet");
    assert_eq!(
        account.lamports, balance,
        "getBalance and getAccountInfo must report the same lamports"
    );

    let mut sorted = fees.clone();
    sorted.sort_unstable();
    eprintln!(
        "LIVE mainnet USDC mint lamports={balance} fee_samples={} fee_p50={:?} fee_max={:?}",
        fees.len(),
        sorted.get(sorted.len() / 2).copied(),
        sorted.last().copied()
    );
}
