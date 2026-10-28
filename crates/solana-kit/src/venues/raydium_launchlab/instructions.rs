//! Raydium LaunchLab (`launchpad`) instruction builders.
//!
//! # Provenance
//!
//! The account lists, the argument order and the writable/signer flags come
//! from the **official vendored IDL**
//! (`crates/solana-kit/idl/raydium_launchpad.json`), whose `instructions`
//! array lists the accounts in exactly the order Anchor expects them, each
//! with an `isMut` / `isSigner` flag.
//!
//! The **trailing** accounts are not in the IDL (the IDL stops at the
//! `program` account) but are required by the program at runtime. Their
//! order comes from Raydium's official SDK
//! (`raydium-io/raydium-sdk-V2` @ `f8549145afe75d4c486e30c43331c2c4da32b1eb`,
//! `src/raydium/launchpad/instrument.ts` → `buyExactInInstruction`):
//!
//! ```text
//!  … program
//!  [share_fee_receiver]      (only when a share fee is paid)
//!  system_program
//!  platform_claim_fee_vault
//!  creator_claim_fee_vault
//! ```
//!
//! # Data layout
//!
//! Anchor instructions are `discriminator (8 bytes) || borsh(args)`. All four
//! trade instructions take three `u64` arguments:
//!
//! | Instruction | Args |
//! |---|---|
//! | `buy_exact_in` | `amount_in`, `minimum_amount_out`, `share_fee_rate` |
//! | `buy_exact_out` | `amount_out`, `maximum_amount_in`, `share_fee_rate` |
//! | `sell_exact_in` | `amount_in`, `minimum_amount_out`, `share_fee_rate` |
//! | `sell_exact_out` | `amount_out`, `maximum_amount_in`, `share_fee_rate` |

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_system_interface::program as system_program;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;

/// Which LaunchLab trade instruction to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeKind {
    /// `buy_exact_in` — the quote input is fixed, the base output is floored.
    BuyExactIn,
    /// `buy_exact_out` — the base output is fixed, the quote input is ceiled.
    BuyExactOut,
    /// `sell_exact_in` — the base input is fixed, the quote output is floored.
    SellExactIn,
    /// `sell_exact_out` — the quote output is fixed, the base input is ceiled.
    SellExactOut,
}

impl TradeKind {
    /// The Anchor instruction name, as spelled in the IDL.
    pub fn instruction_name(self) -> &'static str {
        match self {
            TradeKind::BuyExactIn => "buy_exact_in",
            TradeKind::BuyExactOut => "buy_exact_out",
            TradeKind::SellExactIn => "sell_exact_in",
            TradeKind::SellExactOut => "sell_exact_out",
        }
    }

    /// The Anchor discriminator of the instruction.
    pub fn discriminator(self) -> [u8; 8] {
        match self {
            TradeKind::BuyExactIn => RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN,
            TradeKind::BuyExactOut => RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT,
            TradeKind::SellExactIn => RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN,
            TradeKind::SellExactOut => RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT,
        }
    }

    /// Whether the instruction is a buy (spends the quote token).
    pub fn is_buy(self) -> bool {
        matches!(self, TradeKind::BuyExactIn | TradeKind::BuyExactOut)
    }

    /// Whether the instruction pins the *output* amount.
    pub fn is_exact_out(self) -> bool {
        matches!(self, TradeKind::BuyExactOut | TradeKind::SellExactOut)
    }
}

/// The accounts every LaunchLab trade instruction needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeAccounts {
    /// The trader (signs).
    pub payer: Pubkey,
    /// The pool vault authority.
    pub authority: Pubkey,
    /// The `GlobalConfig` the pool follows.
    pub global_config: Pubkey,
    /// The `PlatformConfig` the pool follows.
    pub platform_config: Pubkey,
    /// The pool account.
    pub pool_state: Pubkey,
    /// The user's base token account.
    pub user_base_token: Pubkey,
    /// The user's quote token account.
    pub user_quote_token: Pubkey,
    /// The pool's base vault.
    pub base_vault: Pubkey,
    /// The pool's quote vault.
    pub quote_vault: Pubkey,
    /// The base mint.
    pub base_token_mint: Pubkey,
    /// The quote mint.
    pub quote_token_mint: Pubkey,
    /// The base token program.
    pub base_token_program: Pubkey,
    /// The quote token program.
    pub quote_token_program: Pubkey,
    /// The platform's claim-fee vault.
    pub platform_claim_fee_vault: Pubkey,
    /// The creator's claim-fee vault.
    pub creator_claim_fee_vault: Pubkey,
    /// The share-fee receiver (optional; only present when a share fee is
    /// paid to a specific wallet).
    pub share_fee_receiver: Option<Pubkey>,
}

