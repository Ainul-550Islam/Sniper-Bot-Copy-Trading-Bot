//! LaunchLab instruction construction.
//!
//! The account order and the instruction data layout are asserted against
//! the vendored official IDL
//! (`crates/solana-kit/idl/raydium_launchpad.json`, upstream
//! `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`)
//! and the official `raydium-io/raydium-sdk-V2` @
//! `f8549145afe75d4c486e30c43331c2c4da32b1eb` instruction builders.

use serde_json::Value;
use solana_kit::consts::*;
use solana_kit::venues::raydium_launchlab::instructions::{
    build_trade_ix, max_in_with_slippage_bps, min_out_with_slippage_bps, TradeAccounts, TradeArgs,
    TradeKind,
};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

const IDL: &str = include_str!("../idl/raydium_launchpad.json");

fn idl() -> Value {
    serde_json::from_str(IDL).expect("the vendored IDL is valid JSON")
}

fn idl_instruction(name: &str) -> Value {
    idl()["instructions"]
        .as_array()
        .expect("the IDL has instructions")
        .iter()
        .find(|ix| ix["name"] == name)
        .unwrap_or_else(|| panic!("the IDL has an instruction named {name}"))
        .clone()
}

fn idl_account_names(name: &str) -> Vec<String> {
    idl_instruction(name)["accounts"]
        .as_array()
        .expect("the instruction has accounts")
        .iter()
        .map(|a| {
            a["name"]
                .as_str()
                .expect("the account has a name")
                .to_string()
        })
        .collect()
}

fn idl_arg_names(name: &str) -> Vec<String> {
    idl_instruction(name)["args"]
        .as_array()
        .expect("the instruction has args")
        .iter()
        .map(|a| a["name"].as_str().expect("the arg has a name").to_string())
        .collect()
}

/// A trade account set with a distinct key per slot, so a mis-ordered
/// account is visible.
fn sample_accounts() -> TradeAccounts {
    TradeAccounts {
        payer: Pubkey::new_from_array([0x01u8; 32]),
        authority: Pubkey::new_from_array([0x02u8; 32]),
        global_config: Pubkey::new_from_array([0x03u8; 32]),
        platform_config: Pubkey::new_from_array([0x04u8; 32]),
        pool_state: Pubkey::new_from_array([0x05u8; 32]),
        user_base_token: Pubkey::new_from_array([0x06u8; 32]),
        user_quote_token: Pubkey::new_from_array([0x07u8; 32]),
        base_vault: Pubkey::new_from_array([0x08u8; 32]),
        quote_vault: Pubkey::new_from_array([0x09u8; 32]),
        base_token_mint: Pubkey::new_from_array([0x0Au8; 32]),
        quote_token_mint: Pubkey::new_from_array([0x0Bu8; 32]),
        base_token_program: *solana_kit::consts::TOKEN_PROGRAM,
        quote_token_program: *solana_kit::consts::TOKEN_PROGRAM,
        platform_claim_fee_vault: Pubkey::new_from_array([0x0Cu8; 32]),
        creator_claim_fee_vault: Pubkey::new_from_array([0x0Du8; 32]),
        share_fee_receiver: None,
    }
}

fn build(kind: TradeKind) -> Instruction {
    build_trade_ix(
        kind,
        &sample_accounts(),
        TradeArgs {
            amount: 1_000,
            limit: 900,
            share_fee_rate: 100,
        },
    )
    .expect("the instruction builds")
}

#[test]
fn the_idl_lists_exactly_the_four_trade_instructions_we_model() {
    let idl = idl();
    let names: Vec<&str> = idl["instructions"]
        .as_array()
        .expect("the IDL has instructions")
        .iter()
        .filter_map(|ix| ix["name"].as_str())
        .filter(|n| n.contains("exact"))
        .collect();
    for expected in [
        "buy_exact_in",
        "buy_exact_out",
        "sell_exact_in",
        "sell_exact_out",
    ] {
        assert!(
            names.contains(&expected),
            "the IDL is missing {expected}: {names:?}"
        );
    }
}

