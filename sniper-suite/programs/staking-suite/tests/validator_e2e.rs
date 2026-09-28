//! End-to-end tests for Module 4 against a real `solana-test-validator`
//! running the compiled BPF object (BUILD PLAN §3 "prove it works").
//!
//! Gated behind `STAKING_E2E=1` so the default `cargo test` stays hermetic:
//!
//! ```text
//! cargo build-sbf                      # produces target/deploy/staking_suite.so
//! STAKING_E2E=1 cargo test --test validator_e2e -- --nocapture
//! ```
//!
//! Environment overrides:
//! * `STAKING_SO`     — path to the `.so` (default: `target/deploy/staking_suite.so`)
//! * `SOL_BIN`        — validator binary (default: `solana-test-validator` on PATH)
//! * `STAKING_LEDGER` — ledger directory (default: `<target>/test-validator-ledger`)
//!
//! What this covers on the real BPF VM (not just host unit tests):
//! program deployment under its declared id, `initialize` (mint + vault +
//! treasury + config PDA creation via CPI), config persistence and borsh
//! layout, re-initialisation guard, stake input guards (below-minimum,
//! paused, unfunded source account → SPL token error, unstake without a
//! stake account), the full governance surface: parameter timelock
//! queue/apply/cancel with hard caps, pause/unpause authorisation, and the
//! two-step admin transfer.
//!
//! The second test closes the former "no genesis distribution" gap:
//! `validator_e2e_funded_staking_lifecycle` runs the FULL money flow on the
//! BPF VM — initialize → one-time `GenesisMint` (admin-only, latched) →
//! stake (fee split vault/treasury) → reward accrual → claim (rewards are
//! minted, supply grows) → unstake (principal returns, vault empties) —
//! plus the genesis replay and non-admin rejections.
//!
//! The third test (`validator_e2e_max_supply_cap_and_metadata`) proves the
//! immutable max-supply cap and the token-metadata integration on the BPF VM:
//! initialize rejects a zero cap, `GenesisMint` fails one unit over the cap
//! and succeeds exactly at it, reward minting CLAMPS to the remaining
//! headroom while claims/unstakes keep succeeding (withdrawals are never
//! gated), and `CreateTokenMetadata` runs against the REAL mpl-token-metadata
//! program (cloned from mainnet-beta — this test needs internet access),
//! including the one-shot replay rejection.
//!
//! Each test spawns its own validator with a tag-suffixed ledger directory;
//! run with `--test-threads=1` on small machines.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use borsh::BorshDeserialize;
use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_request::{RpcError, RpcResponseErrorData};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::InstructionError;
use solana_sdk::program_error::ProgramError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::sysvar;
use solana_sdk::transaction::{Transaction, TransactionError};
use solana_system_interface::program as system_program;

use solana_program::program_pack::Pack;
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::Mint;

use staking_suite::error::StakingError;
use staking_suite::instruction::{
    admin_ix, apply_params_ix, claim_ix, create_token_metadata_ix, genesis_mint_ix, stake_ix,
    unstake_ix, update_params_ix, StakingInstruction,
};
use staking_suite::state::{
    config_pda, metadata_pda, stake_pda, Config, StakeAccount, TOKEN_METADATA_PROGRAM_ID,
};
use staking_suite::ID as PROGRAM_ID;

const SOL: u64 = 1_000_000_000;

// ---------------------------------------------------------------------------
// Validator harness
// ---------------------------------------------------------------------------

struct Validator {
    child: Child,
    rpc: RpcClient,
    log_path: PathBuf,
}

impl Drop for Validator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Validator {
    /// Tail of the validator log, for panic messages.
    fn log_tail(&self) -> String {
        fs::read_to_string(&self.log_path)
            .map(|s| s.lines().rev().take(15).collect::<Vec<_>>().join("\n"))
            .unwrap_or_else(|_| "<no validator log>".into())
    }
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    l.local_addr().expect("local addr").port()
}

fn gated() -> bool {
    std::env::var("STAKING_E2E")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Locate the compiled BPF object.
fn so_path() -> PathBuf {
    if let Ok(p) = std::env::var("STAKING_SO") {
        return PathBuf::from(p);
    }
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .to_string_lossy()
            .into_owned()
    });
    PathBuf::from(target)
        .join("deploy")
        .join("staking_suite.so")
}

/// Spawn `solana-test-validator` with the staking program deployed at its
/// declared id and wait until it reports healthy.
fn spawn_validator(tag: &str) -> Validator {
    spawn_validator_with_args(tag, &[])
}