/// The three `u64` arguments every LaunchLab trade instruction takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeArgs {
    /// For an exact-in trade: the input amount. For an exact-out trade: the
    /// output amount.
    pub amount: u64,
    /// For an exact-in trade: the minimum output. For an exact-out trade: the
    /// maximum input.
    pub limit: u64,
    /// The share fee rate, in hundredths of a bip.
    pub share_fee_rate: u64,
}

/// Build one of the four LaunchLab trade instructions.
///
/// The account order (IDL prefix + the SDK's trailing accounts):
///
/// ```text
///  0 payer               w/s    8 quote_vault          w
///  1 authority          ro     9 base_token_mint      ro
///  2 global_config      ro    10 quote_token_mint     ro
///  3 platform_config    ro    11 base_token_program   ro
///  4 pool_state         w     12 quote_token_program  ro
///  5 user_base_token    w     13 event_authority      ro
///  6 user_quote_token   w     14 program              ro
///  7 base_vault         w
///  [share_fee_receiver] w
///  system_program      ro
///  platform_claim_fee_vault  w
///  creator_claim_fee_vault   w
/// ```
pub fn build_trade_ix(
    kind: TradeKind,
    accounts: &TradeAccounts,
    args: TradeArgs,
) -> BotResult<Instruction> {
    if args.amount == 0 {
        return Err(BotError::invalid(format!(
            "LaunchLab {} amount must be > 0",
            kind.instruction_name()
        )));
    }
    if args.limit == 0 {
        return Err(BotError::invalid(format!(
            "LaunchLab {} limit must be > 0",
            kind.instruction_name()
        )));
    }
    if args.share_fee_rate >= RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR {
        return Err(BotError::invalid(
            "LaunchLab share_fee_rate must be below the fee denominator",
        ));
    }

    let mut data = Vec::with_capacity(32);
    data.extend_from_slice(&kind.discriminator());
    data.extend_from_slice(&args.amount.to_le_bytes());
    data.extend_from_slice(&args.limit.to_le_bytes());
    data.extend_from_slice(&args.share_fee_rate.to_le_bytes());

    let mut metas = vec![
        AccountMeta::new(accounts.payer, true),
        AccountMeta::new_readonly(accounts.authority, false),
        AccountMeta::new_readonly(accounts.global_config, false),
        AccountMeta::new_readonly(accounts.platform_config, false),
        AccountMeta::new(accounts.pool_state, false),
        AccountMeta::new(accounts.user_base_token, false),
        AccountMeta::new(accounts.user_quote_token, false),
        AccountMeta::new(accounts.base_vault, false),
        AccountMeta::new(accounts.quote_vault, false),
        AccountMeta::new_readonly(accounts.base_token_mint, false),
        AccountMeta::new_readonly(accounts.quote_token_mint, false),
        AccountMeta::new_readonly(accounts.base_token_program, false),
        AccountMeta::new_readonly(accounts.quote_token_program, false),
        AccountMeta::new_readonly(
            crate::venues::raydium_launchlab::pda::event_authority(),
            false,
        ),
        AccountMeta::new_readonly(*RAYDIUM_LAUNCHLAB, false),
    ];
    if let Some(receiver) = accounts.share_fee_receiver {
        metas.push(AccountMeta::new(receiver, false));
    }
    metas.push(AccountMeta::new_readonly(system_program::id(), false));
    metas.push(AccountMeta::new(accounts.platform_claim_fee_vault, false));
    metas.push(AccountMeta::new(accounts.creator_claim_fee_vault, false));

    Ok(Instruction {
        program_id: *RAYDIUM_LAUNCHLAB,
        accounts: metas,
        data,
    })
}

/// Build `buy_exact_in` — spend `amount_in` quote, receive at least
/// `minimum_amount_out` base.
pub fn build_buy_exact_in(
    accounts: &TradeAccounts,
    amount_in: u64,
    minimum_amount_out: u64,
    share_fee_rate: u64,
) -> BotResult<Instruction> {
    build_trade_ix(
        TradeKind::BuyExactIn,
        accounts,
        TradeArgs {
            amount: amount_in,
            limit: minimum_amount_out,
            share_fee_rate,
        },
    )
}

/// Build `buy_exact_out` — receive exactly `amount_out` base for at most
/// `maximum_amount_in` quote.
pub fn build_buy_exact_out(
    accounts: &TradeAccounts,
    amount_out: u64,
    maximum_amount_in: u64,
    share_fee_rate: u64,
) -> BotResult<Instruction> {
    build_trade_ix(
        TradeKind::BuyExactOut,
        accounts,
        TradeArgs {
            amount: amount_out,
            limit: maximum_amount_in,
            share_fee_rate,
        },
    )
}

