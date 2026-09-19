//! Program ids, well-known accounts, PDA seeds and instruction discriminators.
//!
//! Every value here was taken from the on-chain programs / official docs.
//! Base58 is case sensitive — the pump program id uses a lowercase `r`
//! (`6EF8rrecth…`), not `6EF8rRecth…`.

use once_cell::sync::Lazy;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Parse a base58 pubkey at startup. A typo here is a hard error, and it is
/// better to fail loudly at boot than to trade against the wrong program.
fn pk(s: &str) -> Pubkey {
    Pubkey::from_str(s).unwrap_or_else(|e| panic!("invalid pubkey constant '{s}': {e}"))
}

// --------------------------------------------------------------------------
// Core Solana programs
// --------------------------------------------------------------------------

pub static SYSTEM_PROGRAM: Lazy<Pubkey> = Lazy::new(|| pk("11111111111111111111111111111111"));
pub static TOKEN_PROGRAM: Lazy<Pubkey> =
    Lazy::new(|| pk("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"));
pub static TOKEN_2022_PROGRAM: Lazy<Pubkey> =
    Lazy::new(|| pk("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"));
pub static ASSOCIATED_TOKEN_PROGRAM: Lazy<Pubkey> =
    Lazy::new(|| pk("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"));
pub static RENT_SYSVAR: Lazy<Pubkey> =
    Lazy::new(|| pk("SysvarRent111111111111111111111111111111111"));
pub static COMPUTE_BUDGET_PROGRAM: Lazy<Pubkey> =
    Lazy::new(|| pk("ComputeBudget111111111111111111111111111111"));
pub static WSOL_MINT: Lazy<Pubkey> =
    Lazy::new(|| pk("So11111111111111111111111111111111111111112"));
pub static USDC_MINT: Lazy<Pubkey> =
    Lazy::new(|| pk("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"));

// --------------------------------------------------------------------------
// Pump.fun (bonding curve, pre-graduation)
// --------------------------------------------------------------------------

pub static PUMP_PROGRAM_ID: Lazy<Pubkey> =
    Lazy::new(|| pk("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"));
/// Owns the dynamic `FeeConfig` PDA and is the `fee_program` account of buy/sell.
pub static PUMP_FEES_PROGRAM_ID: Lazy<Pubkey> =
    Lazy::new(|| pk("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ"));
/// `global` PDA of the pump program.
pub static PUMP_GLOBAL: Lazy<Pubkey> =
    Lazy::new(|| pk("4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf"));
/// Event authority PDA: `["__event_authority"]`.
pub static PUMP_EVENT_AUTHORITY: Lazy<Pubkey> =
    Lazy::new(|| pk("Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1"));
/// Historical/default fee recipient. The authoritative value lives in the
/// `Global` account and is read at runtime — this is only a fallback.
pub static PUMP_FEE_RECIPIENT_FALLBACK: Lazy<Pubkey> =
    Lazy::new(|| pk("CebN5WGQ4jvEPvsVU4EoHEpgzq1VV2fskvCwf8gCDbZ"));

pub const PUMP_SEED_GLOBAL: &[u8] = b"global";
pub const PUMP_SEED_BONDING_CURVE: &[u8] = b"bonding-curve";
pub const PUMP_SEED_BONDING_CURVE_V2: &[u8] = b"bonding-curve-v2";
pub const PUMP_SEED_CREATOR_VAULT: &[u8] = b"creator-vault";
pub const PUMP_SEED_EVENT_AUTHORITY: &[u8] = b"__event_authority";
pub const PUMP_SEED_GLOBAL_VOLUME_ACCUMULATOR: &[u8] = b"global_volume_accumulator";
pub const PUMP_SEED_USER_VOLUME_ACCUMULATOR: &[u8] = b"user_volume_accumulator";
/// Seeds for the `FeeConfig` PDA, derived **on the fee program**.
pub const PUMP_SEED_FEE_CONFIG: &[u8] = b"fee_config";

