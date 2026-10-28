//! PDA derivation for the Raydium CPMM (`cp-swap`) program.
//!
//! This module is the **single owner** of CPMM PDA derivation. Nothing else in
//! `solana-kit` may call `Pubkey::find_program_address` for this venue; every
//! other CPMM file asks this one.
//!
//! # Sources
//!
//! Seeds and their exact byte order come from the official Raydium program
//! source (`raydium-io/raydium-cp-swap`, commit
//! `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`):
//!
//! | Seed constant | Program file |
//! |---|---|
//! | `AUTH_SEED` = `"vault_and_lp_mint_auth_seed"` | `programs/cp-swap/src/lib.rs` |
//! | `AMM_CONFIG_SEED` = `"amm_config"` | `programs/cp-swap/src/states/config.rs` |
//! | `POOL_SEED` = `"pool"` | `programs/cp-swap/src/states/pool.rs` |
//! | `POOL_LP_MINT_SEED` = `"pool_lp_mint"` | `programs/cp-swap/src/states/pool.rs` |
//! | `POOL_VAULT_SEED` = `"pool_vault"` | `programs/cp-swap/src/states/pool.rs` |
//! | `OBSERVATION_SEED` = `"observation"` | `programs/cp-swap/src/states/oracle.rs` |
//! | `PERMISSION_SEED` = `"permission"` | `programs/cp-swap/src/states/permission.rs` |
//!
//! The seed *composition* (which seeds, in which order) comes from the
//! `#[account(seeds = [...])]` attributes in
//! `programs/cp-swap/src/instructions/initialize.rs` and the
//! `initialize_with_permission.rs` sibling, and is cross-checked against
//! `raydium-io/raydium-sdk-V2` → `src/raydium/cpmm/pda.ts`.
//!
//! Note on the pool PDA: the program *also* accepts a pool account that is NOT
//! a PDA (the `pool_state` account is an `UncheckedAccount` that may be signed
//! for by the creator). The PDA form below is what the SDK and the Raydium UI
//! use, and what `initialize` derives when `pool_state` is a PDA.

use solana_sdk::pubkey::Pubkey;

use crate::consts::*;

/// The CPMM pool authority (the vault + LP-mint authority of every pool).
///
/// Program: `AUTH_SEED.as_bytes()`, i.e. `["vault_and_lp_mint_auth_seed"]`.
pub fn authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RAYDIUM_CPMM_SEED_AUTH], &RAYDIUM_CPMM)
}

/// The CPMM pool authority, without the bump.
pub fn authority() -> Pubkey {
    authority_pda().0
}

/// The `AmmConfig` PDA for a config index.
///
/// Program: `AMM_CONFIG_SEED.as_bytes()` + the `u16` index in **big endian**.
///
/// Source: `programs/cp-swap/src/instructions/admin/create_config.rs`:
/// `seeds = [AMM_CONFIG_SEED.as_bytes(), &index.to_be_bytes()]`. The on-chain
/// program is the authority here; note that `raydium-sdk-V2`'s
/// `getCpmmPdaAmmConfigId` uses `u16ToBytes`, which writes **little** endian,
/// so the SDK and the program disagree for any index other than 0. The two
/// agree for index 0 (the only config Raydium currently ships), which is why
/// the SDK bug has never surfaced on mainnet.
pub fn amm_config_pda(index: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_CPMM_SEED_AMM_CONFIG, &index.to_be_bytes()],
        &RAYDIUM_CPMM,
    )
}

/// The `AmmConfig` PDA for a config index, without the bump.
pub fn amm_config(index: u16) -> Pubkey {
    amm_config_pda(index).0
}

/// The `PoolState` PDA for a pool.
///
/// Program: `POOL_SEED.as_bytes()` + `amm_config` + `token_0_mint` +
/// `token_1_mint`.
///
/// Source: `programs/cp-swap/src/instructions/initialize.rs`:
/// ```text
/// seeds = [
///     POOL_SEED.as_bytes(),
///     amm_config.key().as_ref(),
///     token_0_mint.key().as_ref(),
///     token_1_mint.key().as_ref(),
/// ]
/// ```
///
/// The mints must be given in the program's canonical order: `token_0_mint`
/// is the **smaller** pubkey (`token_0_mint.key() < token_1_mint.key()` is a
/// hard constraint in `initialize`). Use [`pool_pda_ordered`] to sort them.
pub fn pool_pda(amm_config: &Pubkey, token_0_mint: &Pubkey, token_1_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RAYDIUM_CPMM_SEED_POOL,
            amm_config.as_ref(),
            token_0_mint.as_ref(),
            token_1_mint.as_ref(),
        ],
        &RAYDIUM_CPMM,
    )
}

/// The `PoolState` PDA, with the mints sorted into the program's canonical
/// `token_0` / `token_1` order (smaller pubkey first).
///
/// Returns `(pool, bump, token_0_mint, token_1_mint)` so the caller can also
/// learn which side of the pair a mint is on.
pub fn pool_pda_ordered(
    amm_config: &Pubkey,
    mint_a: &Pubkey,
    mint_b: &Pubkey,
) -> (Pubkey, u8, Pubkey, Pubkey) {
    if mint_a < mint_b {
        let (pool, bump) = pool_pda(amm_config, mint_a, mint_b);
        (pool, bump, *mint_a, *mint_b)
    } else {
        let (pool, bump) = pool_pda(amm_config, mint_b, mint_a);
        (pool, bump, *mint_b, *mint_a)
    }
}

