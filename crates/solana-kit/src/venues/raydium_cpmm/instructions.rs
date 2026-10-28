//! Raydium CPMM (`cp-swap`) instruction builders.
//!
//! # Provenance
//!
//! The account lists, the argument order and the writable/signer flags come
//! from the **official vendored IDL**
//! (`crates/solana-kit/idl/raydium_cp_swap.json`), whose `instructions` array
//! lists the accounts in exactly the order Anchor expects them, each with an
//! `isMut` / `isSigner` flag. The same shape is confirmed by the program's
//! own `#[derive(Accounts)]` structs in
//! `raydium-io/raydium-cp-swap` @ f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5
//! (`programs/cp-swap/src/instructions/swap_base_input.rs`).
//!
//! # Token-2022 awareness
//!
//! The CPMM program is Token-2022 aware: `input_token_program` and
//! `output_token_program` are `Interface<'info, TokenInterface>` accounts, so
//! they may be either the legacy SPL Token program or the Token-2022 program.
//! The pool state stores which program each side uses, and this builder takes
//! them from there — it never guesses.
//!
//! # Data layout
//!
//! Anchor instructions are `discriminator (8 bytes) || borsh(args)`.
//! `swap_base_input` takes `(amount_in: u64, minimum_amount_out: u64)` and
//! `swap_base_output` takes `(max_amount_in: u64, amount_out: u64)`, both
//! little endian.

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;
use crate::venues::raydium_cpmm::accounts::CpmmSide;
use crate::venues::raydium_cpmm::pda;

/// Which of the two swap instructions to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapKind {
    /// `swap_base_input` — the input amount is fixed, the output is floored.
    BaseInput,
    /// `swap_base_output` — the output amount is fixed, the input is ceiled.
    BaseOutput,
}

impl SwapKind {
    /// The Anchor instruction name, as spelled in the IDL.
    pub fn instruction_name(self) -> &'static str {
        match self {
            SwapKind::BaseInput => "swap_base_input",
            SwapKind::BaseOutput => "swap_base_output",
        }
    }

    /// The Anchor discriminator of the instruction.
    pub fn discriminator(self) -> [u8; 8] {
        match self {
            SwapKind::BaseInput => RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT,
            SwapKind::BaseOutput => RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT,
        }
    }
}

/// Everything the swap instructions need that is not derivable from the pool
/// account alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapAccounts {
    /// The trader (the `payer` account, which signs).
    pub payer: Pubkey,
    /// The user's source token account (the input side).
    pub input_token_account: Pubkey,
    /// The user's destination token account (the output side).
    pub output_token_account: Pubkey,
    /// The input-side token program (SPL Token or Token-2022).
    pub input_token_program: Pubkey,
    /// The output-side token program (SPL Token or Token-2022).
    pub output_token_program: Pubkey,
    /// The input-side mint.
    pub input_token_mint: Pubkey,
    /// The output-side mint.
    pub output_token_mint: Pubkey,
}

/// Build a `swap_base_input` instruction.
///
/// Account order (exactly the IDL's `swap_base_input.accounts`):
///
/// ```text
///  0 payer                w/s     7 output_vault          w
///  1 authority           ro      8 input_token_program   ro
///  2 amm_config          ro      9 output_token_program  ro
///  3 pool_state          w      10 input_token_mint     ro
///  4 input_token_account w      11 output_token_mint    ro
///  5 output_token_acc    w      12 observation_state    w
///  6 input_vault         w
/// ```
///
/// Args: `amount_in: u64`, `minimum_amount_out: u64`.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_base_input(
    pool: &Pubkey,
    amm_config: &Pubkey,
    authority: &Pubkey,
    observation: &Pubkey,
    input_vault: &Pubkey,
    output_vault: &Pubkey,
    accounts: &SwapAccounts,
    amount_in: u64,
    minimum_amount_out: u64,
) -> BotResult<Instruction> {
    if amount_in == 0 {
        return Err(BotError::invalid(
            "CPMM swap_base_input amount_in must be > 0",
        ));
    }
    if minimum_amount_out == 0 {
        return Err(BotError::invalid(
            "CPMM swap_base_input minimum_amount_out must be > 0",
        ));
    }
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    Ok(Instruction {
        program_id: *RAYDIUM_CPMM,
        accounts: vec![
            // The IDL declares the payer as a signer but *not* writable, and
            // the program declares it as `Signer<'info>` (read-only). A
            // writable payer would ask the runtime to lock the trader's
            // account for lamports the swap never touches.
            AccountMeta::new_readonly(accounts.payer, true),
            AccountMeta::new_readonly(*authority, false),
            AccountMeta::new_readonly(*amm_config, false),
            AccountMeta::new(*pool, false),
            AccountMeta::new(accounts.input_token_account, false),
            AccountMeta::new(accounts.output_token_account, false),
            AccountMeta::new(*input_vault, false),
            AccountMeta::new(*output_vault, false),
            AccountMeta::new_readonly(accounts.input_token_program, false),
            AccountMeta::new_readonly(accounts.output_token_program, false),
            AccountMeta::new_readonly(accounts.input_token_mint, false),
            AccountMeta::new_readonly(accounts.output_token_mint, false),
            AccountMeta::new(*observation, false),
        ],
        data,
    })
}

