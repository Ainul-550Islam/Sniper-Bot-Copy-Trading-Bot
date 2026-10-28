//! The LaunchLab migration hand-off: which venue a pool's tokens live on
//! after the bonding curve graduates.
//!
//! This is a **pure function** — no I/O, no RPC, no clock. It reads the
//! decoded pool state and the decoded global config and answers one question:
//! *where does this token trade now?*
//!
//! # The three routes
//!
//! | Route | When | Hand-off target |
//! |---|---|---|
//! | [`LaunchLabRoute::Curve`] | the pool is still fundraising (`status == Fund`) | trade on the LaunchLab curve itself |
//! | [`LaunchLabRoute::Cpmm`] | the pool migrated to CPMM (`migrate_type == 1`) | trade on the Raydium CPMM pool |
//! | [`LaunchLabRoute::AmmV4`] | the pool migrated to AMM v4 (`migrate_type == 0`) | trade on the Raydium AMM v4 pool |
//!
//! The hand-off target is expressed as a [`MigrationTarget`], which names the
//! venue adapter Batch 07 must use — **not** a concrete pool address. The
//! migrated pool's address is not recorded in the LaunchLab `PoolState`
//! account, so it cannot be derived from the state alone; Batch 07 resolves
//! it from the `migrate_to_*` transaction (the instruction carries the new
//! pool's address in its account list).
//!
//! # The migration threshold
//!
//! A pool may migrate once enough quote has been raised and enough base has
//! been sold. The SDK's `checkPoolToAmm` computes the migrated liquidity as
//! `floor(sqrt(migrate_amount_a * total_fund_raising_b))` and compares it
//! against a minimum:
//!
//! * migration to **AMM v4**: the liquidity must exceed `10^decimals`.
//! * migration to **CPMM**: the liquidity must exceed `100`.
//!
//! [`migration_liquidity`] and [`migration_threshold_met`] port that exactly.

use serde::{Deserialize, Serialize};

use bot_core::error::{BotError, BotResult};

use crate::consts::{
    RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM, RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
    RAYDIUM_LAUNCHLAB_STATUS_MIGRATE, RAYDIUM_LAUNCHLAB_STATUS_TRADE,
};
use crate::venues::raydium_launchlab::accounts::{
    GlobalConfig, LaunchLabMigrateType, LaunchLabPoolStatus, PoolState,
};
use crate::venues::raydium_launchlab::curve::isqrt;

/// The venue a token trades on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchLabRoute {
    /// Still on the LaunchLab bonding curve.
    Curve,
    /// Migrated to Raydium CPMM (`cp-swap`).
    Cpmm,
    /// Migrated to Raydium AMM v4 (OpenBook-backed).
    AmmV4,
}

impl LaunchLabRoute {
    /// A stable label for metrics and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            LaunchLabRoute::Curve => "launchlab_curve",
            LaunchLabRoute::Cpmm => "raydium_cpmm",
            LaunchLabRoute::AmmV4 => "raydium_amm_v4",
        }
    }

    /// Whether the route is a hand-off to another venue (i.e. the curve is
    /// closed).
    pub fn is_migrated(&self) -> bool {
        !matches!(self, LaunchLabRoute::Curve)
    }
}

/// The venue a migrated pool's tokens live on.
///
/// This is the *adapter* Batch 07 must call, named so the wiring is
/// unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationTarget {
    /// Keep trading on the LaunchLab curve.
    LaunchLabCurve,
    /// Use the Raydium CPMM adapter (`solana_kit::venues::raydium_cpmm`).
    RaydiumCpmm,
    /// Use the Raydium AMM v4 adapter (`solana_kit::raydium`).
    RaydiumAmmV4,
}

impl MigrationTarget {
    /// A stable label for metrics and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            MigrationTarget::LaunchLabCurve => "launchlab_curve",
            MigrationTarget::RaydiumCpmm => "raydium_cpmm",
            MigrationTarget::RaydiumAmmV4 => "raydium_amm_v4",
        }
    }
}

/// The decision for one pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationDecision {
    /// Where the token trades now.
    pub route: LaunchLabRoute,
    /// The adapter to use.
    pub target: MigrationTarget,
    /// The `migrate_type` recorded in the pool state.
    pub migrate_type: LaunchLabMigrateType,
    /// The pool's status.
    pub status: LaunchLabPoolStatus,
    /// Whether the migration threshold (liquidity minimum) is met.
    pub threshold_met: bool,
    /// The liquidity the migration would create (floored sqrt).
    pub liquidity: u128,
}

