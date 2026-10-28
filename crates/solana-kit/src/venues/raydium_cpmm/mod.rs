//! Raydium CPMM (`cp-swap`) venue adapter.
//!
//! # What this is
//!
//! Raydium CPMM is the Anchor constant-product AMM Raydium ships as
//! `cp-swap`. A pool holds two mints, two vaults and an LP mint; swaps are
//! `swap_base_input` / `swap_base_output`; the pool quotes in hundredths of a
//! bip (10^-6), **not** in the 10 000 basis-point denominator the older
//! Raydium AMM v4 uses.
//!
//! # Provenance
//!
//! | What | Source |
//! |---|---|
//! | Program id, accounts, args, discriminators | `crates/solana-kit/idl/raydium_cp_swap.json` (official vendored IDL, `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`) |
//! | Curve + fee maths | `raydium-io/raydium-cp-swap` @ `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`, `programs/cp-swap/src/curve/**` |
//! | PDA seeds | the same commit, `programs/cp-swap/src/states/**` and `programs/cp-swap/src/lib.rs` |
//!
//! # Module map (one owner per concern)
//!
//! * [`pda`] — the **only** place CPMM PDAs are derived.
//! * [`accounts`] — the **only** place CPMM account bytes are decoded (and
//!   the synthetic encoders used by the fixtures).
//! * [`quote`] — the **only** place CPMM swap maths lives.
//! * [`instructions`] — the **only** place CPMM instructions are built.
//! * [`decode`] — CPMM instructions/events → the crate's neutral event types.
//! * [`adapter`] — the public face: detect, quote, build, pool lookup.
//!
//! # Usage
//!
//! ```
//! use solana_kit::venues::raydium_cpmm::accounts::{
//!     encode_synthetic_amm_config, AmmConfig, PoolState, SyntheticPoolState,
//! };
//! use solana_kit::venues::raydium_cpmm::{pda, CpmmPool, RaydiumCpmm};
//! use solana_sdk::pubkey::Pubkey;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let venue = RaydiumCpmm::new();
//! let raw_state = SyntheticPoolState {
//!     auth_bump: pda::authority_pda().1,
//!     ..SyntheticPoolState::default()
//! }
//! .encode();
//! let raw_config = encode_synthetic_amm_config(0, 2_500, 500, 0, 0);
//! let pool = CpmmPool {
//!     pool: Pubkey::new_unique(),
//!     state: PoolState::decode(&raw_state)?,
//!     config: AmmConfig::decode(&raw_config)?,
//!     token_0_vault_balance: 1_000_000_000,
//!     token_1_vault_balance: 2_000_000_000,
//! };
//!
//! // Quote a buy: spend quote_mint (token_1), receive base_mint (token_0).
//! let base_mint = pool.state.token_0_mint;
//! let quote_mint = pool.state.token_1_mint;
//! let expected_out = venue.quote(&pool, &quote_mint, 1_000_000)?;
//!
//! // Build the buy: spend the quote mint, receive the base mint.
//! let payer = Pubkey::new_unique();
//! let user_base_account = Pubkey::new_unique();
//! let user_quote_account = Pubkey::new_unique();
//! let ix = venue.build_buy_ix(
//!     &pool,
//!     &payer,
//!     &base_mint,
//!     &quote_mint,
//!     &user_base_account,
//!     &user_quote_account,
//!     1_000_000,
//!     expected_out,
//! )?;
//! assert_eq!(ix.accounts.len(), 13);
//! # Ok(()) }
//! ```

pub mod accounts;
pub mod adapter;
pub mod decode;
pub mod instructions;
pub mod pda;
pub mod quote;

pub use accounts::{
    AmmConfig, CpmmSide, CreatorFeeOn, Observation, ObservationState, PoolState, PoolStatus,
};
pub use adapter::{CpmmPool, RaydiumCpmm};
pub use decode::{
    to_decoded_swap, to_decoded_swap_from_instruction, to_pool_creation_event, CpmmPoolInitEvent,
    CpmmSwapEvent, CpmmSwapInstruction, CpmmTradeSide,
};
pub use instructions::{
    build_swap, build_swap_base_input, build_swap_base_output, SwapAccounts, SwapKind,
};
pub use quote::{
    max_in_with_slippage_bps, min_out_with_slippage_bps, swap_base_input, swap_base_output,
    FeeRates, QuoteSide, SwapResult,
};
