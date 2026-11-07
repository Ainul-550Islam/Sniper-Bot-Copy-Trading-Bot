//! Host regressions for the processor's documented last-deposit cooldown.
//! Token CPIs use Solana's acknowledging host stubs: these tests prove program
//! state transitions and refusal ordering, NOT token balances or validator execution.

use borsh::BorshDeserialize;
use solana_program::{
    account_info::AccountInfo, clock::Clock, program_error::ProgramError, program_option::COption,
    program_pack::Pack, pubkey::Pubkey,
};
use spl_token::state::{Account as TokenAccount, AccountState, Mint};
use staking_suite::{
    error::StakingError,
    instruction::StakingInstruction,
    process_instruction,
    state::{config_pda, stake_pda, Config, PendingParams, StakeAccount},
};

const NOW: i64 = 5_000;
const DELAY: i64 = 3_600;

struct AccountFixture {
    key: Pubkey,
    owner: Pubkey,
    lamports: u64,
    data: Vec<u8>,
    signer: bool,
}

impl AccountFixture {
    fn new(key: Pubkey, owner: Pubkey, data: Vec<u8>, signer: bool) -> Self {
        Self {
            key,
            owner,
            lamports: 1,
            data,
            signer,
        }
    }

    fn info(&mut self) -> AccountInfo<'_> {
        AccountInfo::new(
            &self.key,
            self.signer,
            true,
            &mut self.lamports,
            &mut self.data,
            &self.owner,
            false,
            0,
        )
    }
}

fn top_up_then_withdraw(
    at: i64,
    claim_only: bool,
    deposit: u64,
) -> (StakeAccount, Result<(), ProgramError>, StakeAccount) {
    let program = staking_suite::id();
    let (config_key, config_bump) = config_pda(&program);
    let staker = Pubkey::new_unique();
    let (stake_key, stake_bump) = stake_pda(&program, &staker);
    let system = solana_system_interface::program::id();
    let token = spl_token::id();
    let config = Config {
        initialized: true,
        admin: Pubkey::new_unique(),
        mint: Pubkey::new_unique(),
        vault: Pubkey::new_unique(),
        treasury: Pubkey::new_unique(),
        fee_bps: 100,
        reward_rate_bps: 1_000,
        min_stake: 0,
        unstake_delay: DELAY,
        decimals: 6,
        config_bump,
        mint_bump: config_bump,
        paused: false,
        pending_admin: Pubkey::default(),
        timelock_secs: 0,
        pending: PendingParams::default(),
        genesis_done: true,
        max_supply: u64::MAX,
    };
    let position = StakeAccount {
        owner: staker,
        amount: 1_000_000,
        staked_at: 0,
        reward_from: 0,
        pending_rewards: 7,
        bump: stake_bump,
    };
    let expected_rewards = position.accrued_rewards(config.reward_rate_bps, NOW);
    let source = TokenAccount {
        mint: config.mint,
        owner: staker,
        amount: 10_000_000,
        delegate: COption::None,
        state: AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: 0,
        close_authority: COption::None,
    };
    let mut source_data = vec![0; TokenAccount::LEN];
    TokenAccount::pack(source, &mut source_data).unwrap();
    let mint = Mint {
        mint_authority: COption::Some(config_key),
        supply: 0,
        decimals: 6,
        is_initialized: true,
        freeze_authority: COption::None,
    };
    let mut mint_data = vec![0; Mint::LEN];
    Mint::pack(mint, &mut mint_data).unwrap();
    let clock = Clock {
        unix_timestamp: NOW,
        ..Clock::default()
    };
    let mut fixtures = vec![
        AccountFixture::new(staker, system, vec![], true),
        AccountFixture::new(Pubkey::new_unique(), token, source_data, false),
        AccountFixture::new(config.vault, token, vec![], false),
        AccountFixture::new(config.treasury, token, vec![], false),
        AccountFixture::new(stake_key, program, borsh::to_vec(&position).unwrap(), false),
        AccountFixture::new(config_key, program, borsh::to_vec(&config).unwrap(), false),
        AccountFixture::new(token, system, vec![], false),
        AccountFixture::new(system, system, vec![], false),
        AccountFixture::new(
            solana_program::sysvar::clock::id(),
            solana_program::sysvar::id(),
            bincode::serialize(&clock).unwrap(),
            false,
        ),
        AccountFixture::new(config.mint, token, mint_data, false),
    ];
    let accounts: Vec<_> = fixtures.iter_mut().map(AccountFixture::info).collect();
    process_instruction(
        &program,
        &accounts[..9],
        &StakingInstruction::Stake { amount: deposit }
            .pack()
            .unwrap(),
    )
    .unwrap();
    let deposited = StakeAccount::try_from_slice(&accounts[4].data.borrow()).unwrap();
    assert_eq!(
        deposited.amount,
        position.amount + deposit - staking_suite::state::compute_fee(deposit, config.fee_bps),
        "top-up adds only net principal"
    );
    assert_eq!(
        deposited.pending_rewards, expected_rewards,
        "top-up preserves the old principal's already accrued rewards"
    );
    let later_clock = Clock {
        unix_timestamp: at,
        ..Clock::default()
    };
    accounts[8]
        .data
        .borrow_mut()
        .copy_from_slice(&bincode::serialize(&later_clock).unwrap());
    let withdrawal_accounts = vec![
        accounts[0].clone(),
        accounts[1].clone(),
        accounts[2].clone(),
        accounts[9].clone(),
        accounts[4].clone(),
        accounts[5].clone(),
        accounts[6].clone(),
        accounts[8].clone(),
    ];
    let instruction = if claim_only {
        StakingInstruction::Claim
    } else {
        StakingInstruction::Unstake
    };
    let result = process_instruction(&program, &withdrawal_accounts, &instruction.pack().unwrap());
    let after = StakeAccount::try_from_slice(&accounts[4].data.borrow()).unwrap();
    (deposited, result, after)
}

