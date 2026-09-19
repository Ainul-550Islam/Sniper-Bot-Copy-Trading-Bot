//! Module 4 — `staking-suite`: a native Solana program (no Anchor) providing a
//! staking token with a deposit-fee and time-based reward system.
//!
//! Features:
//! * **Token** — `initialize` creates an SPL mint whose authority is the
//!   program's config PDA, so the program can mint rewards.
//! * **Fee system** — every `stake` splits the deposit: `fee_bps` to a treasury
//!   token account, the remainder to the staking vault.
//! * **Staking** — `stake` deposits, `unstake` withdraws principal + rewards
//!   after a cooldown, `claim` takes rewards early, `update_params` lets the
//!   admin tune the fee/rate/min/cooldown.
//! * **Max supply** — `initialize` records an IMMUTABLE `max_supply` cap (not
//!   changeable via `update_params`, so no admin action can raise it). Every
//!   mint is enforced against the LIVE mint supply: `genesis_mint` fails with
//!   `MaxSupplyExceeded` unless `supply + amount <= max_supply`, and reward
//!   minting is clamped to the remaining headroom (withdrawals never fail).
//! * **Token metadata** — `create_token_metadata` is a one-shot admin CPI to
//!   mpl-token-metadata (`CreateMetadataAccountsV3`) creating IMMUTABLE
//!   name/symbol/uri with the config PDA as update authority.
//!
//! Rewards accrue linearly: `amount * rate_bps * elapsed / (10_000 * secs/year)`.
//!
//! Build for BPF with `cargo build-bpf` (or `cargo build-sbf`) from this
//! directory. The `no-entrypoint` feature lets other crates import the
//! instruction builders and state types without pulling in the entrypoint.

#![forbid(unsafe_code)]

pub mod error;
pub mod instruction;
pub mod processor;
pub mod state;

use solana_program::declare_id;

// Placeholder program id — replace with the real one after `solana program deploy`:
// `solana address -k target/deploy/staking_suite-keypair.json`.
declare_id!("3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy");

/// Re-export the processor entry for host-side tests and clients.
pub use processor::process_instruction;

#[cfg(not(feature = "no-entrypoint"))]
use solana_program::entrypoint;

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process);

/// The BPF entrypoint.
#[cfg(not(feature = "no-entrypoint"))]
pub fn process(
    program_id: &solana_program::pubkey::Pubkey,
    accounts: &[solana_program::account_info::AccountInfo],
    instruction_data: &[u8],
) -> solana_program::entrypoint::ProgramResult {
    processor::process_instruction(program_id, accounts, instruction_data)
}