/// Build `sell_exact_in` — sell `amount_in` base for at least
/// `minimum_amount_out` quote.
pub fn build_sell_exact_in(
    accounts: &TradeAccounts,
    amount_in: u64,
    minimum_amount_out: u64,
    share_fee_rate: u64,
) -> BotResult<Instruction> {
    build_trade_ix(
        TradeKind::SellExactIn,
        accounts,
        TradeArgs {
            amount: amount_in,
            limit: minimum_amount_out,
            share_fee_rate,
        },
    )
}

/// Build `sell_exact_out` — receive exactly `amount_out` quote for at most
/// `maximum_amount_in` base.
pub fn build_sell_exact_out(
    accounts: &TradeAccounts,
    amount_out: u64,
    maximum_amount_in: u64,
    share_fee_rate: u64,
) -> BotResult<Instruction> {
    build_trade_ix(
        TradeKind::SellExactOut,
        accounts,
        TradeArgs {
            amount: amount_out,
            limit: maximum_amount_in,
            share_fee_rate,
        },
    )
}

/// The minimum output a trade must deliver, given an expected output and a
/// slippage tolerance in basis points. The bound is floored.
pub fn min_out_with_slippage_bps(expected_out: u64, slippage_bps: u64) -> BotResult<u64> {
    if slippage_bps >= 10_000 {
        return Err(BotError::invalid(
            "LaunchLab slippage must be below 10000 basis points",
        ));
    }
    let numerator = u128::from(expected_out)
        .checked_mul(u128::from(10_000u64.saturating_sub(slippage_bps)))
        .ok_or_else(|| BotError::solana("LaunchLab slippage multiplication overflow"))?;
    Ok((numerator / 10_000) as u64)
}

