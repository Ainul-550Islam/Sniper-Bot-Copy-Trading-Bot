//! Instruction processor.
//!
//! Pure-Rust (no Anchor) account handling with a strict validation layer:
//! every account the program trusts is checked before use.
//!
//! * The global config MUST be the program's `["staking-config"]` PDA and be
//!   owned by this program.
//! * A staker's state MUST be their `["staking-stake", staker]` PDA, owned by
//!   this program, with `owner == staker`.
//! * The vault / mint / treasury MUST equal the addresses stored in the config.
//! * The token / system / associated-token programs MUST be the canonical ids.
//! * The staker's token account MUST be an SPL token account of the config mint
//!   owned by the staker.
//!
//! Every PDA the program owns signs via `invoke_signed` with its derivation
//! seeds:
//! * config — `[CONFIG_SEED, bump]` (also the mint authority, so it can mint
//!   rewards),
//! * stake  — `[STAKE_SEED, staker, bump]`.

use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    instruction::AccountMeta,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    sysvar::{rent::Rent, Sysvar},
};
use solana_system_interface::{instruction as system_instruction, program as system_program};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account_idempotent,
};
use spl_token::{
    instruction::{mint_to, transfer as token_transfer},
    state::{Account as TokenAccount, Mint},
};

use crate::error::StakingError;
use crate::instruction::{validate_metadata_fields, StakingInstruction};
use crate::state::{
    compute_fee, config_pda, fits_under_cap, metadata_pda, stake_pda, supply_headroom, Config,
    PendingParams, StakeAccount, CONFIG_SEED, MAX_FEE_BPS, MAX_REWARD_RATE_BPS, MAX_TIMELOCK_SECS,
    STAKE_SEED, TOKEN_METADATA_PROGRAM_ID,
};

/// Program entrypoint dispatch.
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = StakingInstruction::unpack(instruction_data)?;
    match ix {
        StakingInstruction::Initialize {
            fee_bps,
            reward_rate_bps,
            min_stake,
            unstake_delay,
            decimals,
            timelock_secs,
            max_supply,
        } => process_initialize(
            program_id,
            accounts,
            fee_bps,
            reward_rate_bps,
            min_stake,
            unstake_delay,
            decimals,
            timelock_secs,
            max_supply,
        ),
        StakingInstruction::Stake { amount } => process_stake(program_id, accounts, amount),
        StakingInstruction::Unstake => process_unstake(program_id, accounts, false),
        StakingInstruction::Claim => process_unstake(program_id, accounts, true),
        StakingInstruction::UpdateParams {
            fee_bps,
            reward_rate_bps,
            min_stake,
            unstake_delay,
            timelock_secs,
        } => process_queue_update(
            program_id,
            accounts,
            fee_bps,
            reward_rate_bps,
            min_stake,
            unstake_delay,
            timelock_secs,
        ),
        StakingInstruction::ApplyParams => process_apply_update(program_id, accounts),
        StakingInstruction::CancelParams => process_cancel_update(program_id, accounts),
        StakingInstruction::Pause => process_set_paused(program_id, accounts, true),
        StakingInstruction::Unpause => process_set_paused(program_id, accounts, false),
        StakingInstruction::TransferAdmin { new_admin } => {
            process_transfer_admin(program_id, accounts, new_admin)
        }
        StakingInstruction::AcceptAdmin => process_accept_admin(program_id, accounts),
        StakingInstruction::GenesisMint { amount } => {
            process_genesis_mint(program_id, accounts, amount)
        }
        StakingInstruction::CreateTokenMetadata { name, symbol, uri } => {
            process_create_token_metadata(program_id, accounts, name, symbol, uri)
        }
    }
}

// ---------------------------------------------------------------------------
// Serialisation helpers.
// ---------------------------------------------------------------------------

/// Deserialize borsh state, mapping IO errors to a program error.
fn deserialize<T: BorshDeserialize>(data: &[u8]) -> Result<T, ProgramError> {
    T::try_from_slice(data).map_err(|_| StakingError::InvalidInstructionData.into())
}

/// Serialize borsh state.
fn serialize<T: borsh::BorshSerialize>(value: &T) -> Result<Vec<u8>, ProgramError> {
    borsh::to_vec(value).map_err(|_| StakingError::InvalidInstructionData.into())
}

// ---------------------------------------------------------------------------
// Account-validation helpers (the security layer).
// ---------------------------------------------------------------------------

/// Require `info` to be a signer.
fn require_signer(info: &AccountInfo, name: &str) -> ProgramResult {
    if !info.is_signer {
        msg!("missing required signature: {}", name);
        return Err(ProgramError::MissingRequiredSignature);
    }
    Ok(())
}

/// Require `info.key == expected`, returning `err` otherwise.
fn require_address(
    info: &AccountInfo,
    expected: &Pubkey,
    err: StakingError,
    name: &str,
) -> ProgramResult {
    if info.key != expected {
        msg!(
            "account {} has wrong address {} (expected {})",
            name,
            info.key,
            expected
        );
        return Err(err.into());
    }
    Ok(())
}

/// Require `info.owner == expected_owner`, returning `err` otherwise.
fn require_owner(
    info: &AccountInfo,
    expected_owner: &Pubkey,
    err: StakingError,
    name: &str,
) -> ProgramResult {
    if info.owner != expected_owner {
        msg!(
            "account {} has wrong owner {} (expected {})",
            name,
            info.owner,
            expected_owner
        );
        return Err(err.into());
    }
    Ok(())
}

/// Load and validate the global config: it MUST be this program's config PDA,
/// owned by this program, non-empty and flagged `initialized`.
fn load_config(program_id: &Pubkey, config_acc: &AccountInfo) -> Result<Config, ProgramError> {
    let (expected, _bump) = config_pda(program_id);
    require_address(
        config_acc,
        &expected,
        StakingError::InvalidConfigAccount,
        "config",
    )?;
    require_owner(
        config_acc,
        program_id,
        StakingError::InvalidConfigAccount,
        "config",
    )?;
    if config_acc.data_len() == 0 {
        msg!("config account is not allocated");
        return Err(StakingError::InvalidConfigAccount.into());
    }
    let config: Config = deserialize(&config_acc.data.borrow())?;
    if !config.initialized {
        msg!("config account is not initialized");
        return Err(StakingError::InvalidConfigAccount.into());
    }
    Ok(config)
}