/// Build a `swap_base_output` instruction.
///
/// Account order (exactly the IDL's `swap_base_output.accounts`): identical
/// to `swap_base_input`.
///
/// Args: `max_amount_in: u64`, `amount_out: u64`.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_base_output(
    pool: &Pubkey,
    amm_config: &Pubkey,
    authority: &Pubkey,
    observation: &Pubkey,
    input_vault: &Pubkey,
    output_vault: &Pubkey,
    accounts: &SwapAccounts,
    max_amount_in: u64,
    amount_out: u64,
) -> BotResult<Instruction> {
    if amount_out == 0 {
        return Err(BotError::invalid(
            "CPMM swap_base_output amount_out must be > 0",
        ));
    }
    if max_amount_in == 0 {
        return Err(BotError::invalid(
            "CPMM swap_base_output max_amount_in must be > 0",
        ));
    }
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT);
    data.extend_from_slice(&max_amount_in.to_le_bytes());
    data.extend_from_slice(&amount_out.to_le_bytes());

    Ok(Instruction {
        program_id: *RAYDIUM_CPMM,
        accounts: vec![
            // The IDL declares the payer as a signer but *not* writable, and
            // the program declares it as `Signer<'info>` (read-only). A
            // writable payer would ask the runtime to lock the trader's
            // account for lamports the swap never touches.
            AccountMeta::new_readonly(accounts.payer, true),
            AccountMeta::new_readonly(*authority, false),
            AccountMeta::new_readonly(*amm_config, false),
            AccountMeta::new(*pool, false),
            AccountMeta::new(accounts.input_token_account, false),
            AccountMeta::new(accounts.output_token_account, false),
            AccountMeta::new(*input_vault, false),
            AccountMeta::new(*output_vault, false),
            AccountMeta::new_readonly(accounts.input_token_program, false),
            AccountMeta::new_readonly(accounts.output_token_program, false),
            AccountMeta::new_readonly(accounts.input_token_mint, false),
            AccountMeta::new_readonly(accounts.output_token_mint, false),
            AccountMeta::new(*observation, false),
        ],
        data,
    })
}

/// Build the swap instruction for the given kind.
///
/// The single entry point the venue adapter uses; it keeps the account order
/// and the data layout in one place.
#[allow(clippy::too_many_arguments)]
pub fn build_swap(
    kind: SwapKind,
    pool: &Pubkey,
    amm_config: &Pubkey,
    authority: &Pubkey,
    observation: &Pubkey,
    input_vault: &Pubkey,
    output_vault: &Pubkey,
    accounts: &SwapAccounts,
    amount_in: u64,
    minimum_amount_out: u64,
) -> BotResult<Instruction> {
    match kind {
        SwapKind::BaseInput => build_swap_base_input(
            pool,
            amm_config,
            authority,
            observation,
            input_vault,
            output_vault,
            accounts,
            amount_in,
            minimum_amount_out,
        ),
        SwapKind::BaseOutput => build_swap_base_output(
            pool,
            amm_config,
            authority,
            observation,
            input_vault,
            output_vault,
            accounts,
            amount_in,
            minimum_amount_out,
        ),
    }
}