/// The maximum input a trade may spend, given an expected input and a slippage
/// tolerance in basis points. The bound is ceiled.
pub fn max_in_with_slippage_bps(expected_in: u64, slippage_bps: u64) -> BotResult<u64> {
    if slippage_bps >= 10_000 {
        return Err(BotError::invalid(
            "LaunchLab slippage must be below 10000 basis points",
        ));
    }
    let numerator = u128::from(expected_in)
        .checked_mul(u128::from(10_000u64.saturating_add(slippage_bps)))
        .ok_or_else(|| BotError::solana("LaunchLab slippage multiplication overflow"))?;
    let denominator = 10_000u128;
    let value = numerator
        .checked_add(denominator - 1)
        .ok_or_else(|| BotError::solana("LaunchLab slippage addition overflow"))?
        / denominator;
    u64::try_from(value)
        .map_err(|_| BotError::solana("LaunchLab maximum input exceeds the u64 range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accounts() -> TradeAccounts {
        TradeAccounts {
            payer: Pubkey::new_unique(),
            authority: Pubkey::new_unique(),
            global_config: Pubkey::new_unique(),
            platform_config: Pubkey::new_unique(),
            pool_state: Pubkey::new_unique(),
            user_base_token: Pubkey::new_unique(),
            user_quote_token: Pubkey::new_unique(),
            base_vault: Pubkey::new_unique(),
            quote_vault: Pubkey::new_unique(),
            base_token_mint: Pubkey::new_unique(),
            quote_token_mint: Pubkey::new_unique(),
            base_token_program: *TOKEN_PROGRAM,
            quote_token_program: *TOKEN_PROGRAM,
            platform_claim_fee_vault: Pubkey::new_unique(),
            creator_claim_fee_vault: Pubkey::new_unique(),
            share_fee_receiver: None,
        }
    }

    #[test]
    fn buy_exact_in_has_the_idl_account_order_and_flags() {
        let a = accounts();
        let ix = build_buy_exact_in(&a, 1_000, 900, 0).unwrap();
        assert_eq!(ix.program_id, *RAYDIUM_LAUNCHLAB);
        // 15 IDL accounts + system program + two fee vaults.
        assert_eq!(ix.accounts.len(), 18);
        assert_eq!(ix.accounts[0].pubkey, a.payer);
        assert!(ix.accounts[0].is_signer && ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, a.authority);
        assert!(!ix.accounts[1].is_writable);
        assert_eq!(ix.accounts[2].pubkey, a.global_config);
        assert_eq!(ix.accounts[3].pubkey, a.platform_config);
        assert_eq!(ix.accounts[4].pubkey, a.pool_state);
        assert!(ix.accounts[4].is_writable);
        assert_eq!(ix.accounts[5].pubkey, a.user_base_token);
        assert!(ix.accounts[5].is_writable);
        assert_eq!(ix.accounts[6].pubkey, a.user_quote_token);
        assert!(ix.accounts[6].is_writable);
        assert_eq!(ix.accounts[7].pubkey, a.base_vault);
        assert!(ix.accounts[7].is_writable);
        assert_eq!(ix.accounts[8].pubkey, a.quote_vault);
        assert!(ix.accounts[8].is_writable);
        assert_eq!(ix.accounts[9].pubkey, a.base_token_mint);
        assert!(!ix.accounts[9].is_writable);
        assert_eq!(ix.accounts[10].pubkey, a.quote_token_mint);
        assert_eq!(ix.accounts[11].pubkey, a.base_token_program);
        assert_eq!(ix.accounts[12].pubkey, a.quote_token_program);
        assert_eq!(
            ix.accounts[13].pubkey,
            crate::venues::raydium_launchlab::pda::event_authority()
        );
        assert_eq!(ix.accounts[14].pubkey, *RAYDIUM_LAUNCHLAB);
        // The trailing accounts.
        assert_eq!(ix.accounts[15].pubkey, system_program::id());
        assert_eq!(ix.accounts[16].pubkey, a.platform_claim_fee_vault);
        assert!(ix.accounts[16].is_writable);
        assert_eq!(ix.accounts[17].pubkey, a.creator_claim_fee_vault);
        assert!(ix.accounts[17].is_writable);
    }

    #[test]
    fn a_share_fee_receiver_is_inserted_before_the_trailing_accounts() {
        let mut a = accounts();
        let receiver = Pubkey::new_unique();
        a.share_fee_receiver = Some(receiver);
        let ix = build_buy_exact_in(&a, 1_000, 900, 100).unwrap();
        assert_eq!(ix.accounts.len(), 19);
        assert_eq!(ix.accounts[15].pubkey, receiver);
        assert!(ix.accounts[15].is_writable);
        assert_eq!(ix.accounts[16].pubkey, system_program::id());
        assert_eq!(ix.accounts[17].pubkey, a.platform_claim_fee_vault);
        assert_eq!(ix.accounts[18].pubkey, a.creator_claim_fee_vault);
    }

    #[test]
    fn the_data_layout_is_exact_for_every_instruction() {
        let a = accounts();
        let ix = build_buy_exact_in(&a, 11, 22, 33).unwrap();
        assert_eq!(ix.data.len(), 32);
        assert_eq!(&ix.data[..8], &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN);
        assert_eq!(u64::from_le_bytes(ix.data[8..16].try_into().unwrap()), 11);
        assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 22);
        assert_eq!(u64::from_le_bytes(ix.data[24..32].try_into().unwrap()), 33);

        let ix = build_buy_exact_out(&a, 11, 22, 33).unwrap();
        assert_eq!(&ix.data[..8], &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT);

        let ix = build_sell_exact_in(&a, 11, 22, 33).unwrap();
        assert_eq!(&ix.data[..8], &RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN);

        let ix = build_sell_exact_out(&a, 11, 22, 33).unwrap();
        assert_eq!(&ix.data[..8], &RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT);
    }

    #[test]
    fn zero_amounts_are_refused() {
        let a = accounts();
        assert!(build_buy_exact_in(&a, 0, 1, 0).is_err());
        assert!(build_buy_exact_in(&a, 1, 0, 0).is_err());
        assert!(build_sell_exact_in(&a, 0, 1, 0).is_err());
        assert!(build_sell_exact_out(&a, 0, 1, 0).is_err());
    }

    #[test]
    fn an_out_of_range_share_fee_rate_is_refused() {
        let a = accounts();
        assert!(build_buy_exact_in(&a, 1, 1, RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR).is_err());
        assert!(build_buy_exact_in(&a, 1, 1, u64::MAX).is_err());
    }

    #[test]
    fn the_kind_helpers_are_correct() {
        assert!(TradeKind::BuyExactIn.is_buy());
        assert!(TradeKind::BuyExactOut.is_buy());
        assert!(!TradeKind::SellExactIn.is_buy());
        assert!(!TradeKind::SellExactOut.is_buy());
        assert!(TradeKind::BuyExactOut.is_exact_out());
        assert!(TradeKind::SellExactOut.is_exact_out());
        assert!(!TradeKind::BuyExactIn.is_exact_out());
    }

    #[test]
    fn slippage_helpers_round_the_right_way() {
        assert_eq!(min_out_with_slippage_bps(1_000, 100).unwrap(), 990);
        assert_eq!(max_in_with_slippage_bps(1_000, 100).unwrap(), 1_010);
        assert!(min_out_with_slippage_bps(1_000, 10_000).is_err());
        assert!(max_in_with_slippage_bps(1_000, 10_000).is_err());
    }
}
