//! The Raydium CPMM (`cp-swap`) venue adapter.
//!
//! This type is the **public face** of the CPMM venue: it owns detection,
//! quoting, instruction building and pool lookup. The maths live in
//! [`crate::venues::raydium_cpmm::quote`], the account decoding in
//! [`crate::venues::raydium_cpmm::accounts`], the PDAs in
//! [`crate::venues::raydium_cpmm::pda`] and the instruction builders in
//! [`crate::venues::raydium_cpmm::instructions`] — this file only orchestrates
//! them.
//!
//! # Layout verification status
//!
//! [`VenueAdapter::layout_verified`] returns **`true`**: the CPMM layouts are
//! taken from the official vendored IDL
//! (`crates/solana-kit/idl/raydium_cp_swap.json`, upstream
//! `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`)
//! and the official program source
//! (`raydium-io/raydium-cp-swap` @ `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`),
//! and every discriminator is asserted against both.
//!
//! What is **not** claimed: a mainnet-validated round trip. The fixtures in
//! `crates/solana-kit/fixtures/raydium/` are synthetic, and no live swap has
//! been simulated against the real program by this batch. See
//! `docs/gap-updates/batch-03-rows.md`.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;
use crate::venues::raydium_cpmm::accounts::{AmmConfig, CpmmSide, PoolState};
use crate::venues::raydium_cpmm::instructions::{self, SwapAccounts, SwapKind};
use crate::venues::raydium_cpmm::pda;
use crate::venues::raydium_cpmm::quote::{self, FeeRates, QuoteSide};
use crate::venues::{VenueAdapter, VenueError, VenueSwapRequest};

/// The direction of a CPMM trade.
///
/// The CPMM program has no buy/sell instruction; the direction is taken from
/// the vault ordering. This type makes the intent explicit at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapDirection {
    /// Spend the quote mint, receive the base mint.
    Buy,
    /// Spend the base mint, receive the quote mint.
    Sell,
}

/// A CPMM pool plus the market state needed to trade it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpmmPool {
    /// The pool account address.
    pub pool: Pubkey,
    /// The decoded pool state.
    pub state: PoolState,
    /// The decoded `AmmConfig`.
    pub config: AmmConfig,
    /// The token_0 vault balance (raw).
    pub token_0_vault_balance: u64,
    /// The token_1 vault balance (raw).
    pub token_1_vault_balance: u64,
}

impl CpmmPool {
    /// The tradable reserves, oriented so that `token_0` comes first.
    ///
    /// The vault balances include the accrued protocol/fund/creator fees, which
    /// are owed and not tradable — the program subtracts them before applying
    /// the curve, and so must we.
    pub fn reserves(&self) -> BotResult<(u64, u64)> {
        self.state
            .vault_amount_without_fee(self.token_0_vault_balance, self.token_1_vault_balance)
    }

    /// The fee rates that apply to this pool.
    pub fn fee_rates(&self, input_side: CpmmSide) -> FeeRates {
        FeeRates {
            trade_fee_rate: self.config.trade_fee_rate,
            creator_fee_rate: self
                .state
                .effective_creator_fee_rate(self.config.creator_fee_rate),
            protocol_fee_rate: self.config.protocol_fee_rate,
            fund_fee_rate: self.config.fund_fee_rate,
            creator_fee_on_input: self.state.creator_fee_on_input(input_side),
        }
    }

    /// Which side the given mint is on.
    pub fn side_of(&self, mint: &Pubkey) -> Option<CpmmSide> {
        self.state.side_of(mint)
    }
}

/// The Raydium CPMM venue adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct RaydiumCpmm {
    enabled: bool,
}

impl RaydiumCpmm {
    /// An enabled adapter.
    pub fn new() -> Self {
        RaydiumCpmm { enabled: true }
    }

    /// A config-driven constructor.
    pub fn with_enabled(enabled: bool) -> Self {
        RaydiumCpmm { enabled }
    }

    /// `true` when `account_owner` is the CPMM program.
    ///
    /// This is the layout-free ownership fact: a pool is an account owned by
    /// the CPMM program, whatever its field layout.
    pub fn is_pool_owner(&self, account_owner: &Pubkey) -> bool {
        account_owner == &*RAYDIUM_CPMM
    }

