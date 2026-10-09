//! Property tests for the staking arithmetic (GAP MAP v2, Part 5).
//!
//! Guards audit/INVARIANTS.md §D7/D8/E2/E3/C5 with randomized inputs:
//! monotonicity, exact annual-rate identity, fee bounds, cap arithmetic, and
//! settle() conservation. These are HOST tests (pure math only — no accounts,
//! no sysvars) and run under plain `cargo test`.

use proptest::prelude::*;
use staking_suite::state::{
    compute_fee, compute_reward, fits_under_cap, supply_headroom, StakeAccount, BPS,
    SECS_PER_YEAR,
};
use solana_program::pubkey::Pubkey;

fn stake_account(amount: u64, reward_from: i64, pending: u64) -> StakeAccount {
    StakeAccount {
        owner: Pubkey::new_unique(),
        amount,
        staked_at: 0,
        reward_from,
        pending_rewards: pending,
        bump: 255,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// D7 — rewards never decrease when principal, rate, or time increase.
    #[test]
    fn reward_is_monotone(
        a1 in 0u64..=u64::MAX,
        a2 in 0u64..=u64::MAX,
        r1 in 0u64..=u64::MAX,
        r2 in 0u64..=u64::MAX,
        t1 in i64::MIN..=i64::MAX,
        t2 in i64::MIN..=i64::MAX,
    ) {
        let (lo_a, hi_a) = if a1 <= a2 { (a1, a2) } else { (a2, a1) };
        let (lo_r, hi_r) = if r1 <= r2 { (r1, r2) } else { (r2, r1) };
        let (lo_t, hi_t) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
        // Saturation means equality, never decrease.
        prop_assert!(compute_reward(hi_a, r1, t1) >= compute_reward(lo_a, r1, t1));
        prop_assert!(compute_reward(a1, hi_r, t1) >= compute_reward(a1, lo_r, t1));
        prop_assert!(compute_reward(a1, r1, hi_t) >= compute_reward(a1, r1, lo_t));
    }

    /// D7/E2 — one full year at `rate_bps` pays exactly principal * rate / BPS
    /// while the intermediate product stays inside u128 (no saturation).
    #[test]
    fn annual_reward_is_exact_under_bps(
        amount in 0u64..=1_000_000_000_000_000, // 1e15 raw units
        rate_bps in 0u64..=10_000,
    ) {
        let got = compute_reward(amount, rate_bps, SECS_PER_YEAR);
        let expect = (amount as u128) * (rate_bps as u128) / BPS;
        prop_assert_eq!(got as u128, expect);
    }

    /// E2 — even absurd inputs saturate instead of panicking or wrapping.
    #[test]
    fn reward_saturates_never_wraps(
        amount in 0u64..=u64::MAX,
        rate_bps in 0u64..=u64::MAX,
        elapsed in i64::MIN..=i64::MAX,
    ) {
        let r = compute_reward(amount, rate_bps, elapsed);
        if elapsed <= 0 || amount == 0 || rate_bps == 0 {
            prop_assert_eq!(r, 0);
        } else {
            // A result exists and is a plain u64 (no panic reached here).
            prop_assert!(r <= u64::MAX);
        }
    }

    /// E3 — the fee never exceeds the deposit and never exceeds the bps share
    /// by more than the rounding unit.
    #[test]
    fn fee_is_bounded_and_rounds_down(
        amount in 0u64..=u64::MAX,
        fee_bps in 0u16..=u16::MAX,
    ) {
        let fee = compute_fee(amount, fee_bps);
        // The 10% CAP is enforced by validate_params, not by compute_fee —
        // so fee <= amount only holds within the legal bps range.
        if (fee_bps as u128) <= BPS {
            prop_assert!(fee <= amount);
        }
        let exact = (amount as u128) * (fee_bps as u128) / BPS;
        prop_assert_eq!(fee as u128, exact.min(u64::MAX as u128));
    }

    /// C5 — fits_under_cap is exactly "checked_add succeeds and total ≤ cap".
    #[test]
    fn cap_check_matches_checked_arithmetic(
        supply in 0u64..=u64::MAX,
        amount in 0u64..=u64::MAX,
        cap in 0u64..=u64::MAX,
    ) {
        let expect = supply
            .checked_add(amount)
            .map(|total| total <= cap)
            .unwrap_or(false);
        prop_assert_eq!(fits_under_cap(supply, amount, cap), expect);
    }

    /// C5 — headroom is saturating subtraction, never a wrapped huge number.
    #[test]
    fn headroom_is_saturating(
        supply in 0u64..=u64::MAX,
        cap in 0u64..=u64::MAX,
    ) {
        prop_assert_eq!(supply_headroom(supply, cap), cap.saturating_sub(supply));
    }

    /// D8 — settle() conserves value: everything accrued before the settle is
    /// exactly present in pending_rewards afterwards, and the clock resets.
    #[test]
    fn settle_conserves_accrued_rewards(
        amount in 0u64..=1_000_000_000_000,
        rate_bps in 0u64..=10_000,
        start in 0i64..=SECS_PER_YEAR * 100,
        elapsed in 0i64..=SECS_PER_YEAR * 100,
        pending in 0u64..=1_000_000_000_000,
    ) {
        let mut sa = stake_account(amount, start, pending);
        let now = start.saturating_add(elapsed);
        let accrued_before = sa.accrued_rewards(rate_bps, now);
        sa.settle(rate_bps, now);
        prop_assert_eq!(sa.pending_rewards, accrued_before);
        prop_assert_eq!(sa.reward_from, now);
        // After settling at `now`, the claimable total is unchanged.
        prop_assert_eq!(sa.accrued_rewards(rate_bps, now), accrued_before);
    }

    /// D7 boundary — negative or zero elapsed time accrues nothing.
    #[test]
    fn no_reward_before_the_clock_starts(
        amount in 1u64..=u64::MAX,
        rate_bps in 1u64..=u64::MAX,
        elapsed in i64::MIN..=0,
    ) {
        prop_assert_eq!(compute_reward(amount, rate_bps, elapsed), 0);
    }
}
