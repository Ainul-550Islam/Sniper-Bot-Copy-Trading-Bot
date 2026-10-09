//! Fuzz target: the staking arithmetic must never panic, wrap, or violate its
//! documented bounds for ANY input (audit/INVARIANTS.md C5, D7, E1–E3).
//!
//! cargo fuzz run staking_math

#![no_main]

use libfuzzer_sys::fuzz_target;
use staking_suite::state::{
    compute_fee, compute_reward, fits_under_cap, supply_headroom, StakeAccount, BPS,
};

fuzz_target!(|input: (u64, u64, i64, u16, u64, u64, u64)| {
    let (amount, rate_bps, elapsed_secs, fee_bps, supply, cap, pending) = input;

    // ---- reward math: never panics, saturates, is monotone in time ----
    let r = compute_reward(amount, rate_bps, elapsed_secs);
    if elapsed_secs <= 0 || amount == 0 || rate_bps == 0 {
        assert_eq!(r, 0, "non-positive elapsed time must accrue nothing");
    }
    // One more second can never LOWER the accrual (monotone in time).
    let r_next = compute_reward(amount, rate_bps, elapsed_secs.saturating_add(1));
    assert!(r_next >= r, "reward must be monotone in elapsed time");

    // ---- exact annual identity when intermediates fit in u128 ----
    if (amount as u128) * (rate_bps as u128) <= u128::MAX / 4 {
        let year = compute_reward(amount, rate_bps, 365 * 24 * 60 * 60);
        let expect = ((amount as u128) * (rate_bps as u128) / BPS).min(u64::MAX as u128) as u64;
        assert_eq!(year, expect, "one-year accrual must equal principal*rate/BPS");
    }

    // ---- fee: never above the deposit, rounds down ----
    let fee = compute_fee(amount, fee_bps);
    // compute_fee is pure math; the 10% cap lives in validate_params, so
    // fee <= amount holds exactly within the legal bps range.
    if (fee_bps as u128) <= BPS {
        assert!(fee <= amount, "fee can never exceed the deposit");
    }
    let exact = ((amount as u128) * (fee_bps as u128) / BPS).min(u64::MAX as u128) as u64;
    assert_eq!(fee, exact, "fee must be exact floor division");

    // ---- supply cap: checked arithmetic, fails closed ----
    let fits = fits_under_cap(supply, amount, cap);
    let expect = supply
        .checked_add(amount)
        .map(|total| total <= cap)
        .unwrap_or(false);
    assert_eq!(fits, expect, "fits_under_cap must equal checked_add semantics");

    // ---- headroom: saturating, never wraps ----
    assert_eq!(
        supply_headroom(supply, cap),
        cap.saturating_sub(supply),
        "headroom must saturate"
    );

    // ---- StakeAccount surfaces never panic on hostile inputs ----
    let sa = StakeAccount {
        owner: solana_program::pubkey::Pubkey::default(),
        amount,
        staked_at: elapsed_secs,
        reward_from: elapsed_secs.wrapping_neg(),
        pending_rewards: pending,
        bump: 255,
    };
    let accrued = sa.accrued_rewards(rate_bps, elapsed_secs);
    // accrued = live + pending (saturating) — never less than pending.
    assert!(accrued >= pending);
});