    /// Detect whether a raw account is a CPMM pool and decode it.
    ///
    /// The check is the Anchor discriminator plus the exact length, so a
    /// different CPMM account (a config, an observation) is *not* mistaken for
    /// a pool.
    pub fn detect_pool(data: &[u8]) -> Option<PoolState> {
        if data.len() != RAYDIUM_CPMM_POOL_STATE_LEN {
            return None;
        }
        PoolState::decode(data).ok()
    }

    /// Decode a raw CPMM pool account, failing loudly on a mismatch.
    pub fn decode_pool(data: &[u8]) -> BotResult<PoolState> {
        PoolState::decode(data)
    }

    /// Decode a raw CPMM `AmmConfig` account, failing loudly on a mismatch.
    pub fn decode_amm_config(data: &[u8]) -> BotResult<AmmConfig> {
        AmmConfig::decode(data)
    }

    /// Quote an exact-input trade.
    ///
    /// `input_mint` is the mint the user spends. The returned amount is in the
    /// *output* mint's raw units.
    pub fn quote(&self, pool: &CpmmPool, input_mint: &Pubkey, amount_in: u64) -> BotResult<u64> {
        let side = pool.side_of(input_mint).ok_or_else(|| {
            BotError::invalid(format!(
                "mint {input_mint} is not part of CPMM pool {}",
                pool.pool
            ))
        })?;
        let (reserve_in, reserve_out) = self.reserves_oriented(pool, side)?;
        let rates = pool.fee_rates(side);
        let result = quote::swap_base_input(
            u128::from(amount_in),
            u128::from(reserve_in),
            u128::from(reserve_out),
            rates,
        )?;
        u64::try_from(result.output_amount)
            .map_err(|_| BotError::solana("CPMM quote output exceeds the u64 range"))
    }

    /// Quote an exact-output trade.
    ///
    /// `output_mint` is the mint the user receives. The returned amount is in
    /// the *input* mint's raw units.
    pub fn quote_exact_out(
        &self,
        pool: &CpmmPool,
        output_mint: &Pubkey,
        amount_out: u64,
    ) -> BotResult<u64> {
        let side = pool.side_of(output_mint).ok_or_else(|| {
            BotError::invalid(format!(
                "mint {output_mint} is not part of CPMM pool {}",
                pool.pool
            ))
        })?;
        // The input side is the opposite of the output side.
        let input_side = match side {
            CpmmSide::Token0 => CpmmSide::Token1,
            CpmmSide::Token1 => CpmmSide::Token0,
        };
        let (reserve_in, reserve_out) = self.reserves_oriented(pool, input_side)?;
        let rates = pool.fee_rates(input_side);
        let result = quote::swap_base_output(
            u128::from(amount_out),
            u128::from(reserve_in),
            u128::from(reserve_out),
            rates,
        )?;
        u64::try_from(result.input_amount)
            .map_err(|_| BotError::solana("CPMM quote input exceeds the u64 range"))
    }

    /// The reserves oriented as `(input, output)` for a trade that spends
    /// `side`.
    fn reserves_oriented(&self, pool: &CpmmPool, side: CpmmSide) -> BotResult<(u64, u64)> {
        let (r0, r1) = pool.reserves()?;
        Ok(match side {
            CpmmSide::Token0 => (r0, r1),
            CpmmSide::Token1 => (r1, r0),
        })
    }

    /// The `AmmConfig` a pool belongs to, read from the pool state.
    pub fn amm_config_of(pool: &CpmmPool) -> Pubkey {
        pool.state.amm_config
    }

    /// The pool's observation account.
    pub fn observation_of(pool: &CpmmPool) -> Pubkey {
        pool.state.observation_key
    }

    /// The pool's authority PDA.
    pub fn authority_of(pool: &CpmmPool) -> BotResult<Pubkey> {
        instructions::authority_of(&pool.state)
    }

    /// Build the buy instruction: spend the quote mint, receive the base mint.
    ///
    /// `base_mint` is the token being bought; `quote_mint` is the token being
    /// spent. The CPMM program has no buy/sell instruction — the direction
    /// comes from the *vault* ordering — so the buy is expressed as
    /// "the quote side is the input".
    #[allow(clippy::too_many_arguments)]
    pub fn build_buy_ix(
        &self,
        pool: &CpmmPool,
        user: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        user_base_token_account: &Pubkey,
        user_quote_token_account: &Pubkey,
        amount_in: u64,
        min_out: u64,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        self.build_swap_ix(
            pool,
            user,
            base_mint,
            quote_mint,
            user_base_token_account,
            user_quote_token_account,
            amount_in,
            min_out,
            SwapKind::BaseInput,
            SwapDirection::Buy,
        )
    }

