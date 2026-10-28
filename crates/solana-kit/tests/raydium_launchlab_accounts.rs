//! LaunchLab account decoding, driven by the committed synthetic fixture.
//!
//! The fixture
//! (`crates/solana-kit/fixtures/raydium/launchlab_pool_state.synthetic.json`)
//! is **synthetic**: it is produced by the crate's own encoders from
//! deterministic placeholder keys. It exercises the decoders and their
//! fail-closed behaviour without needing the network.

use serde_json::Value;
use solana_kit::consts::*;
use solana_kit::venues::raydium_launchlab::accounts::{
    GlobalConfig, PlatformConfig, PoolState, SyntheticPoolState,
};
use solana_sdk::pubkey::Pubkey;

const FIXTURE: &str = include_str!("../fixtures/raydium/launchlab_pool_state.synthetic.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the LaunchLab fixture is valid JSON")
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
        fixture()["global_config"]["data_hex"]
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
fn the_fixture_lengths_match_the_official_idl() {
    let f = fixture();
    assert_eq!(
        f["pool_state"]["length"].as_u64().expect("length"),
        RAYDIUM_LAUNCHLAB_POOL_STATE_LEN as u64
    );
    assert_eq!(
        f["global_config"]["length"].as_u64().expect("length"),
        RAYDIUM_LAUNCHLAB_GLOBAL_CONFIG_LEN as u64
    );
    assert_eq!(pool_bytes().len(), RAYDIUM_LAUNCHLAB_POOL_STATE_LEN);
    assert_eq!(config_bytes().len(), RAYDIUM_LAUNCHLAB_GLOBAL_CONFIG_LEN);
}

