//! PDA derivation for the Raydium LaunchLab (`launchpad`) program.
//!
//! This module is the **single owner** of LaunchLab PDA derivation.
//!
//! # Sources
//!
//! The LaunchLab Anchor program source is not published, so the seeds come
//! from Raydium's own official SDK (`raydium-io/raydium-sdk-V2`, commit
//! `f8549145afe75d4c486e30c43331c2c4da32b1eb`):
//! `src/raydium/launchpad/pda.ts`.
//!
//! | Seed | Value | Used for |
//! |---|---|---|
//! | `AUTH_SEED` | `"vault_auth_seed"` | the pool vault authority |
//! | `CONFIG_SEED` | `"global_config"` | the `GlobalConfig` PDA |
//! | `POOL_SEED` | `"pool"` | the `PoolState` PDA |
//! | `POOL_VAULT_SEED` | `"pool_vault"` | the pool token vaults |
//! | `POOL_VESTING_SEED` | `"pool_vesting"` | the vesting record |
//! | `PLATFORM_SEED` | `"platform_config"` | the `PlatformConfig` PDA |
//! | `PLATFORM_FEE_VAULT_AUTH_SEED` | `"platform_fee_vault_auth_seed"` | the platform fee vault authority |
//! | `CREATOR_FEE_VAULT_AUTH_SEED` | `"creator_fee_vault_auth_seed"` | the creator fee vault authority |
//! | `PLATFORM_GLOBAL_ACCESS_SEED` | `"platform_global_access"` | the platform access record |
//! | `__event_authority` | `"__event_authority"` | the Anchor CPI event authority |

use solana_sdk::pubkey::Pubkey;

use crate::consts::*;

/// The LaunchLab pool vault authority PDA.
pub fn auth_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RAYDIUM_LAUNCHLAB_SEED_AUTH], &RAYDIUM_LAUNCHLAB)
}

/// The LaunchLab pool vault authority, without the bump.
pub fn auth() -> Pubkey {
    auth_pda().0
}

/// The `GlobalConfig` PDA for a quote mint, a curve type and a config index.
///
/// SDK: `getPdaLaunchpadConfigId(programId, mintB, curveType, index)` —
/// seeds `["global_config", quote_mint, curve_type (u8), index (u16)]`.
pub fn global_config_pda(quote_mint: &Pubkey, curve_type: u8, index: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RAYDIUM_LAUNCHLAB_SEED_CONFIG,
            quote_mint.as_ref(),
            &[curve_type],
            &index.to_le_bytes(),
        ],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The `GlobalConfig` PDA, without the bump.
pub fn global_config(quote_mint: &Pubkey, curve_type: u8, index: u16) -> Pubkey {
    global_config_pda(quote_mint, curve_type, index).0
}

/// The `PoolState` PDA for a base/quote mint pair.
///
/// SDK: `getPdaLaunchpadPoolId(programId, mintA, mintB)` —
/// seeds `["pool", base_mint, quote_mint]`.
pub fn pool_pda(base_mint: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RAYDIUM_LAUNCHLAB_SEED_POOL,
            base_mint.as_ref(),
            quote_mint.as_ref(),
        ],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The `PoolState` PDA, without the bump.
pub fn pool(base_mint: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
    pool_pda(base_mint, quote_mint).0
}

/// The pool's token vault PDA for one of its two mints.
///
/// SDK: `getPdaLaunchpadVaultId(programId, poolId, mint)` —
/// seeds `["pool_vault", pool, mint]`.
pub fn vault_pda(pool: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RAYDIUM_LAUNCHLAB_SEED_POOL_VAULT,
            pool.as_ref(),
            mint.as_ref(),
        ],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The pool's token vault, without the bump.
pub fn vault(pool: &Pubkey, mint: &Pubkey) -> Pubkey {
    vault_pda(pool, mint).0
}

/// The vesting-record PDA for a pool and a beneficiary.
///
/// SDK: `getPdaVestId(programId, poolId, owner)` —
/// seeds `["pool_vesting", pool, beneficiary]`.
pub fn vesting_pda(pool: &Pubkey, beneficiary: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RAYDIUM_LAUNCHLAB_SEED_POOL_VESTING,
            pool.as_ref(),
            beneficiary.as_ref(),
        ],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The vesting record, without the bump.
pub fn vesting(pool: &Pubkey, beneficiary: &Pubkey) -> Pubkey {
    vesting_pda(pool, beneficiary).0
}

/// The `PlatformConfig` PDA for a platform admin.
///
/// SDK: `getPdaPlatformId(programId, platformAdminWallet)` —
/// seeds `["platform_config", admin]`.
pub fn platform_config_pda(admin: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_LAUNCHLAB_SEED_PLATFORM, admin.as_ref()],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The `PlatformConfig` PDA, without the bump.
pub fn platform_config(admin: &Pubkey) -> Pubkey {
    platform_config_pda(admin).0
}

/// The platform fee-vault authority PDA.
///
/// SDK: `getPdaPlatformFeeVaultAuth(programId)` — seeds
/// `["platform_fee_vault_auth_seed"]`.
pub fn platform_fee_vault_auth_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_LAUNCHLAB_SEED_PLATFORM_FEE_VAULT_AUTH],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The platform fee-vault authority, without the bump.
pub fn platform_fee_vault_auth() -> Pubkey {
    platform_fee_vault_auth_pda().0
}

/// The creator fee-vault authority PDA.
///
/// SDK: `getPdaCreatorFeeVaultAuth(programId)` — seeds
/// `["creator_fee_vault_auth_seed"]`.
pub fn creator_fee_vault_auth_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_LAUNCHLAB_SEED_CREATOR_FEE_VAULT_AUTH],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The creator fee-vault authority, without the bump.
pub fn creator_fee_vault_auth() -> Pubkey {
    creator_fee_vault_auth_pda().0
}