    /// Build the sell instruction: spend the base mint, receive the quote mint.
    #[allow(clippy::too_many_arguments)]
    pub fn build_sell_ix(
        &self,
        pool: &CpmmPool,
        user: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        user_base_token_account: &Pubkey,
        user_quote_token_account: &Pubkey,
        amount_in: u64,
        min_out: u64,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        self.build_swap_ix(
            pool,
            user,
            base_mint,
            quote_mint,
            user_base_token_account,
            user_quote_token_account,
            amount_in,
            min_out,
            SwapKind::BaseInput,
            SwapDirection::Sell,
        )
    }

    /// Build a swap instruction for the given direction and sizing.
    ///
    /// The CPMM program takes the direction from the *vault* pair, not from a
    /// flag: a buy spends the quote vault and receives the base vault, and a
    /// sell does the opposite. [`SwapKind`] only says whether the amount is
    /// the exact input (`swap_base_input`) or the exact output
    /// (`swap_base_output`).
    #[allow(clippy::too_many_arguments)]
    pub fn build_swap_ix(
        &self,
        pool: &CpmmPool,
        user: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        user_base_token_account: &Pubkey,
        user_quote_token_account: &Pubkey,
        amount_in: u64,
        min_out: u64,
        kind: SwapKind,
        direction: SwapDirection,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        let base_side = pool.side_of(base_mint).ok_or_else(|| {
            BotError::invalid(format!(
                "base mint {base_mint} is not part of CPMM pool {}",
                pool.pool
            ))
        })?;
        let quote_side = pool.side_of(quote_mint).ok_or_else(|| {
            BotError::invalid(format!(
                "quote mint {quote_mint} is not part of CPMM pool {}",
                pool.pool
            ))
        })?;
        if base_side == quote_side {
            return Err(BotError::invalid(
                "CPMM base and quote mints must be the two different sides of the pool",
            ));
        }
        // A buy spends the quote side; a sell spends the base side.
        let (input_side, input_account, output_account) = match direction {
            SwapDirection::Buy => (
                quote_side,
                user_quote_token_account,
                user_base_token_account,
            ),
            SwapDirection::Sell => (base_side, user_base_token_account, user_quote_token_account),
        };
        let (input_vault, output_vault) = instructions::vault_pair(&pool.state, input_side);
        let accounts = instructions::swap_accounts_for(
            &pool.state,
            user,
            input_side,
            input_account,
            output_account,
        );
        let authority = instructions::authority_of(&pool.state)?;
        instructions::build_swap(
            kind,
            &pool.pool,
            &pool.state.amm_config,
            &authority,
            &pool.state.observation_key,
            &input_vault,
            &output_vault,
            &accounts,
            amount_in,
            min_out,
        )
    }

    /// Build the sell instruction with the explicit direction (spend the base
    /// mint).
    ///
    /// [`build_sell_ix`](Self::build_sell_ix) is the ergonomic entry point;
    /// this one spells out the direction so a caller can never accidentally
    /// build a buy when it meant a sell.
    #[allow(clippy::too_many_arguments)]
    pub fn build_sell_ix_directed(
        &self,
        pool: &CpmmPool,
        user: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        user_base_token_account: &Pubkey,
        user_quote_token_account: &Pubkey,
        amount_in: u64,
        min_out: u64,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        self.build_swap_ix(
            pool,
            user,
            base_mint,
            quote_mint,
            user_base_token_account,
            user_quote_token_account,
            amount_in,
            min_out,
            SwapKind::BaseInput,
            SwapDirection::Sell,
        )
    }

    /// Build the buy instruction with the explicit direction (spend the quote
    /// mint).
    #[allow(clippy::too_many_arguments)]
    pub fn build_buy_ix_directed(
        &self,
        pool: &CpmmPool,
        user: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        user_base_token_account: &Pubkey,
        user_quote_token_account: &Pubkey,
        amount_in: u64,
        min_out: u64,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        self.build_swap_ix(
            pool,
            user,
            base_mint,
            quote_mint,
            user_base_token_account,
            user_quote_token_account,
            amount_in,
            min_out,
            SwapKind::BaseInput,
            SwapDirection::Buy,
        )
    }