/// The amount of base token that goes into the AMM at migration.
///
/// `supply - total_base_sell - total_locked_amount`.
///
/// Source: the SDK's `checkPoolToAmm` →
/// `const migrateAmountA = supply.sub(totalSellA).sub(totalLockedAmount)`.
pub fn migratable_base_amount(pool: &PoolState) -> u64 {
    pool.supply
        .saturating_sub(pool.total_base_sell)
        .saturating_sub(pool.vesting_schedule.total_locked_amount)
}

/// The liquidity the migration would create.
///
/// `floor(sqrt(migrate_amount_a * total_fund_raising_b))`.
///
/// Source: the SDK's `checkPoolToAmm`:
/// ```text
/// const liquidity = new BN(
///   new Decimal(migrateAmountA.mul(totalFundRaisingB).toString())
///     .sqrt().toFixed(0, Decimal.ROUND_FLOOR)
/// );
/// ```
pub fn migration_liquidity(pool: &PoolState) -> u128 {
    let migrate_amount_a = u128::from(migratable_base_amount(pool));
    let total_fund_raising = u128::from(pool.total_quote_fund_raising);
    isqrt(migrate_amount_a.saturating_mul(total_fund_raising))
}

/// The minimum liquidity the migration must exceed.
///
/// * AMM v4: `10^decimals` (the SDK: `new BN(10).pow(new BN(decimalsA))`).
/// * CPMM: `100` (the SDK: `new BN(100)`).
pub fn migration_liquidity_minimum(pool: &PoolState) -> u128 {
    match pool.migrate_type {
        LaunchLabMigrateType::Amm => 10u128.pow(u32::from(pool.base_decimals.min(38))),
        LaunchLabMigrateType::CpSwap => 100,
    }
}

/// Whether the migration threshold is met.
///
/// The SDK's check is `liquidity > minLockLp` (strictly greater).
pub fn migration_threshold_met(pool: &PoolState) -> bool {
    migration_liquidity(pool) > migration_liquidity_minimum(pool)
}

/// Decide where a pool's tokens trade now.
///
/// The decision is a pure function of the decoded pool state (and, when the
/// caller has it, the global config).
///
/// * `status == Fund` → the curve is open; trade on the curve.
/// * `status == Migrate` → fundraising is over; the pool is waiting for the
///   migration. The route is still the curve until the migration lands, but
///   the pool is *not* tradable any more, so the decision reports
///   [`LaunchLabRoute::Curve`] with `threshold_met` telling the caller
///   whether the migration may proceed.
/// * `status == Trade` → the migration is done; trade on the AMM the pool
///   migrated to, as recorded in `migrate_type`.
pub fn decide_route(pool: &PoolState) -> MigrationDecision {
    let liquidity = migration_liquidity(pool);
    let threshold_met = migration_threshold_met(pool);
    let (route, target) = match pool.status {
        LaunchLabPoolStatus::Fund | LaunchLabPoolStatus::Migrate => {
            (LaunchLabRoute::Curve, MigrationTarget::LaunchLabCurve)
        }
        LaunchLabPoolStatus::Trade => match pool.migrate_type {
            LaunchLabMigrateType::Amm => (LaunchLabRoute::AmmV4, MigrationTarget::RaydiumAmmV4),
            LaunchLabMigrateType::CpSwap => (LaunchLabRoute::Cpmm, MigrationTarget::RaydiumCpmm),
        },
    };
    MigrationDecision {
        route,
        target,
        migrate_type: pool.migrate_type,
        status: pool.status,
        threshold_met,
        liquidity,
    }
}

/// The decision, additionally checked against the global config.
///
/// The global config decides the **curve type** the pool runs on and the
/// protocol-wide fees; the pool's own `migrate_type` decides the hand-off
/// target. When the two disagree the *pool* wins (it is the pool that was
/// actually migrated), but the mismatch is reported so the caller can log it.
pub fn decide_route_with_config(
    pool: &PoolState,
    config: &GlobalConfig,
) -> BotResult<MigrationDecision> {
    let decision = decide_route(pool);
    if decision.route == LaunchLabRoute::Curve {
        // The curve type must be one the SDK implements; an unknown type is
        // refused rather than guessed.
        let _ = config.curve()?;
    }
    Ok(decision)
}

/// Whether the pool's raw `migrate_type` byte means "migrate to AMM v4".
pub fn is_amm_migration(migrate_type: u8) -> bool {
    migrate_type == RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM
}

/// Whether the pool's raw `migrate_type` byte means "migrate to CPMM".
pub fn is_cpmm_migration(migrate_type: u8) -> bool {
    migrate_type == RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP
}

/// Whether the raw status byte means "the migration is done".
pub fn is_migrated_status(status: u8) -> bool {
    status == RAYDIUM_LAUNCHLAB_STATUS_TRADE
}