/// Like [`spawn_validator`] but passes extra CLI args (e.g.
/// `--clone-upgradeable-program <id>` to pull a mainnet program like
/// mpl-token-metadata — program AND programdata — onto the local ledger).
fn spawn_validator_with_args(tag: &str, extra: &[&str]) -> Validator {
    let so = so_path();
    assert!(
        so.exists(),
        "compiled BPF object not found at {} — run `cargo build-sbf` first \
         (or set STAKING_SO=/path/to/staking_suite.so)",
        so.display()
    );

    let bin = std::env::var("SOL_BIN").unwrap_or_else(|_| "solana-test-validator".into());

    let ledger = std::env::var("STAKING_LEDGER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("target")
                    .to_string_lossy()
                    .into_owned()
            });
            PathBuf::from(target).join("test-validator-ledger")
        });
    // Suffix per test so two validators never share (and wipe) one ledger.
    let ledger = PathBuf::from(format!("{}-{tag}", ledger.display()));
    let _ = fs::remove_dir_all(&ledger);
    fs::create_dir_all(&ledger).expect("create ledger dir");
    let log_path = ledger.join("validator.log");

    let rpc_port = free_port();
    let faucet_port = free_port();

    let log = fs::File::create(&log_path).expect("create validator log");
    let mut cmd = Command::new(&bin);
    cmd.arg("--ledger")
        .arg(&ledger)
        .arg("--rpc-port")
        .arg(rpc_port.to_string())
        .arg("--faucet-port")
        .arg(faucet_port.to_string())
        .arg("--bpf-program")
        .arg(PROGRAM_ID.to_string())
        .arg(&so)
        .arg("--reset")
        .arg("--quiet");
    for arg in extra {
        cmd.arg(arg);
    }
    let child = cmd
        .stdout(Stdio::from(log.try_clone().expect("clone log handle")))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {bin}: {e} (set SOL_BIN to override)"));

    let mut validator = Validator {
        child,
        rpc: RpcClient::new_with_commitment(
            format!("http://127.0.0.1:{rpc_port}"),
            CommitmentConfig::confirmed(),
        ),
        log_path,
    };

    // Wait for health (genesis + BPF load typically < 20 s locally).
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(status) = validator.child.try_wait() {
            if status.is_some() {
                panic!(
                    "validator exited early ({status:?}); log tail:\n{}",
                    validator.log_tail()
                );
            }
        }
        match validator.rpc.get_health() {
            Ok(_) => break,
            Err(e) if Instant::now() > deadline => {
                panic!(
                    "validator not healthy after 120 s ({e}); log tail:\n{}",
                    validator.log_tail()
                );
            }
            Err(_) => sleep(Duration::from_millis(500)),
        }
    }

    // The program must actually be deployed at its declared id.
    let acc = validator
        .rpc
        .get_account(&PROGRAM_ID)
        .expect("program account must exist (deployed via --bpf-program)");
    assert_eq!(
        acc.owner,
        solana_sdk::bpf_loader_upgradeable::id(),
        "program account must be owned by the upgradeable loader"
    );

    validator
}

// ---------------------------------------------------------------------------
// Transaction helpers
// ---------------------------------------------------------------------------

// `ClientError` is inherently large (solana-client's boxed error kinds);
// this test helper returns it verbatim for the expect_custom assertions.
#[allow(clippy::result_large_err)]
fn send(
    rpc: &RpcClient,
    ixs: &[solana_sdk::instruction::Instruction],
    signers: &[&Keypair],
) -> Result<solana_sdk::signature::Signature, ClientError> {
    let payer = signers[0];
    let mut tx = Transaction::new_with_payer(ixs, Some(&payer.pubkey()));
    let blockhash = rpc.get_latest_blockhash()?;
    tx.sign(signers, blockhash);
    rpc.send_and_confirm_transaction(&tx)
}

/// Airdrop and wait until the balance actually shows up.
fn fund(rpc: &RpcClient, to: &Pubkey, lamports: u64) {
    rpc.request_airdrop(to, lamports)
        .expect("local faucet airdrop must succeed");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if rpc.get_balance(to).unwrap_or(0) >= lamports {
            return;
        }
        assert!(Instant::now() < deadline, "airdrop to {to} never landed");
        sleep(Duration::from_millis(250));
    }
}

/// The numeric custom error code the program maps this variant to.
fn code_of(e: StakingError) -> u32 {
    match ProgramError::from(e) {
        ProgramError::Custom(c) => c,
        other => unreachable!("expected a custom error, got {other:?}"),
    }
}

/// Extract `InstructionError::Custom(_)` from a preflight/simulation failure.
fn custom_code(err: &ClientError) -> Option<u32> {
    let tx_err = match &err.kind {
        ClientErrorKind::RpcError(RpcError::RpcResponseError {
            data: RpcResponseErrorData::SendTransactionPreflightFailure(sim),
            ..
        }) => sim.err.as_ref(),
        _ => None,
    };
    match tx_err {
        Some(TransactionError::InstructionError(_, InstructionError::Custom(c))) => Some(*c),
        _ => None,
    }
}

fn expect_custom(
    result: Result<solana_sdk::signature::Signature, ClientError>,
    expected: StakingError,
    ctx: &str,
) {
    let err = result.unwrap_err_or_else(ctx);
    let want = code_of(expected.clone());
    let got = custom_code(&err);
    assert_eq!(
        got,
        Some(want),
        "{ctx}: expected custom error {want} ({expected}), got {got:?} / {err}"
    );
}

trait UnwrapErrOrElse {
    fn unwrap_err_or_else(self, ctx: &str) -> ClientError;
}

impl UnwrapErrOrElse for Result<solana_sdk::signature::Signature, ClientError> {
    fn unwrap_err_or_else(self, ctx: &str) -> ClientError {
        match self {
            Ok(sig) => panic!("{ctx}: expected failure, but the transaction confirmed ({sig})"),
            Err(e) => e,
        }
    }
}

fn read_config(rpc: &RpcClient) -> Config {
    let (key, _) = config_pda(&PROGRAM_ID);
    let acc = rpc.get_account(&key).expect("config PDA must exist");
    assert_eq!(acc.owner, PROGRAM_ID, "config PDA must be program-owned");
    Config::try_from_slice(&acc.data).expect("config must deserialize (borsh)")
}