/// The `PlatformGlobalAccess` PDA for a platform and a config.
///
/// SDK: `getPdaPlatformConfigAccess(programId, platformId, configId)` — seeds
/// `["platform_global_access", platform, config]`.
pub fn platform_global_access_pda(platform: &Pubkey, config: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RAYDIUM_LAUNCHLAB_SEED_PLATFORM_GLOBAL_ACCESS,
            platform.as_ref(),
            config.as_ref(),
        ],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The `PlatformGlobalAccess` PDA, without the bump.
pub fn platform_global_access(platform: &Pubkey, config: &Pubkey) -> Pubkey {
    platform_global_access_pda(platform, config).0
}

/// The platform's fee vault for a quote mint.
///
/// SDK: `getPdaPlatformVault(programId, platformId, mintB)` — seeds
/// `[platform, quote_mint]`. Note the platform id is the **first** seed here
/// (there is no `platform_vault` prefix).
pub fn platform_vault_pda(platform: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[platform.as_ref(), quote_mint.as_ref()],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The platform's fee vault, without the bump.
pub fn platform_vault(platform: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
    platform_vault_pda(platform, quote_mint).0
}

/// The creator's fee vault for a quote mint.
///
/// SDK: `getPdaCreatorVault(programId, creator, mintB)` — seeds
/// `[creator, quote_mint]`.
pub fn creator_vault_pda(creator: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[creator.as_ref(), quote_mint.as_ref()], &RAYDIUM_LAUNCHLAB)
}

/// The creator's fee vault, without the bump.
pub fn creator_vault(creator: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
    creator_vault_pda(creator, quote_mint).0
}

/// The Anchor CPI event authority PDA.
///
/// SDK: `getPdaCpiEvent(programId)` — seeds `["__event_authority"]`.
pub fn event_authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[RAYDIUM_LAUNCHLAB_SEED_EVENT_AUTHORITY],
        &RAYDIUM_LAUNCHLAB,
    )
}

/// The Anchor CPI event authority, without the bump.
pub fn event_authority() -> Pubkey {
    event_authority_pda().0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_pda_is_stable() {
        let (addr, bump) = auth_pda();
        assert_eq!(addr, auth());
        let recreated = Pubkey::create_program_address(
            &[RAYDIUM_LAUNCHLAB_SEED_AUTH, &[bump]],
            &RAYDIUM_LAUNCHLAB,
        )
        .expect("auth is a valid PDA");
        assert_eq!(recreated, addr);
    }

    #[test]
    fn global_config_pda_depends_on_every_seed() {
        let quote = Pubkey::new_unique();
        let a = global_config(&quote, 0, 0);
        let b = global_config(&quote, 1, 0);
        let c = global_config(&quote, 0, 1);
        let d = global_config(&Pubkey::new_unique(), 0, 0);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn pool_pda_matches_the_sdk_seed_order() {
        let base = Pubkey::new_unique();
        let quote = Pubkey::new_unique();
        let expected = Pubkey::find_program_address(
            &[b"pool".as_ref(), base.as_ref(), quote.as_ref()],
            &RAYDIUM_LAUNCHLAB,
        );
        let (pool_addr, bump) = pool_pda(&base, &quote);
        assert_eq!(pool_addr, expected.0);
        assert_eq!(bump, expected.1);
        assert_eq!(pool_addr, pool(&base, &quote));
    }

    #[test]
    fn vault_pda_matches_the_sdk_seed_order() {
        let p = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let expected = Pubkey::find_program_address(
            &[b"pool_vault".as_ref(), p.as_ref(), mint.as_ref()],
            &RAYDIUM_LAUNCHLAB,
        );
        assert_eq!(vault(&p, &mint), expected.0);
    }

    #[test]
    fn vesting_pda_matches_the_sdk_seed_order() {
        let p = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let expected = Pubkey::find_program_address(
            &[b"pool_vesting".as_ref(), p.as_ref(), owner.as_ref()],
            &RAYDIUM_LAUNCHLAB,
        );
        assert_eq!(vesting(&p, &owner), expected.0);
    }

    #[test]
    fn platform_and_creator_vaults_use_their_documented_seed_orders() {
        let platform = Pubkey::new_unique();
        let creator = Pubkey::new_unique();
        let quote = Pubkey::new_unique();

        let expected_platform =
            Pubkey::find_program_address(&[platform.as_ref(), quote.as_ref()], &RAYDIUM_LAUNCHLAB);
        assert_eq!(platform_vault(&platform, &quote), expected_platform.0);

        let expected_creator =
            Pubkey::find_program_address(&[creator.as_ref(), quote.as_ref()], &RAYDIUM_LAUNCHLAB);
        assert_eq!(creator_vault(&creator, &quote), expected_creator.0);

        // The platform config uses the "platform_config" prefix.
        let admin = Pubkey::new_unique();
        let expected_config = Pubkey::find_program_address(
            &[b"platform_config".as_ref(), admin.as_ref()],
            &RAYDIUM_LAUNCHLAB,
        );
        assert_eq!(platform_config(&admin), expected_config.0);
    }

    #[test]
    fn event_authority_matches_the_anchor_convention() {
        let expected =
            Pubkey::find_program_address(&[b"__event_authority".as_ref()], &RAYDIUM_LAUNCHLAB);
        assert_eq!(event_authority(), expected.0);
    }

    #[test]
    fn fee_vault_authorities_are_distinct() {
        assert_ne!(platform_fee_vault_auth(), creator_fee_vault_auth());
    }
}
