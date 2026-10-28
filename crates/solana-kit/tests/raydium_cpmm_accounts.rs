//! CPMM account decoding, driven by the committed synthetic fixture.
//!
//! The fixture (`crates/solana-kit/fixtures/raydium/cpmm_pool_state.synthetic.json`)
//! is **synthetic**: it is produced by the crate's own encoders from
//! deterministic placeholder keys. It exercises the decoders and their
//! fail-closed behaviour without needing the network.

use serde_json::Value;
use solana_kit::consts::*;
use solana_kit::venues::raydium_cpmm::accounts::{AmmConfig, PoolState};
use solana_sdk::pubkey::Pubkey;

const FIXTURE: &str = include_str!("../fixtures/raydium/cpmm_pool_state.synthetic.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the CPMM fixture is valid JSON")
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    assert!(
        hex.len() % 2 == 0,
        "the hex string must have an even length"
    );
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn pool_bytes() -> Vec<u8> {
    hex_to_bytes(
        fixture()["pool_state"]["data_hex"]
            .as_str()
            .expect("data_hex"),
    )
}

fn config_bytes() -> Vec<u8> {
    hex_to_bytes(
        fixture()["amm_config"]["data_hex"]
            .as_str()
            .expect("data_hex"),
    )
}

#[test]
fn the_fixture_is_labelled_synthetic() {
    let f = fixture();
    assert_eq!(f["synthetic"], Value::Bool(true));
    let comment = f["_comment"]
        .as_array()
        .expect("_comment is an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        comment.contains("SYNTHETIC") && comment.contains("NOT MAINNET DATA"),
        "the fixture must say, in plain words, that it is synthetic: {comment}"
    );
}

#[test]
fn the_fixture_lengths_match_the_official_program() {
    let f = fixture();
    assert_eq!(
        f["pool_state"]["length"].as_u64().expect("length"),
        RAYDIUM_CPMM_POOL_STATE_LEN as u64
    );
    assert_eq!(
        f["amm_config"]["length"].as_u64().expect("length"),
        RAYDIUM_CPMM_AMM_CONFIG_LEN as u64
    );
    assert_eq!(
        pool_bytes().len(),
        RAYDIUM_CPMM_POOL_STATE_LEN,
        "the pool fixture must be exactly the official PoolState length"
    );
    assert_eq!(
        config_bytes().len(),
        RAYDIUM_CPMM_AMM_CONFIG_LEN,
        "the config fixture must be exactly the official AmmConfig length"
    );
}