#[test]
fn the_buy_exact_in_account_order_matches_the_official_idl() {
    let ix = build(TradeKind::BuyExactIn);
    let names = idl_account_names("buy_exact_in");
    // The IDL declares 15 accounts; the builder appends the three the SDK
    // adds (`system_program`, the platform fee vault, the creator fee
    // vault).
    assert_eq!(names.len(), 15);
    assert_eq!(ix.accounts.len(), names.len() + 3);
    // The first four slots are the ones the SDK's `instrument.ts` fixes.
    assert_eq!(names[0], "payer");
    assert_eq!(names[1], "authority");
    assert_eq!(names[2], "global_config");
    assert_eq!(names[3], "platform_config");
    assert_eq!(names[4], "pool_state");
    assert_eq!(names[5], "user_base_token");
    assert_eq!(names[6], "user_quote_token");
    assert_eq!(names[7], "base_vault");
    assert_eq!(names[8], "quote_vault");
    assert_eq!(names[9], "base_token_mint");
    assert_eq!(names[10], "quote_token_mint");
    assert_eq!(names[11], "base_token_program");
    assert_eq!(names[12], "quote_token_program");
    assert_eq!(names[13], "event_authority");
    assert_eq!(names[14], "program");
}

#[test]
fn the_sell_account_order_matches_the_official_idl() {
    let ix = build(TradeKind::SellExactIn);
    let names = idl_account_names("sell_exact_in");
    // The IDL declares 15 accounts; the builder appends the three the SDK
    // adds (`system_program`, the platform fee vault, the creator fee
    // vault).
    assert_eq!(names.len(), 15);
    assert_eq!(ix.accounts.len(), names.len() + 3);
    assert_eq!(names[0], "payer");
    assert_eq!(names[1], "authority");
    assert_eq!(names[2], "global_config");
    assert_eq!(names[3], "platform_config");
    assert_eq!(names[4], "pool_state");
    assert_eq!(names[5], "user_base_token");
    assert_eq!(names[6], "user_quote_token");
    assert_eq!(names[7], "base_vault");
    assert_eq!(names[8], "quote_vault");
    assert_eq!(names[9], "base_token_mint");
    assert_eq!(names[10], "quote_token_mint");
    assert_eq!(names[11], "base_token_program");
    assert_eq!(names[12], "quote_token_program");
    assert_eq!(names[13], "event_authority");
    assert_eq!(names[14], "program");
}

#[test]
fn all_four_trade_instructions_share_the_same_account_order() {
    let reference = idl_account_names("buy_exact_in");
    for name in ["buy_exact_out", "sell_exact_in", "sell_exact_out"] {
        assert_eq!(idl_account_names(name), reference, "{name}");
    }
}

#[test]
fn the_argument_names_match_the_official_idl() {
    // The IDL names the first two arguments `amount_in` /
    // `minimum_amount_out` for the exact-in trades and `amount_out` /
    // `maximum_amount_in` for the exact-out trades, and the third is always
    // `share_fee_rate`.
    for (name, first, second) in [
        ("buy_exact_in", "amount_in", "minimum_amount_out"),
        ("buy_exact_out", "amount_out", "maximum_amount_in"),
        ("sell_exact_in", "amount_in", "minimum_amount_out"),
        ("sell_exact_out", "amount_out", "maximum_amount_in"),
    ] {
        let args = idl_arg_names(name);
        assert_eq!(args.len(), 3, "{name}: {args:?}");
        assert_eq!(args[0], first, "{name}: {args:?}");
        assert_eq!(args[1], second, "{name}: {args:?}");
        assert_eq!(args[2], "share_fee_rate", "{name}: {args:?}");
    }
}

#[test]
fn the_data_is_the_discriminator_plus_the_three_arguments() {
    let ix = build(TradeKind::BuyExactIn);
    assert_eq!(&ix.data[..8], &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN);
    assert_eq!(ix.data.len(), 8 + 8 + 8 + 8);
    assert_eq!(
        u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
        1_000
    );
    assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 900);
    assert_eq!(u64::from_le_bytes(ix.data[24..32].try_into().unwrap()), 100);
}

#[test]
fn each_trade_kind_uses_its_own_discriminator() {
    assert_eq!(
        &build(TradeKind::BuyExactIn).data[..8],
        &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN
    );
    assert_eq!(
        &build(TradeKind::BuyExactOut).data[..8],
        &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT
    );
    assert_eq!(
        &build(TradeKind::SellExactIn).data[..8],
        &RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN
    );
    assert_eq!(
        &build(TradeKind::SellExactOut).data[..8],
        &RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT
    );
}