/// Derive the vault pair for a trade that spends `input_side`.
///
/// Returns `(input_vault, output_vault)` for the given pool state.
pub fn vault_pair(
    pool_state: &crate::venues::raydium_cpmm::accounts::PoolState,
    input_side: CpmmSide,
) -> (Pubkey, Pubkey) {
    let input_vault = pool_state.vault_of(input_side);
    let output_vault = pool_state.vault_of(match input_side {
        CpmmSide::Token0 => CpmmSide::Token1,
        CpmmSide::Token1 => CpmmSide::Token0,
    });
    (input_vault, output_vault)
}

/// The default CPMM account set for a trade, derived from a decoded pool.
///
/// This is the helper the adapter uses: it fills in the vaults, the token
/// programs, the mints and the observation account from the pool state, so a
/// caller only has to supply the trader's token accounts.
pub fn swap_accounts_for(
    pool_state: &crate::venues::raydium_cpmm::accounts::PoolState,
    payer: &Pubkey,
    input_side: CpmmSide,
    input_token_account: &Pubkey,
    output_token_account: &Pubkey,
) -> SwapAccounts {
    let output_side = match input_side {
        CpmmSide::Token0 => CpmmSide::Token1,
        CpmmSide::Token1 => CpmmSide::Token0,
    };
    SwapAccounts {
        payer: *payer,
        input_token_account: *input_token_account,
        output_token_account: *output_token_account,
        input_token_program: pool_state.token_program_of(input_side),
        output_token_program: pool_state.token_program_of(output_side),
        input_token_mint: pool_state.mint_of(input_side),
        output_token_mint: pool_state.mint_of(output_side),
    }
}

/// The pool's observation account, derived from the pool state.
pub fn observation_of(pool_state: &crate::venues::raydium_cpmm::accounts::PoolState) -> Pubkey {
    pool_state.observation_key
}

/// The pool's authority PDA, derived from the pool state's stored bump.
///
/// Source: `programs/cp-swap/src/instructions/swap_base_input.rs` — the
/// vault transfer signs with `&[AUTH_SEED.as_bytes(), &[pool_state.auth_bump]]`.
pub fn authority_of(
    pool_state: &crate::venues::raydium_cpmm::accounts::PoolState,
) -> BotResult<Pubkey> {
    let bump = [pool_state.auth_bump];
    Pubkey::create_program_address(&[RAYDIUM_CPMM_SEED_AUTH, &bump], &RAYDIUM_CPMM)
        .map_err(|e| BotError::solana(format!("derive CPMM pool authority: {e}")))
}