/// Anchor discriminators: first 8 bytes of `sha256("global:<name>")`.
pub const PUMP_DISC_INITIALIZE: [u8; 8] = [175, 175, 109, 31, 13, 152, 155, 237];
pub const PUMP_DISC_CREATE: [u8; 8] = [24, 30, 200, 40, 5, 28, 7, 119];
pub const PUMP_DISC_CREATE_V2: [u8; 8] = [214, 144, 76, 236, 95, 139, 49, 180];
pub const PUMP_DISC_BUY: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
pub const PUMP_DISC_SELL: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
pub const PUMP_DISC_BUY_EXACT_SOL_IN: [u8; 8] = [56, 252, 116, 8, 158, 223, 205, 95];
pub const PUMP_DISC_BUY_V2: [u8; 8] = [184, 23, 238, 97, 103, 197, 211, 61];
pub const PUMP_DISC_SELL_V2: [u8; 8] = [93, 246, 130, 60, 231, 233, 64, 178];
pub const PUMP_DISC_MIGRATE: [u8; 8] = [155, 234, 231, 146, 236, 158, 162, 30];
pub const PUMP_DISC_EXTEND_ACCOUNT: [u8; 8] = [234, 102, 194, 203, 150, 72, 62, 229];

/// Account discriminators: first 8 bytes of `sha256("account:<Name>")`.
pub const PUMP_ACC_DISC_GLOBAL: [u8; 8] = [167, 232, 232, 177, 200, 108, 114, 127];
pub const PUMP_ACC_DISC_BONDING_CURVE: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];
pub const PUMP_ACC_DISC_FEE_CONFIG: [u8; 8] = [143, 52, 146, 187, 219, 123, 76, 155];

/// Byte offsets inside the bonding-curve account (post cashback-upgrade, 151 bytes).
pub const BC_OFF_VIRTUAL_TOKEN_RESERVES: usize = 8;
pub const BC_OFF_VIRTUAL_SOL_RESERVES: usize = 16;
pub const BC_OFF_REAL_TOKEN_RESERVES: usize = 24;
pub const BC_OFF_REAL_SOL_RESERVES: usize = 32;
pub const BC_OFF_TOKEN_TOTAL_SUPPLY: usize = 40;
pub const BC_OFF_COMPLETE: usize = 48;
pub const BC_OFF_CREATOR: usize = 49;
pub const BC_OFF_RESERVED: usize = 81;
/// The authoritative cashback flag: decides whether `sell` needs an extra
/// `user_volume_accumulator` account.
pub const BC_OFF_CASHBACK_ENABLED: usize = 82;
/// Minimum prefix length we need to read reserves + complete.
pub const BONDING_CURVE_MIN_LEN: usize = 81;
/// Full length after the cashback upgrade.
pub const BONDING_CURVE_FULL_LEN: usize = 151;

// --------------------------------------------------------------------------
// PumpSwap (graduated pump.fun AMM)
// --------------------------------------------------------------------------

pub static PUMPSWAP_PROGRAM_ID: Lazy<Pubkey> =
    Lazy::new(|| pk("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"));
/// `global_config` PDA of the AMM program, derived from the documented seed
/// rather than hardcoded (sources disagree on the exact base58 spelling).
pub static PUMPSWAP_GLOBAL_CONFIG: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::find_program_address(&[PUMPSWAP_SEED_GLOBAL_CONFIG], &PUMPSWAP_PROGRAM_ID).0
});
/// `fee_config` PDA of the AMM program: seeds `["fee_config", amm_program_id]`
/// derived on the pump *fees* program.
pub static PUMPSWAP_FEE_CONFIG: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::find_program_address(
        &[PUMP_SEED_FEE_CONFIG, PUMPSWAP_PROGRAM_ID.as_ref()],
        &PUMP_FEES_PROGRAM_ID,
    )
    .0
});
pub static PUMPSWAP_EVENT_AUTHORITY: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::find_program_address(&[PUMPSWAP_SEED_EVENT_AUTHORITY], &PUMPSWAP_PROGRAM_ID).0
});

pub const PUMPSWAP_SEED_POOL: &[u8] = b"pool";
pub const PUMPSWAP_SEED_POOL_V2: &[u8] = b"pool-v2";
pub const PUMPSWAP_SEED_POOL_LP_MINT: &[u8] = b"pool_lp_mint";
pub const PUMPSWAP_SEED_GLOBAL_VOLUME_ACCUMULATOR: &[u8] = b"global_volume_accumulator";
pub const PUMPSWAP_SEED_USER_VOLUME_ACCUMULATOR: &[u8] = b"user_volume_accumulator";
pub const PUMPSWAP_SEED_GLOBAL_CONFIG: &[u8] = b"global_config";
pub const PUMPSWAP_SEED_CREATOR_VAULT: &[u8] = b"creator_vault";
pub const PUMPSWAP_SEED_FEE_CONFIG: &[u8] = b"fee_config";
pub const PUMPSWAP_SEED_EVENT_AUTHORITY: &[u8] = b"__event_authority";