#[allow(clippy::type_complexity)]
fn initialize_ix(
    payer: &Pubkey,
    mint: &Pubkey,
    treasury_wallet: &Pubkey,
    params: (u16, u64, u64, i64, u8, i64, u64),
) -> solana_sdk::instruction::Instruction {
    let (config_key, _) = config_pda(&PROGRAM_ID);
    let vault = get_associated_token_address(&config_key, mint);
    let treasury = get_associated_token_address(treasury_wallet, mint);
    let (fee_bps, reward_rate_bps, min_stake, unstake_delay, decimals, timelock_secs, max_supply) =
        params;
    solana_sdk::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            solana_sdk::instruction::AccountMeta::new(*payer, true),
            solana_sdk::instruction::AccountMeta::new(config_key, false),
            solana_sdk::instruction::AccountMeta::new(*mint, true),
            solana_sdk::instruction::AccountMeta::new(vault, false),
            solana_sdk::instruction::AccountMeta::new(treasury, false),
            solana_sdk::instruction::AccountMeta::new_readonly(*treasury_wallet, false),
            solana_sdk::instruction::AccountMeta::new_readonly(spl_token::id(), false),
            solana_sdk::instruction::AccountMeta::new_readonly(
                spl_associated_token_account::id(),
                false,
            ),
            solana_sdk::instruction::AccountMeta::new_readonly(system_program::id(), false),
            solana_sdk::instruction::AccountMeta::new_readonly(sysvar::rent::id(), false),
        ],
        data: StakingInstruction::Initialize {
            fee_bps,
            reward_rate_bps,
            min_stake,
            unstake_delay,
            decimals,
            timelock_secs,
            max_supply,
        }
        .pack()
        .expect("initialize packs"),
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[test]
fn validator_e2e_full_governance_lifecycle() {
    if !gated() {
        eprintln!(
            "SKIP validator_e2e: set STAKING_E2E=1 (requires `cargo build-sbf` \
             and `solana-test-validator` on PATH)"
        );
        return;
    }

    let v = spawn_validator("governance");
    let rpc = &v.rpc;

    // ---- actors ------------------------------------------------------------
    let admin = Keypair::new();
    let mint = Keypair::new(); // becomes the SPL mint (signer at initialize)
    let treasury_wallet = Keypair::new();
    let staker = Keypair::new();
    fund(rpc, &admin.pubkey(), 10 * SOL);
    fund(rpc, &staker.pubkey(), SOL); // for its ATA rent + tx fees later

    // ---- 1. initialize ------------------------------------------------------
    let (config_key, config_bump) = config_pda(&PROGRAM_ID);
    let vault = get_associated_token_address(&config_key, &mint.pubkey());
    let treasury = get_associated_token_address(&treasury_wallet.pubkey(), &mint.pubkey());

    send(
        rpc,
        &[initialize_ix(
            &admin.pubkey(),
            &mint.pubkey(),
            &treasury_wallet.pubkey(),
            (100, 1_000, 1_000_000, 1, 6, 0, 2_000_000_000),
        )],
        &[&admin, &mint],
    )
    .expect("initialize must confirm on the BPF VM");

    // Config persisted with exactly the parameters we asked for.
    let cfg = read_config(rpc);
    assert!(cfg.initialized);
    assert_eq!(cfg.admin, admin.pubkey());
    assert_eq!(cfg.mint, mint.pubkey());
    assert_eq!(cfg.vault, vault);
    assert_eq!(cfg.treasury, treasury);
    assert_eq!(cfg.fee_bps, 100);
    assert_eq!(cfg.reward_rate_bps, 1_000);
    assert_eq!(cfg.min_stake, 1_000_000);
    assert_eq!(cfg.unstake_delay, 1);
    assert_eq!(cfg.decimals, 6);
    assert_eq!(cfg.config_bump, config_bump);
    assert_eq!(cfg.mint_bump, config_bump);
    assert!(!cfg.paused);
    assert_eq!(cfg.pending_admin, Pubkey::default());
    assert_eq!(cfg.timelock_secs, 0);
    assert!(!cfg.pending.active);
    assert!(!cfg.genesis_done, "genesis latch starts open");
    assert_eq!(cfg.max_supply, 2_000_000_000, "cap persisted verbatim");

    // The mint was created by CPI: token-owned, program PDA is mint authority,
    // zero supply (genesis is a separate, admin-gated instruction — see the
    // funded-lifecycle test below).
    let mint_acc = rpc.get_account(&mint.pubkey()).expect("mint exists");
    assert_eq!(mint_acc.owner, spl_token::id());
    let mint_state = Mint::unpack(&mint_acc.data).expect("mint unpacks");
    assert_eq!(mint_state.decimals, 6);
    assert_eq!(mint_state.supply, 0);
    assert_eq!(
        Option::<Pubkey>::from(mint_state.mint_authority),
        Some(config_key),
        "mint authority must be the config PDA"
    );
    assert_eq!(
        Option::<Pubkey>::from(mint_state.freeze_authority),
        None,
        "initialize_mint2 sets no freeze authority"
    );

    // Vault + treasury ATAs exist with zero balances.
    for (ata, owner) in [
        (&vault, &config_key),
        (&treasury, &treasury_wallet.pubkey()),
    ] {
        let acc = rpc
            .get_account(ata)
            .unwrap_or_else(|e| panic!("ATA {ata}: {e}"));
        assert_eq!(acc.owner, spl_token::id());
        let t = spl_token::state::Account::unpack(&acc.data).expect("token account");
        assert_eq!(t.mint, mint.pubkey());
        assert_eq!(t.owner, *owner);
        assert_eq!(t.amount, 0);
    }

    // ---- 2. re-initialisation guard ------------------------------------------
    let mint2 = Keypair::new();
    expect_custom(
        send(
            rpc,
            &[initialize_ix(
                &admin.pubkey(),
                &mint2.pubkey(),
                &treasury_wallet.pubkey(),
                (100, 1_000, 1_000_000, 1, 6, 0, 2_000_000_000),
            )],
            &[&admin, &mint2],
        ),
        StakingError::AlreadyInitialized,
        "second initialize",
    );

    // ---- 3. stake guards (no tokens needed) ----------------------------------
    // Give the staker an (empty) token account for the mint so the guards that
    // run *before* the SPL transfer are what we exercise.
    let staker_ata = get_associated_token_address(&staker.pubkey(), &mint.pubkey());
    send(
        rpc,
        &[
            spl_associated_token_account::instruction::create_associated_token_account(
                &admin.pubkey(),
                &staker.pubkey(),
                &mint.pubkey(),
                &spl_token::id(),
            ),
        ],
        &[&admin],
    )
    .expect("staker ATA creation");

    // 3a. below the minimum stake
    expect_custom(
        send(
            rpc,
            &[stake_ix(
                &PROGRAM_ID,
                &staker.pubkey(),
                &staker_ata,
                &vault,
                &treasury,
                999_999, // min_stake - 1
            )
            .unwrap()],
            &[&admin, &staker],
        ),
        StakingError::BelowMinimum,
        "stake below minimum",
    );

    // 3b. valid amount but the source account holds nothing: the full account
    // wiring is accepted and the SPL token transfer itself fails (token error
    // #1 InsufficientFunds). Proves stake reaches the money movement step.
    {
        let err = send(
            rpc,
            &[stake_ix(
                &PROGRAM_ID,
                &staker.pubkey(),
                &staker_ata,
                &vault,
                &treasury,
                1_000_000,
            )
            .unwrap()],
            &[&admin, &staker],
        )
        .expect_err("unfunded stake must fail");
        assert_eq!(
            custom_code(&err),
            Some(1),
            "expected SPL token InsufficientFunds (custom 1), got {err}"
        );
    }

    // 3c. unstake with no stake account
    expect_custom(
        send(
            rpc,
            &[unstake_ix(
                &PROGRAM_ID,
                &staker.pubkey(),
                &staker_ata,
                &vault,
                &mint.pubkey(),
            )
            .unwrap()],
            &[&admin, &staker],
        ),
        StakingError::InvalidStakeAccount,
        "unstake without a stake account",
    );

    // ---- 4. pause / unpause ---------------------------------------------------
    // Non-admin may not pause.
    expect_custom(
        send(
            rpc,
            &[admin_ix(&PROGRAM_ID, &staker.pubkey(), StakingInstruction::Pause).unwrap()],
            &[&admin, &staker],
        ),
        StakingError::Unauthorized,
        "pause by non-admin",
    );

    send(
        rpc,
        &[admin_ix(&PROGRAM_ID, &admin.pubkey(), StakingInstruction::Pause).unwrap()],
        &[&admin],
    )
    .expect("admin pause");
    assert!(read_config(rpc).paused);

    // While paused, deposits are rejected before any balance is looked at.
    expect_custom(
        send(
            rpc,
            &[stake_ix(
                &PROGRAM_ID,
                &staker.pubkey(),
                &staker_ata,
                &vault,
                &treasury,
                1, // would be BelowMinimum if the pause check came later
            )
            .unwrap()],
            &[&admin, &staker],
        ),
        StakingError::Paused,
        "stake while paused",
    );

    send(
        rpc,
        &[admin_ix(&PROGRAM_ID, &admin.pubkey(), StakingInstruction::Unpause).unwrap()],
        &[&admin],
    )
    .expect("admin unpause");
    assert!(!read_config(rpc).paused);
    // Sanity: un-paused, the same tiny stake fails on the minimum again.
    expect_custom(
        send(
            rpc,
            &[stake_ix(
                &PROGRAM_ID,
                &staker.pubkey(),
                &staker_ata,
                &vault,
                &treasury,
                1,
            )
            .unwrap()],
            &[&admin, &staker],
        ),
        StakingError::BelowMinimum,
        "stake after unpause",
    );

    // ---- 5. parameter timelock (timelock_secs = 0 → immediate apply) ---------
    send(
        rpc,
        &[update_params_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            Some(150),
            Some(2_000),
            None,
            None,
            None,
        )
        .unwrap()],
        &[&admin],
    )
    .expect("queue update");
    {
        let pending = read_config(rpc).pending;
        assert!(pending.active);
        assert_eq!(pending.fee_bps, 150);
        assert_eq!(pending.reward_rate_bps, 2_000);
        assert!(pending.queued_at > 0, "queued_at stamped from Clock");
    }

    // A second queue while one is pending is rejected.
    expect_custom(
        send(
            rpc,
            &[update_params_ix(
                &PROGRAM_ID,
                &admin.pubkey(),
                Some(160),
                None,
                None,
                None,
                None,
            )
            .unwrap()],
            &[&admin],
        ),
        StakingError::UpdateAlreadyQueued,
        "double queue",
    );

    // Apply is permissionless: the *staker* pays and signs, not the admin.
    send(rpc, &[apply_params_ix(&PROGRAM_ID).unwrap()], &[&staker]).expect("permissionless apply");
    {
        let cfg = read_config(rpc);
        assert_eq!(cfg.fee_bps, 150, "queued fee took effect");
        assert_eq!(cfg.reward_rate_bps, 2_000, "queued rate took effect");
        assert_eq!(cfg.min_stake, 1_000_000, "None fields keep live values");
        assert!(!cfg.pending.active);
    }

    // Applying again with nothing queued fails.
    expect_custom(
        send(rpc, &[apply_params_ix(&PROGRAM_ID).unwrap()], &[&admin]),
        StakingError::NoPendingUpdate,
        "apply with nothing queued",
    );

    // Hard caps are enforced at queue time.
    expect_custom(
        send(
            rpc,
            &[update_params_ix(
                &PROGRAM_ID,
                &admin.pubkey(),
                Some(1_001), // MAX_FEE_BPS + 1
                None,
                None,
                None,
                None,
            )
            .unwrap()],
            &[&admin],
        ),
        StakingError::FeeTooHigh,
        "queue over-cap fee",
    );
    expect_custom(
        send(
            rpc,
            &[update_params_ix(
                &PROGRAM_ID,
                &admin.pubkey(),
                None,
                Some(10_001), // MAX_REWARD_RATE_BPS + 1
                None,
                None,
                None,
            )
            .unwrap()],
            &[&admin],
        ),
        StakingError::RewardRateTooHigh,
        "queue over-cap reward rate",
    );

    // Cancel works and leaves the config untouched.
    send(
        rpc,
        &[update_params_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            Some(900),
            None,
            None,
            None,
            None,
        )
        .unwrap()],
        &[&admin],
    )
    .expect("queue then cancel: queue");
    send(
        rpc,
        &[admin_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            StakingInstruction::CancelParams,
        )
        .unwrap()],
        &[&admin],
    )
    .expect("queue then cancel: cancel");
    {
        let cfg = read_config(rpc);
        assert!(!cfg.pending.active);
        assert_eq!(cfg.fee_bps, 150, "cancelled update must not apply");
    }
    // Non-admin may not cancel either (queue one, try to cancel as staker).
    send(
        rpc,
        &[update_params_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            Some(151),
            None,
            None,
            None,
            None,
        )
        .unwrap()],
        &[&admin],
    )
    .expect("queue for cancel-auth test");
    expect_custom(
        send(
            rpc,
            &[admin_ix(
                &PROGRAM_ID,
                &staker.pubkey(),
                StakingInstruction::CancelParams,
            )
            .unwrap()],
            &[&admin, &staker],
        ),
        StakingError::Unauthorized,
        "cancel by non-admin",
    );
    send(
        rpc,
        &[admin_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            StakingInstruction::CancelParams,
        )
        .unwrap()],
        &[&admin],
    )
    .expect("admin cancels its own queued update");

    // ---- 6. two-step admin transfer ------------------------------------------
    send(
        rpc,
        &[admin_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            StakingInstruction::TransferAdmin {
                new_admin: staker.pubkey(),
            },
        )
        .unwrap()],
        &[&admin],
    )
    .expect("propose new admin");
    assert_eq!(read_config(rpc).pending_admin, staker.pubkey());

    // Wrong key cannot accept.
    expect_custom(
        send(
            rpc,
            &[admin_ix(
                &PROGRAM_ID,
                &treasury_wallet.pubkey(),
                StakingInstruction::AcceptAdmin,
            )
            .unwrap()],
            &[&admin, &treasury_wallet],
        ),
        StakingError::NotPendingAdmin,
        "accept by wrong key",
    );
    // (the failed accept must not have changed anything)
    assert_eq!(read_config(rpc).admin, admin.pubkey());

    // The pending admin accepts; the treasury wallet signs to pay nothing —
    // admin pays the fee, staker signs the accept.
    send(
        rpc,
        &[admin_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            StakingInstruction::AcceptAdmin,
        )
        .unwrap()],
        &[&admin, &staker],
    )
    .expect("pending admin accepts");
    {
        let cfg = read_config(rpc);
        assert_eq!(cfg.admin, staker.pubkey(), "admin rotated");
        assert_eq!(cfg.pending_admin, Pubkey::default(), "pending cleared");
    }

    // The old admin lost authority; the new one has it.
    expect_custom(
        send(
            rpc,
            &[admin_ix(&PROGRAM_ID, &admin.pubkey(), StakingInstruction::Pause).unwrap()],
            &[&admin],
        ),
        StakingError::Unauthorized,
        "pause by former admin",
    );
    send(
        rpc,
        &[admin_ix(&PROGRAM_ID, &staker.pubkey(), StakingInstruction::Pause).unwrap()],
        &[&staker],
    )
    .expect("new admin pauses");
    assert!(read_config(rpc).paused);
    send(
        rpc,
        &[admin_ix(&PROGRAM_ID, &staker.pubkey(), StakingInstruction::Unpause).unwrap()],
        &[&staker],
    )
    .expect("new admin unpauses");

    // The stake PDA address used by the guards above is the documented one.
    let (stake_key, _) = stake_pda(&PROGRAM_ID, &staker.pubkey());
    assert!(
        rpc.get_account(&stake_key).is_err(),
        "no stake account was ever created (all deposits failed by design of \
         these guard tests)"
    );
}

