//! Fuzz target: borsh deserialization must be panic-free on arbitrary bytes,
//! and every ACCEPTED value must round-trip exactly (audit/INVARIANTS.md E4/E5).
//!
//! cargo fuzz run staking_deserialize

#![no_main]

use borsh::{BorshDeserialize, BorshSerialize};
use libfuzzer_sys::fuzz_target;
use staking_suite::instruction::StakingInstruction;
use staking_suite::state::{Config, StakeAccount};

/// Deserialize `T` from arbitrary bytes: must never panic; if it accepts, the
/// value must re-serialize to the SAME canonical prefix of the input.
fn roundtrip<T>(data: &[u8])
where
    T: BorshDeserialize + BorshSerialize + PartialEq + std::fmt::Debug,
{
    if let Ok(value) = T::try_from_slice(data) {
        let encoded = borsh::to_vec(&value).expect("serializing an owned value cannot fail");
        let back = T::try_from_slice(&encoded).expect("own output must deserialize");
        assert_eq!(value, back, "borsh round-trip must be lossless");
        // Canonical form: re-encoding must reproduce the consumed prefix, so a
        // trailing-garbage variant can't re-encode to something different.
        assert!(
            data.starts_with(&encoded),
            "accepted bytes must be the canonical encoding prefix"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    // Instruction data (the live attack surface of the on-chain dispatcher).
    roundtrip::<StakingInstruction>(data);
    // On-chain state blobs (defense in depth: an attacker who could ever write
    // account data must not be able to craft a layout that deserializes into a
    // surprising value).
    roundtrip::<Config>(data);
    roundtrip::<StakeAccount>(data);

    // The program's own unpack wrapper must agree with raw borsh on
    // accept/reject (it maps IO errors to InvalidInstructionData).
    match (
        StakingInstruction::unpack(data),
        StakingInstruction::try_from_slice(data),
    ) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "unpack must agree with borsh"),
        (Err(_), Err(_)) => {}
        (a, b) => panic!("unpack/borsh disagreement: {a:?} vs {b:?}"),
    }
});