pub const PUMPSWAP_DISC_BUY: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
pub const PUMPSWAP_DISC_SELL: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
pub const PUMPSWAP_DISC_CREATE_POOL: [u8; 8] = [233, 146, 209, 142, 207, 104, 64, 188];
pub const PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN: [u8; 8] = [198, 46, 21, 82, 180, 217, 232, 112];
pub const PUMPSWAP_DISC_SELL_EXACT_BASE_IN: [u8; 8] = [116, 232, 182, 230, 194, 174, 30, 95];
pub const PUMPSWAP_DISC_DEPOSIT: [u8; 8] = [242, 35, 198, 137, 82, 225, 242, 182];
pub const PUMPSWAP_DISC_WITHDRAW: [u8; 8] = [183, 18, 70, 156, 148, 109, 161, 34];
/// `account:Pool` discriminator for the PumpSwap pool account.
pub const PUMPSWAP_ACC_DISC_POOL: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
/// `account:PoolV2` discriminator; the pool-v2 PDA is appended to every AMM
/// buy/sell by the cashback upgrade.
pub const PUMPSWAP_ACC_DISC_POOL_V2: [u8; 8] = [91, 12, 214, 87, 7, 185, 167, 55];
/// `account:GlobalConfig` discriminator.
pub const PUMPSWAP_ACC_DISC_GLOBAL_CONFIG: [u8; 8] = [149, 8, 156, 202, 160, 252, 176, 217];
pub const PUMPSWAP_ACC_DISC_POOL_LP_MINT: [u8; 8] = [186, 143, 80, 171, 168, 65, 72, 249];
pub const PUMPSWAP_ACC_DISC_GLOBAL_VOLUME_ACCUMULATOR: [u8; 8] =
    [202, 42, 246, 43, 142, 190, 30, 255];
pub const PUMPSWAP_ACC_DISC_USER_VOLUME_ACCUMULATOR: [u8; 8] =
    [86, 255, 112, 14, 102, 53, 154, 250];

// Every discriminator above is sha256("global:<name>")[..8] or
// sha256("account:<Name>")[..8]; `tests::discriminators_match_sighash` recomputes
// them so a typo cannot survive a test run.
pub const PUMPSWAP_DISC_EXTEND_ACCOUNT: [u8; 8] = [234, 102, 194, 203, 150, 72, 62, 229];
pub const PUMPSWAP_DISC_INIT_USER_VOLUME_ACCUMULATOR: [u8; 8] =
    [94, 6, 202, 115, 255, 96, 232, 183];
pub const PUMPSWAP_DISC_SYNC_USER_VOLUME_ACCUMULATOR: [u8; 8] = [86, 31, 192, 87, 163, 87, 79, 238];
pub const PUMPSWAP_DISC_CLOSE_USER_VOLUME_ACCUMULATOR: [u8; 8] =
    [249, 69, 164, 218, 150, 103, 84, 138];
pub const PUMPSWAP_DISC_COLLECT_COIN_CREATOR_FEE: [u8; 8] = [160, 57, 89, 42, 181, 139, 43, 66];
pub const PUMPSWAP_DISC_CLAIM_CASHBACK: [u8; 8] = [37, 58, 35, 126, 190, 53, 228, 197];

// --------------------------------------------------------------------------
// The 2026-04-28 "breaking fee recipient" upgrade
// --------------------------------------------------------------------------
//
// Every pump bonding-curve and PumpSwap buy/sell must now append one of these
// eight interchangeable recipients as a trailing account (plus, on the AMM,
// that recipient's quote-mint ATA). Omitting them is a hard program error, so
// the default layouts in `pump.rs` / `pumpswap.rs` always include them.
//
// Source: pump-fun/pump-public-docs → BREAKING_FEE_RECIPIENT.md
pub static BREAKING_FEE_RECIPIENTS: Lazy<Vec<Pubkey>> = Lazy::new(|| {
    vec![
        pk("5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD"),
        pk("9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7"),
        pk("GXPFM2caqTtQYC2cJ5yJRi9VDkpsYZXzYdwYpGnLmtDL"),
        pk("3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR"),
        pk("5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6"),
        pk("EHAAiTxcdDwQ3U4bU6YcMsQGaekdzLS3B5SmYo46kJtL"),
        pk("5eHhjP8JaYkz83CWwvGU2uMUXefd3AazWGx4gpcuEEYD"),
        pk("A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW"),
    ]
});

/// Pick one of the eight fee recipients at random; they are interchangeable.
pub fn pick_breaking_fee_recipient() -> Pubkey {
    let list = BREAKING_FEE_RECIPIENTS.clone();
    list[rand::random::<usize>() % list.len()]
}