// ---------------------------------------------------------------------------
// Funded money-flow lifecycle (closes the former genesis gap)
// ---------------------------------------------------------------------------

/// Exact token balance of an SPL account (raw units).
fn token_balance(rpc: &RpcClient, ata: &Pubkey) -> u64 {
    rpc.get_token_account_balance(ata)
        .unwrap_or_else(|e| panic!("token balance for {ata}: {e}"))
        .amount
        .parse::<u64>()
        .expect("balance parses as u64")
}

#[test]
fn validator_e2e_funded_staking_lifecycle() {
    if !gated() {
        eprintln!(
            "SKIP validator_e2e funded lifecycle: set STAKING_E2E=1 \
             (requires `cargo build-sbf` and `solana-test-validator` on PATH)"
        );
        return;
    }

    let v = spawn_validator("funded");
    let rpc = &v.rpc;

    // ---- actors ------------------------------------------------------------
    let admin = Keypair::new();
    let mint = Keypair::new();
    let treasury_wallet = Keypair::new();
    let staker = Keypair::new();
    fund(rpc, &admin.pubkey(), 10 * SOL);
    fund(rpc, &staker.pubkey(), SOL);
    fund(rpc, &treasury_wallet.pubkey(), SOL);

    // ---- 1. initialize ------------------------------------------------------
    // fee 1%, reward 10_000 bps (= 100%/yr — the max, so accrual is visible
    // within a ~70 s test window), min stake 1 token, cooldown 1 s, 6 decimals.
    send(
        rpc,
        &[initialize_ix(
            &admin.pubkey(),
            &mint.pubkey(),
            &treasury_wallet.pubkey(),
            (100, 10_000, 1_000_000, 1, 6, 0, 2_000_000_000),
        )],
        &[&admin, &mint],
    )
    .expect("initialize must confirm");

    let (config_key, _) = config_pda(&PROGRAM_ID);
    let vault = get_associated_token_address(&config_key, &mint.pubkey());
    let treasury = get_associated_token_address(&treasury_wallet.pubkey(), &mint.pubkey());
    let staker_ata = get_associated_token_address(&staker.pubkey(), &mint.pubkey());
    let (stake_key, _) = stake_pda(&PROGRAM_ID, &staker.pubkey());

    send(
        rpc,
        &[
            spl_associated_token_account::instruction::create_associated_token_account(
                &staker.pubkey(),
                &staker.pubkey(),
                &mint.pubkey(),
                &spl_token::id(),
            ),
        ],
        &[&staker],
    )
    .expect("staker ATA creation");

    // ---- 2. genesis mint (one-time initial distribution) --------------------
    let genesis_amount: u64 = 1_000_000_000; // 1000 tokens @ 6 decimals
    send(
        rpc,
        &[genesis_mint_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            &mint.pubkey(),
            &staker_ata,
            genesis_amount,
        )
        .expect("genesis ix packs")],
        &[&admin],
    )
    .expect("genesis mint must confirm on the BPF VM");

    let mint_state =
        Mint::unpack(&rpc.get_account(&mint.pubkey()).expect("mint").data).expect("mint unpacks");
    assert_eq!(mint_state.supply, genesis_amount, "supply == genesis");
    assert_eq!(token_balance(rpc, &staker_ata), genesis_amount);
    assert!(read_config(rpc).genesis_done, "latch flipped");

    // Replay is refused (supply inflation guard).
    expect_custom(
        send(
            rpc,
            &[
                genesis_mint_ix(&PROGRAM_ID, &admin.pubkey(), &mint.pubkey(), &staker_ata, 1)
                    .expect("ix"),
            ],
            &[&admin],
        ),
        StakingError::GenesisAlreadyDone,
        "second genesis mint",
    );

    // A non-admin cannot genesis-mint (checked BEFORE the latch on purpose:
    // authorization must not depend on genesis state).
    expect_custom(
        send(
            rpc,
            &[genesis_mint_ix(
                &PROGRAM_ID,
                &treasury_wallet.pubkey(),
                &mint.pubkey(),
                &staker_ata,
                1,
            )
            .expect("ix")],
            &[&treasury_wallet],
        ),
        StakingError::Unauthorized,
        "non-admin genesis mint",
    );

    // ---- 3. stake 500 tokens (1% fee splits vault/treasury) ------------------
    let stake_amount: u64 = 500_000_000;
    let fee = stake_amount / 100; // 5_000_000
    let net = stake_amount - fee; // 495_000_000
    send(
        rpc,
        &[stake_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            &staker_ata,
            &vault,
            &treasury,
            stake_amount,
        )
        .expect("stake ix")],
        &[&staker],
    )
    .expect("stake must confirm");

    assert_eq!(token_balance(rpc, &vault), net, "principal net of fee");
    assert_eq!(token_balance(rpc, &treasury), fee, "fee to treasury");
    assert_eq!(
        token_balance(rpc, &staker_ata),
        genesis_amount - stake_amount
    );

    let sa = StakeAccount::try_from_slice(
        &rpc.get_account(&stake_key)
            .expect("stake PDA created by the program")
            .data,
    )
    .expect("stake account borsh");
    assert_eq!(sa.owner, staker.pubkey());
    assert_eq!(sa.amount, net);

    // ---- 4. reward accrual + claim ------------------------------------------
    // 100%/yr on 495e6 raw ≈ 15.7 raw/s → ~1100 raw after 70 s.
    sleep(Duration::from_secs(70));

    let supply_before = Mint::unpack(&rpc.get_account(&mint.pubkey()).unwrap().data)
        .unwrap()
        .supply;
    let bal_before = token_balance(rpc, &staker_ata);
    send(
        rpc,
        &[claim_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            &staker_ata,
            &vault,
            &mint.pubkey(),
        )
        .expect("claim ix")],
        &[&staker],
    )
    .expect("claim must confirm");

    let rewards = token_balance(rpc, &staker_ata) - bal_before;
    assert!(
        rewards > 0,
        "rewards accrued and were minted: {rewards} raw"
    );
    assert_eq!(
        token_balance(rpc, &vault),
        net,
        "claim never touches the principal"
    );
    let supply_after = Mint::unpack(&rpc.get_account(&mint.pubkey()).unwrap().data)
        .unwrap()
        .supply;
    assert_eq!(
        supply_after - supply_before,
        rewards,
        "rewards are MINTED (supply grows by exactly the payout)"
    );
    // Sanity: within 5x of the analytic value for the elapsed window.
    let expected_approx = net * 70 / 31_536_000; // rate 10_000 bps == 100%/yr
    assert!(
        rewards >= expected_approx / 5 && rewards <= expected_approx * 5,
        "rewards {rewards} near analytic {expected_approx}"
    );

    // ---- 5. unstake: principal (+ residual rewards) returns ------------------
    send(
        rpc,
        &[unstake_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            &staker_ata,
            &vault,
            &mint.pubkey(),
        )
        .expect("unstake ix")],
        &[&staker],
    )
    .expect("unstake must confirm");

    assert_eq!(token_balance(rpc, &vault), 0, "vault fully drained");
    let final_bal = token_balance(rpc, &staker_ata);
    assert!(
        final_bal >= bal_before + rewards + net,
        "staker holds principal + rewards: {final_bal}"
    );
    let sa_after = StakeAccount::try_from_slice(&rpc.get_account(&stake_key).unwrap().data)
        .expect("stake account still readable");
    assert_eq!(sa_after.amount, 0, "position zeroed");
    assert_eq!(sa_after.pending_rewards, 0, "nothing left claimable");
}