#[test]
fn the_pool_state_decodes_to_the_documented_values() {
    let f = fixture();
    let pool = PoolState::decode(&pool_bytes()).expect("the synthetic pool decodes");
    let expected = &f["decoded_expectations"];
    assert_eq!(
        pool.amm_config.to_string(),
        expected["amm_config"].as_str().unwrap()
    );
    assert_eq!(
        pool.pool_creator.to_string(),
        expected["pool_creator"].as_str().unwrap()
    );
    assert_eq!(
        pool.token_0_vault.to_string(),
        expected["token_0_vault"].as_str().unwrap()
    );
    assert_eq!(
        pool.token_1_vault.to_string(),
        expected["token_1_vault"].as_str().unwrap()
    );
    assert_eq!(
        pool.lp_mint.to_string(),
        expected["lp_mint"].as_str().unwrap()
    );
    assert_eq!(
        pool.token_0_mint.to_string(),
        expected["token_0_mint"].as_str().unwrap()
    );
    assert_eq!(
        pool.token_1_mint.to_string(),
        expected["token_1_mint"].as_str().unwrap()
    );
    assert_eq!(
        pool.observation_key.to_string(),
        expected["observation_key"].as_str().unwrap()
    );
    assert_eq!(
        pool.auth_bump,
        expected["auth_bump"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.status.bits,
        expected["status_bits"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.lp_mint_decimals,
        expected["lp_mint_decimals"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.mint_0_decimals,
        expected["mint_0_decimals"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.mint_1_decimals,
        expected["mint_1_decimals"].as_u64().unwrap() as u8
    );
    assert_eq!(pool.lp_supply, expected["lp_supply"].as_u64().unwrap());
    assert_eq!(pool.open_time, expected["open_time"].as_u64().unwrap());
    assert_eq!(
        pool.recent_epoch,
        expected["recent_epoch"].as_u64().unwrap()
    );
    assert_eq!(
        pool.creator_fee_on.to_u8(),
        expected["creator_fee_on"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.enable_creator_fee,
        expected["enable_creator_fee"].as_bool().unwrap()
    );
    assert_eq!(
        pool.creator_fees_token_0,
        expected["creator_fees_token_0"].as_u64().unwrap()
    );
    assert_eq!(
        pool.creator_fees_token_1,
        expected["creator_fees_token_1"].as_u64().unwrap()
    );
}

#[test]
fn the_amm_config_decodes_to_the_documented_values() {
    let config = AmmConfig::decode(&config_bytes()).expect("the synthetic config decodes");
    assert_eq!(config.bump, 254);
    assert!(!config.disable_create_pool);
    assert_eq!(config.index, 0);
    assert_eq!(config.trade_fee_rate, 2_500);
    assert_eq!(config.protocol_fee_rate, 500);
    assert_eq!(config.fund_fee_rate, 500);
    assert_eq!(config.create_pool_fee, 0);
    assert_eq!(config.creator_fee_rate, 1_000);
    // The program's `creator_fee_share_rate` sits in the slot the IDL still
    // calls `padding[0]`.
    assert_eq!(config.creator_fee_share_rate, 0);
}

#[test]
fn a_wrong_discriminator_is_refused() {
    let mut bytes = pool_bytes();
    bytes[0] ^= 0xff;
    let err = PoolState::decode(&bytes).unwrap_err();
    assert!(err.to_string().contains("discriminator"), "{err}");
}

#[test]
fn a_wrong_length_is_refused() {
    let bytes = pool_bytes();
    let err = PoolState::decode(&bytes[..bytes.len() - 1]).unwrap_err();
    assert!(
        err.to_string()
            .contains(&format!("expected {RAYDIUM_CPMM_POOL_STATE_LEN}")),
        "{err}"
    );
    let err = PoolState::decode(&[]).unwrap_err();
    assert!(err.to_string().contains("discriminator"), "{err}");
}

#[test]
fn identical_mints_are_refused() {
    let base = solana_kit::venues::raydium_cpmm::accounts::SyntheticPoolState::default();
    let s = solana_kit::venues::raydium_cpmm::accounts::SyntheticPoolState {
        token_1_mint: base.token_0_mint,
        ..base
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("identical"), "{err}");
}

#[test]
fn a_zero_mint_is_refused() {
    let s = solana_kit::venues::raydium_cpmm::accounts::SyntheticPoolState {
        token_0_mint: Pubkey::default(),
        ..Default::default()
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("zero mint"), "{err}");
}

#[test]
fn an_invalid_creator_fee_on_is_refused() {
    let s = solana_kit::venues::raydium_cpmm::accounts::SyntheticPoolState {
        creator_fee_on: 7,
        ..Default::default()
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("creator_fee_on"), "{err}");
}

#[test]
fn a_fee_rate_at_the_denominator_is_refused() {
    let bytes = solana_kit::venues::raydium_cpmm::accounts::encode_synthetic_amm_config(
        0,
        RAYDIUM_CPMM_FEE_RATE_DENOMINATOR,
        0,
        0,
        0,
    );
    let err = AmmConfig::decode(&bytes).unwrap_err();
    assert!(err.to_string().contains("fee rate"), "{err}");
}

#[test]
fn the_status_bits_follow_the_program() {
    use solana_kit::venues::raydium_cpmm::accounts::PoolStatus;
    // bit0 = deposit disabled, bit1 = withdraw disabled, bit2 = swap disabled.
    assert!(!PoolStatus { bits: 0 }.deposits_disabled());
    assert!(PoolStatus { bits: 1 }.deposits_disabled());
    assert!(!PoolStatus { bits: 0 }.withdrawals_disabled());
    assert!(PoolStatus { bits: 2 }.withdrawals_disabled());
    assert!(!PoolStatus { bits: 0 }.swaps_disabled());
    assert!(PoolStatus { bits: 4 }.swaps_disabled());
}

#[test]
fn the_mint_side_lookup_is_exact() {
    let pool = PoolState::decode(&pool_bytes()).unwrap();
    assert_eq!(
        pool.side_of(&pool.token_0_mint),
        Some(solana_kit::venues::raydium_cpmm::accounts::CpmmSide::Token0)
    );
    assert_eq!(
        pool.side_of(&pool.token_1_mint),
        Some(solana_kit::venues::raydium_cpmm::accounts::CpmmSide::Token1)
    );
    assert_eq!(pool.side_of(&Pubkey::new_unique()), None);
}
