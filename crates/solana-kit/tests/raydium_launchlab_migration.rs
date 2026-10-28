//! Integration tests for the Raydium LaunchLab migration hand-off.
//!
//! The migration decision is a pure function of the decoded pool state, so
//! these tests build **synthetic** pool accounts byte-by-byte. No network
//! access, no real keys, no mainnet data.

use solana_sdk::pubkey::Pubkey;

use solana_kit::consts::*;
use solana_kit::venues::raydium_launchlab::accounts::{
    GlobalConfig, LaunchLabMigrateType, LaunchLabPoolStatus, PoolState, SyntheticPoolState,
};
use solana_kit::venues::raydium_launchlab::migration::{
    decide_route, decide_route_with_config, is_amm_migration, is_awaiting_migration_status,
    is_cpmm_migration, is_migrated_status, migratable_base_amount, migration_liquidity,
    migration_liquidity_minimum, migration_threshold_met, validate_status, LaunchLabRoute,
    MigrationTarget,
};

/// Build a synthetic pool account with the given status and migrate type.
fn pool_with(status: u8, migrate_type: u8) -> PoolState {
    let s = SyntheticPoolState {
        status,
        migrate_type,
        ..Default::default()
    };
    PoolState::decode(&s.encode()).expect("synthetic pool decodes")
}

#[test]
fn a_funding_pool_routes_to_the_launchlab_curve() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
    );
    let d = decide_route(&pool);
    assert_eq!(d.route, LaunchLabRoute::Curve);
    assert_eq!(d.target, MigrationTarget::LaunchLabCurve);
    assert_eq!(d.status, LaunchLabPoolStatus::Fund);
    assert_eq!(d.migrate_type, LaunchLabMigrateType::CpSwap);
    assert!(!d.route.is_migrated());
    assert_eq!(d.route.as_str(), "launchlab_curve");
}

#[test]
fn a_pool_awaiting_migration_is_still_on_the_curve() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_MIGRATE,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
    );
    let d = decide_route(&pool);
    assert_eq!(d.route, LaunchLabRoute::Curve);
    assert_eq!(d.target, MigrationTarget::LaunchLabCurve);
    assert_eq!(d.status, LaunchLabPoolStatus::Migrate);
}

#[test]
fn a_migrated_pool_routes_to_cpswap() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_TRADE,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
    );
    let d = decide_route(&pool);
    assert_eq!(d.route, LaunchLabRoute::Cpmm);
    assert_eq!(d.target, MigrationTarget::RaydiumCpmm);
    assert!(d.route.is_migrated());
    assert_eq!(d.route.as_str(), "raydium_cpmm");
}

#[test]
fn a_migrated_pool_routes_to_amm_v4() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_TRADE,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
    );
    let d = decide_route(&pool);
    assert_eq!(d.route, LaunchLabRoute::AmmV4);
    assert_eq!(d.target, MigrationTarget::RaydiumAmmV4);
    assert!(d.route.is_migrated());
    assert_eq!(d.route.as_str(), "raydium_amm_v4");
}

#[test]
fn the_migratable_base_excludes_sold_and_locked_tokens() {
    let base = SyntheticPoolState::default();
    let mut vesting_schedule = base.vesting_schedule;
    vesting_schedule.total_locked_amount = 100_000;
    let s = SyntheticPoolState {
        supply: 1_000_000,
        total_base_sell: 200_000,
        vesting_schedule,
        ..base
    };
    let pool = PoolState::decode(&s.encode()).unwrap();
    // supply - total_base_sell - total_locked_amount
    assert_eq!(migratable_base_amount(&pool), 700_000);
}

#[test]
fn the_migration_liquidity_is_the_floored_square_root() {
    let base = SyntheticPoolState::default();
    let mut vesting_schedule = base.vesting_schedule;
    vesting_schedule.total_locked_amount = 0;
    let mut s = SyntheticPoolState {
        supply: 1_000_000,
        total_base_sell: 0,
        vesting_schedule,
        total_quote_fund_raising: 1_000_000,
        ..base
    };
    let pool = PoolState::decode(&s.encode()).unwrap();
    // sqrt(1_000_000 * 1_000_000) = 1_000_000 exactly.
    assert_eq!(migration_liquidity(&pool), 1_000_000);

    // A non-square product floors.
    s.total_quote_fund_raising = 3;
    let pool = PoolState::decode(&s.encode()).unwrap();
    // sqrt(1_000_000 * 3) = 1732.05... -> 1732.
    assert_eq!(migration_liquidity(&pool), 1732);
}

#[test]
fn the_amm_v4_minimum_is_ten_to_the_decimals() {
    let mut s = SyntheticPoolState {
        base_decimals: 6,
        migrate_type: RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
        ..Default::default()
    };
    let pool = PoolState::decode(&s.encode()).unwrap();
    assert_eq!(migration_liquidity_minimum(&pool), 1_000_000);

    s.base_decimals = 9;
    let pool = PoolState::decode(&s.encode()).unwrap();
    assert_eq!(migration_liquidity_minimum(&pool), 1_000_000_000);

    s.base_decimals = 0;
    let pool = PoolState::decode(&s.encode()).unwrap();
    assert_eq!(migration_liquidity_minimum(&pool), 1);
}