    /// The PDA of the pool for a given config and mint pair.
    pub fn pool_pda(amm_config: &Pubkey, mint_a: &Pubkey, mint_b: &Pubkey) -> Pubkey {
        pda::pool_pda_ordered(amm_config, mint_a, mint_b).0
    }
}

impl VenueAdapter for RaydiumCpmm {
    fn id(&self) -> &'static str {
        "raydium_cpmm"
    }

    fn label(&self) -> &'static str {
        "Raydium CPMM"
    }

    fn program_id(&self) -> &Pubkey {
        &RAYDIUM_CPMM
    }

    fn layout_verified(&self) -> bool {
        // The CPMM layouts come from the official vendored IDL
        // (`idl/raydium_cp_swap.json`) and the official program source
        // (`raydium-io/raydium-cp-swap` @ f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5).
        // Every discriminator and every account length is asserted in
        // `tests/raydium_cpmm_discriminators.rs` and
        // `tests/raydium_cpmm_accounts.rs`.
        true
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    /// Build a swap through the neutral venue request.
    ///
    /// The neutral request does not carry the vault balances, so the pool must
    /// be supplied through [`RaydiumCpmm::build_buy_ix`] /
    /// [`RaydiumCpmm::build_sell_ix`] instead. This override exists so the
    /// generic trait path fails with a *precise* error rather than the
    /// generic "unverified" one.
    fn build_swap(&self, _request: &VenueSwapRequest) -> BotResult<Vec<u8>> {
        Err(VenueError::NoPool.into_bot_error(self.id()))
    }
}

/// The trade direction of a CPMM pool, exposed for the migration handoff.
pub fn direction_of(base_side: CpmmSide) -> QuoteSide {
    match base_side {
        CpmmSide::Token0 => QuoteSide::Token0ToToken1,
        CpmmSide::Token1 => QuoteSide::Token1ToToken0,
    }
}

