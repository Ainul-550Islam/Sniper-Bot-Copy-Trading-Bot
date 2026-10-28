//! CPMM PDA derivation assertions.
//!
//! Every derivation is re-computed here from the raw seeds, independently of
//! the crate's helper, so a change to the helper cannot silently move a
//! PDA. The seeds and their order come from the official program source
//! (`raydium-io/raydium-cp-swap` @
//! `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`).

use solana_kit::consts::{RAYDIUM_CPMM, RAYDIUM_CPMM_SEED_AMM_CONFIG, RAYDIUM_CPMM_SEED_AUTH};
use solana_kit::venues::raydium_cpmm::pda;
use solana_sdk::pubkey::Pubkey;

#[test]
fn the_authority_pda_uses_only_the_auth_seed() {
    let expected = Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &RAYDIUM_CPMM);
    assert_eq!(pda::authority_pda(), expected);
    assert_eq!(pda::authority(), expected.0);
    assert_eq!(RAYDIUM_CPMM_SEED_AUTH, b"vault_and_lp_mint_auth_seed");
}

#[test]
fn the_amm_config_pda_uses_the_config_seed_and_a_big_endian_index() {
    // The program's `create_config` uses `&index.to_be_bytes()`.
    for index in [0u16, 1, 2, 255, 256, 65_535] {
        let expected = Pubkey::find_program_address(
            &[RAYDIUM_CPMM_SEED_AMM_CONFIG, &index.to_be_bytes()],
            &RAYDIUM_CPMM,
        );
        assert_eq!(
            pda::amm_config_pda(index),
            expected,
            "the AmmConfig PDA for index {index} must match the program's derivation"
        );
    }
}

#[test]
fn the_amm_config_index_is_big_endian_not_little_endian() {
    // Index 0 is the only config Raydium ships, and for it the two byte
    // orders agree. Every other index must follow the *program*.
    assert_eq!(
        pda::amm_config(0),
        Pubkey::find_program_address(
            &[RAYDIUM_CPMM_SEED_AMM_CONFIG, &0u16.to_be_bytes()],
            &RAYDIUM_CPMM
        )
        .0
    );
    assert_ne!(
        pda::amm_config(1),
        Pubkey::find_program_address(
            &[RAYDIUM_CPMM_SEED_AMM_CONFIG, &1u16.to_le_bytes()],
            &RAYDIUM_CPMM
        )
        .0,
        "the little-endian derivation is the SDK's, not the program's"
    );
}

#[test]
fn the_pool_pda_uses_the_pool_seed_then_config_then_the_two_mints() {
    let config = Pubkey::new_unique();
    let mint_0 = Pubkey::new_unique();
    let mint_1 = Pubkey::new_unique();
    let expected = Pubkey::find_program_address(
        &[
            b"pool".as_ref(),
            config.as_ref(),
            mint_0.as_ref(),
            mint_1.as_ref(),
        ],
        &RAYDIUM_CPMM,
    );
    assert_eq!(pda::pool_pda(&config, &mint_0, &mint_1), expected);
}

#[test]
fn the_pool_pda_is_ordered_and_not_sorted() {
    // The program derives the pool PDA from `token_0_mint` and
    // `token_1_mint` **in that order**; the mints are not sorted, so
    // swapping them yields a different address.
    let config = Pubkey::new_unique();
    let a = Pubkey::new_unique();
    let b = Pubkey::new_unique();
    assert_ne!(
        pda::pool_pda(&config, &a, &b),
        pda::pool_pda(&config, &b, &a)
    );
}

#[test]
fn the_ordered_pool_pda_sorts_the_mints_and_reports_the_sides() {
    // `initialize` requires `token_0_mint.key() < token_1_mint.key()`, so
    // the helper sorts the pair and tells the caller which side each mint
    // ended up on.
    let config = Pubkey::new_unique();
    let a = Pubkey::new_unique();
    let b = Pubkey::new_unique();
    let (lower, higher) = if a < b { (a, b) } else { (b, a) };

    let (pool, bump, token_0, token_1) = pda::pool_pda_ordered(&config, &a, &b);
    assert_eq!(token_0, lower);
    assert_eq!(token_1, higher);
    assert_eq!((pool, bump), pda::pool_pda(&config, &token_0, &token_1));

    // Sorting is idempotent.
    let (pool2, bump2, token_0b, token_1b) = pda::pool_pda_ordered(&config, &b, &a);
    assert_eq!((pool2, bump2), (pool, bump));
    assert_eq!((token_0b, token_1b), (token_0, token_1));
}