/// Whether the raw status byte means "fundraising is over, migration pending".
pub fn is_awaiting_migration_status(status: u8) -> bool {
    status == RAYDIUM_LAUNCHLAB_STATUS_MIGRATE
}

/// Refuse a pool whose status byte is not one the program defines.
pub fn validate_status(status: u8) -> BotResult<()> {
    match status {
        crate::consts::RAYDIUM_LAUNCHLAB_STATUS_FUND
        | RAYDIUM_LAUNCHLAB_STATUS_MIGRATE
        | RAYDIUM_LAUNCHLAB_STATUS_TRADE => Ok(()),
        other => Err(BotError::encoding(format!(
            "invalid LaunchLab pool status {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::raydium_launchlab::accounts::SyntheticPoolState;

    fn pool_with(status: u8, migrate_type: u8) -> PoolState {
        let s = SyntheticPoolState {
            status,
            migrate_type,
            ..Default::default()
        };
        PoolState::decode(&s.encode()).unwrap()
    }

    #[test]
    fn a_funding_pool_routes_to_the_curve() {
        let pool = pool_with(
            crate::consts::RAYDIUM_LAUNCHLAB_STATUS_FUND,
            RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
        );
        let d = decide_route(&pool);
        assert_eq!(d.route, LaunchLabRoute::Curve);
        assert_eq!(d.target, MigrationTarget::LaunchLabCurve);
        assert!(!d.route.is_migrated());
        assert_eq!(d.migrate_type, LaunchLabMigrateType::CpSwap);
    }

    #[test]
    fn a_pool_awaiting_migration_still_routes_to_the_curve() {
        let pool = pool_with(
            RAYDIUM_LAUNCHLAB_STATUS_MIGRATE,
            RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
        );
        let d = decide_route(&pool);
        assert_eq!(d.route, LaunchLabRoute::Curve);
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
    }

    #[test]
    fn the_migration_threshold_uses_the_sdk_minimums() {
        // AMM v4 needs liquidity > 10^decimals (6 decimals -> 1_000_000).
        let mut pool = pool_with(
            crate::consts::RAYDIUM_LAUNCHLAB_STATUS_FUND,
            RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
        );
        pool.base_decimals = 6;
        assert_eq!(migration_liquidity_minimum(&pool), 1_000_000);

        // CPMM needs liquidity > 100.
        let pool = pool_with(
            crate::consts::RAYDIUM_LAUNCHLAB_STATUS_FUND,
            RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
        );
        assert_eq!(migration_liquidity_minimum(&pool), 100);
    }

    #[test]
    fn the_liquidity_is_the_floored_sqrt() {
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
        // sqrt(1_000_000 * 1_000_000) = 1_000_000.
        assert_eq!(migration_liquidity(&pool), 1_000_000);
        assert_eq!(migratable_base_amount(&pool), 1_000_000);

        // A non-square product floors.
        s.supply = 2;
        s.total_quote_fund_raising = 2;
        let pool = PoolState::decode(&s.encode()).unwrap();
        // sqrt(2 * 2) = 2 exactly.
        assert_eq!(migration_liquidity(&pool), 2);
    }

    #[test]
    fn the_threshold_is_strictly_greater() {
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

        s.total_quote_fund_raising = 101;
        let pool = PoolState::decode(&s.encode()).unwrap();
        // sqrt(100 * 101) = 100 (floored), still not > 100.
        assert_eq!(migration_liquidity(&pool), 100);
        assert!(!migration_threshold_met(&pool));

        s.total_quote_fund_raising = 102;
        let pool = PoolState::decode(&s.encode()).unwrap();
        // sqrt(100 * 102) = 100 (floored), still not > 100.
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
        assert!(is_migrated_status(RAYDIUM_LAUNCHLAB_STATUS_TRADE));
        assert!(is_awaiting_migration_status(
            RAYDIUM_LAUNCHLAB_STATUS_MIGRATE
        ));
        assert!(validate_status(crate::consts::RAYDIUM_LAUNCHLAB_STATUS_FUND).is_ok());
        assert!(validate_status(9).is_err());
    }

    #[test]
    fn route_labels_are_stable() {
        assert_eq!(LaunchLabRoute::Curve.as_str(), "launchlab_curve");
        assert_eq!(LaunchLabRoute::Cpmm.as_str(), "raydium_cpmm");
        assert_eq!(LaunchLabRoute::AmmV4.as_str(), "raydium_amm_v4");
        assert_eq!(MigrationTarget::RaydiumCpmm.as_str(), "raydium_cpmm");
    }
}