/// The swap account set for a trade, exposed so callers can inspect it.
pub fn accounts_for(
    pool_state: &PoolState,
    payer: &Pubkey,
    input_side: CpmmSide,
    input_token_account: &Pubkey,
    output_token_account: &Pubkey,
) -> SwapAccounts {
    instructions::swap_accounts_for(
        pool_state,
        payer,
        input_side,
        input_token_account,
        output_token_account,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::raydium_cpmm::accounts::SyntheticPoolState;

    fn sample_pool() -> CpmmPool {
        let s = SyntheticPoolState {
            token_0_mint: Pubkey::new_unique(),
            token_1_mint: Pubkey::new_unique(),
            token_0_vault: Pubkey::new_unique(),
            token_1_vault: Pubkey::new_unique(),
            auth_bump: crate::venues::raydium_cpmm::pda::authority_pda().1,
            ..Default::default()
        };
        let state = PoolState::decode(&s.encode()).unwrap();
        let config = AmmConfig::decode(
            &crate::venues::raydium_cpmm::accounts::encode_synthetic_amm_config(
                0, 2_500, 500, 0, 0,
            ),
        )
        .unwrap();
        CpmmPool {
            pool: Pubkey::new_unique(),
            state,
            config,
            token_0_vault_balance: 1_000_000_000,
            token_1_vault_balance: 2_000_000_000,
        }
    }

    #[test]
    fn pool_owner_check_matches_the_pinned_program() {
        let venue = RaydiumCpmm::new();
        assert!(venue.is_pool_owner(&RAYDIUM_CPMM));
        assert!(!venue.is_pool_owner(&Pubkey::new_unique()));
    }

    #[test]
    fn disabled_adapter_reports_disabled() {
        assert!(!RaydiumCpmm::with_enabled(false).enabled());
        assert!(RaydiumCpmm::with_enabled(true).enabled());
    }

    #[test]
    fn detect_pool_accepts_a_pool_and_rejects_anything_else() {
        let s = SyntheticPoolState::default();
        let bytes = s.encode();
        assert!(RaydiumCpmm::detect_pool(&bytes).is_some());
        // A truncated account is not a pool.
        assert!(RaydiumCpmm::detect_pool(&bytes[..100]).is_none());
        // An AmmConfig is not a pool.
        let cfg = crate::venues::raydium_cpmm::accounts::encode_synthetic_amm_config(0, 1, 0, 0, 0);
        assert!(RaydiumCpmm::detect_pool(&cfg).is_none());
        // Random bytes are not a pool.
        assert!(RaydiumCpmm::detect_pool(&[0u8; RAYDIUM_CPMM_POOL_STATE_LEN]).is_none());
    }

    #[test]
    fn decode_pool_is_fail_closed() {
        let s = SyntheticPoolState::default();
        let mut bytes = s.encode();
        bytes[0] ^= 0xff;
        assert!(RaydiumCpmm::decode_pool(&bytes).is_err());
    }

    #[test]
    fn quote_matches_the_manual_calculation() {
        let pool = sample_pool();
        let venue = RaydiumCpmm::new();
        let input = pool.state.token_0_mint;
        let out = venue.quote(&pool, &input, 1_000_000).unwrap();
        // Trade fee 2_500 -> curve input 997_500, reserves 1e9 / 2e9.
        // floor(997_500 * 2_000_000_000 / (1_000_000_000 + 997_500)) = 1_993_011.
        assert_eq!(out, 1_993_011);
    }

    #[test]
    fn quote_refuses_an_unknown_mint() {
        let pool = sample_pool();
        let venue = RaydiumCpmm::new();
        assert!(venue.quote(&pool, &Pubkey::new_unique(), 1).is_err());
    }

    #[test]
    fn quote_exact_out_is_consistent_with_quote() {
        let pool = sample_pool();
        let venue = RaydiumCpmm::new();
        let input = pool.state.token_0_mint;
        let out = venue.quote(&pool, &input, 1_000_000).unwrap();
        // Asking for exactly that much out must cost at most what the exact-in
        // quote produced (the two round differently, so the bound is one way).
        let cost = venue
            .quote_exact_out(&pool, &pool.state.token_1_mint, out)
            .unwrap();
        assert!(
            cost <= 1_000_000,
            "exact-out cost {cost} exceeds the exact-in quote"
        );
    }

    #[test]
    fn buy_and_sell_use_opposite_vaults() {
        let pool = sample_pool();
        let venue = RaydiumCpmm::new();
        let user = Pubkey::new_unique();
        let user_base = Pubkey::new_unique();
        let user_quote = Pubkey::new_unique();

        let buy = venue
            .build_buy_ix_directed(
                &pool,
                &user,
                &pool.state.token_0_mint,
                &pool.state.token_1_mint,
                &user_base,
                &user_quote,
                1_000,
                900,
            )
            .unwrap();
        // The buy spends the quote vault.
        assert_eq!(buy.accounts[6].pubkey, pool.state.token_1_vault);
        assert_eq!(buy.accounts[7].pubkey, pool.state.token_0_vault);
        assert_eq!(buy.accounts[4].pubkey, user_quote);
        assert_eq!(buy.accounts[5].pubkey, user_base);

        let sell = venue
            .build_sell_ix_directed(
                &pool,
                &user,
                &pool.state.token_0_mint,
                &pool.state.token_1_mint,
                &user_base,
                &user_quote,
                1_000,
                900,
            )
            .unwrap();
        // The sell spends the base vault.
        assert_eq!(sell.accounts[6].pubkey, pool.state.token_0_vault);
        assert_eq!(sell.accounts[7].pubkey, pool.state.token_1_vault);
        assert_eq!(sell.accounts[4].pubkey, user_base);
        assert_eq!(sell.accounts[5].pubkey, user_quote);
    }

    #[test]
    fn building_refuses_mints_outside_the_pool() {
        let pool = sample_pool();
        let venue = RaydiumCpmm::new();
        let user = Pubkey::new_unique();
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        assert!(venue
            .build_buy_ix_directed(
                &pool,
                &user,
                &Pubkey::new_unique(),
                &pool.state.token_1_mint,
                &a,
                &b,
                1,
                1
            )
            .is_err());
        assert!(venue
            .build_buy_ix_directed(
                &pool,
                &user,
                &pool.state.token_0_mint,
                &pool.state.token_0_mint,
                &a,
                &b,
                1,
                1
            )
            .is_err());
    }

    #[test]
    fn the_neutral_trait_path_reports_no_pool() {
        let venue = RaydiumCpmm::new();
        let req = VenueSwapRequest {
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            buy: true,
            amount_in: 1,
            min_out: 1,
        };
        let err = venue.build_swap(&req).unwrap_err();
        assert!(err.to_string().contains("no pool"), "{err}");
    }
}