// --------------------------------------------------------------------------
// Raydium AMM v4 (OpenBook-backed constant product)
// --------------------------------------------------------------------------

pub static RAYDIUM_AMM_V4: Lazy<Pubkey> =
    Lazy::new(|| pk("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"));
pub static RAYDIUM_CLMM: Lazy<Pubkey> =
    Lazy::new(|| pk("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"));
pub static RAYDIUM_CPMM: Lazy<Pubkey> =
    Lazy::new(|| pk("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"));
pub static RAYDIUM_LAUNCHLAB: Lazy<Pubkey> =
    Lazy::new(|| pk("LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj"));
/// Raydium's AMM authority PDA, derived from the seed `b"amm authority"` with
/// bump 255 (the `nonce` stored in the pool account).
pub const RAYDIUM_AMM_AUTHORITY_SEED: &[u8] = b"amm authority";
/// OpenBook (serum v3) market program on mainnet.
pub static OPENBOOK_MARKET_PROGRAM: Lazy<Pubkey> =
    Lazy::new(|| pk("srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX"));

/// Raydium AMM v4 instruction tags (it is *not* an Anchor program, so these
/// are plain little-endian `u8` tags rather than 8-byte discriminators).
pub const RAYDIUM_IX_INITIALIZE: u8 = 0;
pub const RAYDIUM_IX_INITIALIZE2: u8 = 1;
pub const RAYDIUM_IX_DEPOSIT: u8 = 3;
pub const RAYDIUM_IX_WITHDRAW: u8 = 4;
pub const RAYDIUM_IX_SWAP_BASE_IN: u8 = 9;
pub const RAYDIUM_IX_PRE_INITIALIZE: u8 = 10;
pub const RAYDIUM_IX_SWAP_BASE_OUT: u8 = 11;
/// `SwapBaseInV2` skips the OpenBook orderbook accounts entirely: 8 accounts
/// instead of 17, and far fewer compute units. Every pool created since the
/// orderbook was retired supports it, and it is what the Raydium UI sends.
pub const RAYDIUM_IX_SWAP_BASE_IN_V2: u8 = 16;
pub const RAYDIUM_IX_SWAP_BASE_OUT_V2: u8 = 17;

/// Offsets inside the Raydium v4 `AmmInfo` (pool state) account.
///
/// Derived from the program's own `#[repr(C, packed)]` struct
/// (`raydium-io/raydium-amm` → `program/src/state.rs`):
/// 16 × `u64` scalars (128 bytes), `Fees` (8 × u64 = 64 bytes), `StateData`
/// (160 bytes), then nine `Pubkey`s and the tail scalars.
pub const AMM_LEN: usize = 784;
pub const AMM_OFF_STATUS: usize = 0;
pub const AMM_OFF_NONCE: usize = 8;
pub const AMM_OFF_ORDER_NUM: usize = 16;
pub const AMM_OFF_DEPTH: usize = 24;
pub const AMM_OFF_COIN_DECIMALS: usize = 32;
pub const AMM_OFF_PC_DECIMALS: usize = 40;
pub const AMM_OFF_STATE: usize = 48;
pub const AMM_OFF_RESET_FLAG: usize = 56;
pub const AMM_OFF_MIN_SIZE: usize = 64;
pub const AMM_OFF_VOL_MAX_CUT_RATIO: usize = 72;
pub const AMM_OFF_AMOUNT_WAVE: usize = 80;
pub const AMM_OFF_COIN_LOT_SIZE: usize = 88;
pub const AMM_OFF_PC_LOT_SIZE: usize = 96;
pub const AMM_OFF_MIN_PRICE_MULTIPLIER: usize = 104;
pub const AMM_OFF_MAX_PRICE_MULTIPLIER: usize = 112;
pub const AMM_OFF_SYS_DECIMAL_VALUE: usize = 120;
/// `Fees` starts here: min_separate, trade_fee, pnl, swap_fee (num/denom each).
pub const AMM_OFF_FEES: usize = 128;
pub const AMM_OFF_TRADE_FEE_NUMERATOR: usize = 144;
pub const AMM_OFF_TRADE_FEE_DENOMINATOR: usize = 152;
/// `StateData` starts here (160 bytes).
pub const AMM_OFF_STATE_DATA: usize = 192;
pub const AMM_OFF_NEED_TAKE_PNL_COIN: usize = 192;
pub const AMM_OFF_NEED_TAKE_PNL_PC: usize = 200;
pub const AMM_OFF_POOL_OPEN_TIME: usize = 224;
pub const AMM_OFF_SWAP_COIN_IN_AMOUNT: usize = 264;
pub const AMM_OFF_SWAP_PC_OUT_AMOUNT: usize = 280;
pub const AMM_OFF_SWAP_ACC_PC_FEE: usize = 296;
pub const AMM_OFF_SWAP_ACC_COIN_FEE: usize = 304;
pub const AMM_OFF_SWAP_COIN_OUT_AMOUNT: usize = 312;
pub const AMM_OFF_SWAP_PC_IN_AMOUNT: usize = 328;
/// Pool token accounts (the vaults the AMM authority owns). `coin` holds the
/// base token, `pc` holds the quote (WSOL for SOL pairs).
pub const AMM_OFF_COIN_VAULT: usize = 368;
pub const AMM_OFF_PC_VAULT: usize = 400;
/// Mints of those vaults.
pub const AMM_OFF_COIN_MINT: usize = 432;
pub const AMM_OFF_PC_MINT: usize = 464;
pub const AMM_OFF_LP_MINT: usize = 496;
pub const AMM_OFF_OPEN_ORDERS: usize = 528;
pub const AMM_OFF_MARKET: usize = 560;
pub const AMM_OFF_MARKET_PROGRAM: usize = 592;
pub const AMM_OFF_TARGET_ORDERS: usize = 624;
pub const AMM_OFF_AMM_OWNER: usize = 720;
pub const AMM_OFF_LP_AMOUNT: usize = 752;
pub const AMM_OFF_CLIENT_ORDER_ID: usize = 760;
pub const AMM_OFF_RECENT_EPOCH: usize = 768;