#[test]
fn top_up_restarts_cooldown_and_refuses_immediate_unstake() {
    let (deposited, result, after) = top_up_then_withdraw(NOW, false, 100_000);
    assert_eq!(
        deposited.staked_at, NOW,
        "cooldown starts at the latest principal change"
    );
    assert_eq!(result, Err(StakingError::CooldownActive.into()));
    assert_eq!(
        after, deposited,
        "refused withdrawal must not mutate position state"
    );
}

#[test]
fn top_up_cannot_withdraw_one_second_before_the_new_deadline() {
    let (deposited, result, after) = top_up_then_withdraw(NOW + DELAY - 1, false, 100_000);
    assert_eq!(result, Err(StakingError::CooldownActive.into()));
    assert_eq!(after, deposited);
}

#[test]
fn top_up_can_withdraw_exactly_at_the_new_deadline() {
    let (_, result, after) = top_up_then_withdraw(NOW + DELAY, false, 100_000);
    assert_eq!(result, Ok(()));
    assert_eq!(after.amount, 0);
    assert_eq!(after.pending_rewards, 0);
}

#[test]
fn top_up_does_not_block_reward_claim_during_cooldown() {
    let (deposited, result, after) = top_up_then_withdraw(NOW, true, 100_000);
    assert_eq!(result, Ok(()));
    assert_eq!(
        after.amount, deposited.amount,
        "claim must preserve principal"
    );
    assert_eq!(after.pending_rewards, 0);
}

#[test]
fn zero_net_deposit_does_not_restart_a_mature_positions_cooldown() {
    let (deposited, result, after) = top_up_then_withdraw(NOW, false, 0);
    assert_eq!(
        deposited.staked_at, 0,
        "no principal change means no new cooldown"
    );
    assert_eq!(result, Ok(()));
    assert_eq!(after.amount, 0);
}