#[test]
fn the_lp_mint_pda_uses_the_lp_mint_seed_and_the_pool() {
    let pool = Pubkey::new_unique();
    let expected =
        Pubkey::find_program_address(&[b"pool_lp_mint".as_ref(), pool.as_ref()], &RAYDIUM_CPMM);
    assert_eq!(pda::lp_mint_pda(&pool), expected);
    assert_eq!(pda::lp_mint(&pool), expected.0);
}

#[test]
fn the_vault_pda_uses_the_vault_seed_the_pool_and_the_mint() {
    let pool = Pubkey::new_unique();
    let mint = Pubkey::new_unique();
    let expected = Pubkey::find_program_address(
        &[b"pool_vault".as_ref(), pool.as_ref(), mint.as_ref()],
        &RAYDIUM_CPMM,
    );
    assert_eq!(pda::vault_pda(&pool, &mint), expected);
    assert_eq!(pda::vault(&pool, &mint), expected.0);

    // The two vaults differ only by the mint.
    let other = Pubkey::new_unique();
    assert_ne!(pda::vault(&pool, &mint), pda::vault(&pool, &other));
}

#[test]
fn the_observation_pda_uses_the_observation_seed_and_the_pool() {
    let pool = Pubkey::new_unique();
    let expected =
        Pubkey::find_program_address(&[b"observation".as_ref(), pool.as_ref()], &RAYDIUM_CPMM);
    assert_eq!(pda::observation_pda(&pool), expected);
    assert_eq!(pda::observation(&pool), expected.0);
}

#[test]
fn the_permission_pda_uses_the_permission_seed_and_the_owner() {
    let owner = Pubkey::new_unique();
    let expected =
        Pubkey::find_program_address(&[b"permission".as_ref(), owner.as_ref()], &RAYDIUM_CPMM);
    assert_eq!(pda::permission_pda(&owner), expected);
    assert_eq!(pda::permission(&owner), expected.0);
}

/// Re-derive an address from its seed list and the canonical bump.
fn recreate(seeds: &[&[u8]], bump: u8) -> Option<Pubkey> {
    let bump_bytes = [bump];
    let mut all: Vec<&[u8]> = seeds.to_vec();
    all.push(&bump_bytes);
    Pubkey::create_program_address(&all, &RAYDIUM_CPMM).ok()
}

#[test]
fn every_pda_is_a_valid_program_address_whose_bump_reproduces_it() {
    type PdaCase<'a> = (&'a str, (Pubkey, u8), Vec<&'a [u8]>);
    let config = Pubkey::new_unique();
    let mint_0 = Pubkey::new_unique();
    let mint_1 = Pubkey::new_unique();
    let pool = pda::pool_pda(&config, &mint_0, &mint_1).0;
    let cases: [PdaCase<'_>; 7] = [
        (
            "authority",
            pda::authority_pda(),
            vec![b"vault_and_lp_mint_auth_seed"],
        ),
        (
            "amm_config",
            pda::amm_config_pda(0),
            vec![b"amm_config", &[0u8, 0]],
        ),
        (
            "pool",
            pda::pool_pda(&config, &mint_0, &mint_1),
            vec![b"pool", config.as_ref(), mint_0.as_ref(), mint_1.as_ref()],
        ),
        (
            "lp_mint",
            pda::lp_mint_pda(&pool),
            vec![b"pool_lp_mint", pool.as_ref()],
        ),
        (
            "vault",
            pda::vault_pda(&pool, &mint_0),
            vec![b"pool_vault", pool.as_ref(), mint_0.as_ref()],
        ),
        (
            "observation",
            pda::observation_pda(&pool),
            vec![b"observation", pool.as_ref()],
        ),
        (
            "permission",
            pda::permission_pda(&config),
            vec![b"permission", config.as_ref()],
        ),
    ];
    for (name, (address, bump), seeds) in cases {
        assert_ne!(
            address,
            Pubkey::default(),
            "{name} must not be the default address"
        );
        let recreated = recreate(&seeds, bump)
            .unwrap_or_else(|| panic!("the {name} bump must reproduce the address"));
        assert_eq!(recreated, address, "{name} must reproduce from its bump");
    }
}