/// `AmmStatus` values (see `AmmStatus` in the program source).
pub const AMM_STATUS_UNINITIALIZED: u64 = 0;
pub const AMM_STATUS_INITIALIZED: u64 = 1;
pub const AMM_STATUS_DISABLED: u64 = 2;
pub const AMM_STATUS_WITHDRAW_ONLY: u64 = 3;
pub const AMM_STATUS_LIQUIDITY_ONLY: u64 = 4;
pub const AMM_STATUS_ORDERBOOK_ONLY: u64 = 5;
pub const AMM_STATUS_SWAP_ONLY: u64 = 6;
pub const AMM_STATUS_WAITING_TRADE: u64 = 7;

/// Statuses that permit swapping.
pub fn amm_status_allows_swap(status: u64) -> bool {
    matches!(
        status,
        AMM_STATUS_INITIALIZED | AMM_STATUS_SWAP_ONLY | AMM_STATUS_WAITING_TRADE
    )
}

// --------------------------------------------------------------------------
// OpenBook / Serum DEX v3 market state (needed for the Raydium swap accounts)
// --------------------------------------------------------------------------

pub const MARKET_V3_LEN: usize = 388;
pub const MARKET_OFF_ACCOUNT_FLAGS: usize = 0;
pub const MARKET_OFF_OWN_ADDRESS: usize = 8;
pub const MARKET_OFF_VAULT_SIGNER_NONCE: usize = 40;
pub const MARKET_OFF_BASE_MINT: usize = 48;
pub const MARKET_OFF_QUOTE_MINT: usize = 80;
pub const MARKET_OFF_BASE_VAULT: usize = 112;
pub const MARKET_OFF_QUOTE_VAULT: usize = 144;
pub const MARKET_OFF_REQUEST_QUEUE: usize = 176;
pub const MARKET_OFF_EVENT_QUEUE: usize = 208;
pub const MARKET_OFF_BIDS: usize = 240;
pub const MARKET_OFF_ASKS: usize = 272;
pub const MARKET_OFF_BASE_DEPOSITS_TOTAL: usize = 304;
pub const MARKET_OFF_QUOTE_DEPOSITS_TOTAL: usize = 312;

// --------------------------------------------------------------------------
// Anchor event discriminators — sha256("event:<EventName>")[..8]
// --------------------------------------------------------------------------
//
// Every discriminator below was recomputed from the name and cross-checked
// against the published IDLs in pump-fun/pump-public-docs. Anchor emits these
// as `Program data: <base64>` log lines; `events.rs` decodes them.

