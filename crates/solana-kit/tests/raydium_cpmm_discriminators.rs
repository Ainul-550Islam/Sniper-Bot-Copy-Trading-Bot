//! CPMM discriminator assertions.
//!
//! Every discriminator is checked twice:
//!
//! 1. against the **Anchor sighash convention** —
//!    `sha256("global:<snake_name>")[..8]` for instructions and
//!    `sha256("account:<Name>")[..8]` for accounts;
//! 2. against the **vendored official IDL**
//!    (`crates/solana-kit/idl/raydium_cp_swap.json`, upstream
//!    `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`).
//!
//! If either source ever changes, one of these tests fails loudly.

use sha2::{Digest, Sha256};
use solana_kit::consts::*;
use solana_kit::venues::raydium_cpmm::instructions::SwapKind;

/// The Anchor sighash: the first eight bytes of `sha256("global:<name>")`.
fn global(name: &str) -> [u8; 8] {
    let digest = Sha256::digest(format!("global:{name}").as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    out
}

/// The Anchor account sighash: `sha256("account:<Name>")[..8]`.
fn account(name: &str) -> [u8; 8] {
    let digest = Sha256::digest(format!("account:{name}").as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    out
}

/// Read the IDL's discriminator for an instruction.
fn idl_instruction(name: &str) -> Vec<u8> {
    let raw = include_str!("../idl/raydium_cp_swap.json");
    let doc: serde_json::Value = serde_json::from_str(raw).expect("the vendored IDL is valid JSON");
    let ix = doc["instructions"]
        .as_array()
        .expect("the IDL has instructions")
        .iter()
        .find(|ix| ix["name"] == name)
        .unwrap_or_else(|| panic!("the IDL has an instruction named {name}"));
    ix["discriminator"]
        .as_array()
        .expect("the instruction has a discriminator")
        .iter()
        .map(|v| v.as_u64().expect("the discriminator is a number") as u8)
        .collect()
}

/// Read the IDL's discriminator for an account.
fn idl_account(name: &str) -> Vec<u8> {
    let raw = include_str!("../idl/raydium_cp_swap.json");
    let doc: serde_json::Value = serde_json::from_str(raw).expect("the vendored IDL is valid JSON");
    let acc = doc["accounts"]
        .as_array()
        .expect("the IDL has accounts")
        .iter()
        .find(|acc| acc["name"] == name)
        .unwrap_or_else(|| panic!("the IDL has an account named {name}"));
    acc["discriminator"]
        .as_array()
        .expect("the account has a discriminator")
        .iter()
        .map(|v| v.as_u64().expect("the discriminator is a number") as u8)
        .collect()
}

#[test]
fn the_swap_instruction_discriminators_follow_the_anchor_sighash() {
    assert_eq!(RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT, global("swap_base_input"));
    assert_eq!(
        RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT,
        global("swap_base_output")
    );
}

#[test]
fn the_swap_instruction_discriminators_match_the_official_idl() {
    assert_eq!(
        RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT.to_vec(),
        idl_instruction("swap_base_input")
    );
    assert_eq!(
        RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT.to_vec(),
        idl_instruction("swap_base_output")
    );
}

#[test]
fn the_initialize_instruction_discriminators_follow_the_anchor_sighash() {
    assert_eq!(RAYDIUM_CPMM_DISC_INITIALIZE, global("initialize"));
    assert_eq!(
        RAYDIUM_CPMM_DISC_INITIALIZE_WITH_PERMISSION,
        global("initialize_with_permission")
    );
}

#[test]
fn the_initialize_instruction_discriminators_match_the_official_idl() {
    assert_eq!(
        RAYDIUM_CPMM_DISC_INITIALIZE.to_vec(),
        idl_instruction("initialize")
    );
    assert_eq!(
        RAYDIUM_CPMM_DISC_INITIALIZE_WITH_PERMISSION.to_vec(),
        idl_instruction("initialize_with_permission")
    );
}

#[test]
fn the_account_discriminators_follow_the_anchor_sighash() {
    assert_eq!(RAYDIUM_CPMM_ACC_DISC_POOL_STATE, account("PoolState"));
    assert_eq!(RAYDIUM_CPMM_ACC_DISC_AMM_CONFIG, account("AmmConfig"));
    assert_eq!(
        RAYDIUM_CPMM_ACC_DISC_OBSERVATION_STATE,
        account("ObservationState")
    );
}

#[test]
fn the_account_discriminators_match_the_official_idl() {
    assert_eq!(
        RAYDIUM_CPMM_ACC_DISC_POOL_STATE.to_vec(),
        idl_account("PoolState")
    );
    assert_eq!(
        RAYDIUM_CPMM_ACC_DISC_AMM_CONFIG.to_vec(),
        idl_account("AmmConfig")
    );
    assert_eq!(
        RAYDIUM_CPMM_ACC_DISC_OBSERVATION_STATE.to_vec(),
        idl_account("ObservationState")
    );
}

#[test]
fn the_swap_kind_enum_covers_both_idl_instructions() {
    assert_eq!(SwapKind::BaseInput.instruction_name(), "swap_base_input");
    assert_eq!(SwapKind::BaseOutput.instruction_name(), "swap_base_output");
    assert_eq!(
        SwapKind::BaseInput.discriminator(),
        RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT
    );
    assert_eq!(
        SwapKind::BaseOutput.discriminator(),
        RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT
    );
}

#[test]
fn the_idl_lists_exactly_the_two_swap_instructions_we_model() {
    let raw = include_str!("../idl/raydium_cp_swap.json");
    let doc: serde_json::Value = serde_json::from_str(raw).expect("the vendored IDL is valid JSON");
    let names: Vec<&str> = doc["instructions"]
        .as_array()
        .expect("the IDL has instructions")
        .iter()
        .filter_map(|ix| ix["name"].as_str())
        .filter(|n| n.starts_with("swap"))
        .collect();
    assert_eq!(
        names,
        vec!["swap_base_input", "swap_base_output"],
        "the CPMM program has no buy/sell instruction: the direction comes from \
         the vault ordering, so the adapter must not invent one"
    );
}