#[test]
fn the_discriminators_match_the_official_idl() {
    for (name, disc) in [
        ("buy_exact_in", RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN),
        ("buy_exact_out", RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT),
        ("sell_exact_in", RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN),
        ("sell_exact_out", RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT),
    ] {
        let idl_disc: Vec<u8> = idl_instruction(name)["discriminator"]
            .as_array()
            .expect("the instruction has a discriminator")
            .iter()
            .map(|v| v.as_u64().expect("the discriminator is a number") as u8)
            .collect();
        assert_eq!(idl_disc, disc.to_vec(), "{name}");
    }
}

#[test]
fn the_program_id_is_the_official_one() {
    assert_eq!(build(TradeKind::BuyExactIn).program_id, *RAYDIUM_LAUNCHLAB);
    assert_eq!(
        RAYDIUM_LAUNCHLAB.to_string(),
        "LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj"
    );
}

#[test]
fn the_payer_is_the_first_signer() {
    let ix = build(TradeKind::BuyExactIn);
    let signers: Vec<bool> = ix.accounts.iter().map(|a| a.is_signer).collect();
    assert!(signers[0], "the payer must sign");
    assert!(
        signers.iter().filter(|s| **s).count() >= 1,
        "at least the payer must sign"
    );
}

#[test]
fn the_accounts_are_in_the_declared_order() {
    let accounts = sample_accounts();
    let ix = build(TradeKind::BuyExactIn);
    assert_eq!(ix.accounts[0].pubkey, accounts.payer);
    assert_eq!(ix.accounts[1].pubkey, accounts.authority);
    assert_eq!(ix.accounts[2].pubkey, accounts.global_config);
    assert_eq!(ix.accounts[3].pubkey, accounts.platform_config);
    assert_eq!(ix.accounts[4].pubkey, accounts.pool_state);
    assert_eq!(ix.accounts[5].pubkey, accounts.user_base_token);
    assert_eq!(ix.accounts[6].pubkey, accounts.user_quote_token);
    assert_eq!(ix.accounts[7].pubkey, accounts.base_vault);
    assert_eq!(ix.accounts[8].pubkey, accounts.quote_vault);
    assert_eq!(ix.accounts[9].pubkey, accounts.base_token_mint);
    assert_eq!(ix.accounts[10].pubkey, accounts.quote_token_mint);
    assert_eq!(ix.accounts[11].pubkey, accounts.base_token_program);
    assert_eq!(ix.accounts[12].pubkey, accounts.quote_token_program);
    // The trailing SDK accounts.
    assert_eq!(
        ix.accounts[ix.accounts.len() - 1].pubkey,
        accounts.creator_claim_fee_vault
    );
    assert_eq!(
        ix.accounts[ix.accounts.len() - 2].pubkey,
        accounts.platform_claim_fee_vault
    );
    assert_eq!(
        ix.accounts[ix.accounts.len() - 3].pubkey,
        solana_sdk::system_program::id()
    );
}

#[test]
fn the_slippage_helpers_round_the_right_way() {
    // The minimum output is floored: 1_000 with 1% slippage is 990.
    assert_eq!(min_out_with_slippage_bps(1_000, 100).unwrap(), 990);
    // The maximum input is ceiled: 1_000 with 1% slippage is 1_010.
    assert_eq!(max_in_with_slippage_bps(1_000, 100).unwrap(), 1_010);
    // A 100% tolerance is refused.
    assert!(min_out_with_slippage_bps(1_000, 10_000).is_err());
    assert!(max_in_with_slippage_bps(1_000, 10_000).is_err());
}

#[test]
fn the_trade_kind_names_match_the_official_ones() {
    assert_eq!(TradeKind::BuyExactIn.instruction_name(), "buy_exact_in");
    assert_eq!(TradeKind::BuyExactOut.instruction_name(), "buy_exact_out");
    assert_eq!(TradeKind::SellExactIn.instruction_name(), "sell_exact_in");
    assert_eq!(TradeKind::SellExactOut.instruction_name(), "sell_exact_out");
}