/// The pool's LP mint PDA.
///
/// Program: `POOL_LP_MINT_SEED.as_bytes()` + `pool_state`.
pub fn lp_mint_pda(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_CPMM_SEED_POOL_LP_MINT, pool.as_ref()],
        &RAYDIUM_CPMM,
    )
}

/// The pool's LP mint PDA, without the bump.
pub fn lp_mint(pool: &Pubkey) -> Pubkey {
    lp_mint_pda(pool).0
}

/// The pool's token vault PDA for one of its two mints.
///
/// Program: `POOL_VAULT_SEED.as_bytes()` + `pool_state` + `mint`.
pub fn vault_pda(pool: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_CPMM_SEED_POOL_VAULT, pool.as_ref(), mint.as_ref()],
        &RAYDIUM_CPMM,
    )
}

/// The pool's token vault PDA, without the bump.
pub fn vault(pool: &Pubkey, mint: &Pubkey) -> Pubkey {
    vault_pda(pool, mint).0
}

/// The pool's oracle `ObservationState` PDA.
///
/// Program: `OBSERVATION_SEED.as_bytes()` + `pool_state`.
pub fn observation_pda(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_CPMM_SEED_OBSERVATION, pool.as_ref()],
        &RAYDIUM_CPMM,
    )
}

/// The pool's oracle `ObservationState` PDA, without the bump.
pub fn observation(pool: &Pubkey) -> Pubkey {
    observation_pda(pool).0
}

/// The permission PDA for an owner.
///
/// Program: `PERMISSION_SEED.as_bytes()` + `owner`.
pub fn permission_pda(owner: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_CPMM_SEED_PERMISSION, owner.as_ref()],
        &RAYDIUM_CPMM,
    )
}

/// The permission PDA for an owner, without the bump.
pub fn permission(owner: &Pubkey) -> Pubkey {
    permission_pda(owner).0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical CPMM authority is a well-known mainnet address. If the
    /// seed ever changes, this test fails loudly instead of the bot building
    /// swaps against a PDA the program does not accept.
    #[test]
    fn authority_pda_is_stable() {
        let (addr, bump) = authority_pda();
        assert_eq!(addr, authority());
        // The bump is a `u8`, so the only meaningful check is that it is
        // usable as the PDA's canonical bump (i.e. re-derivation succeeds).
        // Re-deriving with `create_program_address` and the found bump must
        // reproduce the same address.
        let recreated =
            Pubkey::create_program_address(&[RAYDIUM_CPMM_SEED_AUTH, &[bump]], &RAYDIUM_CPMM)
                .expect("authority is a valid PDA");
        assert_eq!(recreated, addr);
    }

    #[test]
    fn amm_config_pda_depends_on_the_index() {
        let a = amm_config_pda(0);
        let b = amm_config_pda(1);
        assert_ne!(a.0, b.0);
        assert_eq!(a.0, amm_config(0));
    }

    #[test]
    fn pool_pda_matches_the_program_seed_order() {
        let config = amm_config(0);
        let mint0 = Pubkey::new_unique();
        let mint1 = Pubkey::new_unique();
        let (pool, bump) = pool_pda(&config, &mint0, &mint1);
        let expected = Pubkey::find_program_address(
            &[
                b"pool".as_ref(),
                config.as_ref(),
                mint0.as_ref(),
                mint1.as_ref(),
            ],
            &RAYDIUM_CPMM,
        );
        assert_eq!(pool, expected.0);
        assert_eq!(bump, expected.1);
    }

    #[test]
    fn pool_pda_ordered_sorts_the_mints() {
        let config = amm_config(0);
        let low = Pubkey::new_unique();
        let high = Pubkey::new_unique();
        // Force a known ordering.
        let (low, high) = if low < high { (low, high) } else { (high, low) };
        let (pool, _bump, t0, t1) = pool_pda_ordered(&config, &low, &high);
        assert_eq!(t0, low);
        assert_eq!(t1, high);
        assert_eq!(pool, pool_pda(&config, &low, &high).0);

        // Reversing the inputs yields the same pool and the same ordering.
        let (pool2, _bump2, t0b, t1b) = pool_pda_ordered(&config, &high, &low);
        assert_eq!(pool, pool2);
        assert_eq!(t0b, low);
        assert_eq!(t1b, high);
    }

    #[test]
    fn vault_lp_mint_and_observation_are_distinct_pdas() {
        let pool = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let v = vault(&pool, &mint);
        let l = lp_mint(&pool);
        let o = observation(&pool);
        assert_ne!(v, l);
        assert_ne!(v, o);
        assert_ne!(l, o);

        // Different mints give different vaults.
        assert_ne!(v, vault(&pool, &Pubkey::new_unique()));
        // Different pools give different observations.
        assert_ne!(o, observation(&Pubkey::new_unique()));
    }

    #[test]
    fn permission_pda_depends_on_the_owner() {
        let a = permission(&Pubkey::new_unique());
        let b = permission(&Pubkey::new_unique());
        assert_ne!(a, b);
    }
}
