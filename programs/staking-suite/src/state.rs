//! On-chain program state: the global [`Config`] and each staker's
//! [`StakeAccount`].
//!
//! Both are borsh-serialised into program-owned accounts. PDAs:
//! * config — `["staking-config"]`
//! * stake  — `["staking-stake", staker]`
//! * vault  — the associated token account of the config PDA for the mint.
//! * token metadata — derived by the mpl-token-metadata program itself
//!   (`["metadata", metadata_program, mint]` under the metadata program);
//!   [`metadata_pda`] mirrors that derivation for this program's checks.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::{pubkey, Pubkey};

/// Seed for the global config PDA.
pub const CONFIG_SEED: &[u8] = b"staking-config";
/// Seed prefix for per-user stake PDAs.
pub const STAKE_SEED: &[u8] = b"staking-stake";

/// Seconds in a year, for annualising the reward rate.
pub const SECS_PER_YEAR: i64 = 365 * 24 * 60 * 60;
/// Basis-point denominator.
pub const BPS: u128 = 10_000;

/// Hard cap on the deposit fee, in basis points (10%). A higher fee would let
/// the admin confiscate deposits, so `initialize`/`update_params` reject it.
pub const MAX_FEE_BPS: u16 = 1_000;
/// Hard cap on the annual reward rate, in basis points (100% APR). Rewards are
/// minted, so an uncapped rate would let a compromised admin inflate the token
/// without limit; `initialize`/`update_params` reject anything above this.
pub const MAX_REWARD_RATE_BPS: u64 = 10_000;
/// Upper bound on the parameter timelock delay (30 days). `0` is allowed (no
/// delay) but a production deployment SHOULD use at least 24h so users can
/// review queued changes and exit before they take effect.
pub const MAX_TIMELOCK_SECS: i64 = 30 * 24 * 60 * 60;

/// The mpl-token-metadata program (Metaplex) on Solana mainnet/devnet — the
/// same address on both clusters.
pub const TOKEN_METADATA_PROGRAM_ID: Pubkey =
    pubkey!("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
/// Seed prefix the metadata program uses for its PDA: `["metadata", program, mint]`.
pub const METADATA_SEED: &[u8] = b"metadata";
/// mpl-token-metadata enforces a 32-BYTE name; longer values are rejected
/// client-side AND by `create_token_metadata` so the CPI cannot fail late.
pub const METADATA_NAME_MAX_LEN: usize = 32;
/// mpl-token-metadata symbol limit (10 bytes).
pub const METADATA_SYMBOL_MAX_LEN: usize = 10;
/// mpl-token-metadata URI limit (200 bytes).
pub const METADATA_URI_MAX_LEN: usize = 200;

/// Global staking configuration (one per program).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Guard against re-initialisation.
    pub initialized: bool,
    /// Authority that can update parameters.
    pub admin: Pubkey,
    /// The reward/staking token mint (program is the mint authority).
    pub mint: Pubkey,
    /// Program token account holding all staked principal.
    pub vault: Pubkey,
    /// Token account that collects deposit fees.
    pub treasury: Pubkey,
    /// Deposit fee in basis points.
    pub fee_bps: u16,
    /// Annual reward rate in basis points (1000 == 10%).
    pub reward_rate_bps: u64,
    /// Minimum stake, in raw token units.
    pub min_stake: u64,
    /// Cooldown (seconds) before an unstake settles.
    pub unstake_delay: i64,
    /// Token decimals (informational).
    pub decimals: u8,
    /// Bump for the config PDA.
    pub config_bump: u8,
    /// Bump for the mint-authority (config PDA) used to mint rewards.
    pub mint_bump: u8,
    /// Emergency stop. When `true`, deposits (`stake`) are rejected. Withdrawals
    /// (`unstake`/`claim`) are *never* blocked by this flag, so the admin can
    /// halt new money during an incident but can never freeze user funds.
    pub paused: bool,
    /// Pending admin in a two-step transfer (all-zero pubkey when none is set).
    /// `transfer_admin` records it; `accept_admin` (signed by it) promotes it to
    /// `admin`. Prevents handing control to a wrong or unowned key.
    pub pending_admin: Pubkey,
    /// Delay (seconds) a queued parameter update must wait before it can be
    /// applied. Changeable only *through* a queued update, so shortening the
    /// delay is itself subject to the current delay.
    pub timelock_secs: i64,
    /// Parameter update queued via `UpdateParams`, awaiting timelock expiry.
    pub pending: PendingParams,
    /// One-time genesis mint latch. `GenesisMint` may succeed exactly once per
    /// deployment; after that the only minting left is reward accrual.
    pub genesis_done: bool,
    /// ABSOLUTE maximum total supply of the mint, in raw token units. Set once
    /// at `initialize` (must be > 0) and IMMUTABLE afterwards — deliberately
    /// not part of `UpdateParams`, so no admin action can ever raise it.
    ///
    /// Enforcement (in `processor`):
    /// * `GenesisMint` fails with `MaxSupplyExceeded` unless
    ///   `mint.supply + amount <= max_supply` (checked arithmetic);
    /// * reward minting (`unstake`/`claim`) is clamped to the remaining
    ///   headroom `max_supply - mint.supply`, because withdrawals must never
    ///   fail — once the cap is reached, further rewards simply cannot be
    ///   minted (the shortfall is logged on-chain via `msg!`).
    ///
    /// The cap is measured against the LIVE mint supply (the SPL mint account
    /// is the authoritative total), not a self-tracked counter, so no mint
    /// path can escape it.
    pub max_supply: u64,
}