#[test]
fn the_cpmm_minimum_is_one_hundred() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
    );
    assert_eq!(migration_liquidity_minimum(&pool), 100);
}

#[test]
fn the_threshold_is_strictly_greater_than_the_minimum() {
    let base = SyntheticPoolState::default();
    let mut vesting_schedule = base.vesting_schedule;
    vesting_schedule.total_locked_amount = 0;
    let mut s = SyntheticPoolState {
        supply: 100,
        total_base_sell: 0,
        vesting_schedule,
        total_quote_fund_raising: 100,
        migrate_type: RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
        ..base
    };
    let pool = PoolState::decode(&s.encode()).unwrap();
    // liquidity = sqrt(100 * 100) = 100, which is NOT > 100.
    assert_eq!(migration_liquidity(&pool), 100);
    assert!(!migration_threshold_met(&pool));

    s.total_quote_fund_raising = 102;
    let pool = PoolState::decode(&s.encode()).unwrap();
    // sqrt(100 * 102) = 100 (floored), still not > 100.
    assert_eq!(migration_liquidity(&pool), 100);
    assert!(!migration_threshold_met(&pool));

    s.supply = 200;
    s.total_quote_fund_raising = 200;
    let pool = PoolState::decode(&s.encode()).unwrap();
    // sqrt(200 * 200) = 200 > 100 -> the threshold is met.
    assert_eq!(migration_liquidity(&pool), 200);
    assert!(migration_threshold_met(&pool));
}

#[test]
fn the_raw_byte_helpers_match_the_typed_ones() {
    assert!(is_amm_migration(RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM));
    assert!(!is_amm_migration(RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP));
    assert!(is_cpmm_migration(RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP));
    assert!(!is_cpmm_migration(RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM));

    assert!(is_migrated_status(RAYDIUM_LAUNCHLAB_STATUS_TRADE));
    assert!(!is_migrated_status(RAYDIUM_LAUNCHLAB_STATUS_FUND));
    assert!(is_awaiting_migration_status(
        RAYDIUM_LAUNCHLAB_STATUS_MIGRATE
    ));
    assert!(!is_awaiting_migration_status(
        RAYDIUM_LAUNCHLAB_STATUS_TRADE
    ));
}

#[test]
fn an_unknown_status_byte_is_refused() {
    assert!(validate_status(RAYDIUM_LAUNCHLAB_STATUS_FUND).is_ok());
    assert!(validate_status(RAYDIUM_LAUNCHLAB_STATUS_MIGRATE).is_ok());
    assert!(validate_status(RAYDIUM_LAUNCHLAB_STATUS_TRADE).is_ok());
    let err = validate_status(9).expect_err("unknown status");
    assert!(err.to_string().contains("status"), "{err}");
}

#[test]
fn the_decision_is_checked_against_the_global_config() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
    );
    let config = GlobalConfig::decode(
        &solana_kit::venues::raydium_launchlab::accounts::encode_synthetic_global_config(
            RAYDIUM_LAUNCHLAB_CURVE_CONSTANT,
            &Pubkey::new_unique(),
        ),
    )
    .expect("synthetic global config decodes");

    let d = decide_route_with_config(&pool, &config).expect("decision succeeds");
    assert_eq!(d.route, LaunchLabRoute::Curve);
    assert_eq!(d.target, MigrationTarget::LaunchLabCurve);
}

#[test]
fn a_global_config_with_an_unknown_curve_type_is_refused() {
    // The `GlobalConfig` decoder itself refuses a curve type the program does
    // not define, so an unknown curve never reaches the migration decision.
    let err = GlobalConfig::decode(
        &solana_kit::venues::raydium_launchlab::accounts::encode_synthetic_global_config(
            9,
            &Pubkey::new_unique(),
        ),
    )
    .expect_err("an unknown curve type must be refused");
    let msg = err.to_string();
    assert!(msg.contains("curve_type"), "{msg}");
}

#[test]
fn the_migration_target_labels_are_stable() {
    assert_eq!(MigrationTarget::LaunchLabCurve.as_str(), "launchlab_curve");
    assert_eq!(MigrationTarget::RaydiumCpmm.as_str(), "raydium_cpmm");
    assert_eq!(MigrationTarget::RaydiumAmmV4.as_str(), "raydium_amm_v4");
}

#[test]
fn a_migrated_pool_reports_the_same_route_through_the_config_path() {
    let pool = pool_with(
        RAYDIUM_LAUNCHLAB_STATUS_TRADE,
        RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
    );
    let config = GlobalConfig::decode(
        &solana_kit::venues::raydium_launchlab::accounts::encode_synthetic_global_config(
            RAYDIUM_LAUNCHLAB_CURVE_CONSTANT,
            &Pubkey::new_unique(),
        ),
    )
    .unwrap();

    let d = decide_route_with_config(&pool, &config).expect("decision succeeds");
    assert_eq!(d.route, LaunchLabRoute::Cpmm);
    assert_eq!(d.target, MigrationTarget::RaydiumCpmm);
    assert_eq!(d.status, LaunchLabPoolStatus::Trade);
}