#[test]
fn validator_e2e_max_supply_cap_and_metadata() {
    if !gated() {
        eprintln!(
            "SKIP validator_e2e cap+metadata: set STAKING_E2E=1 \
             (requires `cargo build-sbf`, `solana-test-validator` on PATH and \
             INTERNET ACCESS — the validator clones mpl-token-metadata from \
             mainnet-beta for the metadata CPI)"
        );
        return;
    }

    // Clone the real mpl-token-metadata program so CreateMetadataAccountsV3
    // runs against the production implementation, not a mock.
    // `--clone-upgradeable-program` (not plain `--clone`) is required: mpl is
    // an upgradeable-loader program, and plain `--clone` copies only the
    // 36-byte program account WITHOUT its programdata account, which makes
    // the loader report "Program is not deployed" at execution time
    // (observed on Agave 2.1.21).
    let v = spawn_validator_with_args(
        "capmeta",
        &[
            "--clone-upgradeable-program",
            "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s",
            "--url",
            "https://api.mainnet-beta.solana.com",
        ],
    );
    let rpc = &v.rpc;

    let admin = Keypair::new();
    let mint = Keypair::new();
    let treasury_wallet = Keypair::new();
    let staker = Keypair::new();
    fund(rpc, &admin.pubkey(), 10 * SOL);
    fund(rpc, &staker.pubkey(), SOL);
    fund(rpc, &treasury_wallet.pubkey(), SOL);

    // ---- 1. max_supply = 0 is rejected (config never created) ---------------
    expect_custom(
        send(
            rpc,
            &[initialize_ix(
                &admin.pubkey(),
                &mint.pubkey(),
                &treasury_wallet.pubkey(),
                (100, 10_000, 1, 1, 6, 0, 0),
            )],
            &[&admin, &mint],
        ),
        StakingError::InvalidMaxSupply,
        "initialize with zero cap",
    );

    // ---- 2. real initialize with a TIGHT cap ---------------------------------
    // 100% APR so reward accrual becomes visible within the test window.
    let cap: u64 = 1_000_000;
    send(
        rpc,
        &[initialize_ix(
            &admin.pubkey(),
            &mint.pubkey(),
            &treasury_wallet.pubkey(),
            (100, 10_000, 1, 1, 6, 0, cap),
        )],
        &[&admin, &mint],
    )
    .expect("initialize must confirm");
    assert_eq!(read_config(rpc).max_supply, cap);

    let (config_key, _) = config_pda(&PROGRAM_ID);
    let vault = get_associated_token_address(&config_key, &mint.pubkey());
    let treasury = get_associated_token_address(&treasury_wallet.pubkey(), &mint.pubkey());
    let staker_ata = get_associated_token_address(&staker.pubkey(), &mint.pubkey());
    let (stake_key, _) = stake_pda(&PROGRAM_ID, &staker.pubkey());

    send(
        rpc,
        &[
            spl_associated_token_account::instruction::create_associated_token_account(
                &staker.pubkey(),
                &staker.pubkey(),
                &mint.pubkey(),
                &spl_token::id(),
            ),
        ],
        &[&staker],
    )
    .expect("staker ATA creation");

    // ---- 3. genesis: one over the cap fails, exactly-at-cap succeeds ---------
    expect_custom(
        send(
            rpc,
            &[genesis_mint_ix(
                &PROGRAM_ID,
                &admin.pubkey(),
                &mint.pubkey(),
                &staker_ata,
                cap + 1,
            )
            .expect("ix")],
            &[&admin],
        ),
        StakingError::MaxSupplyExceeded,
        "genesis one over cap",
    );
    // The rejected attempt must NOT have flipped the latch.
    assert!(!read_config(rpc).genesis_done);

    send(
        rpc,
        &[genesis_mint_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            &mint.pubkey(),
            &staker_ata,
            cap,
        )
        .expect("ix")],
        &[&admin],
    )
    .expect("genesis exactly at cap must confirm");
    let mint_state =
        Mint::unpack(&rpc.get_account(&mint.pubkey()).expect("mint").data).expect("unpack");
    assert_eq!(mint_state.supply, cap, "supply sits exactly at the cap");
    assert!(read_config(rpc).genesis_done);

    // ---- 4. token metadata: authz, success, one-shot replay ------------------
    expect_custom(
        send(
            rpc,
            &[create_token_metadata_ix(
                &PROGRAM_ID,
                &treasury_wallet.pubkey(), // NOT the admin
                &mint.pubkey(),
                "Cap Test Token",
                "CAP",
                "https://example.invalid/cap.json",
            )
            .expect("ix")],
            &[&treasury_wallet],
        ),
        StakingError::Unauthorized,
        "non-admin metadata",
    );

    send(
        rpc,
        &[create_token_metadata_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            &mint.pubkey(),
            "Cap Test Token",
            "CAP",
            "https://example.invalid/cap.json",
        )
        .expect("ix")],
        &[&admin],
    )
    .expect("metadata creation must confirm against the real mpl program");

    let (meta_key, _) = metadata_pda(&mint.pubkey());
    let meta_acc = rpc
        .get_account(&meta_key)
        .expect("metadata PDA must exist after the CPI");
    assert_eq!(meta_acc.owner, TOKEN_METADATA_PROGRAM_ID);
    assert!(meta_acc.lamports > 0);
    // The mpl Metadata layout is not parsed here (no mpl dependency in this
    // crate); the borsh-encoded name/symbol/uri MUST appear verbatim in the
    // account data, and the mint address is part of the metadata struct.
    assert!(
        meta_acc.data.windows(14).any(|w| w == b"Cap Test Token"),
        "metadata data must contain the name"
    );
    assert!(
        meta_acc.data.windows(3).any(|w| w == b"CAP"),
        "metadata data must contain the symbol"
    );
    assert!(
        meta_acc
            .data
            .windows("https://example.invalid/cap.json".len())
            .any(|w| w == b"https://example.invalid/cap.json"),
        "metadata data must contain the uri"
    );
    assert!(
        meta_acc
            .data
            .windows(32)
            .any(|w| w == mint.pubkey().to_bytes()),
        "metadata must reference the mint"
    );

    expect_custom(
        send(
            rpc,
            &[create_token_metadata_ix(
                &PROGRAM_ID,
                &admin.pubkey(),
                &mint.pubkey(),
                "Other Name",
                "OTH",
                "https://example.invalid/other.json",
            )
            .expect("ix")],
            &[&admin],
        ),
        StakingError::MetadataAlreadyExists,
        "metadata replay",
    );

    // ---- 5. rewards clamp at the cap: claim succeeds, supply never grows -----
    send(
        rpc,
        &[stake_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            &staker_ata,
            &vault,
            &treasury,
            500_000,
        )
        .expect("stake ix")],
        &[&staker],
    )
    .expect("stake must confirm");

    // 100% APR on a 495_000 net stake accrues > 1 raw unit after ~64 s.
    sleep(Duration::from_secs(70));

    send(
        rpc,
        &[claim_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            &staker_ata,
            &vault,
            &mint.pubkey(),
        )
        .expect("claim ix")],
        &[&staker],
    )
    .expect("claim at zero headroom must STILL succeed (withdrawals never gated)");

    let mint_state =
        Mint::unpack(&rpc.get_account(&mint.pubkey()).expect("mint").data).expect("unpack");
    assert_eq!(
        mint_state.supply, cap,
        "supply must never exceed the cap — rewards clamped to zero headroom"
    );
    let sa = StakeAccount::try_from_slice(&rpc.get_account(&stake_key).expect("stake").data)
        .expect("stake account");
    assert_eq!(sa.pending_rewards, 0, "clamped reward state reset");
    assert_eq!(sa.amount, 495_000, "principal untouched by claim");

    // ---- 6. unstake returns the principal; supply still capped ---------------
    send(
        rpc,
        &[unstake_ix(
            &PROGRAM_ID,
            &staker.pubkey(),
            &staker_ata,
            &vault,
            &mint.pubkey(),
        )
        .expect("unstake ix")],
        &[&staker],
    )
    .expect("unstake must confirm");
    // 1_000_000 genesis - 500_000 staked + 495_000 principal back + 0 rewards
    // (clamped at the cap); the 5_000 deposit fee stays in the treasury.
    assert_eq!(token_balance(rpc, &staker_ata), 995_000);
    let mint_state =
        Mint::unpack(&rpc.get_account(&mint.pubkey()).expect("mint").data).expect("unpack");
    assert_eq!(mint_state.supply, cap);
}