/// A parameter change queued by the admin, published on-chain for the whole
/// timelock window before it can take effect. Values are resolved against the
/// live config at queue time, so `ApplyParams` is a pure copy + cap re-check.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct PendingParams {
    /// `false` when no update is queued (all other fields are then zero).
    pub active: bool,
    /// Unix timestamp when the update was queued.
    pub queued_at: i64,
    /// Queued deposit fee (bps).
    pub fee_bps: u16,
    /// Queued annual reward rate (bps).
    pub reward_rate_bps: u64,
    /// Queued minimum stake.
    pub min_stake: u64,
    /// Queued unstake cooldown (seconds).
    pub unstake_delay: i64,
    /// Queued new timelock delay (takes effect when this update is applied).
    pub timelock_secs: i64,
}

/// A staker's position.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct StakeAccount {
    /// The staker (also the PDA seed).
    pub owner: Pubkey,
    /// Principal currently staked (net of deposit fee), raw units.
    pub amount: u64,
    /// When the current principal was (last) staked.
    pub staked_at: i64,
    /// Live rewards accrue from this timestamp on `amount`.
    pub reward_from: i64,
    /// Rewards settled but not yet minted to the staker (folded in when the
    /// principal changes, so top-ups never lose accrued rewards).
    pub pending_rewards: u64,
    /// Bump for the stake PDA.
    pub bump: u8,
}

impl StakeAccount {
    /// Total claimable rewards up to `now`, in raw token units: the amount
    /// accrued on the live principal since `reward_from`, plus anything already
    /// settled into `pending_rewards`.
    pub fn accrued_rewards(&self, rate_bps: u64, now: i64) -> u64 {
        compute_reward(self.amount, rate_bps, now.saturating_sub(self.reward_from))
            .saturating_add(self.pending_rewards)
    }

    /// Settle live accrual into `pending_rewards` and reset the clock to `now`.
    /// Called before the principal changes (a top-up stake).
    pub fn settle(&mut self, rate_bps: u64, now: i64) {
        let accrued = compute_reward(self.amount, rate_bps, now.saturating_sub(self.reward_from));
        self.pending_rewards = self.pending_rewards.saturating_add(accrued);
        self.reward_from = now;
    }
}

/// Pure reward math, exposed for unit tests and off-chain estimation.
pub fn compute_reward(amount: u64, rate_bps: u64, elapsed_secs: i64) -> u64 {
    if amount == 0 || rate_bps == 0 || elapsed_secs <= 0 {
        return 0;
    }
    let numerator = (amount as u128)
        .checked_mul(rate_bps as u128)
        .and_then(|v| v.checked_mul(elapsed_secs as u128));
    let Some(numerator) = numerator else {
        return u64::MAX;
    };
    let denominator = BPS.saturating_mul(SECS_PER_YEAR as u128);
    (numerator / denominator).min(u64::MAX as u128) as u64
}

/// Deposit fee for `amount` at `fee_bps` (raw units, rounded down).
pub fn compute_fee(amount: u64, fee_bps: u16) -> u64 {
    if fee_bps == 0 {
        return 0;
    }
    ((amount as u128) * (fee_bps as u128) / BPS).min(u64::MAX as u128) as u64
}