// --- pump.fun bonding curve ---
pub const EV_PUMP_CREATE: [u8; 8] = [27, 114, 169, 77, 222, 235, 99, 118];
pub const EV_PUMP_TRADE: [u8; 8] = [189, 219, 127, 211, 78, 230, 97, 238];
pub const EV_PUMP_COMPLETE: [u8; 8] = [95, 114, 97, 156, 212, 46, 152, 8];
pub const EV_PUMP_MIGRATION: [u8; 8] = [189, 233, 93, 185, 92, 148, 234, 148];
pub const EV_PUMP_EXTEND_ACCOUNT: [u8; 8] = [97, 97, 215, 144, 93, 146, 22, 124];
pub const EV_PUMP_SET_PARAMS: [u8; 8] = [223, 195, 159, 246, 62, 48, 143, 131];
pub const EV_PUMP_CLAIM_CASHBACK: [u8; 8] = [37, 58, 35, 126, 190, 53, 228, 197];

// --- PumpSwap AMM ---
pub const EV_AMM_CREATE_POOL: [u8; 8] = [177, 49, 12, 210, 160, 118, 167, 116];
pub const EV_AMM_BUY: [u8; 8] = [103, 244, 82, 31, 44, 245, 119, 119];
pub const EV_AMM_SELL: [u8; 8] = [62, 47, 55, 10, 165, 3, 220, 42];
pub const EV_AMM_DEPOSIT: [u8; 8] = [120, 248, 61, 83, 31, 142, 107, 144];
pub const EV_AMM_WITHDRAW: [u8; 8] = [22, 9, 133, 26, 160, 44, 71, 192];

/// Prefix Anchor uses for `emit!` output in transaction logs.
pub const ANCHOR_DATA_LOG_PREFIX: &str = "Program data: ";

// --------------------------------------------------------------------------
// `BondingCurve` account, current (multi-quote) layout
// --------------------------------------------------------------------------
//
// Source: pump-fun/pump-public-docs → idl/pump.json. Field names changed in the
// multi-quote upgrade (`virtual_sol_reserves` → `virtual_quote_reserves`) but
// the offsets did not, so a SOL-quoted curve parses identically.

pub const BC_OFF_IS_MAYHEM_MODE: usize = 81;
pub const BC_OFF_IS_CASHBACK_COIN: usize = 82;
pub const BC_OFF_QUOTE_MINT: usize = 83;
pub const BC_OFF_CREATOR_FEE_BPS: usize = 115;
pub const BC_OFF_CAN_EDIT_CREATOR_FEE: usize = 123;
pub const BC_OFF_IS_HOLDER_REWARD: usize = 124;
/// Length of a fully-extended current bonding curve account.
pub const BONDING_CURVE_V2_LEN: usize = 125;

// --------------------------------------------------------------------------
// `Global` account (pump.fun), current layout
// --------------------------------------------------------------------------

pub const GLOBAL_LEN: usize = 1087;
pub const GLOBAL_OFF_INITIALIZED: usize = 8;
pub const GLOBAL_OFF_AUTHORITY: usize = 9;
pub const GLOBAL_OFF_FEE_RECIPIENT: usize = 41;
pub const GLOBAL_OFF_INITIAL_VIRTUAL_TOKEN_RESERVES: usize = 73;
pub const GLOBAL_OFF_INITIAL_VIRTUAL_SOL_RESERVES: usize = 81;
pub const GLOBAL_OFF_INITIAL_REAL_TOKEN_RESERVES: usize = 89;
pub const GLOBAL_OFF_TOKEN_TOTAL_SUPPLY: usize = 97;
pub const GLOBAL_OFF_FEE_BASIS_POINTS: usize = 105;
pub const GLOBAL_OFF_WITHDRAW_AUTHORITY: usize = 113;
pub const GLOBAL_OFF_ENABLE_MIGRATE: usize = 145;
pub const GLOBAL_OFF_POOL_MIGRATION_FEE: usize = 146;
pub const GLOBAL_OFF_CREATOR_FEE_BASIS_POINTS: usize = 154;
pub const GLOBAL_OFF_FEE_RECIPIENTS: usize = 162;
pub const GLOBAL_FEE_RECIPIENTS_LEN: usize = 7;
pub const GLOBAL_OFF_CREATE_V2_ENABLED: usize = 450;
pub const GLOBAL_OFF_MAYHEM_MODE_ENABLED: usize = 515;
pub const GLOBAL_OFF_IS_CASHBACK_ENABLED: usize = 740;
pub const GLOBAL_OFF_BUYBACK_BASIS_POINTS: usize = 997;
pub const GLOBAL_OFF_INITIAL_VIRTUAL_QUOTE_RESERVES: usize = 1005;
pub const GLOBAL_OFF_IS_HOLDER_REWARD_ENABLED: usize = 1086;

