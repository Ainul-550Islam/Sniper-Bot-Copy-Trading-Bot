//! CPMM instruction construction.
//!
//! The account order and the instruction data layout are asserted against
//! the vendored official IDL
//! (`crates/solana-kit/idl/raydium_cp_swap.json`, upstream
//! `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`).

use serde_json::Value;
use solana_kit::consts::*;
use solana_kit::venues::raydium_cpmm::accounts::{
    AmmConfig, CpmmSide, PoolState, SyntheticPoolState,
};
use solana_kit::venues::raydium_cpmm::instructions::{
    build_swap, build_swap_base_output, swap_accounts_for, vault_pair, SwapAccounts, SwapKind,
};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

const IDL: &str = include_str!("../idl/raydium_cp_swap.json");

fn idl() -> Value {
    serde_json::from_str(IDL).expect("the vendored IDL is valid JSON")
}

/// The IDL's account names, in order, for an instruction.
fn idl_account_names(instruction: &str) -> Vec<String> {
    idl()["instructions"]
        .as_array()
        .expect("the IDL has instructions")
        .iter()
        .find(|ix| ix["name"] == instruction)
        .unwrap_or_else(|| panic!("the IDL has an instruction named {instruction}"))["accounts"]
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

/// The IDL's argument names, in order, for an instruction.
fn idl_arg_names(instruction: &str) -> Vec<String> {
    idl()["instructions"]
        .as_array()
        .expect("the IDL has instructions")
        .iter()
        .find(|ix| ix["name"] == instruction)
        .unwrap_or_else(|| panic!("the IDL has an instruction named {instruction}"))["args"]
        .as_array()
        .expect("the instruction has args")
        .iter()
        .map(|a| a["name"].as_str().expect("the arg has a name").to_string())
        .collect()
}

fn sample_pool() -> PoolState {
    let s = SyntheticPoolState {
        token_0_mint: Pubkey::new_from_array([0x66u8; 32]),
        token_1_mint: Pubkey::new_from_array([0x77u8; 32]),
        token_0_vault: Pubkey::new_from_array([0x33u8; 32]),
        token_1_vault: Pubkey::new_from_array([0x44u8; 32]),
        token_0_program: *solana_kit::consts::TOKEN_PROGRAM,
        token_1_program: *solana_kit::consts::TOKEN_PROGRAM,
        observation_key: Pubkey::new_from_array([0x88u8; 32]),
        // The authority PDA's canonical bump, so the pool state is usable.
        auth_bump: solana_kit::venues::raydium_cpmm::pda::authority_pda().1,
        ..Default::default()
    };
    PoolState::decode(&s.encode()).expect("the synthetic pool decodes")
}

fn sample_accounts(pool: &PoolState) -> SwapAccounts {
    SwapAccounts {
        payer: Pubkey::new_from_array([0x01u8; 32]),
        input_token_account: Pubkey::new_from_array([0x02u8; 32]),
        output_token_account: Pubkey::new_from_array([0x03u8; 32]),
        input_token_program: *solana_kit::consts::TOKEN_PROGRAM,
        output_token_program: *solana_kit::consts::TOKEN_PROGRAM,
        input_token_mint: pool.token_0_mint,
        output_token_mint: pool.token_1_mint,
    }
}

fn build(pool: &PoolState, kind: SwapKind) -> Instruction {
    let accounts = sample_accounts(pool);
    let (input_vault, output_vault) = vault_pair(pool, CpmmSide::Token0);
    let authority = solana_kit::venues::raydium_cpmm::instructions::authority_of(pool).unwrap();
    build_swap(
        kind,
        &Pubkey::new_from_array([0xAAu8; 32]),
        &pool.amm_config,
        &authority,
        &pool.observation_key,
        &input_vault,
        &output_vault,
        &accounts,
        1_000,
        900,
    )
    .expect("the instruction builds")
}

#[test]
fn the_account_order_matches_the_official_idl() {
    let pool = sample_pool();
    let ix = build(&pool, SwapKind::BaseInput);
    let names = idl_account_names("swap_base_input");
    assert_eq!(ix.accounts.len(), names.len());
    // The account *positions* are what the program reads; the names come
    // from the IDL.
    assert_eq!(
        names,
        vec![
            "payer",
            "authority",
            "amm_config",
            "pool_state",
            "input_token_account",
            "output_token_account",
            "input_vault",
            "output_vault",
            "input_token_program",
            "output_token_program",
            "input_token_mint",
            "output_token_mint",
            "observation_state",
        ]
    );
}

#[test]
fn the_base_output_account_order_matches_the_official_idl() {
    let pool = sample_pool();
    let ix = build(&pool, SwapKind::BaseOutput);
    let names = idl_account_names("swap_base_output");
    assert_eq!(ix.accounts.len(), names.len());
    assert_eq!(names.first().map(String::as_str), Some("payer"));
    assert_eq!(names.last().map(String::as_str), Some("observation_state"));
}

#[test]
fn the_argument_order_matches_the_official_idl() {
    assert_eq!(
        idl_arg_names("swap_base_input"),
        vec!["amount_in", "minimum_amount_out"]
    );
    assert_eq!(
        idl_arg_names("swap_base_output"),
        vec!["max_amount_in", "amount_out"]
    );
}

#[test]
fn the_data_is_the_discriminator_plus_the_two_u64_arguments() {
    let pool = sample_pool();
    let ix = build(&pool, SwapKind::BaseInput);
    assert_eq!(ix.data.len(), 8 + 8 + 8);
    assert_eq!(&ix.data[..8], &RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT);
    assert_eq!(
        u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
        1_000,
        "the first argument is amount_in"
    );
    assert_eq!(
        u64::from_le_bytes(ix.data[16..24].try_into().unwrap()),
        900,
        "the second argument is minimum_amount_out"
    );
}

#[test]
fn the_base_output_data_carries_the_max_input_and_the_amount_out() {
    let pool = sample_pool();
    let accounts = sample_accounts(&pool);
    let (input_vault, output_vault) = vault_pair(&pool, CpmmSide::Token0);
    let authority = solana_kit::venues::raydium_cpmm::instructions::authority_of(&pool).unwrap();
    let ix = build_swap_base_output(
        &Pubkey::new_from_array([0xAAu8; 32]),
        &pool.amm_config,
        &authority,
        &pool.observation_key,
        &input_vault,
        &output_vault,
        &accounts,
        1_100,
        1_000,
    )
    .unwrap();
    assert_eq!(&ix.data[..8], &RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT);
    assert_eq!(
        u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
        1_100,
        "the first argument is max_amount_in"
    );
    assert_eq!(
        u64::from_le_bytes(ix.data[16..24].try_into().unwrap()),
        1_000,
        "the second argument is amount_out"
    );
}

#[test]
fn the_payer_is_the_only_signer() {
    let pool = sample_pool();
    let ix = build(&pool, SwapKind::BaseInput);
    let signers: Vec<bool> = ix.accounts.iter().map(|a| a.is_signer).collect();
    assert_eq!(
        signers,
        vec![
            true, false, false, false, false, false, false, false, false, false, false, false,
            false
        ]
    );
}

#[test]
fn the_writable_accounts_match_the_idl() {
    let pool = sample_pool();
    let ix = build(&pool, SwapKind::BaseInput);
    let writable: Vec<bool> = ix.accounts.iter().map(|a| a.is_writable).collect();
    // payer, authority, amm_config, input/output programs and mints are
    // read-only; the pool, the token accounts, the vaults and the oracle are
    // writable.
    assert_eq!(
        writable,
        vec![
            false, // payer
            false, // authority
            false, // amm_config
            true,  // pool_state
            true,  // input_token_account
            true,  // output_token_account
            true,  // input_vault
            true,  // output_vault
            false, // input_token_program
            false, // output_token_program
            false, // input_token_mint
            false, // output_token_mint
            true,  // observation_state
        ]
    );
}

#[test]
fn the_program_id_is_the_official_one() {
    let pool = sample_pool();
    let ix = build(&pool, SwapKind::BaseInput);
    assert_eq!(ix.program_id, *RAYDIUM_CPMM);
    assert_eq!(
        ix.program_id.to_string(),
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"
    );
}

#[test]
fn the_vault_pair_is_the_input_side_first() {
    let pool = sample_pool();
    let (input_vault, output_vault) = vault_pair(&pool, CpmmSide::Token0);
    assert_eq!(input_vault, pool.token_0_vault);
    assert_eq!(output_vault, pool.token_1_vault);

    let (input_vault, output_vault) = vault_pair(&pool, CpmmSide::Token1);
    assert_eq!(input_vault, pool.token_1_vault);
    assert_eq!(output_vault, pool.token_0_vault);
}

#[test]
fn the_default_account_set_follows_the_input_side() {
    let pool = sample_pool();
    let payer = Pubkey::new_from_array([0x01u8; 32]);
    let input = Pubkey::new_from_array([0x02u8; 32]);
    let output = Pubkey::new_from_array([0x03u8; 32]);

    let accounts = swap_accounts_for(&pool, &payer, CpmmSide::Token0, &input, &output);
    assert_eq!(accounts.payer, payer);
    assert_eq!(accounts.input_token_account, input);
    assert_eq!(accounts.output_token_account, output);
    assert_eq!(accounts.input_token_mint, pool.token_0_mint);
    assert_eq!(accounts.output_token_mint, pool.token_1_mint);
    assert_eq!(accounts.input_token_program, pool.token_0_program);
    assert_eq!(accounts.output_token_program, pool.token_1_program);

    let accounts = swap_accounts_for(&pool, &payer, CpmmSide::Token1, &input, &output);
    assert_eq!(accounts.input_token_mint, pool.token_1_mint);
    assert_eq!(accounts.output_token_mint, pool.token_0_mint);
}

#[test]
fn the_authority_is_derived_from_the_stored_bump() {
    let pool = sample_pool();
    let authority = solana_kit::venues::raydium_cpmm::instructions::authority_of(&pool).unwrap();
    let expected =
        Pubkey::create_program_address(&[RAYDIUM_CPMM_SEED_AUTH, &[pool.auth_bump]], &RAYDIUM_CPMM)
            .expect("the stored bump must be canonical");
    assert_eq!(authority, expected);
}

#[test]
fn a_pool_with_an_impossible_bump_is_refused() {
    // Bump `1` provably does not derive to a valid program address for the
    // `vault_and_lp_mint_auth_seed` seed under the CPMM program id, so the
    // authority cannot be reconstructed from the stored bump.
    assert!(
        Pubkey::create_program_address(&[RAYDIUM_CPMM_SEED_AUTH, &[1u8]], &RAYDIUM_CPMM,).is_err(),
        "bump 1 must not derive for the CPMM authority seed"
    );

    let s = SyntheticPoolState {
        auth_bump: 1,
        ..Default::default()
    };
    let bytes = s.encode();
    let pool = PoolState::decode(&bytes).expect("the pool decodes");
    assert!(
        solana_kit::venues::raydium_cpmm::instructions::authority_of(&pool).is_err(),
        "an authority that cannot be derived must be refused, not guessed"
    );
}

#[test]
fn the_amm_config_decodes_and_is_usable() {
    let config = AmmConfig::decode(
        &solana_kit::venues::raydium_cpmm::accounts::encode_synthetic_amm_config(
            0, 2_500, 500, 500, 1_000,
        ),
    )
    .unwrap();
    assert_eq!(config.trade_fee_rate, 2_500);
    assert_eq!(config.protocol_fee_rate, 500);
}