/// Remaining mint headroom under the max-supply cap: how many raw units may
/// still be minted in total (genesis + all future rewards). Saturates at 0 —
/// an over-cap supply (impossible through this program, but defensive) yields
/// "no headroom", never a wrapped huge number.
pub fn supply_headroom(mint_supply: u64, max_supply: u64) -> u64 {
    max_supply.saturating_sub(mint_supply)
}

/// Exact-amount cap test for `GenesisMint`: `true` iff `supply + amount` is
/// representable and `<= max_supply`. Overflowing `u64` fails closed.
pub fn fits_under_cap(mint_supply: u64, amount: u64, max_supply: u64) -> bool {
    mint_supply
        .checked_add(amount)
        .is_some_and(|total| total <= max_supply)
}

/// Derive the config PDA and bump.
pub fn config_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CONFIG_SEED], program_id)
}

/// Derive a staker's PDA and bump.
pub fn stake_pda(program_id: &Pubkey, staker: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[STAKE_SEED, staker.as_ref()], program_id)
}

/// Derive the mpl-token-metadata PDA for `mint`:
/// `["metadata", TOKEN_METADATA_PROGRAM_ID, mint]` under the metadata program.
/// Mirrors the canonical Metaplex derivation; `create_token_metadata` rejects
/// any other address so the CPI can never be redirected.
pub fn metadata_pda(mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            METADATA_SEED,
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            mint.as_ref(),
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reward_is_zero_for_zero_inputs() {
        assert_eq!(compute_reward(0, 1000, 100), 0);
        assert_eq!(compute_reward(100, 0, 100), 0);
        assert_eq!(compute_reward(100, 1000, 0), 0);
        assert_eq!(compute_reward(100, 1000, -5), 0);
    }

    #[test]
    fn reward_scales_with_time() {
        // 1_000_000 raw at 10% APY for one full year ≈ 100_000 raw.
        let one_year = compute_reward(1_000_000, 1000, SECS_PER_YEAR);
        assert!((one_year as i64 - 100_000).abs() <= 1);
        let half_year = compute_reward(1_000_000, 1000, SECS_PER_YEAR / 2);
        assert!((half_year as i64 - 50_000).abs() <= 1);
    }

    #[test]
    fn reward_does_not_overflow() {
        // A huge principal for a huge duration must saturate, not panic.
        let r = compute_reward(u64::MAX, u64::MAX, SECS_PER_YEAR * 100);
        assert_eq!(r, u64::MAX);
    }

    #[test]
    fn fee_rounds_down() {
        assert_eq!(compute_fee(1_000_000, 100), 10_000); // 1%
        assert_eq!(compute_fee(999, 100), 9); // 0.999 -> 9
        assert_eq!(compute_fee(1_000_000, 0), 0);
    }

    #[test]
    fn stake_account_accrues_from_reward_from() {
        let sa = StakeAccount {
            owner: Pubkey::new_unique(),
            amount: 1_000_000,
            staked_at: 0,
            reward_from: 100,
            pending_rewards: 0,
            bump: 255,
        };
        // At now=100 no time has passed since reward_from.
        assert_eq!(sa.accrued_rewards(1000, 100), 0);
        // One year after reward_from.
        let r = sa.accrued_rewards(1000, 100 + SECS_PER_YEAR);
        assert!((r as i64 - 100_000).abs() <= 1);
    }

    #[test]
    fn settle_folds_accrual_into_pending() {
        let mut sa = StakeAccount {
            owner: Pubkey::new_unique(),
            amount: 1_000_000,
            staked_at: 0,
            reward_from: 0,
            pending_rewards: 0,
            bump: 255,
        };
        sa.settle(1000, SECS_PER_YEAR);
        // A year of accrual is now pending, and the clock reset.
        assert!((sa.pending_rewards as i64 - 100_000).abs() <= 1);
        assert_eq!(sa.reward_from, SECS_PER_YEAR);
        // Immediately after settling, nothing new has accrued.
        assert_eq!(sa.accrued_rewards(1000, SECS_PER_YEAR), sa.pending_rewards);
    }

    #[test]
    fn pending_rewards_add_to_accrual() {
        let sa = StakeAccount {
            owner: Pubkey::new_unique(),
            amount: 0,
            staked_at: 0,
            reward_from: 0,
            pending_rewards: 7,
            bump: 255,
        };
        assert_eq!(sa.accrued_rewards(1000, 1000), 7);
    }

    #[test]
    fn pda_derivation_is_deterministic() {
        let pid = Pubkey::new_unique();
        let user = Pubkey::new_unique();
        let (a, ba) = stake_pda(&pid, &user);
        let (b, bb) = stake_pda(&pid, &user);
        assert_eq!(a, b);
        assert_eq!(ba, bb);
        let (c1, _) = config_pda(&pid);
        let (c2, _) = config_pda(&pid);
        assert_eq!(c1, c2);
        assert_ne!(c1, a);
    }

    #[test]
    fn supply_headroom_saturates() {
        assert_eq!(supply_headroom(0, 1_000), 1_000);
        assert_eq!(supply_headroom(400, 1_000), 600);
        // Exact cap reached -> zero headroom (boundary).
        assert_eq!(supply_headroom(1_000, 1_000), 0);
        // Defensive: supply somehow above cap -> saturates to 0, never wraps.
        assert_eq!(supply_headroom(1_001, 1_000), 0);
        assert_eq!(supply_headroom(u64::MAX, 0), 0);
        assert_eq!(supply_headroom(0, u64::MAX), u64::MAX);
    }

    #[test]
    fn fits_under_cap_boundaries() {
        // Minting exactly to the cap is allowed.
        assert!(fits_under_cap(0, 1_000, 1_000));
        assert!(fits_under_cap(999, 1, 1_000));
        // One unit over the cap is rejected.
        assert!(!fits_under_cap(0, 1_001, 1_000));
        assert!(!fits_under_cap(1_000, 1, 1_000));
        assert!(!fits_under_cap(999, 2, 1_000));
        // u64 overflow in supply + amount fails closed.
        assert!(!fits_under_cap(u64::MAX, 1, u64::MAX));
        assert!(!fits_under_cap(u64::MAX, u64::MAX, u64::MAX));
        // Overflowing but "would fit" under a bigger type is still rejected.
        assert!(!fits_under_cap(u64::MAX, 2, u64::MAX));
        // Zero cap: nothing may ever mint.
        assert!(!fits_under_cap(0, 1, 0));
    }

    #[test]
    fn metadata_pda_matches_the_metaplex_derivation() {
        let mint = Pubkey::new_unique();
        let (a, ba) = metadata_pda(&mint);
        let (b, bb) = metadata_pda(&mint);
        assert_eq!(a, b);
        assert_eq!(ba, bb);
        // Independent re-derivation with literal seeds (the canonical mpl
        // formula) must agree.
        let (expect_key, expect_bump) = Pubkey::find_program_address(
            &[
                b"metadata",
                TOKEN_METADATA_PROGRAM_ID.as_ref(),
                mint.as_ref(),
            ],
            &TOKEN_METADATA_PROGRAM_ID,
        );
        assert_eq!(a, expect_key);
        assert_eq!(ba, expect_bump);
        // Different mints -> different metadata PDAs; and it is not one of
        // this program's own PDAs.
        let (other, _) = metadata_pda(&Pubkey::new_unique());
        assert_ne!(a, other);
        let (cfg_pda, _) = config_pda(&crate::id());
        let (stake, _) = stake_pda(&crate::id(), &mint);
        assert_ne!(a, cfg_pda);
        assert_ne!(a, stake);
    }

    #[test]
    fn metadata_program_id_is_the_canonical_mainnet_address() {
        assert_eq!(
            TOKEN_METADATA_PROGRAM_ID.to_string(),
            "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s"
        );
    }

    #[test]
    fn config_borsh_roundtrip() {
        let cfg = Config {
            initialized: true,
            admin: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            vault: Pubkey::new_unique(),
            treasury: Pubkey::new_unique(),
            fee_bps: 100,
            reward_rate_bps: 1000,
            min_stake: 1_000_000,
            unstake_delay: 3600,
            decimals: 6,
            config_bump: 254,
            mint_bump: 253,
            paused: false,
            pending_admin: Pubkey::default(),
            timelock_secs: 86_400,
            pending: PendingParams::default(),
            genesis_done: false,
            max_supply: 1_000_000_000_000,
        };
        let bytes = borsh::to_vec(&cfg).unwrap();
        let back = Config::try_from_slice(&bytes).unwrap();
        assert_eq!(cfg, back);
    }
}