/// Validate the staker's SPL token account: owned by the token program, of the
/// config mint, and owned by `staker`.
fn require_staker_token(
    staker_token: &AccountInfo,
    staker: &Pubkey,
    mint: &Pubkey,
) -> ProgramResult {
    require_owner(
        staker_token,
        &spl_token::id(),
        StakingError::InvalidStakerToken,
        "staker_token",
    )?;
    let acct = {
        let data = staker_token.data.borrow();
        TokenAccount::unpack(&data).map_err(|_| StakingError::InvalidStakerToken)?
    };
    if acct.mint != *mint {
        msg!("staker_token mint {} != config mint {}", acct.mint, mint);
        return Err(StakingError::InvalidStakerToken.into());
    }
    if acct.owner != *staker {
        msg!("staker_token owner {} != staker {}", acct.owner, staker);
        return Err(StakingError::InvalidStakerToken.into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parameter caps + config persistence.
// ---------------------------------------------------------------------------

/// Enforce the hard caps on the fee and reward rate so neither `initialize` nor
/// a (possibly compromised) admin via `update_params` can set a confiscatory
/// deposit fee or an inflationary reward rate.
fn validate_params(fee_bps: u16, reward_rate_bps: u64) -> ProgramResult {
    if fee_bps > MAX_FEE_BPS {
        msg!("fee_bps {} exceeds cap {}", fee_bps, MAX_FEE_BPS);
        return Err(StakingError::FeeTooHigh.into());
    }
    if reward_rate_bps > MAX_REWARD_RATE_BPS {
        msg!(
            "reward_rate_bps {} exceeds cap {}",
            reward_rate_bps,
            MAX_REWARD_RATE_BPS
        );
        return Err(StakingError::RewardRateTooHigh.into());
    }
    Ok(())
}

/// Enforce the allowed range for the timelock delay: `[0, MAX_TIMELOCK_SECS]`.
fn validate_timelock(secs: i64) -> ProgramResult {
    if !(0..=MAX_TIMELOCK_SECS).contains(&secs) {
        msg!(
            "timelock_secs {} out of range [0, {}]",
            secs,
            MAX_TIMELOCK_SECS
        );
        return Err(StakingError::TimelockOutOfRange.into());
    }
    Ok(())
}

/// Serialize `config` back into its (already-allocated) account. Returns an
/// error rather than panicking if the account is somehow too small.
fn save_config(config_acc: &AccountInfo, config: &Config) -> ProgramResult {
    let serialized = serialize(config)?;
    let mut data = config_acc.data.borrow_mut();
    if data.len() < serialized.len() {
        msg!(
            "config account too small: {} < {}",
            data.len(),
            serialized.len()
        );
        return Err(StakingError::InvalidConfigAccount.into());
    }
    data[..serialized.len()].copy_from_slice(&serialized);
    Ok(())
}

// ---------------------------------------------------------------------------
// Instruction handlers.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn process_initialize(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    fee_bps: u16,
    reward_rate_bps: u64,
    min_stake: u64,
    unstake_delay: i64,
    decimals: u8,
    timelock_secs: i64,
    max_supply: u64,
) -> ProgramResult {
    // Reject confiscatory/inflationary parameters before doing any work.
    validate_params(fee_bps, reward_rate_bps)?;
    validate_timelock(timelock_secs)?;
    // A zero cap would make every mint (genesis AND rewards) impossible; the
    // operator must state the intended total supply explicitly.
    if max_supply == 0 {
        msg!("max_supply must be greater than zero");
        return Err(StakingError::InvalidMaxSupply.into());
    }

    let ai = &mut accounts.iter();
    let payer = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;
    let mint_acc = next_account_info(ai)?;
    let vault_acc = next_account_info(ai)?;
    let treasury_acc = next_account_info(ai)?;
    let treasury_wallet = next_account_info(ai)?;
    let token_program = next_account_info(ai)?;
    let assoc_program = next_account_info(ai)?;
    let system_program_acc = next_account_info(ai)?;
    let rent_acc = next_account_info(ai)?;

    require_signer(payer, "payer")?;
    require_signer(mint_acc, "mint")?;

    // The programs invoked below MUST be the canonical ones, otherwise a caller
    // could substitute a malicious program and receive the config PDA's
    // signature via `invoke_signed`.
    require_address(
        token_program,
        &spl_token::id(),
        StakingError::InvalidTokenProgram,
        "token_program",
    )?;
    require_address(
        assoc_program,
        &spl_associated_token_account::id(),
        StakingError::InvalidAssociatedTokenProgram,
        "assoc_program",
    )?;
    require_address(
        system_program_acc,
        &system_program::id(),
        StakingError::InvalidSystemProgram,
        "system_program",
    )?;

    let (config_key, config_bump) = config_pda(program_id);
    require_address(
        config_acc,
        &config_key,
        StakingError::InvalidConfigAccount,
        "config",
    )?;

    // Reject re-initialisation.
    if config_acc.owner == program_id && config_acc.data_len() > 0 {
        let existing: Config = deserialize(&config_acc.data.borrow())?;
        if existing.initialized {
            return Err(StakingError::AlreadyInitialized.into());
        }
    }

    // The vault MUST be the config PDA's associated token account and the
    // treasury MUST be the treasury wallet's associated token account, both for
    // the new mint. This pins their addresses deterministically.
    let expected_vault = get_associated_token_address(&config_key, mint_acc.key);
    require_address(
        vault_acc,
        &expected_vault,
        StakingError::InvalidVault,
        "vault",
    )?;
    let expected_treasury = get_associated_token_address(treasury_wallet.key, mint_acc.key);
    require_address(
        treasury_acc,
        &expected_treasury,
        StakingError::InvalidTreasury,
        "treasury",
    )?;

    // 1. Create the mint with the config PDA as mint authority (so the program
    //    can mint rewards later). spl-token 6 has no `create_mint` helper, so we
    //    allocate the account with the system program and initialise it with
    //    `initialize_mint2` (which needs no rent sysvar).
    let rent = &Rent::from_account_info(rent_acc)?;
    let mint_len = Mint::get_packed_len();
    let mint_rent = rent.minimum_balance(mint_len);
    invoke(
        &system_instruction::create_account(
            payer.key,
            mint_acc.key,
            mint_rent,
            mint_len as u64,
            token_program.key,
        ),
        &[payer.clone(), mint_acc.clone(), system_program_acc.clone()],
    )?;
    invoke(
        &spl_token::instruction::initialize_mint2(
            token_program.key,
            mint_acc.key,
            &config_key,
            None,
            decimals,
        )?,
        &[mint_acc.clone(), token_program.clone()],
    )?;

    // 2. Create the vault (owner = config PDA) and treasury (owner = wallet)
    //    associated token accounts, idempotently.
    let create_vault = create_associated_token_account_idempotent(
        payer.key,
        &config_key,
        mint_acc.key,
        token_program.key,
    );
    invoke(
        &create_vault,
        &[
            payer.clone(),
            vault_acc.clone(),
            config_acc.clone(),
            mint_acc.clone(),
            system_program_acc.clone(),
            token_program.clone(),
            assoc_program.clone(),
        ],
    )?;
    let create_treasury = create_associated_token_account_idempotent(
        payer.key,
        treasury_wallet.key,
        mint_acc.key,
        token_program.key,
    );
    invoke(
        &create_treasury,
        &[
            payer.clone(),
            treasury_acc.clone(),
            treasury_wallet.clone(),
            mint_acc.clone(),
            system_program_acc.clone(),
            token_program.clone(),
            assoc_program.clone(),
        ],
    )?;

    // 3. Allocate the config PDA and persist the config.
    let config = Config {
        initialized: true,
        admin: *payer.key,
        mint: *mint_acc.key,
        vault: *vault_acc.key,
        treasury: *treasury_acc.key,
        fee_bps,
        reward_rate_bps,
        min_stake,
        unstake_delay,
        decimals,
        config_bump,
        // The mint authority is the config PDA itself, signed with the same seeds.
        mint_bump: config_bump,
        // Freshly initialized: deposits open, no admin transfer in flight.
        paused: false,
        pending_admin: Pubkey::default(),
        timelock_secs,
        pending: PendingParams::default(),
        // Genesis distribution has not happened yet.
        genesis_done: false,
        // Immutable total-supply cap (never exposed via UpdateParams).
        max_supply,
    };
    let serialized = serialize(&config)?;
    let lamports = rent.minimum_balance(serialized.len());
    invoke_signed(
        &system_instruction::create_account(
            payer.key,
            &config_key,
            lamports,
            serialized.len() as u64,
            program_id,
        ),
        &[
            payer.clone(),
            config_acc.clone(),
            system_program_acc.clone(),
        ],
        &[&[CONFIG_SEED, &[config_bump]]],
    )?;
    config_acc.data.borrow_mut()[..serialized.len()].copy_from_slice(&serialized);

    msg!(
        "staking-suite initialized: fee {}bps, apy {}bps",
        fee_bps,
        reward_rate_bps
    );
    Ok(())
}

fn process_stake(program_id: &Pubkey, accounts: &[AccountInfo], amount: u64) -> ProgramResult {
    let ai = &mut accounts.iter();
    let staker = next_account_info(ai)?;
    let staker_token = next_account_info(ai)?;
    let vault = next_account_info(ai)?;
    let treasury = next_account_info(ai)?;
    let stake_acc = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;
    let token_program = next_account_info(ai)?;
    let system_program_acc = next_account_info(ai)?;
    let clock_acc = next_account_info(ai)?;

    require_signer(staker, "staker")?;
    require_address(
        token_program,
        &spl_token::id(),
        StakingError::InvalidTokenProgram,
        "token_program",
    )?;
    require_address(
        system_program_acc,
        &system_program::id(),
        StakingError::InvalidSystemProgram,
        "system_program",
    )?;

    // Config MUST be the validated program PDA.
    let config = load_config(program_id, config_acc)?;

    // Emergency stop: no new deposits while paused. (Withdrawals are never
    // gated, so this can't be used to freeze user funds.)
    if config.paused {
        return Err(StakingError::Paused.into());
    }

    // Vault / treasury MUST be the ones pinned in the config.
    require_address(vault, &config.vault, StakingError::InvalidVault, "vault")?;
    require_address(
        treasury,
        &config.treasury,
        StakingError::InvalidTreasury,
        "treasury",
    )?;

    // The staker's source token account MUST be theirs and of the config mint.
    require_staker_token(staker_token, staker.key, &config.mint)?;

    if amount < config.min_stake {
        return Err(StakingError::BelowMinimum.into());
    }

    let fee = compute_fee(amount, config.fee_bps);
    let net = amount.checked_sub(fee).ok_or(StakingError::Overflow)?;

    // Move the net principal to the vault and the fee to the treasury.
    invoke(
        &token_transfer(
            token_program.key,
            staker_token.key,
            vault.key,
            staker.key,
            &[],
            net,
        )?,
        &[
            staker_token.clone(),
            vault.clone(),
            staker.clone(),
            token_program.clone(),
        ],
    )?;
    if fee > 0 {
        invoke(
            &token_transfer(
                token_program.key,
                staker_token.key,
                treasury.key,
                staker.key,
                &[],
                fee,
            )?,
            &[
                staker_token.clone(),
                treasury.clone(),
                staker.clone(),
                token_program.clone(),
            ],
        )?;
    }

    let now = Clock::from_account_info(clock_acc)?.unix_timestamp;
    let (stake_key, bump) = stake_pda(program_id, staker.key);
    require_address(
        stake_acc,
        &stake_key,
        StakingError::InvalidStakeAccount,
        "stake",
    )?;

    // Load the existing stake account or allocate a fresh one. A non-empty
    // stake account MUST be owned by this program and belong to the staker.
    let mut sa = if stake_acc.data_len() > 0 {
        require_owner(
            stake_acc,
            program_id,
            StakingError::InvalidStakeAccount,
            "stake",
        )?;
        let loaded: StakeAccount = deserialize(&stake_acc.data.borrow())?;
        if loaded.owner != *staker.key {
            msg!("stake account owner is not the staker");
            return Err(StakingError::Unauthorized.into());
        }
        loaded
    } else {
        let template = StakeAccount {
            owner: *staker.key,
            amount: 0,
            staked_at: now,
            reward_from: now,
            pending_rewards: 0,
            bump,
        };
        let serialized = serialize(&template)?;
        let rent = Rent::get()?;
        let lamports = rent.minimum_balance(serialized.len());
        invoke_signed(
            &system_instruction::create_account(
                staker.key,
                &stake_key,
                lamports,
                serialized.len() as u64,
                program_id,
            ),
            &[
                staker.clone(),
                stake_acc.clone(),
                system_program_acc.clone(),
            ],
            &[&[STAKE_SEED, staker.key.as_ref(), &[bump]]],
        )?;
        template
    };

    // Fold any live accrual into pending before the principal changes.
    sa.settle(config.reward_rate_bps, now);
    if sa.amount == 0 {
        sa.staked_at = now;
    }
    sa.amount = sa.amount.checked_add(net).ok_or(StakingError::Overflow)?;

    let serialized = serialize(&sa)?;
    stake_acc.data.borrow_mut()[..serialized.len()].copy_from_slice(&serialized);
    msg!("staked {} (fee {})", net, fee);
    Ok(())
}

/// Shared unstake/claim path. `claim_only` mints rewards but keeps principal.
fn process_unstake(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    claim_only: bool,
) -> ProgramResult {
    let ai = &mut accounts.iter();
    let staker = next_account_info(ai)?;
    let staker_token = next_account_info(ai)?;
    let vault = next_account_info(ai)?;
    let mint = next_account_info(ai)?;
    let stake_acc = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;
    let token_program = next_account_info(ai)?;
    let clock_acc = next_account_info(ai)?;

    require_signer(staker, "staker")?;
    require_address(
        token_program,
        &spl_token::id(),
        StakingError::InvalidTokenProgram,
        "token_program",
    )?;

    // Config MUST be the validated program PDA.
    let config = load_config(program_id, config_acc)?;

    // Vault / mint MUST be the ones pinned in the config.
    require_address(vault, &config.vault, StakingError::InvalidVault, "vault")?;
    require_address(mint, &config.mint, StakingError::InvalidMint, "mint")?;

    // The destination token account MUST be the staker's and of the config mint,
    // so principal/rewards can only ever be sent to the staker.
    require_staker_token(staker_token, staker.key, &config.mint)?;

    // The stake account MUST be the staker's program-owned PDA.
    let (stake_key, _bump) = stake_pda(program_id, staker.key);
    require_address(
        stake_acc,
        &stake_key,
        StakingError::InvalidStakeAccount,
        "stake",
    )?;
    require_owner(
        stake_acc,
        program_id,
        StakingError::InvalidStakeAccount,
        "stake",
    )?;
    if stake_acc.data_len() == 0 {
        msg!("stake account is not allocated");
        return Err(StakingError::InvalidStakeAccount.into());
    }
    let mut sa: StakeAccount = deserialize(&stake_acc.data.borrow())?;
    if sa.owner != *staker.key {
        return Err(StakingError::Unauthorized.into());
    }

    let now = Clock::from_account_info(clock_acc)?.unix_timestamp;
    let config_key = config_pda(program_id).0;

    if !claim_only {
        if sa.amount == 0 {
            return Err(StakingError::InsufficientStake.into());
        }
        if now.saturating_sub(sa.staked_at) < config.unstake_delay {
            return Err(StakingError::CooldownActive.into());
        }
        // Return the principal from the vault (owned by the config PDA).
        invoke_signed(
            &token_transfer(
                token_program.key,
                vault.key,
                staker_token.key,
                &config_key,
                &[],
                sa.amount,
            )?,
            &[
                vault.clone(),
                staker_token.clone(),
                config_acc.clone(),
                token_program.clone(),
            ],
            &[&[CONFIG_SEED, &[config.config_bump]]],
        )?;
    }

    // Mint accrued rewards to the staker, bounded by the immutable
    // max-supply cap. Withdrawals/claims must NEVER fail (user funds are
    // never frozen), so when the cap is reached the reward is CLAMPED to the
    // remaining headroom instead of reverting the transaction; the shortfall
    // is forfeited and logged on-chain.
    let rewards = sa.accrued_rewards(config.reward_rate_bps, now);
    let mintable = if rewards > 0 {
        require_owner(mint, &spl_token::id(), StakingError::InvalidMint, "mint")?;
        let mint_state = {
            let data = mint.data.borrow();
            Mint::unpack(&data).map_err(|_| StakingError::InvalidMint)?
        };
        let headroom = supply_headroom(mint_state.supply, config.max_supply);
        let mintable = rewards.min(headroom);
        if mintable < rewards {
            msg!(
                "reward clamped to max-supply headroom: accrued {} mintable {} (supply {}, cap {})",
                rewards,
                mintable,
                mint_state.supply,
                config.max_supply
            );
        }
        mintable
    } else {
        0
    };
    if mintable > 0 {
        invoke_signed(
            &mint_to(
                token_program.key,
                mint.key,
                staker_token.key,
                &config_key,
                &[],
                mintable,
            )?,
            &[
                mint.clone(),
                staker_token.clone(),
                config_acc.clone(),
                token_program.clone(),
            ],
            &[&[CONFIG_SEED, &[config.config_bump]]],
        )?;
    }

    if claim_only {
        sa.pending_rewards = 0;
        sa.reward_from = now;
    } else {
        sa.amount = 0;
        sa.pending_rewards = 0;
        sa.staked_at = 0;
        sa.reward_from = now;
    }

    let serialized = serialize(&sa)?;
    stake_acc.data.borrow_mut()[..serialized.len()].copy_from_slice(&serialized);
    msg!(
        "{} rewards {} (accrued {})",
        if claim_only { "claimed" } else { "unstaked" },
        mintable,
        rewards
    );
    Ok(())
}

/// Admin: QUEUE a parameter update. Nothing changes immediately — the new
/// values sit in `config.pending` for the whole timelock window, published
/// on-chain, before anyone can apply them. Withdrawals are never gated, so
/// users who dislike a queued change can exit before it takes effect.
#[allow(clippy::too_many_arguments)]
fn process_queue_update(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    fee_bps: Option<u16>,
    reward_rate_bps: Option<u64>,
    min_stake: Option<u64>,
    unstake_delay: Option<i64>,
    timelock_secs: Option<i64>,
) -> ProgramResult {
    let ai = &mut accounts.iter();
    let admin = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;
    let clock_acc = next_account_info(ai)?;

    require_signer(admin, "admin")?;

    // Config MUST be the validated program PDA.
    let mut config = load_config(program_id, config_acc)?;

    // Only the recorded admin may queue an update.
    if config.admin != *admin.key {
        return Err(StakingError::Unauthorized.into());
    }
    if config.pending.active {
        msg!("an update is already queued; cancel it first");
        return Err(StakingError::UpdateAlreadyQueued.into());
    }

    // Resolve `None` fields against the live config and validate the result up
    // front, so a doomed update cannot be queued at all.
    let new_fee = fee_bps.unwrap_or(config.fee_bps);
    let new_rate = reward_rate_bps.unwrap_or(config.reward_rate_bps);
    let new_min = min_stake.unwrap_or(config.min_stake);
    let new_delay = unstake_delay.unwrap_or(config.unstake_delay);
    let new_timelock = timelock_secs.unwrap_or(config.timelock_secs);
    validate_params(new_fee, new_rate)?;
    validate_timelock(new_timelock)?;

    let now = Clock::from_account_info(clock_acc)?.unix_timestamp;
    config.pending = PendingParams {
        active: true,
        queued_at: now,
        fee_bps: new_fee,
        reward_rate_bps: new_rate,
        min_stake: new_min,
        unstake_delay: new_delay,
        timelock_secs: new_timelock,
    };
    save_config(config_acc, &config)?;
    msg!(
        "update queued at {} (applies after {}s): fee {}bps, apy {}bps, min {}, delay {}, timelock {}s",
        now,
        config.timelock_secs,
        new_fee,
        new_rate,
        new_min,
        new_delay,
        new_timelock
    );
    Ok(())
}

/// Apply the queued update once the timelock has elapsed. Permissionless:
/// keeping it open means a queued change cannot be held back (griefed) by an
/// unresponsive admin, and applying only ever copies already-validated values.
fn process_apply_update(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let ai = &mut accounts.iter();
    let config_acc = next_account_info(ai)?;
    let clock_acc = next_account_info(ai)?;

    let mut config = load_config(program_id, config_acc)?;
    if !config.pending.active {
        return Err(StakingError::NoPendingUpdate.into());
    }

    let now = Clock::from_account_info(clock_acc)?.unix_timestamp;
    let eligible_at = config
        .pending
        .queued_at
        .saturating_add(config.timelock_secs);
    if now < eligible_at {
        msg!("timelock active: {} < {}", now, eligible_at);
        return Err(StakingError::TimelockNotElapsed.into());
    }

    // Re-validate at apply time (defence in depth: the queued values were
    // already checked, but caps are cheap to enforce twice).
    let pending = config.pending.clone();
    validate_params(pending.fee_bps, pending.reward_rate_bps)?;
    validate_timelock(pending.timelock_secs)?;

    config.fee_bps = pending.fee_bps;
    config.reward_rate_bps = pending.reward_rate_bps;
    config.min_stake = pending.min_stake;
    config.unstake_delay = pending.unstake_delay;
    // A change to the delay itself only takes effect now — i.e. it waited out
    // the OLD delay (same rule as OpenZeppelin's TimelockController).
    config.timelock_secs = pending.timelock_secs;
    config.pending = PendingParams::default();

    save_config(config_acc, &config)?;
    msg!(
        "update applied: fee {}bps, apy {}bps, min {}, delay {}, timelock {}s",
        config.fee_bps,
        config.reward_rate_bps,
        config.min_stake,
        config.unstake_delay,
        config.timelock_secs
    );
    Ok(())
}

/// Admin: cancel the queued update (e.g. it is no longer wanted).
fn process_cancel_update(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let ai = &mut accounts.iter();
    let admin = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;

    require_signer(admin, "admin")?;
    let mut config = load_config(program_id, config_acc)?;
    if config.admin != *admin.key {
        return Err(StakingError::Unauthorized.into());
    }
    if !config.pending.active {
        return Err(StakingError::NoPendingUpdate.into());
    }
    config.pending = PendingParams::default();
    save_config(config_acc, &config)?;
    msg!("queued update cancelled");
    Ok(())
}

/// Admin: toggle the emergency `paused` flag (halts new deposits; withdrawals
/// are never gated).
fn process_set_paused(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    paused: bool,
) -> ProgramResult {
    let ai = &mut accounts.iter();
    let admin = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;

    require_signer(admin, "admin")?;
    let mut config = load_config(program_id, config_acc)?;
    if config.admin != *admin.key {
        return Err(StakingError::Unauthorized.into());
    }
    config.paused = paused;
    save_config(config_acc, &config)?;
    msg!(
        "staking deposits {}",
        if paused { "paused" } else { "resumed" }
    );
    Ok(())
}

/// Admin: propose `new_admin` (step 1 of the two-step transfer). The change
/// only takes effect once the proposed key signs `accept_admin`.
fn process_transfer_admin(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    new_admin: Pubkey,
) -> ProgramResult {
    let ai = &mut accounts.iter();
    let admin = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;

    require_signer(admin, "admin")?;
    let mut config = load_config(program_id, config_acc)?;
    if config.admin != *admin.key {
        return Err(StakingError::Unauthorized.into());
    }
    if new_admin == Pubkey::default() {
        msg!("refusing to transfer admin to the zero pubkey");
        return Err(StakingError::InvalidAccount.into());
    }
    config.pending_admin = new_admin;
    save_config(config_acc, &config)?;
    msg!("pending admin proposed: {}", new_admin);
    Ok(())
}

/// Admin: one-time genesis mint of the initial supply.
///
/// This is the ONLY sanctioned initial-distribution path. It is latched by
/// `Config::genesis_done`: the first successful call flips the flag and every
/// later call fails with `GenesisAlreadyDone`, so the admin cannot silently
/// inflate supply after launch. Reward minting (`unstake`/`claim`) is separate
/// and bounded by the accrued-reward math.
///
/// Accounts:
/// 0. `[signer]` Admin.
/// 1. `[writable]` Config PDA.
/// 2. `[writable]` Mint (must equal `Config::mint`).
/// 3. `[writable]` Recipient token account (of the mint).
/// 4. `[]` Token program.
fn process_genesis_mint(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    amount: u64,
) -> ProgramResult {
    let ai = &mut accounts.iter();
    let admin = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;
    let mint_acc = next_account_info(ai)?;
    let recipient_acc = next_account_info(ai)?;
    let token_program = next_account_info(ai)?;

    require_signer(admin, "admin")?;
    let mut config = load_config(program_id, config_acc)?;
    if config.admin != *admin.key {
        return Err(StakingError::Unauthorized.into());
    }
    if token_program.key != &spl_token::id() {
        return Err(StakingError::InvalidTokenProgram.into());
    }
    if mint_acc.key != &config.mint {
        return Err(StakingError::InvalidMint.into());
    }
    if !recipient_acc.is_writable {
        return Err(StakingError::InvalidStakerToken.into());
    }
    if amount == 0 {
        return Err(StakingError::InvalidAmount.into());
    }
    if config.genesis_done {
        msg!("genesis mint rejected: already performed for this deployment");
        return Err(StakingError::GenesisAlreadyDone.into());
    }

    // Max-supply enforcement. The LIVE mint account is the authoritative
    // total (the config PDA is the only mint authority, so this reading
    // cannot be inflated behind our back); the cap itself is immutable — it
    // is not part of UpdateParams, so no admin action can raise it.
    require_owner(
        mint_acc,
        &spl_token::id(),
        StakingError::InvalidMint,
        "mint",
    )?;
    let mint_state = {
        let data = mint_acc.data.borrow();
        Mint::unpack(&data).map_err(|_| StakingError::InvalidMint)?
    };
    if !fits_under_cap(mint_state.supply, amount, config.max_supply) {
        msg!(
            "genesis mint {} rejected: supply {} + amount exceeds max_supply {}",
            amount,
            mint_state.supply,
            config.max_supply
        );
        return Err(StakingError::MaxSupplyExceeded.into());
    }

    let config_key = config_pda(program_id).0;
    invoke_signed(
        &mint_to(
            token_program.key,
            mint_acc.key,
            recipient_acc.key,
            &config_key,
            &[],
            amount,
        )?,
        &[
            mint_acc.clone(),
            recipient_acc.clone(),
            config_acc.clone(),
            token_program.clone(),
        ],
        &[&[CONFIG_SEED, &[config.config_bump]]],
    )?;

    config.genesis_done = true;
    save_config(config_acc, &config)?;
    msg!("genesis mint complete: {amount} raw units");
    Ok(())
}

/// Discriminant of mpl-token-metadata's `CreateMetadataAccountsV3`
/// instruction. Verified against the enum order in the mpl-token-metadata
/// source deployed to mainnet-beta (tag `token-metadata@v1.14.0`,
/// `programs/token-metadata/program/src/instruction/mod.rs`: variant index 33;
/// index 19 is `Utilize`) and re-verified by executing the CPI against the
/// REAL mainnet-cloned mpl program in `validator_e2e_max_supply_cap_and_metadata`.
const MPL_CREATE_METADATA_ACCOUNTS_V3: u8 = 33;

/// Hand-rolled borsh encoding of
/// `CreateMetadataAccountsV3 { data: DataV2 { name, symbol, uri,
/// seller_fee_basis_points: 0, creators: None, collection: None, uses: None },
/// is_mutable: false, collection_details: None }`.
///
/// Borsh encodes `String` as a u32-LE byte length followed by UTF-8 bytes and
/// `Option::None` as a single `0` byte. Kept explicit (no mpl dependency in
/// the BPF object) and pinned by a byte-layout unit test.
fn create_metadata_accounts_v3_data(name: &str, symbol: &str, uri: &str) -> Vec<u8> {
    let mut d = Vec::with_capacity(1 + (4 + name.len()) + (4 + symbol.len()) + (4 + uri.len()) + 7);
    d.push(MPL_CREATE_METADATA_ACCOUNTS_V3);
    for s in [name, symbol, uri] {
        d.extend_from_slice(&(s.len() as u32).to_le_bytes());
        d.extend_from_slice(s.as_bytes());
    }
    d.extend_from_slice(&0u16.to_le_bytes()); // seller_fee_basis_points
    d.push(0); // creators: None
    d.push(0); // collection: None
    d.push(0); // uses: None
    d.push(0); // is_mutable: false — the metadata is permanent
    d.push(0); // collection_details: None
    d
}

/// Admin: create the token's metadata account (one-shot CPI to
/// mpl-token-metadata). See `StakingInstruction::CreateTokenMetadata` for the
/// account list and the authority/immutability model.
///
/// Validation order (every failure happens BEFORE the CPI):
/// 1. admin signature + config admin match;
/// 2. mint == config.mint, metadata program == canonical mpl id, system
///    program canonical;
/// 3. field byte-length limits (mpl would reject late otherwise);
/// 4. metadata account == the canonical mpl PDA for this mint;
/// 5. one-shot: the PDA must not already exist (no lamports, no data).
fn process_create_token_metadata(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    name: String,
    symbol: String,
    uri: String,
) -> ProgramResult {
    let ai = &mut accounts.iter();
    let admin = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;
    let mint_acc = next_account_info(ai)?;
    let metadata_acc = next_account_info(ai)?;
    let metadata_program = next_account_info(ai)?;
    let system_program_acc = next_account_info(ai)?;
    let rent_acc = next_account_info(ai)?;

    require_signer(admin, "admin")?;
    let config = load_config(program_id, config_acc)?;
    if config.admin != *admin.key {
        return Err(StakingError::Unauthorized.into());
    }
    require_address(mint_acc, &config.mint, StakingError::InvalidMint, "mint")?;
    require_address(
        metadata_program,
        &TOKEN_METADATA_PROGRAM_ID,
        StakingError::InvalidMetadataProgram,
        "metadata_program",
    )?;
    require_address(
        system_program_acc,
        &system_program::id(),
        StakingError::InvalidSystemProgram,
        "system_program",
    )?;
    validate_metadata_fields(&name, &symbol, &uri).map_err(ProgramError::from)?;

    let (expected_metadata, _bump) = metadata_pda(mint_acc.key);
    require_address(
        metadata_acc,
        &expected_metadata,
        StakingError::InvalidAccount,
        "metadata",
    )?;
    if !metadata_acc.is_writable {
        msg!("metadata account must be writable");
        return Err(StakingError::InvalidAccount.into());
    }
    // One-shot: refuse when the account already exists (funded or with data).
    if metadata_acc.lamports() > 0 || metadata_acc.data_len() > 0 {
        msg!("token metadata already exists — instruction is one-shot");
        return Err(StakingError::MetadataAlreadyExists.into());
    }

    // CPI: mint authority AND update authority are the config PDA (it signs
    // via this program's PDA seeds); the admin pays the rent. is_mutable is
    // false, so nobody — including a future compromised admin — can rewrite
    // the metadata through mpl either.
    let config_key = config_pda(program_id).0;
    let ix = solana_program::instruction::Instruction {
        program_id: TOKEN_METADATA_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*metadata_acc.key, false),
            AccountMeta::new_readonly(*mint_acc.key, false),
            AccountMeta::new_readonly(config_key, true),
            AccountMeta::new(*admin.key, true),
            AccountMeta::new_readonly(config_key, false),
            AccountMeta::new_readonly(*system_program_acc.key, false),
            AccountMeta::new_readonly(*rent_acc.key, false),
        ],
        data: create_metadata_accounts_v3_data(&name, &symbol, &uri),
    };
    invoke_signed(
        &ix,
        &[
            metadata_acc.clone(),
            mint_acc.clone(),
            config_acc.clone(),
            admin.clone(),
            system_program_acc.clone(),
            rent_acc.clone(),
        ],
        &[&[CONFIG_SEED, &[config.config_bump]]],
    )?;
    msg!("token metadata created: {} ({}) uri {}", name, symbol, uri);
    Ok(())
}

