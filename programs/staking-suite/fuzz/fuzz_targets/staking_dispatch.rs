//! Fuzz target: for ANY byte stream that deserializes into a valid
//! `StakingInstruction`, dispatching it with ZERO accounts must fail with a
//! typed error — never panic (audit/INVARIANTS.md E1).
//!
//! This pins the property that every handler reads its account list through
//! `next_account_info`, which returns `NotEnoughAccountKeys` instead of
//! indexing. A future handler that indexes `accounts[N]` directly would panic
//! here and be caught immediately.
//!
//! cargo fuzz run staking_dispatch

#![no_main]

use libfuzzer_sys::fuzz_target;
use solana_program::pubkey::Pubkey;
use staking_suite::instruction::StakingInstruction;
use staking_suite::process_instruction;

fuzz_target!(|data: &[u8]| {
    // Only well-formed instruction enums reach the dispatcher (malformed ones
    // are covered by staking_deserialize).
    let Ok(ix) = StakingInstruction::unpack(data) else {
        return;
    };
    // Re-pack to a canonical encoding so this target is stable across borsh
    // versions' tolerance for trailing bytes.
    let packed = ix.pack().expect("packing an owned instruction cannot fail");

    let program_id = Pubkey::new_unique();
    let result = process_instruction(&program_id, &[], &packed);
    assert!(
        result.is_err(),
        "an instruction with zero accounts must be rejected, got: {result:?} for {ix:?}"
    );
});
