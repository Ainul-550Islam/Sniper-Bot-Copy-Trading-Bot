//! The Raydium LaunchLab (`launchpad`) venue.
//!
//! LaunchLab is Raydium's token-launch venue: a bonding curve that can be a
//! constant-product curve, a fixed-price curve or a linear-price curve, and
//! that may later migrate to a CPMM pool once it is fully sold out.
//!
//! The module set is:
//!
//! * [`pda`] — the program-derived addresses.
//! * [`accounts`] — fail-closed decoders for the LaunchLab accounts.
//! * [`curve`] — the exact integer curve maths and fee handling.
//! * [`instructions`] — the four trade instruction builders.
//! * [`migration`] — the route decision and the migration liquidity target.
//! * [`decode`] — the event decoders.
//! * [`adapter`] — the [`VenueAdapter`] implementation.
//!
//! Everything here is derived from the official vendored IDL
//! (`crates/solana-kit/idl/raydium_launchpad.json`, upstream
//! `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`)
//! and the official `raydium-sdk-V2` LaunchLab sources.

pub mod accounts;
pub mod adapter;
pub mod curve;
pub mod decode;
pub mod instructions;
pub mod migration;
pub mod pda;

pub use accounts::{
    encode_synthetic_global_config, encode_synthetic_platform_config,
    encode_synthetic_vesting_record, GlobalConfig, PlatformConfig, PoolState, SyntheticPoolState,
    VestingAccount,
};
pub use adapter::{LaunchLabPool, RaydiumLaunchLab};
pub use curve::{
    calculate_fee, calculate_pre_fee, split_fee, total_fee_rate, ConstantProductCurve, CurveState,
    CurveType, FixedPriceCurve, LinearPriceCurve,
};
pub use decode::{
    to_decoded_swap, to_decoded_swap_from_instruction, to_migration_event, to_pool_creation_event,
    LaunchLabMigrationEvent, LaunchLabPoolCreateEvent, LaunchLabPoolInitEvent,
    LaunchLabSwapInstruction, LaunchLabTradeDirection, LaunchLabTradeEvent,
};
pub use instructions::{
    build_buy_exact_in, build_buy_exact_out, build_sell_exact_in, build_sell_exact_out,
    build_trade_ix, max_in_with_slippage_bps, min_out_with_slippage_bps, TradeAccounts, TradeArgs,
    TradeKind,
};
pub use migration::{
    decide_route, decide_route_with_config, migration_liquidity, migration_liquidity_minimum,
    migration_threshold_met, LaunchLabRoute, MigrationDecision, MigrationTarget,
};
pub use pda::{
    auth, auth_pda, creator_fee_vault_auth, creator_vault, event_authority, global_config,
    global_config_pda, platform_config, platform_config_pda, platform_fee_vault_auth,
    platform_vault, pool, pool_pda, vault, vault_pda, vesting, vesting_pda,
};