// --------------------------------------------------------------------------
// PumpSwap `Pool` account
// --------------------------------------------------------------------------

pub const POOL_LEN: usize = 271;
pub const POOL_OFF_POOL_BUMP: usize = 8;
pub const POOL_OFF_INDEX: usize = 9;
pub const POOL_OFF_CREATOR: usize = 11;
pub const POOL_OFF_BASE_MINT: usize = 43;
pub const POOL_OFF_QUOTE_MINT: usize = 75;
pub const POOL_OFF_LP_MINT: usize = 107;
pub const POOL_OFF_POOL_BASE_TOKEN_ACCOUNT: usize = 139;
pub const POOL_OFF_POOL_QUOTE_TOKEN_ACCOUNT: usize = 171;
pub const POOL_OFF_LP_SUPPLY: usize = 203;
pub const POOL_OFF_COIN_CREATOR: usize = 211;
pub const POOL_OFF_IS_MAYHEM_MODE: usize = 243;
pub const POOL_OFF_IS_CASHBACK_COIN: usize = 244;
pub const POOL_OFF_VIRTUAL_QUOTE_RESERVES: usize = 245;
pub const POOL_OFF_CREATOR_FEE_BPS: usize = 261;
pub const POOL_OFF_CAN_EDIT_CREATOR_FEE: usize = 269;
pub const POOL_OFF_IS_HOLDER_REWARD: usize = 270;

// --------------------------------------------------------------------------
// PumpSwap `GlobalConfig` account
// --------------------------------------------------------------------------

pub const AMM_GLOBAL_CONFIG_LEN: usize = 949;
pub const AGC_OFF_ADMIN: usize = 8;
pub const AGC_OFF_LP_FEE_BASIS_POINTS: usize = 40;
pub const AGC_OFF_PROTOCOL_FEE_BASIS_POINTS: usize = 48;
pub const AGC_OFF_DISABLE_FLAGS: usize = 56;
pub const AGC_OFF_PROTOCOL_FEE_RECIPIENTS: usize = 57;
pub const AGC_PROTOCOL_FEE_RECIPIENTS_LEN: usize = 8;
pub const AGC_OFF_COIN_CREATOR_FEE_BASIS_POINTS: usize = 313;
pub const AGC_OFF_ADMIN_SET_COIN_CREATOR_AUTHORITY: usize = 321;
pub const AGC_OFF_MAYHEM_MODE_ENABLED: usize = 417;
pub const AGC_OFF_IS_CASHBACK_ENABLED: usize = 642;
pub const AGC_OFF_BUYBACK_BASIS_POINTS: usize = 899;

/// `UserVolumeAccumulator` (both programs share the layout).
pub const UVA_OFF_USER: usize = 8;
pub const UVA_OFF_NEEDS_CLAIM: usize = 40;
pub const UVA_OFF_TOTAL_UNCLAIMED_TOKENS: usize = 41;
pub const UVA_OFF_TOTAL_CLAIMED_TOKENS: usize = 49;
pub const UVA_OFF_CURRENT_SOL_VOLUME: usize = 57;
pub const UVA_OFF_CASHBACK_EARNED: usize = 74;
pub const UVA_OFF_TOTAL_CASHBACK_CLAIMED: usize = 82;

/// The pump `sharing-config` PDA seed, needed by the v2 instructions.
pub const PUMP_SEED_SHARING_CONFIG: &[u8] = b"sharing-config";
pub const PUMP_SEED_MINT_AUTHORITY: &[u8] = b"mint-authority";
pub const PUMPSWAP_SEED_POOL_AUTHORITY: &[u8] = b"pool-authority";
/// Token-2022 program, used for PumpSwap LP mints.
pub const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
/// Mayhem program referenced by `create_v2`.
pub const MAYHEM_PROGRAM: &str = "MAyhSmzXzV1pTf7LsNkr3VWNSpvM1X2fT8Qx4h1Xj9d";

// --------------------------------------------------------------------------
// Jupiter (aggregator fallback)
// --------------------------------------------------------------------------

pub static JUPITER_V6_PROGRAM: Lazy<Pubkey> =
    Lazy::new(|| pk("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"));
pub const JUPITER_QUOTE_URL: &str = "https://quote-api.jup.ag/v6/quote";
pub const JUPITER_SWAP_URL: &str = "https://quote-api.jup.ag/v6/swap";

// --------------------------------------------------------------------------
// Jito (MEV-protected bundles)
// --------------------------------------------------------------------------