/// Pending admin: accept the transfer, becoming the admin (step 2). Clears the
/// pending slot so the transfer cannot be replayed.
fn process_accept_admin(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let ai = &mut accounts.iter();
    let pending = next_account_info(ai)?;
    let config_acc = next_account_info(ai)?;

    require_signer(pending, "pending_admin")?;
    let mut config = load_config(program_id, config_acc)?;
    if config.pending_admin == Pubkey::default() || config.pending_admin != *pending.key {
        return Err(StakingError::NotPendingAdmin.into());
    }
    config.admin = config.pending_admin;
    config.pending_admin = Pubkey::default();
    save_config(config_acc, &config)?;
    msg!("admin transferred to {}", config.admin);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::program_option::COption;
    use spl_token::state::AccountState;

    /// Build an `AccountInfo` borrowing caller-owned buffers. The `key`,
    /// `owner`, `lamports` and `data` bindings MUST outlive the returned value.
    fn acct<'a>(
        key: &'a Pubkey,
        owner: &'a Pubkey,
        lamports: &'a mut u64,
        data: &'a mut [u8],
        is_signer: bool,
        is_writable: bool,
    ) -> AccountInfo<'a> {
        AccountInfo::new(key, is_signer, is_writable, lamports, data, owner, false, 0)
    }

    fn sample_config(bump: u8) -> Config {
        Config {
            initialized: true,
            admin: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            vault: Pubkey::new_unique(),
            treasury: Pubkey::new_unique(),
            fee_bps: 100,
            reward_rate_bps: 1_000,
            min_stake: 1,
            unstake_delay: 0,
            decimals: 9,
            config_bump: bump,
            mint_bump: bump,
            paused: false,
            pending_admin: Pubkey::default(),
            timelock_secs: 3_600,
            pending: PendingParams::default(),
            genesis_done: false,
            max_supply: 1_000_000_000_000,
        }
    }

    // ------------------------------------------------------- signer / address --

    #[test]
    fn require_signer_enforces_signature() {
        let key = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let mut lp1 = 0u64;
        let mut d1 = [0u8; 0];
        let non_signer = acct(&key, &owner, &mut lp1, &mut d1, false, false);
        assert_eq!(
            require_signer(&non_signer, "staker"),
            Err(ProgramError::MissingRequiredSignature)
        );

        let mut lp2 = 0u64;
        let mut d2 = [0u8; 0];
        let signer = acct(&key, &owner, &mut lp2, &mut d2, true, false);
        assert!(require_signer(&signer, "staker").is_ok());
    }

    #[test]
    fn require_address_matches_key() {
        let expected = Pubkey::new_unique();
        let impostor = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let mut lp1 = 0u64;
        let mut d1 = [0u8; 0];
        let good = acct(&expected, &owner, &mut lp1, &mut d1, false, false);
        assert!(require_address(&good, &expected, StakingError::InvalidVault, "vault").is_ok());

        let mut lp2 = 0u64;
        let mut d2 = [0u8; 0];
        let bad = acct(&impostor, &owner, &mut lp2, &mut d2, false, false);
        assert_eq!(
            require_address(&bad, &expected, StakingError::InvalidVault, "vault"),
            Err(StakingError::InvalidVault.into())
        );
    }

    #[test]
    fn require_owner_matches() {
        let key = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();

        let mut lp1 = 0u64;
        let mut d1 = [0u8; 0];
        let good = acct(&key, &program, &mut lp1, &mut d1, false, false);
        assert!(require_owner(
            &good,
            &program,
            StakingError::InvalidConfigAccount,
            "config"
        )
        .is_ok());

        let mut lp2 = 0u64;
        let mut d2 = [0u8; 0];
        let bad = acct(&key, &attacker, &mut lp2, &mut d2, false, false);
        assert_eq!(
            require_owner(&bad, &program, StakingError::InvalidConfigAccount, "config"),
            Err(StakingError::InvalidConfigAccount.into())
        );
    }

    // ------------------------------------------------------------ load_config --

    #[test]
    fn load_config_accepts_the_real_pda() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut lp = 1u64;
        let info = acct(&config_key, &program_id, &mut lp, &mut data, false, true);
        let loaded = load_config(&program_id, &info).expect("valid config must load");
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn load_config_rejects_wrong_address() {
        // Attacker passes a random account (correct owner + valid bytes) that is
        // NOT the program's config PDA.
        let program_id = crate::id();
        let bump = config_pda(&program_id).1;
        let cfg = sample_config(bump);
        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut lp = 1u64;
        let impostor = Pubkey::new_unique();
        let info = acct(&impostor, &program_id, &mut lp, &mut data, false, true);
        assert_eq!(
            load_config(&program_id, &info),
            Err(StakingError::InvalidConfigAccount.into())
        );
    }

    #[test]
    fn load_config_rejects_wrong_owner() {
        // Correct PDA address, but the account is owned by something else.
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut lp = 1u64;
        let attacker = Pubkey::new_unique();
        let info = acct(&config_key, &attacker, &mut lp, &mut data, false, true);
        assert_eq!(
            load_config(&program_id, &info),
            Err(StakingError::InvalidConfigAccount.into())
        );
    }

    #[test]
    fn load_config_rejects_unallocated() {
        let program_id = crate::id();
        let (config_key, _bump) = config_pda(&program_id);
        let mut data: [u8; 0] = [];
        let mut lp = 0u64;
        let info = acct(&config_key, &program_id, &mut lp, &mut data, false, true);
        assert_eq!(
            load_config(&program_id, &info),
            Err(StakingError::InvalidConfigAccount.into())
        );
    }

    #[test]
    fn load_config_rejects_uninitialized_flag() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let mut cfg = sample_config(bump);
        cfg.initialized = false;
        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut lp = 1u64;
        let info = acct(&config_key, &program_id, &mut lp, &mut data, false, true);
        assert_eq!(
            load_config(&program_id, &info),
            Err(StakingError::InvalidConfigAccount.into())
        );
    }

    // ------------------------------------------------------ require_staker_token --

    #[test]
    fn require_staker_token_validates_mint_and_owner() {
        let staker = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let token_program = spl_token::id();
        let key = Pubkey::new_unique();

        let ta = TokenAccount {
            mint,
            owner: staker,
            amount: 0,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut data = vec![0u8; TokenAccount::LEN];
        Pack::pack(ta, &mut data).unwrap();
        let mut lp = 1u64;
        let info = acct(&key, &token_program, &mut lp, &mut data, false, true);

        assert!(require_staker_token(&info, &staker, &mint).is_ok());

        let other_mint = Pubkey::new_unique();
        assert_eq!(
            require_staker_token(&info, &staker, &other_mint),
            Err(StakingError::InvalidStakerToken.into())
        );

        let other_staker = Pubkey::new_unique();
        assert_eq!(
            require_staker_token(&info, &other_staker, &mint),
            Err(StakingError::InvalidStakerToken.into())
        );
    }

    #[test]
    fn require_staker_token_rejects_wrong_program_owner() {
        let staker = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let key = Pubkey::new_unique();
        let not_the_token_program = Pubkey::new_unique();

        let ta = TokenAccount {
            mint,
            owner: staker,
            amount: 0,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut data = vec![0u8; TokenAccount::LEN];
        Pack::pack(ta, &mut data).unwrap();
        let mut lp = 1u64;
        let info = acct(
            &key,
            &not_the_token_program,
            &mut lp,
            &mut data,
            false,
            true,
        );
        assert_eq!(
            require_staker_token(&info, &staker, &mint),
            Err(StakingError::InvalidStakerToken.into())
        );
    }

    // ------------------------------------------------------- parameter caps --

    #[test]
    fn validate_params_enforces_caps() {
        assert!(validate_params(0, 0).is_ok());
        assert!(validate_params(MAX_FEE_BPS, MAX_REWARD_RATE_BPS).is_ok());
        assert_eq!(
            validate_params(MAX_FEE_BPS + 1, 0),
            Err(StakingError::FeeTooHigh.into())
        );
        assert_eq!(
            validate_params(0, MAX_REWARD_RATE_BPS + 1),
            Err(StakingError::RewardRateTooHigh.into())
        );
    }

    // ---------------------------------------------------------------- pause --

    #[test]
    fn pause_and_unpause_toggle_the_flag() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let owner = Pubkey::new_unique();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);

        process_set_paused(
            &program_id,
            &[admin_info.clone(), config_info.clone()],
            true,
        )
        .unwrap();
        assert!(
            deserialize::<Config>(&config_info.data.borrow())
                .unwrap()
                .paused
        );

        process_set_paused(
            &program_id,
            &[admin_info.clone(), config_info.clone()],
            false,
        )
        .unwrap();
        assert!(
            !deserialize::<Config>(&config_info.data.borrow())
                .unwrap()
                .paused
        );
    }

    #[test]
    fn pause_rejects_a_non_admin() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut x_lp = 0u64;
        let mut x_d = [0u8; 0];
        let attacker = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let attacker_info = acct(&attacker, &owner, &mut x_lp, &mut x_d, true, false);

        let res = process_set_paused(
            &program_id,
            &[attacker_info.clone(), config_info.clone()],
            true,
        );
        assert_eq!(res, Err(StakingError::Unauthorized.into()));
        // Flag untouched.
        assert!(
            !deserialize::<Config>(&config_info.data.borrow())
                .unwrap()
                .paused
        );
    }

    // -------------------------------------------------------- admin transfer --

    #[test]
    fn two_step_admin_transfer_completes() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;
        let new_admin_key = Pubkey::new_unique();

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut n_lp = 0u64;
        let mut n_d = [0u8; 0];
        let owner = Pubkey::new_unique();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);
        let new_admin_info = acct(&new_admin_key, &owner, &mut n_lp, &mut n_d, true, false);

        // Step 1: the current admin proposes the new key.
        process_transfer_admin(
            &program_id,
            &[admin_info.clone(), config_info.clone()],
            new_admin_key,
        )
        .unwrap();
        let mid = deserialize::<Config>(&config_info.data.borrow()).unwrap();
        assert_eq!(mid.pending_admin, new_admin_key);
        assert_eq!(mid.admin, admin_key, "admin unchanged until accepted");

        // The old admin (not the pending key) cannot accept.
        let res = process_accept_admin(&program_id, &[admin_info.clone(), config_info.clone()]);
        assert_eq!(res, Err(StakingError::NotPendingAdmin.into()));

        // Step 2: the proposed key accepts and is promoted.
        process_accept_admin(&program_id, &[new_admin_info.clone(), config_info.clone()]).unwrap();
        let after = deserialize::<Config>(&config_info.data.borrow()).unwrap();
        assert_eq!(after.admin, new_admin_key);
        assert_eq!(after.pending_admin, Pubkey::default(), "pending cleared");
    }

    #[test]
    fn transfer_admin_rejects_a_non_admin() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut x_lp = 0u64;
        let mut x_d = [0u8; 0];
        let attacker = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let attacker_info = acct(&attacker, &owner, &mut x_lp, &mut x_d, true, false);

        let res = process_transfer_admin(
            &program_id,
            &[attacker_info.clone(), config_info.clone()],
            Pubkey::new_unique(),
        );
        assert_eq!(res, Err(StakingError::Unauthorized.into()));
    }

    #[test]
    fn transfer_admin_rejects_the_zero_key() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let owner = Pubkey::new_unique();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);

        let res = process_transfer_admin(
            &program_id,
            &[admin_info.clone(), config_info.clone()],
            Pubkey::default(),
        );
        assert_eq!(res, Err(StakingError::InvalidAccount.into()));
    }

    #[test]
    fn accept_admin_without_a_pending_transfer_fails() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let owner = Pubkey::new_unique();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);

        // No transfer proposed, so pending_admin is the zero key -> reject.
        let res = process_accept_admin(&program_id, &[admin_info.clone(), config_info.clone()]);
        assert_eq!(res, Err(StakingError::NotPendingAdmin.into()));
    }

    // ------------------------------------------------------------ timelock --

    /// Build a Clock sysvar account whose `unix_timestamp` is `ts`. Sysvar data
    /// is bincode-encoded, which `Clock::from_account_info` decodes.
    fn clock_acct<'a>(
        key: &'a Pubkey,
        owner: &'a Pubkey,
        lamports: &'a mut u64,
        data: &'a mut Vec<u8>,
        ts: i64,
    ) -> AccountInfo<'a> {
        let clock = Clock {
            unix_timestamp: ts,
            ..Clock::default()
        };
        *data = bincode::serialize(&clock).unwrap();
        acct(key, owner, lamports, data, false, false)
    }

    fn read_config(info: &AccountInfo) -> Config {
        deserialize::<Config>(&info.data.borrow()).unwrap()
    }

    // ------------------------------------------------------------- genesis --

    /// Five-account fixture for genesis validation tests. The mint_to CPI is
    /// never reached in these cases (all fail earlier), so token-program-owned
    /// accounts can be empty shells; only keys/flags matter.
    // Test fixture: one borrowed buffer per mocked AccountInfo, passed
    // flat so every lifetime stays visible to the borrow checker.
    #[allow(clippy::too_many_arguments)]
    fn genesis_accounts<'a>(
        program_id: &'a Pubkey,
        config_key: &'a Pubkey,
        token_id: &'a Pubkey,
        cfg: &Config,
        signer: &'a Pubkey,
        mint: &'a Pubkey,
        recipient: &'a Pubkey,
        buf: &'a mut Vec<u8>,
        lps: &'a mut [u64; 5],
        ds: &'a mut [[u8; 0]; 4],
        owner: &'a Pubkey,
    ) -> Vec<AccountInfo<'a>> {
        *buf = borsh::to_vec(cfg).unwrap();
        // Destructure so each AccountInfo gets a disjoint &mut binding
        // (indexing through one &mut array reference would alias-reborrow).
        let [cfg_lp, signer_lp, mint_lp, recip_lp, token_lp] = lps;
        let [signer_d, mint_d, recip_d, token_d] = ds;
        *cfg_lp = 1;
        vec![
            // is_signer=true even for the impostor: the test asserts the
            // admin CHECK rejects, not the missing-signature check.
            acct(signer, owner, signer_lp, signer_d, true, false),
            acct(config_key, program_id, cfg_lp, buf, false, true),
            acct(mint, token_id, mint_lp, mint_d, false, true),
            acct(recipient, token_id, recip_lp, recip_d, false, true),
            acct(token_id, owner, token_lp, token_d, false, false),
        ]
    }

    fn genesis_cfg(bump: u8) -> Config {
        let mut cfg = sample_config(bump);
        cfg.mint = Pubkey::new_unique();
        cfg
    }

    #[test]
    fn genesis_rejects_non_admin() {
        let program_id = crate::id();
        let (_, bump) = config_pda(&program_id);
        let cfg = genesis_cfg(bump);
        let impostor = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let mut buf = Vec::new();
        let mut lps = [0u64; 5];
        let mut ds = [[0u8; 0]; 4];
        let mint = cfg.mint;
        let (config_key, _) = config_pda(&program_id);
        let token_id = spl_token::id();
        let accs = genesis_accounts(
            &program_id,
            &config_key,
            &token_id,
            &cfg,
            &impostor,
            &mint,
            &recipient,
            &mut buf,
            &mut lps,
            &mut ds,
            &owner,
        );
        let res = process_genesis_mint(&program_id, &accs, 1_000);
        assert_eq!(res, Err(StakingError::Unauthorized.into()));
        assert!(!read_config(&accs[1]).genesis_done, "latch untouched");
    }

    #[test]
    fn genesis_rejects_zero_amount() {
        let program_id = crate::id();
        let (_, bump) = config_pda(&program_id);
        let cfg = genesis_cfg(bump);
        let recipient = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let mut buf = Vec::new();
        let mut lps = [0u64; 5];
        let mut ds = [[0u8; 0]; 4];
        let admin = cfg.admin;
        let mint = cfg.mint;
        let (config_key, _) = config_pda(&program_id);
        let token_id = spl_token::id();
        let accs = genesis_accounts(
            &program_id,
            &config_key,
            &token_id,
            &cfg,
            &admin,
            &mint,
            &recipient,
            &mut buf,
            &mut lps,
            &mut ds,
            &owner,
        );
        let res = process_genesis_mint(&program_id, &accs, 0);
        assert_eq!(res, Err(StakingError::InvalidAmount.into()));
        assert!(!read_config(&accs[1]).genesis_done);
    }

    #[test]
    fn genesis_rejects_wrong_mint() {
        let program_id = crate::id();
        let (_, bump) = config_pda(&program_id);
        let cfg = genesis_cfg(bump);
        let recipient = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let wrong_mint = Pubkey::new_unique();
        let mut buf = Vec::new();
        let mut lps = [0u64; 5];
        let mut ds = [[0u8; 0]; 4];
        let admin = cfg.admin;
        let (config_key, _) = config_pda(&program_id);
        let token_id = spl_token::id();
        let accs = genesis_accounts(
            &program_id,
            &config_key,
            &token_id,
            &cfg,
            &admin,
            &wrong_mint,
            &recipient,
            &mut buf,
            &mut lps,
            &mut ds,
            &owner,
        );
        let res = process_genesis_mint(&program_id, &accs, 1_000);
        assert_eq!(res, Err(StakingError::InvalidMint.into()));
    }

    #[test]
    fn genesis_rejects_second_attempt_via_latch() {
        let program_id = crate::id();
        let (_, bump) = config_pda(&program_id);
        let mut cfg = genesis_cfg(bump);
        cfg.genesis_done = true; // first mint already happened
        let recipient = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let mut buf = Vec::new();
        let mut lps = [0u64; 5];
        let mut ds = [[0u8; 0]; 4];
        let admin = cfg.admin;
        let mint = cfg.mint;
        let (config_key, _) = config_pda(&program_id);
        let token_id = spl_token::id();
        let accs = genesis_accounts(
            &program_id,
            &config_key,
            &token_id,
            &cfg,
            &admin,
            &mint,
            &recipient,
            &mut buf,
            &mut lps,
            &mut ds,
            &owner,
        );
        let res = process_genesis_mint(&program_id, &accs, 1_000);
        assert_eq!(res, Err(StakingError::GenesisAlreadyDone.into()));
    }

    #[test]
    fn validate_timelock_enforces_the_range() {
        assert!(validate_timelock(0).is_ok());
        assert!(validate_timelock(MAX_TIMELOCK_SECS).is_ok());
        assert_eq!(
            validate_timelock(-1),
            Err(StakingError::TimelockOutOfRange.into())
        );
        assert_eq!(
            validate_timelock(MAX_TIMELOCK_SECS + 1),
            Err(StakingError::TimelockOutOfRange.into())
        );
    }

    #[test]
    fn queue_update_resolves_nones_and_changes_nothing_yet() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut c_lp = 1u64;
        let mut c_data = Vec::new();
        let owner = Pubkey::new_unique();
        let sysvar_owner = solana_program::sysvar::id();
        let clock_key = solana_program::sysvar::clock::id();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);
        let clock_info = clock_acct(&clock_key, &sysvar_owner, &mut c_lp, &mut c_data, 1_000_000);

        process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            Some(200),
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let after = read_config(&config_info);
        // Live values are untouched until apply.
        assert_eq!(after.fee_bps, cfg.fee_bps);
        assert_eq!(after.reward_rate_bps, cfg.reward_rate_bps);
        // The pending update is published with resolved values.
        assert!(after.pending.active);
        assert_eq!(after.pending.queued_at, 1_000_000);
        assert_eq!(after.pending.fee_bps, 200);
        assert_eq!(after.pending.reward_rate_bps, cfg.reward_rate_bps);
        assert_eq!(after.pending.timelock_secs, cfg.timelock_secs);
    }

    #[test]
    fn queue_update_rejects_non_admin_and_double_queue_and_bad_values() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut x_lp = 0u64;
        let mut x_d = [0u8; 0];
        let mut c_lp = 1u64;
        let mut c_data = Vec::new();
        let owner = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        let sysvar_owner = solana_program::sysvar::id();
        let clock_key = solana_program::sysvar::clock::id();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);
        let attacker_info = acct(&attacker, &owner, &mut x_lp, &mut x_d, true, false);
        let clock_info = clock_acct(&clock_key, &sysvar_owner, &mut c_lp, &mut c_data, 500);

        // Non-admin cannot queue.
        let res = process_queue_update(
            &program_id,
            &[
                attacker_info.clone(),
                config_info.clone(),
                clock_info.clone(),
            ],
            Some(10),
            None,
            None,
            None,
            None,
        );
        assert_eq!(res, Err(StakingError::Unauthorized.into()));

        // Over-cap values are rejected at queue time.
        let res = process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            Some(MAX_FEE_BPS + 1),
            None,
            None,
            None,
            None,
        );
        assert_eq!(res, Err(StakingError::FeeTooHigh.into()));
        let res = process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            None,
            Some(MAX_REWARD_RATE_BPS + 1),
            None,
            None,
            None,
        );
        assert_eq!(res, Err(StakingError::RewardRateTooHigh.into()));
        let res = process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            None,
            None,
            None,
            None,
            Some(-5),
        );
        assert_eq!(res, Err(StakingError::TimelockOutOfRange.into()));

        // A second queue while one is pending is rejected.
        process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            Some(10),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let res = process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            Some(20),
            None,
            None,
            None,
            None,
        );
        assert_eq!(res, Err(StakingError::UpdateAlreadyQueued.into()));
    }

    #[test]
    fn apply_is_permissionless_and_waits_out_the_timelock() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump); // timelock 3_600
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut c1_lp = 1u64;
        let mut c1_data = Vec::new();
        let owner = Pubkey::new_unique();
        let sysvar_owner = solana_program::sysvar::id();
        let clock_key = solana_program::sysvar::clock::id();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);
        let t0 = 2_000_000;
        let clock1 = clock_acct(&clock_key, &sysvar_owner, &mut c1_lp, &mut c1_data, t0);

        // No pending update -> apply fails.
        let res = process_apply_update(&program_id, &[config_info.clone(), clock1.clone()]);
        assert_eq!(res, Err(StakingError::NoPendingUpdate.into()));

        // Queue a fee + rate change at t0.
        process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock1.clone()],
            Some(250),
            Some(500),
            None,
            None,
            None,
        )
        .unwrap();

        // One second before the delay elapses -> still locked.
        let mut c2_lp = 1u64;
        let mut c2_data = Vec::new();
        let clock_early = clock_acct(
            &clock_key,
            &sysvar_owner,
            &mut c2_lp,
            &mut c2_data,
            t0 + 3_599,
        );
        let res = process_apply_update(&program_id, &[config_info.clone(), clock_early.clone()]);
        assert_eq!(res, Err(StakingError::TimelockNotElapsed.into()));

        // Exactly at the delay -> applies. No signer in the account list:
        // apply is permissionless.
        let mut c3_lp = 1u64;
        let mut c3_data = Vec::new();
        let clock_due = clock_acct(
            &clock_key,
            &sysvar_owner,
            &mut c3_lp,
            &mut c3_data,
            t0 + 3_600,
        );
        process_apply_update(&program_id, &[config_info.clone(), clock_due.clone()]).unwrap();

        let after = read_config(&config_info);
        assert_eq!(after.fee_bps, 250);
        assert_eq!(after.reward_rate_bps, 500);
        assert_eq!(after.min_stake, cfg.min_stake, "None kept the old value");
        assert!(!after.pending.active, "pending cleared");

        // Applying again with nothing queued fails.
        let res = process_apply_update(&program_id, &[config_info.clone(), clock_due.clone()]);
        assert_eq!(res, Err(StakingError::NoPendingUpdate.into()));
    }

    #[test]
    fn timelock_change_itself_waits_out_the_old_delay() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump); // timelock 3_600
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut c1_lp = 1u64;
        let mut c1_data = Vec::new();
        let owner = Pubkey::new_unique();
        let sysvar_owner = solana_program::sysvar::id();
        let clock_key = solana_program::sysvar::clock::id();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);
        let t0 = 7_000_000;
        let clock1 = clock_acct(&clock_key, &sysvar_owner, &mut c1_lp, &mut c1_data, t0);

        // Queue shortening the delay to 60s.
        process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock1.clone()],
            None,
            None,
            None,
            None,
            Some(60),
        )
        .unwrap();
        assert_eq!(
            read_config(&config_info).timelock_secs,
            3_600,
            "delay unchanged while queued"
        );

        // The shortening must still wait out the OLD 3600s delay.
        let mut c2_lp = 1u64;
        let mut c2_data = Vec::new();
        let clock_mid = clock_acct(
            &clock_key,
            &sysvar_owner,
            &mut c2_lp,
            &mut c2_data,
            t0 + 120,
        );
        let res = process_apply_update(&program_id, &[config_info.clone(), clock_mid.clone()]);
        assert_eq!(res, Err(StakingError::TimelockNotElapsed.into()));

        let mut c3_lp = 1u64;
        let mut c3_data = Vec::new();
        let clock_due = clock_acct(
            &clock_key,
            &sysvar_owner,
            &mut c3_lp,
            &mut c3_data,
            t0 + 3_600,
        );
        process_apply_update(&program_id, &[config_info.clone(), clock_due.clone()]).unwrap();
        assert_eq!(read_config(&config_info).timelock_secs, 60);
    }

    #[test]
    fn cancel_requires_admin_and_clears_pending() {
        let program_id = crate::id();
        let (config_key, bump) = config_pda(&program_id);
        let cfg = sample_config(bump);
        let admin_key = cfg.admin;

        let mut data = borsh::to_vec(&cfg).unwrap();
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut x_lp = 0u64;
        let mut x_d = [0u8; 0];
        let mut c_lp = 1u64;
        let mut c_data = Vec::new();
        let owner = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        let sysvar_owner = solana_program::sysvar::id();
        let clock_key = solana_program::sysvar::clock::id();

        let config_info = acct(
            &config_key,
            &program_id,
            &mut cfg_lp,
            &mut data,
            false,
            true,
        );
        let admin_info = acct(&admin_key, &owner, &mut a_lp, &mut a_d, true, false);
        let attacker_info = acct(&attacker, &owner, &mut x_lp, &mut x_d, true, false);
        let clock_info = clock_acct(&clock_key, &sysvar_owner, &mut c_lp, &mut c_data, 42);

        // Nothing queued yet.
        let res = process_cancel_update(&program_id, &[admin_info.clone(), config_info.clone()]);
        assert_eq!(res, Err(StakingError::NoPendingUpdate.into()));

        process_queue_update(
            &program_id,
            &[admin_info.clone(), config_info.clone(), clock_info.clone()],
            Some(300),
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // A non-admin cannot cancel.
        let res = process_cancel_update(&program_id, &[attacker_info.clone(), config_info.clone()]);
        assert_eq!(res, Err(StakingError::Unauthorized.into()));
        assert!(read_config(&config_info).pending.active);

        // The admin can.
        process_cancel_update(&program_id, &[admin_info.clone(), config_info.clone()]).unwrap();
        assert!(!read_config(&config_info).pending.active);
    }

    // ------------------------------------------------------- max supply cap --

    /// Pack an SPL `Mint` with the given supply into a fresh buffer.
    fn packed_mint(supply: u64, decimals: u8, authority: &Pubkey) -> Vec<u8> {
        let m = Mint {
            mint_authority: COption::Some(*authority),
            supply,
            decimals,
            is_initialized: true,
            freeze_authority: COption::None,
        };
        let mut data = vec![0u8; Mint::LEN];
        Pack::pack(m, &mut data).unwrap();
        data
    }

    /// Run process_genesis_mint with a REAL packed mint account (supply-aware).
    /// Returns the result plus the post-call config (latch state).
    fn genesis_with_supply(
        cfg: &Config,
        mint_supply: u64,
        amount: u64,
        signer_override: Option<&Pubkey>,
    ) -> (ProgramResult, bool) {
        let program_id = crate::id();
        let (config_key, _) = config_pda(&program_id);
        let token_id = spl_token::id();
        let signer = signer_override.unwrap_or(&cfg.admin);
        let recipient = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let mut buf = borsh::to_vec(cfg).unwrap();
        let mut mint_data = packed_mint(mint_supply, cfg.decimals, &config_key);
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut m_lp = 1u64;
        let mut r_lp = 0u64;
        let mut r_d = [0u8; 0];
        let mut t_lp = 0u64;
        let mut t_d = [0u8; 0];

        let accs = vec![
            acct(signer, &owner, &mut a_lp, &mut a_d, true, false),
            acct(&config_key, &program_id, &mut cfg_lp, &mut buf, false, true),
            acct(&cfg.mint, &token_id, &mut m_lp, &mut mint_data, false, true),
            acct(&recipient, &token_id, &mut r_lp, &mut r_d, false, true),
            acct(&token_id, &owner, &mut t_lp, &mut t_d, false, false),
        ];
        let res = process_genesis_mint(&program_id, &accs, amount);
        let latch = deserialize::<Config>(&accs[1].data.borrow())
            .map(|c| c.genesis_done)
            .unwrap_or(cfg.genesis_done);
        (res, latch)
    }

    #[test]
    fn genesis_rejects_one_over_the_cap() {
        let (_, bump) = config_pda(&crate::id());
        let mut cfg = genesis_cfg(bump);
        cfg.max_supply = 1_000;
        // supply 500 + amount 501 = 1001 > 1000 -> rejected, latch untouched.
        let (res, latch) = genesis_with_supply(&cfg, 500, 501, None);
        assert_eq!(res, Err(StakingError::MaxSupplyExceeded.into()));
        assert!(!latch, "a rejected genesis must not flip the latch");
    }

    #[test]
    fn genesis_allows_minting_exactly_to_the_cap() {
        let (_, bump) = config_pda(&crate::id());
        let mut cfg = genesis_cfg(bump);
        cfg.max_supply = 1_000;
        // supply 500 + amount 500 = exactly the cap -> validation passes and
        // the mint CPI proceeds (host syscall stub acknowledges the invoke).
        let (res, latch) = genesis_with_supply(&cfg, 500, 500, None);
        assert_eq!(res, Ok(()));
        assert!(latch, "a successful genesis flips the latch");
    }

    #[test]
    fn genesis_cap_is_measured_against_the_live_mint_supply() {
        let (_, bump) = config_pda(&crate::id());
        let mut cfg = genesis_cfg(bump);
        cfg.max_supply = 1_000;
        // The mint ALREADY holds the full cap (e.g. cap lowered at init or a
        // prior deployment state): even amount=1 must fail.
        let (res, latch) = genesis_with_supply(&cfg, 1_000, 1, None);
        assert_eq!(res, Err(StakingError::MaxSupplyExceeded.into()));
        assert!(!latch);
        // supply above cap (defensive) -> no headroom, still rejected.
        let (res, _) = genesis_with_supply(&cfg, 5_000, 1, None);
        assert_eq!(res, Err(StakingError::MaxSupplyExceeded.into()));
    }

    #[test]
    fn genesis_cap_check_still_requires_admin_first() {
        // Authorization is checked BEFORE the cap: an over-cap amount from a
        // non-admin must fail with Unauthorized, not MaxSupplyExceeded.
        let (_, bump) = config_pda(&crate::id());
        let mut cfg = genesis_cfg(bump);
        cfg.max_supply = 1_000;
        let impostor = Pubkey::new_unique();
        let (res, latch) = genesis_with_supply(&cfg, 0, 10_000, Some(&impostor));
        assert_eq!(res, Err(StakingError::Unauthorized.into()));
        assert!(!latch);
    }

    #[test]
    fn genesis_rejects_a_mint_account_that_is_not_a_valid_mint() {
        // Admin + amount under cap, but the mint account holds garbage: the
        // supply cannot be verified -> InvalidMint (fail closed, never skip
        // the cap check).
        let program_id = crate::id();
        let (_, bump) = config_pda(&program_id);
        let mut cfg = genesis_cfg(bump);
        cfg.max_supply = 1_000;
        let (config_key, _) = config_pda(&program_id);
        let token_id = spl_token::id();
        let recipient = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let mut buf = borsh::to_vec(&cfg).unwrap();
        let mut garbage = vec![7u8; Mint::LEN]; // unpacks to an invalid state
        let mut cfg_lp = 1u64;
        let mut a_lp = 0u64;
        let mut a_d = [0u8; 0];
        let mut m_lp = 1u64;
        let mut r_lp = 0u64;
        let mut r_d = [0u8; 0];
        let mut t_lp = 0u64;
        let mut t_d = [0u8; 0];
        let admin = cfg.admin;
        let mint = cfg.mint;
        let accs = vec![
            acct(&admin, &owner, &mut a_lp, &mut a_d, true, false),
            acct(&config_key, &program_id, &mut cfg_lp, &mut buf, false, true),
            acct(&mint, &token_id, &mut m_lp, &mut garbage, false, true),
            acct(&recipient, &token_id, &mut r_lp, &mut r_d, false, true),
            acct(&token_id, &owner, &mut t_lp, &mut t_d, false, false),
        ];
        let res = process_genesis_mint(&program_id, &accs, 10);
        assert_eq!(res, Err(StakingError::InvalidMint.into()));
    }

    // ------------------------------------------------- reward clamp at cap --

    /// Claim with `pending_rewards = 8` against a mint whose remaining
    /// headroom under the cap is `headroom`; returns the handler result and
    /// the post-call StakeAccount. The claim path only MINTS (no principal
    /// transfer), so with the host syscall stub acknowledging the CPI the
    /// full clamp logic runs on the host.
    fn claim_with_headroom(headroom: u64) -> (ProgramResult, StakeAccount) {
        let program_id = crate::id();
        let (config_key, config_bump) = config_pda(&program_id);
        let staker = Pubkey::new_unique();
        let (stake_key, stake_bump) = stake_pda(&program_id, &staker);
        let token_id = spl_token::id();
        let sysvar_owner = solana_program::sysvar::id();
        let clock_key = solana_program::sysvar::clock::id();
        let owner = Pubkey::new_unique();

        let mut cfg = sample_config(config_bump);
        cfg.reward_rate_bps = 0; // no live accrual: rewards == pending only
        cfg.max_supply = 1_000 + headroom; // supply will be pinned at 1_000

        let sa = StakeAccount {
            owner: staker,
            amount: 0,
            staked_at: 0,
            reward_from: 0,
            pending_rewards: 8,
            bump: stake_bump,
        };

        let mut buf = borsh::to_vec(&cfg).unwrap();
        let mut stake_buf = borsh::to_vec(&sa).unwrap();
        let ta = TokenAccount {
            mint: cfg.mint,
            owner: staker,
            amount: 0,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut token_buf = vec![0u8; TokenAccount::LEN];
        Pack::pack(ta, &mut token_buf).unwrap();
        let mut mint_buf = packed_mint(1_000, cfg.decimals, &config_key);
        let clock = Clock {
            unix_timestamp: 5_000,
            ..Clock::default()
        };
        let mut clock_buf = bincode::serialize(&clock).unwrap();

        let (mut cfg_lp, mut staker_lp, mut vault_lp) = (1u64, 0u64, 1u64);
        let (mut stake_lp, mut token_lp, mut mint_lp, mut clock_lp) = (1u64, 1u64, 1u64, 1u64);
        let mut tprog_lp = 0u64;
        let (mut staker_d, mut vault_d, mut token_prog_d) = ([0u8; 0], [0u8; 0], [0u8; 0]);

        let mint = cfg.mint;
        let vault = cfg.vault;
        let accs = vec![
            acct(&staker, &owner, &mut staker_lp, &mut staker_d, true, true),
            acct(
                &staker,
                &token_id,
                &mut token_lp,
                &mut token_buf,
                false,
                true,
            ),
            acct(&vault, &token_id, &mut vault_lp, &mut vault_d, false, true),
            acct(&mint, &token_id, &mut mint_lp, &mut mint_buf, false, true),
            acct(
                &stake_key,
                &program_id,
                &mut stake_lp,
                &mut stake_buf,
                false,
                true,
            ),
            acct(&config_key, &program_id, &mut cfg_lp, &mut buf, false, true),
            acct(
                &token_id,
                &owner,
                &mut tprog_lp,
                &mut token_prog_d,
                false,
                false,
            ),
            acct(
                &clock_key,
                &sysvar_owner,
                &mut clock_lp,
                &mut clock_buf,
                false,
                false,
            ),
        ];
        let res = process_unstake(&program_id, &accs, true);
        let after: StakeAccount = deserialize(&accs[4].data.borrow()).unwrap_or(sa.clone());
        (res, after)
    }

    #[test]
    fn claim_mints_full_rewards_while_under_the_cap() {
        // Headroom 100 >= pending 8: the claim succeeds and the reward state
        // resets (pending zeroed, clock moved to now).
        let (res, sa) = claim_with_headroom(100);
        assert_eq!(res, Ok(()));
        assert_eq!(sa.pending_rewards, 0, "pending folded into the mint");
        assert_eq!(sa.reward_from, 5_000, "reward clock advanced to now");
    }

    #[test]
    fn claim_clamps_rewards_to_the_remaining_headroom() {
        // Headroom 3 < pending 8: withdrawal must STILL succeed (user funds are
        // never frozen) — the reward is clamped to the cap and the shortfall
        // forfeited, with the state reset exactly like a full claim.
        let (res, sa) = claim_with_headroom(3);
        assert_eq!(res, Ok(()));
        assert_eq!(sa.pending_rewards, 0);
        assert_eq!(sa.reward_from, 5_000);
    }

    #[test]
    fn claim_at_zero_headroom_still_succeeds_with_no_mint() {
        // Supply already AT the cap: nothing may be minted, but the claim
        // itself must not fail — the invariant "withdrawals are never gated"
        // outranks reward delivery.
        let (res, sa) = claim_with_headroom(0);
        assert_eq!(res, Ok(()));
        assert_eq!(sa.pending_rewards, 0);
    }

    // ------------------------------------------------------- metadata (mpl) --

    #[test]
    fn create_metadata_accounts_v3_data_layout_is_pinned() {
        // Discriminant 33 (mpl-token-metadata v1.14.0 enum order — the layout
        // the REAL mainnet program accepts; verified end-to-end by the
        // mainnet-cloned mpl e2e), three borsh strings (u32-LE len + bytes),
        // zero seller fee, three None options, is_mutable=false, no collection
        // details. Any mpl-side layout change breaks this test loudly.
        let d = create_metadata_accounts_v3_data("N", "SY", "uri");
        let mut expect: Vec<u8> = vec![33];
        expect.extend_from_slice(&1u32.to_le_bytes());
        expect.extend_from_slice(b"N");
        expect.extend_from_slice(&2u32.to_le_bytes());
        expect.extend_from_slice(b"SY");
        expect.extend_from_slice(&3u32.to_le_bytes());
        expect.extend_from_slice(b"uri");
        expect.extend_from_slice(&0u16.to_le_bytes()); // seller_fee_basis_points
        expect.push(0); // creators: None
        expect.push(0); // collection: None
        expect.push(0); // uses: None
        expect.push(0); // is_mutable: false
        expect.push(0); // collection_details: None
        assert_eq!(d, expect);
    }

    /// 7-account metadata fixture; all buffers caller-owned.
    struct MetaFixture {
        buf: Vec<u8>,
        cfg_lp: u64,
        a_lp: u64,
        a_d: [u8; 0],
        m_lp: u64,
        m_d: [u8; 0],
        meta_lp: u64,
        meta_d: [u8; 0],
        p_lp: u64,
        p_d: [u8; 0],
        s_lp: u64,
        s_d: [u8; 0],
        r_lp: u64,
        r_d: [u8; 0],
    }

    fn run_metadata(
        cfg: &Config,
        signer: &Pubkey,
        metadata_key_override: Option<&Pubkey>,
        metadata_program_override: Option<&Pubkey>,
        metadata_lamports: u64,
        fields: (&str, &str, &str),
    ) -> ProgramResult {
        let program_id = crate::id();
        let (config_key, _) = config_pda(&program_id);
        let (meta_pda, _) = metadata_pda(&cfg.mint);
        let token_id = spl_token::id();
        let system_id = solana_system_interface::program::id();
        let rent_id = solana_program::sysvar::rent::id();
        let owner = Pubkey::new_unique();

        let mut fx = MetaFixture {
            buf: borsh::to_vec(cfg).unwrap(),
            cfg_lp: 1,
            a_lp: 0,
            a_d: [0u8; 0],
            m_lp: 1,
            m_d: [0u8; 0],
            meta_lp: metadata_lamports,
            meta_d: [0u8; 0],
            p_lp: 0,
            p_d: [0u8; 0],
            s_lp: 0,
            s_d: [0u8; 0],
            r_lp: 0,
            r_d: [0u8; 0],
        };
        let meta_key = metadata_key_override.unwrap_or(&meta_pda);
        let prog_key = metadata_program_override.unwrap_or(&TOKEN_METADATA_PROGRAM_ID);
        let accs = vec![
            acct(signer, &owner, &mut fx.a_lp, &mut fx.a_d, true, true),
            acct(
                &config_key,
                &program_id,
                &mut fx.cfg_lp,
                &mut fx.buf,
                false,
                true,
            ),
            acct(
                &cfg.mint,
                &token_id,
                &mut fx.m_lp,
                &mut fx.m_d,
                false,
                false,
            ),
            acct(
                meta_key,
                &TOKEN_METADATA_PROGRAM_ID,
                &mut fx.meta_lp,
                &mut fx.meta_d,
                false,
                true,
            ),
            acct(prog_key, &owner, &mut fx.p_lp, &mut fx.p_d, false, false),
            acct(&system_id, &owner, &mut fx.s_lp, &mut fx.s_d, false, false),
            acct(&rent_id, &owner, &mut fx.r_lp, &mut fx.r_d, false, false),
        ];
        process_create_token_metadata(
            &program_id,
            &accs,
            fields.0.to_string(),
            fields.1.to_string(),
            fields.2.to_string(),
        )
    }

    #[test]
    fn metadata_requires_the_admin() {
        let (_, bump) = config_pda(&crate::id());
        let cfg = sample_config(bump);
        let impostor = Pubkey::new_unique();
        let res = run_metadata(&cfg, &impostor, None, None, 0, ("N", "S", "u"));
        assert_eq!(res, Err(StakingError::Unauthorized.into()));
    }

    #[test]
    fn metadata_rejects_a_wrong_metadata_program() {
        let (_, bump) = config_pda(&crate::id());
        let cfg = sample_config(bump);
        let fake_program = Pubkey::new_unique();
        let res = run_metadata(
            &cfg,
            &cfg.admin,
            None,
            Some(&fake_program),
            0,
            ("N", "S", "u"),
        );
        assert_eq!(res, Err(StakingError::InvalidMetadataProgram.into()));
    }

    #[test]
    fn metadata_rejects_a_wrong_mint() {
        let (_, bump) = config_pda(&crate::id());
        let mut cfg = sample_config(bump);
        cfg.mint = Pubkey::new_unique();
        // The fixture derives the metadata PDA from cfg.mint, but we pass a
        // config whose mint was changed AFTER deriving — simulate by giving
        // the handler a mint account that is not config.mint.
        let program_id = crate::id();
        let (config_key, _) = config_pda(&program_id);
        let (meta_pda, _) = metadata_pda(&cfg.mint);
        let wrong_mint = Pubkey::new_unique();
        let token_id = spl_token::id();
        let system_id = solana_system_interface::program::id();
        let rent_id = solana_program::sysvar::rent::id();
        let owner = Pubkey::new_unique();
        let mut buf = borsh::to_vec(&cfg).unwrap();
        let (mut cfg_lp, mut a_lp, mut m_lp, mut meta_lp) = (1u64, 0u64, 1u64, 0u64);
        let (mut p_lp, mut s_lp, mut r_lp) = (0u64, 0u64, 0u64);
        let (mut a_d, mut m_d, mut meta_d, mut p_d, mut s_d, mut r_d) =
            ([0u8; 0], [0u8; 0], [0u8; 0], [0u8; 0], [0u8; 0], [0u8; 0]);
        let admin = cfg.admin;
        let accs = vec![
            acct(&admin, &owner, &mut a_lp, &mut a_d, true, true),
            acct(&config_key, &program_id, &mut cfg_lp, &mut buf, false, true),
            acct(&wrong_mint, &token_id, &mut m_lp, &mut m_d, false, false),
            acct(
                &meta_pda,
                &TOKEN_METADATA_PROGRAM_ID,
                &mut meta_lp,
                &mut meta_d,
                false,
                true,
            ),
            acct(
                &TOKEN_METADATA_PROGRAM_ID,
                &owner,
                &mut p_lp,
                &mut p_d,
                false,
                false,
            ),
            acct(&system_id, &owner, &mut s_lp, &mut s_d, false, false),
            acct(&rent_id, &owner, &mut r_lp, &mut r_d, false, false),
        ];
        let res =
            process_create_token_metadata(&program_id, &accs, "N".into(), "S".into(), "u".into());
        assert_eq!(res, Err(StakingError::InvalidMint.into()));
    }

    #[test]
    fn metadata_is_one_shot() {
        let (_, bump) = config_pda(&crate::id());
        let cfg = sample_config(bump);
        // An already-funded metadata PDA -> rejected before any CPI.
        let res = run_metadata(&cfg, &cfg.admin, None, None, 1_000_000, ("N", "S", "u"));
        assert_eq!(res, Err(StakingError::MetadataAlreadyExists.into()));
    }

    #[test]
    fn metadata_rejects_a_non_pda_metadata_account() {
        let (_, bump) = config_pda(&crate::id());
        let cfg = sample_config(bump);
        let impostor_meta = Pubkey::new_unique();
        let res = run_metadata(
            &cfg,
            &cfg.admin,
            Some(&impostor_meta),
            None,
            0,
            ("N", "S", "u"),
        );
        assert_eq!(res, Err(StakingError::InvalidAccount.into()));
    }

    #[test]
    fn metadata_rejects_bad_fields_before_the_cpi() {
        let (_, bump) = config_pda(&crate::id());
        let cfg = sample_config(bump);
        for fields in [
            ("", "S", "u"),
            ("N", "", "u"),
            ("N", "S", ""),
            (&"n".repeat(33)[..], "S", "u"),
            ("N", &"s".repeat(11)[..], "u"),
            ("N", "S", &"u".repeat(201)[..]),
        ] {
            let res = run_metadata(&cfg, &cfg.admin, None, None, 0, fields);
            assert_eq!(res, Err(StakingError::MetadataFieldTooLong.into()));
        }
    }

    #[test]
    fn metadata_succeeds_for_the_admin_with_valid_inputs() {
        let (_, bump) = config_pda(&crate::id());
        let cfg = sample_config(bump);
        // All validations pass; the CPI is acknowledged by the host syscall
        // stub (the on-chain effect itself is covered by the gated validator
        // e2e test).
        let res = run_metadata(
            &cfg,
            &cfg.admin,
            None,
            None,
            0,
            (
                "Sniper Suite Token",
                "SNPR",
                "https://example.com/snpr.json",
            ),
        );
        assert_eq!(res, Ok(()));
    }
}