#[test]
fn the_pool_state_decodes_to_the_documented_values() {
    let f = fixture();
    let pool = PoolState::decode(&pool_bytes()).expect("the synthetic pool decodes");
    let e = &f["decoded_expectations"];
    assert_eq!(pool.epoch, e["epoch"].as_u64().unwrap());
    assert_eq!(pool.auth_bump, e["auth_bump"].as_u64().unwrap() as u8);
    assert_eq!(pool.status.to_u8(), e["status"].as_u64().unwrap() as u8);
    assert_eq!(
        pool.base_decimals,
        e["base_decimals"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.quote_decimals,
        e["quote_decimals"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.migrate_type.to_u8(),
        e["migrate_type"].as_u64().unwrap() as u8
    );
    assert_eq!(pool.supply, e["supply"].as_u64().unwrap());
    assert_eq!(pool.total_base_sell, e["total_base_sell"].as_u64().unwrap());
    assert_eq!(pool.virtual_base, e["virtual_base"].as_u64().unwrap());
    assert_eq!(pool.virtual_quote, e["virtual_quote"].as_u64().unwrap());
    assert_eq!(pool.real_base, e["real_base"].as_u64().unwrap());
    assert_eq!(pool.real_quote, e["real_quote"].as_u64().unwrap());
    assert_eq!(
        pool.total_quote_fund_raising,
        e["total_quote_fund_raising"].as_u64().unwrap()
    );
    assert_eq!(
        pool.token_program_flag,
        e["token_program_flag"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.amm_creator_fee_on.to_u8(),
        e["amm_creator_fee_on"].as_u64().unwrap() as u8
    );
    assert_eq!(
        pool.platform_vesting_share,
        e["platform_vesting_share"].as_u64().unwrap()
    );
    assert_eq!(
        pool.global_config.to_string(),
        e["global_config"].as_str().unwrap()
    );
    assert_eq!(
        pool.platform_config.to_string(),
        e["platform_config"].as_str().unwrap()
    );
    assert_eq!(pool.base_mint.to_string(), e["base_mint"].as_str().unwrap());
    assert_eq!(
        pool.quote_mint.to_string(),
        e["quote_mint"].as_str().unwrap()
    );
    assert_eq!(
        pool.base_vault.to_string(),
        e["base_vault"].as_str().unwrap()
    );
    assert_eq!(
        pool.quote_vault.to_string(),
        e["quote_vault"].as_str().unwrap()
    );
    assert_eq!(pool.creator.to_string(), e["creator"].as_str().unwrap());
}

#[test]
fn the_global_config_decodes_to_the_documented_values() {
    let cfg = GlobalConfig::decode(&config_bytes()).expect("the synthetic config decodes");
    assert_eq!(cfg.curve_type, RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
    assert_eq!(cfg.trade_fee_rate, 2_500);
    assert_eq!(cfg.index, 0);
    assert_eq!(cfg.requires_platform_auth, 0);
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
            .contains(&format!("expected {RAYDIUM_LAUNCHLAB_POOL_STATE_LEN}")),
        "{err}"
    );
}

#[test]
fn identical_mints_are_refused() {
    let base = SyntheticPoolState::default();
    let s = SyntheticPoolState {
        quote_mint: base.base_mint,
        ..base
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("identical"), "{err}");
}

#[test]
fn a_zero_mint_is_refused() {
    let s = SyntheticPoolState {
        base_mint: Pubkey::default(),
        ..Default::default()
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("zero mint"), "{err}");
}

#[test]
fn a_sell_target_above_the_supply_is_refused() {
    let s = SyntheticPoolState {
        supply: 1_000,
        total_base_sell: 1_001,
        ..Default::default()
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("exceeds supply"), "{err}");
}

#[test]
fn an_unknown_status_is_refused() {
    let s = SyntheticPoolState {
        status: 9,
        ..Default::default()
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("status"), "{err}");
}

#[test]
fn an_unknown_migration_type_is_refused() {
    let s = SyntheticPoolState {
        migrate_type: 9,
        ..Default::default()
    };
    let err = PoolState::decode(&s.encode()).unwrap_err();
    assert!(err.to_string().contains("migrate_type"), "{err}");
}

#[test]
fn the_curve_is_open_only_while_fundraising() {
    use solana_kit::venues::raydium_launchlab::accounts::LaunchLabPoolStatus;
    let mut s = SyntheticPoolState {
        status: RAYDIUM_LAUNCHLAB_STATUS_FUND,
        ..Default::default()
    };
    assert!(PoolState::decode(&s.encode()).unwrap().is_curve_open());
    s.status = RAYDIUM_LAUNCHLAB_STATUS_MIGRATE;
    assert!(!PoolState::decode(&s.encode()).unwrap().is_curve_open());
    s.status = RAYDIUM_LAUNCHLAB_STATUS_TRADE;
    assert!(!PoolState::decode(&s.encode()).unwrap().is_curve_open());
    // The status enum round-trips.
    assert_eq!(
        LaunchLabPoolStatus::Fund.to_u8(),
        RAYDIUM_LAUNCHLAB_STATUS_FUND
    );
    assert_eq!(
        LaunchLabPoolStatus::Migrate.to_u8(),
        RAYDIUM_LAUNCHLAB_STATUS_MIGRATE
    );
    assert_eq!(
        LaunchLabPoolStatus::Trade.to_u8(),
        RAYDIUM_LAUNCHLAB_STATUS_TRADE
    );
}

#[test]
fn the_token_program_flag_bits_select_the_program() {
    let mut s = SyntheticPoolState {
        token_program_flag: 0,
        ..Default::default()
    };
    let pool = PoolState::decode(&s.encode()).unwrap();
    assert!(!pool.base_is_token_2022());
    assert!(!pool.quote_is_token_2022());
    assert_eq!(
        pool.base_token_program(),
        *solana_kit::consts::TOKEN_PROGRAM
    );
    assert_eq!(
        pool.quote_token_program(),
        *solana_kit::consts::TOKEN_PROGRAM
    );

    s.token_program_flag = 0b01;
    let pool = PoolState::decode(&s.encode()).unwrap();
    assert!(pool.base_is_token_2022());
    assert!(!pool.quote_is_token_2022());
    assert_eq!(
        pool.base_token_program(),
        *solana_kit::consts::TOKEN_2022_PROGRAM
    );
    assert_eq!(
        pool.quote_token_program(),
        *solana_kit::consts::TOKEN_PROGRAM
    );

    s.token_program_flag = 0b10;
    let pool = PoolState::decode(&s.encode()).unwrap();
    assert!(!pool.base_is_token_2022());
    assert!(pool.quote_is_token_2022());
    assert_eq!(
        pool.base_token_program(),
        *solana_kit::consts::TOKEN_PROGRAM
    );
    assert_eq!(
        pool.quote_token_program(),
        *solana_kit::consts::TOKEN_2022_PROGRAM
    );
}

#[test]
fn the_remaining_amounts_are_computed() {
    let base = SyntheticPoolState::default();
    let mut vesting_schedule = base.vesting_schedule;
    vesting_schedule.total_locked_amount = 100;
    let s = SyntheticPoolState {
        total_base_sell: 1_000,
        real_base: 400,
        total_quote_fund_raising: 10_000,
        real_quote: 2_500,
        supply: 2_000,
        vesting_schedule,
        ..base
    };
    let p = PoolState::decode(&s.encode()).unwrap();
    assert_eq!(p.remaining_base(), 600);
    assert_eq!(p.remaining_quote_fund_raising(), 7_500);
    // `supply - total_base_sell - total_locked_amount`, the SDK's
    // `amountMigrate`.
    assert_eq!(p.migratable_base(), 900);
}

#[test]
fn the_platform_config_decodes_and_rejects_a_short_buffer() {
    let bytes = solana_kit::venues::raydium_launchlab::accounts::encode_synthetic_platform_config(
        500, 1_000,
    );
    let cfg = PlatformConfig::decode(&bytes).expect("the synthetic platform config decodes");
    assert_eq!(cfg.fee_rate, 500);
    assert_eq!(cfg.creator_fee_rate, 1_000);
    // A truncated buffer is refused.
    assert!(PlatformConfig::decode(&bytes[..bytes.len() - 1]).is_err());
}

#[test]
fn the_global_config_rejects_an_unknown_curve_type() {
    let bytes = solana_kit::venues::raydium_launchlab::accounts::encode_synthetic_global_config(
        9,
        &Pubkey::new_unique(),
    );
    let err = GlobalConfig::decode(&bytes).unwrap_err();
    assert!(err.to_string().contains("curve_type"), "{err}");
}