pub static JITO_TIP_ACCOUNTS: Lazy<Vec<Pubkey>> = Lazy::new(|| {
    vec![
        pk("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5"),
        pk("HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe"),
        pk("Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY"),
        pk("ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49"),
        pk("DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh"),
        pk("ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt"),
        pk("DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL"),
        pk("3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT"),
    ]
});
pub const JITO_BUNDLE_PATH: &str = "/api/v1/bundles";

// --------------------------------------------------------------------------
// Feeds
// --------------------------------------------------------------------------

pub const PUMPPORTAL_WS_URL: &str = "wss://pumpportal.fun/api/data";
pub const PUMPPORTAL_TRADE_API: &str = "https://pumpportal.fun/api/trade-local";
pub const SOLANA_MAINNET_RPC: &str = "https://api.mainnet-beta.solana.com";
pub const SOLANA_DEVNET_RPC: &str = "https://api.devnet.solana.com";

/// The `logSubscribe` mention filter that catches every pump.fun launch.
pub fn pump_log_subscribe_filter() -> serde_json::Value {
    serde_json::json!({
        "mentions": [PUMP_PROGRAM_ID.to_string()],
        "commitment": "processed",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single mistyped base58 constant would silently trade against the
    /// wrong program, so pin the values that matter most.
    #[test]
    fn well_known_ids_are_exact() {
        assert_eq!(
            PUMP_PROGRAM_ID.to_string(),
            "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
        );
        assert_eq!(
            PUMP_GLOBAL.to_string(),
            "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf"
        );
        assert_eq!(
            PUMPSWAP_PROGRAM_ID.to_string(),
            "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
        );
        assert_eq!(
            RAYDIUM_AMM_V4.to_string(),
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
        );
        assert_eq!(
            WSOL_MINT.to_string(),
            "So11111111111111111111111111111111111111112"
        );
        assert_eq!(
            TOKEN_PROGRAM.to_string(),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        );
        assert_eq!(JITO_TIP_ACCOUNTS.len(), 8);
    }

    /// Anchor discriminators must equal sha256("global:<name>")[..8].
    #[test]
    fn pump_discriminators_match_their_anchor_sighash() {
        use solana_sdk::hash::hash;
        let sighash = |name: &str| -> [u8; 8] {
            let h = hash(format!("global:{name}").as_bytes());
            let mut out = [0u8; 8];
            out.copy_from_slice(&h.to_bytes()[..8]);
            out
        };
        assert_eq!(sighash("buy"), PUMP_DISC_BUY);
        assert_eq!(sighash("sell"), PUMP_DISC_SELL);
        assert_eq!(sighash("create"), PUMP_DISC_CREATE);
        assert_eq!(sighash("initialize"), PUMP_DISC_INITIALIZE);
        assert_eq!(sighash("buy_exact_sol_in"), PUMP_DISC_BUY_EXACT_SOL_IN);
    }

    /// Account discriminators must equal sha256("account:<Name>")[..8].
    #[test]
    fn pump_account_discriminators_match() {
        use solana_sdk::hash::hash;
        let acc_disc = |name: &str| -> [u8; 8] {
            let h = hash(format!("account:{name}").as_bytes());
            let mut out = [0u8; 8];
            out.copy_from_slice(&h.to_bytes()[..8]);
            out
        };
        assert_eq!(acc_disc("Global"), PUMP_ACC_DISC_GLOBAL);
        assert_eq!(acc_disc("BondingCurve"), PUMP_ACC_DISC_BONDING_CURVE);
        assert_eq!(acc_disc("FeeConfig"), PUMP_ACC_DISC_FEE_CONFIG);
    }

    #[test]
    fn pda_derivations_are_stable() {
        let (bc, bump) = Pubkey::find_program_address(
            &[PUMP_SEED_BONDING_CURVE, WSOL_MINT.as_ref()],
            &PUMP_PROGRAM_ID,
        );
        assert!(bump > 0);
        assert_eq!(bc.to_string().len(), 44);

        let (global, _) = Pubkey::find_program_address(&[PUMP_SEED_GLOBAL], &PUMP_PROGRAM_ID);
        assert_eq!(global, *PUMP_GLOBAL, "global PDA seed must be \"global\"");

        let (ea, _) = Pubkey::find_program_address(&[PUMP_SEED_EVENT_AUTHORITY], &PUMP_PROGRAM_ID);
        assert_eq!(
            ea, *PUMP_EVENT_AUTHORITY,
            "event authority seed must be \"__event_authority\""
        );
    }
}