/// The canonical CPMM authority, which must match [`authority_of`] for a
/// healthy pool.
pub fn canonical_authority() -> Pubkey {
    pda::authority()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_accounts() -> SwapAccounts {
        SwapAccounts {
            payer: Pubkey::new_unique(),
            input_token_account: Pubkey::new_unique(),
            output_token_account: Pubkey::new_unique(),
            input_token_program: *TOKEN_PROGRAM,
            output_token_program: *TOKEN_2022_PROGRAM,
            input_token_mint: Pubkey::new_unique(),
            output_token_mint: Pubkey::new_unique(),
        }
    }

    #[test]
    fn swap_base_input_has_the_idl_account_order_and_flags() {
        let pool = Pubkey::new_unique();
        let amm_config = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let observation = Pubkey::new_unique();
        let input_vault = Pubkey::new_unique();
        let output_vault = Pubkey::new_unique();
        let accounts = sample_accounts();

        let ix = build_swap_base_input(
            &pool,
            &amm_config,
            &authority,
            &observation,
            &input_vault,
            &output_vault,
            &accounts,
            1_000,
            900,
        )
        .unwrap();

        assert_eq!(ix.program_id, *RAYDIUM_CPMM);
        assert_eq!(ix.accounts.len(), 13, "the IDL lists 13 accounts");
        assert_eq!(ix.accounts[0].pubkey, accounts.payer);
        // The official IDL and the Anchor source declare `payer` as a
        // signer-only, read-only account: the program never debits it, it
        // only signs the transaction.
        assert!(ix.accounts[0].is_signer && !ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, authority);
        assert!(!ix.accounts[1].is_writable && !ix.accounts[1].is_signer);
        assert_eq!(ix.accounts[2].pubkey, amm_config);
        assert!(!ix.accounts[2].is_writable);
        assert_eq!(ix.accounts[3].pubkey, pool);
        assert!(ix.accounts[3].is_writable);
        assert_eq!(ix.accounts[4].pubkey, accounts.input_token_account);
        assert!(ix.accounts[4].is_writable);
        assert_eq!(ix.accounts[5].pubkey, accounts.output_token_account);
        assert!(ix.accounts[5].is_writable);
        assert_eq!(ix.accounts[6].pubkey, input_vault);
        assert!(ix.accounts[6].is_writable);
        assert_eq!(ix.accounts[7].pubkey, output_vault);
        assert!(ix.accounts[7].is_writable);
        assert_eq!(ix.accounts[8].pubkey, accounts.input_token_program);
        assert!(!ix.accounts[8].is_writable);
        assert_eq!(ix.accounts[9].pubkey, accounts.output_token_program);
        assert!(!ix.accounts[9].is_writable);
        assert_eq!(ix.accounts[10].pubkey, accounts.input_token_mint);
        assert!(!ix.accounts[10].is_writable);
        assert_eq!(ix.accounts[11].pubkey, accounts.output_token_mint);
        assert!(!ix.accounts[11].is_writable);
        assert_eq!(ix.accounts[12].pubkey, observation);
        assert!(ix.accounts[12].is_writable);
    }

    #[test]
    fn swap_base_input_data_layout_is_exact() {
        let accounts = sample_accounts();
        let ix = build_swap_base_input(
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &accounts,
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
        )
        .unwrap();
        assert_eq!(ix.data.len(), 24);
        assert_eq!(&ix.data[..8], &RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT);
        assert_eq!(
            u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
        assert_eq!(
            u64::from_le_bytes(ix.data[16..24].try_into().unwrap()),
            0x1112_1314_1516_1718
        );
    }

    #[test]
    fn swap_base_output_data_layout_is_exact() {
        let accounts = sample_accounts();
        let ix = build_swap_base_output(
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &accounts,
            42,
            7,
        )
        .unwrap();
        assert_eq!(ix.accounts.len(), 13);
        assert_eq!(ix.data.len(), 24);
        assert_eq!(&ix.data[..8], &RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT);
        assert_eq!(u64::from_le_bytes(ix.data[8..16].try_into().unwrap()), 42);
        assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 7);
    }

    #[test]
    fn zero_amounts_are_refused() {
        let accounts = sample_accounts();
        let args = (
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
        );
        assert!(build_swap_base_input(
            args.0, args.1, args.2, args.3, args.4, args.5, &accounts, 0, 1
        )
        .is_err());
        assert!(build_swap_base_input(
            args.0, args.1, args.2, args.3, args.4, args.5, &accounts, 1, 0
        )
        .is_err());
        assert!(build_swap_base_output(
            args.0, args.1, args.2, args.3, args.4, args.5, &accounts, 0, 1
        )
        .is_err());
        assert!(build_swap_base_output(
            args.0, args.1, args.2, args.3, args.4, args.5, &accounts, 1, 0
        )
        .is_err());
    }

    #[test]
    fn token_2022_programs_flow_through() {
        let accounts = sample_accounts();
        assert_eq!(accounts.input_token_program, *TOKEN_PROGRAM);
        assert_eq!(accounts.output_token_program, *TOKEN_2022_PROGRAM);
        let ix = build_swap_base_input(
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &accounts,
            1,
            1,
        )
        .unwrap();
        assert_eq!(ix.accounts[8].pubkey, *TOKEN_PROGRAM);
        assert_eq!(ix.accounts[9].pubkey, *TOKEN_2022_PROGRAM);
    }

    #[test]
    fn build_swap_dispatches_on_the_kind() {
        let accounts = sample_accounts();
        let ix = build_swap(
            SwapKind::BaseInput,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &accounts,
            5,
            4,
        )
        .unwrap();
        assert_eq!(&ix.data[..8], &RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT);

        let ix = build_swap(
            SwapKind::BaseOutput,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &accounts,
            5,
            4,
        )
        .unwrap();
        assert_eq!(&ix.data[..8], &RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT);
    }
}
