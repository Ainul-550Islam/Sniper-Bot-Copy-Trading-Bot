//! Decoders (and synthetic encoders) for the Raydium LaunchLab (`launchpad`)
//! accounts.
//!
//! # Fail-closed
//!
//! Every decoder checks the Anchor discriminator **and** the exact account
//! length before reading a field. A mismatch is a typed [`BotError`], never a
//! default value.
//!
//! # Layout provenance
//!
//! The LaunchLab Anchor program source is not published, so the layouts come
//! from the **official vendored IDL**
//! (`crates/solana-kit/idl/raydium_launchpad.json`, upstream
//! `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`),
//! field by field in declaration order — which is the order Anchor
//! serialises in. The same layouts appear in Raydium's official SDK
//! (`raydium-io/raydium-sdk-V2` @ `f8549145afe75d4c486e30c43331c2c4da32b1eb`,
//! `src/raydium/launchpad/layout.ts`), which is used here as the
//! cross-check.
//!
//! # Synthetic encoders
//!
//! The `encode_*` helpers build *synthetic* account bytes for the test
//! fixtures. They are labelled synthetic everywhere they are used and are
//! never described as captured mainnet data.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;

/// A bounds-checked little-endian reader over a raw account buffer.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    label: &'static str,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], label: &'static str) -> Self {
        Reader {
            data,
            pos: 0,
            label,
        }
    }

    fn take(&mut self, n: usize) -> BotResult<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(BotError::encoding(format!(
                "{} account is truncated: wanted {n} bytes at offset {}, have {}",
                self.label,
                self.pos,
                self.data.len().saturating_sub(self.pos)
            )));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> BotResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> BotResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u16(&mut self) -> BotResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u64(&mut self) -> BotResult<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    fn pubkey(&mut self) -> BotResult<Pubkey> {
        let b = self.take(32)?;
        let mut a = [0u8; 32];
        a.copy_from_slice(b);
        Ok(Pubkey::new_from_array(a))
    }

    fn skip(&mut self, n: usize) -> BotResult<()> {
        self.take(n)?;
        Ok(())
    }
}

/// Check the Anchor discriminator of an account.
fn check_discriminator(data: &[u8], expected: [u8; 8], label: &str) -> BotResult<()> {
    if data.len() < 8 {
        return Err(BotError::encoding(format!(
            "{label} account is {} bytes, too short to carry an 8-byte discriminator",
            data.len()
        )));
    }
    if data[..8] != expected {
        return Err(BotError::encoding(format!(
            "{label} account has discriminator {:02x?}, expected {:02x?}",
            &data[..8],
            expected
        )));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// PoolState
// --------------------------------------------------------------------------

/// The LaunchLab pool status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchLabPoolStatus {
    /// Still fundraising — the curve is open.
    Fund,
    /// Fundraising finished, waiting for the migration.
    Migrate,
    /// Migrated to an AMM — the curve is closed.
    Trade,
}

impl LaunchLabPoolStatus {
    /// Decode the raw `u8` status. An unknown value is an error.
    pub fn from_u8(value: u8) -> BotResult<Self> {
        match value {
            RAYDIUM_LAUNCHLAB_STATUS_FUND => Ok(LaunchLabPoolStatus::Fund),
            RAYDIUM_LAUNCHLAB_STATUS_MIGRATE => Ok(LaunchLabPoolStatus::Migrate),
            RAYDIUM_LAUNCHLAB_STATUS_TRADE => Ok(LaunchLabPoolStatus::Trade),
            other => Err(BotError::encoding(format!(
                "invalid LaunchLab pool status {other}"
            ))),
        }
    }

    /// Encode back to the raw `u8`.
    pub fn to_u8(self) -> u8 {
        match self {
            LaunchLabPoolStatus::Fund => RAYDIUM_LAUNCHLAB_STATUS_FUND,
            LaunchLabPoolStatus::Migrate => RAYDIUM_LAUNCHLAB_STATUS_MIGRATE,
            LaunchLabPoolStatus::Trade => RAYDIUM_LAUNCHLAB_STATUS_TRADE,
        }
    }
}

/// Where a LaunchLab pool migrates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchLabMigrateType {
    /// Raydium AMM v4 (OpenBook-backed).
    Amm,
    /// Raydium CPMM (`cp-swap`).
    CpSwap,
}

impl LaunchLabMigrateType {
    /// Decode the raw `u8`. An unknown value is an error.
    pub fn from_u8(value: u8) -> BotResult<Self> {
        match value {
            RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM => Ok(LaunchLabMigrateType::Amm),
            RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP => Ok(LaunchLabMigrateType::CpSwap),
            other => Err(BotError::encoding(format!(
                "invalid LaunchLab migrate_type {other}"
            ))),
        }
    }

    /// Encode back to the raw `u8`.
    pub fn to_u8(self) -> u8 {
        match self {
            LaunchLabMigrateType::Amm => RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
            LaunchLabMigrateType::CpSwap => RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
        }
    }
}

/// Which token the creator fee is charged in after the migration to CPMM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmmCreatorFeeOn {
    /// The quote token only.
    QuoteToken,
    /// Both tokens.
    BothToken,
}

impl AmmCreatorFeeOn {
    /// Decode the raw `u8` enum discriminant. An unknown value is an error.
    pub fn from_u8(value: u8) -> BotResult<Self> {
        match value {
            0 => Ok(AmmCreatorFeeOn::QuoteToken),
            1 => Ok(AmmCreatorFeeOn::BothToken),
            other => Err(BotError::encoding(format!(
                "invalid LaunchLab amm_creator_fee_on {other}"
            ))),
        }
    }

    /// Encode back to the raw `u8`.
    pub fn to_u8(self) -> u8 {
        match self {
            AmmCreatorFeeOn::QuoteToken => 0,
            AmmCreatorFeeOn::BothToken => 1,
        }
    }
}

/// The LaunchLab vesting schedule of the base token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VestingSchedule {
    /// The total amount of tokens to be unlocked.
    pub total_locked_amount: u64,
    /// The waiting time in seconds before unlocking after fundraising ends.
    pub cliff_period: u64,
    /// The unlocking period in seconds.
    pub unlock_period: u64,
    /// The start of the schedule.
    pub start_time: u64,
    /// The total allocated share.
    pub total_allocated_share: u64,
}

/// The decoded LaunchLab `PoolState` account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolState {
    /// Account update epoch.
    pub epoch: u64,
    /// Bump of the pool authority PDA.
    pub auth_bump: u8,
    /// The pool status.
    pub status: LaunchLabPoolStatus,
    /// Decimals of the base token.
    pub base_decimals: u8,
    /// Decimals of the quote token.
    pub quote_decimals: u8,
    /// Where the pool migrates to.
    pub migrate_type: LaunchLabMigrateType,
    /// The supply of the base token.
    pub supply: u64,
    /// The total amount of base token sold so far.
    pub total_base_sell: u64,
    /// The curve's virtual base parameter.
    ///
    /// * constant product: virtual liquidity (the initial price is
    ///   `virtual_quote / virtual_base`).
    /// * linear price: the price slope parameter `a`.
    /// * fixed price: the virtual base reserve.
    pub virtual_base: u64,
    /// The curve's virtual quote parameter.
    pub virtual_quote: u64,
    /// The real base token amount in the pool.
    pub real_base: u64,
    /// The real quote token amount in the pool.
    pub real_quote: u64,
    /// The total quote the pool is fundraising.
    pub total_quote_fund_raising: u64,
    /// Accrued protocol fees (quote tokens).
    pub quote_protocol_fee: u64,
    /// Accrued platform fees (quote tokens).
    pub platform_fee: u64,
    /// The migration fee.
    pub migrate_fee: u64,
    /// The vesting schedule of the base token.
    pub vesting_schedule: VestingSchedule,
    /// The `GlobalConfig` this pool follows.
    pub global_config: Pubkey,
    /// The `PlatformConfig` this pool follows.
    pub platform_config: Pubkey,
    /// The base mint.
    pub base_mint: Pubkey,
    /// The quote mint.
    pub quote_mint: Pubkey,
    /// The base token vault.
    pub base_vault: Pubkey,
    /// The quote token vault.
    pub quote_vault: Pubkey,
    /// The creator of the token.
    pub creator: Pubkey,
    /// The token-program flag bits (bit0: base is Token-2022, bit1: quote is
    /// Token-2022).
    pub token_program_flag: u8,
    /// Which token the creator fee is charged in after the CPMM migration.
    pub amm_creator_fee_on: AmmCreatorFeeOn,
    /// The platform's vesting share of the base token.
    pub platform_vesting_share: u64,
}

impl PoolState {
    /// Decode a raw LaunchLab `PoolState` account.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(
            data,
            RAYDIUM_LAUNCHLAB_ACC_DISC_POOL_STATE,
            "LaunchLab PoolState",
        )?;
        if data.len() != RAYDIUM_LAUNCHLAB_POOL_STATE_LEN {
            return Err(BotError::encoding(format!(
                "LaunchLab PoolState account is {} bytes, expected {RAYDIUM_LAUNCHLAB_POOL_STATE_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "LaunchLab PoolState");
        let epoch = r.u64()?;
        let auth_bump = r.u8()?;
        let status_raw = r.u8()?;
        let base_decimals = r.u8()?;
        let quote_decimals = r.u8()?;
        let migrate_type_raw = r.u8()?;
        let supply = r.u64()?;
        let total_base_sell = r.u64()?;
        let virtual_base = r.u64()?;
        let virtual_quote = r.u64()?;
        let real_base = r.u64()?;
        let real_quote = r.u64()?;
        let total_quote_fund_raising = r.u64()?;
        let quote_protocol_fee = r.u64()?;
        let platform_fee = r.u64()?;
        let migrate_fee = r.u64()?;
        let vesting_schedule = VestingSchedule {
            total_locked_amount: r.u64()?,
            cliff_period: r.u64()?,
            unlock_period: r.u64()?,
            start_time: r.u64()?,
            total_allocated_share: r.u64()?,
        };
        let global_config = r.pubkey()?;
        let platform_config = r.pubkey()?;
        let base_mint = r.pubkey()?;
        let quote_mint = r.pubkey()?;
        let base_vault = r.pubkey()?;
        let quote_vault = r.pubkey()?;
        let creator = r.pubkey()?;
        let token_program_flag = r.u8()?;
        let amm_creator_fee_on_raw = r.u8()?;
        let platform_vesting_share = r.u64()?;
        r.skip(54)?; // padding

        if base_mint == quote_mint {
            return Err(BotError::encoding(
                "LaunchLab PoolState has identical base_mint and quote_mint",
            ));
        }
        if base_mint == Pubkey::default() || quote_mint == Pubkey::default() {
            return Err(BotError::encoding(
                "LaunchLab PoolState contains a zero mint address",
            ));
        }
        if total_base_sell > supply {
            return Err(BotError::encoding(format!(
                "LaunchLab PoolState total_base_sell {total_base_sell} exceeds supply {supply}"
            )));
        }

        Ok(PoolState {
            epoch,
            auth_bump,
            status: LaunchLabPoolStatus::from_u8(status_raw)?,
            base_decimals,
            quote_decimals,
            migrate_type: LaunchLabMigrateType::from_u8(migrate_type_raw)?,
            supply,
            total_base_sell,
            virtual_base,
            virtual_quote,
            real_base,
            real_quote,
            total_quote_fund_raising,
            quote_protocol_fee,
            platform_fee,
            migrate_fee,
            vesting_schedule,
            global_config,
            platform_config,
            base_mint,
            quote_mint,
            base_vault,
            quote_vault,
            creator,
            token_program_flag,
            amm_creator_fee_on: AmmCreatorFeeOn::from_u8(amm_creator_fee_on_raw)?,
            platform_vesting_share,
        })
    }

    /// Whether the base token uses the Token-2022 program (bit 0 of the flag).
    pub fn base_is_token_2022(&self) -> bool {
        self.token_program_flag & 1 != 0
    }

    /// Whether the quote token uses the Token-2022 program (bit 1 of the flag).
    pub fn quote_is_token_2022(&self) -> bool {
        self.token_program_flag & 2 != 0
    }

    /// The token program of the base token.
    pub fn base_token_program(&self) -> Pubkey {
        if self.base_is_token_2022() {
            *crate::consts::TOKEN_2022_PROGRAM
        } else {
            *crate::consts::TOKEN_PROGRAM
        }
    }

    /// The token program of the quote token.
    pub fn quote_token_program(&self) -> Pubkey {
        if self.quote_is_token_2022() {
            *crate::consts::TOKEN_2022_PROGRAM
        } else {
            *crate::consts::TOKEN_PROGRAM
        }
    }

    /// Whether the curve is still open for trading.
    ///
    /// The pool is tradable while it is fundraising (`Fund`); once it has
    /// migrated (`Trade`) the curve is closed and the AMM takes over.
    pub fn is_curve_open(&self) -> bool {
        self.status == LaunchLabPoolStatus::Fund
    }

    /// The amount of base token still available to sell.
    pub fn remaining_base(&self) -> u64 {
        self.total_base_sell.saturating_sub(self.real_base)
    }

    /// The amount of quote still to be raised.
    pub fn remaining_quote_fund_raising(&self) -> u64 {
        self.total_quote_fund_raising
            .saturating_sub(self.real_quote)
    }

    /// The base amount that is neither sold nor locked (the amount that goes
    /// into the AMM pool on migration).
    pub fn migratable_base(&self) -> u64 {
        self.supply
            .saturating_sub(self.total_base_sell)
            .saturating_sub(self.vesting_schedule.total_locked_amount)
    }
}

// --------------------------------------------------------------------------
// GlobalConfig
// --------------------------------------------------------------------------

/// The decoded LaunchLab `GlobalConfig` account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Account update epoch.
    pub epoch: u64,
    /// The curve type this config is for.
    pub curve_type: u8,
    /// The config index.
    pub index: u16,
    /// The migration fee.
    pub migrate_fee: u64,
    /// The trade fee rate, in hundredths of a bip.
    pub trade_fee_rate: u64,
    /// The maximum share fee rate, in hundredths of a bip.
    pub max_share_fee_rate: u64,
    /// The minimum base supply (without decimals).
    pub min_base_supply: u64,
    /// The maximum lock rate, in hundredths of a bip.
    pub max_lock_rate: u64,
    /// The minimum base sell rate, in hundredths of a bip.
    pub min_base_sell_rate: u64,
    /// The minimum base migrate rate, in hundredths of a bip.
    pub min_base_migrate_rate: u64,
    /// The minimum quote fund raising (with decimals).
    pub min_quote_fund_raising: u64,
    /// The quote mint of the pools using this config.
    pub quote_mint: Pubkey,
    /// The protocol fee owner.
    pub protocol_fee_owner: Pubkey,
    /// The migration fee owner.
    pub migrate_fee_owner: Pubkey,
    /// The wallet that controls the migration to AMM.
    pub migrate_to_amm_wallet: Pubkey,
    /// The wallet that controls the migration to CPMM.
    pub migrate_to_cpswap_wallet: Pubkey,
    /// Whether a platform must be explicitly authorised to use this config.
    pub requires_platform_auth: u8,
}

impl GlobalConfig {
    /// Decode a raw LaunchLab `GlobalConfig` account.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(
            data,
            RAYDIUM_LAUNCHLAB_ACC_DISC_GLOBAL_CONFIG,
            "LaunchLab GlobalConfig",
        )?;
        if data.len() != RAYDIUM_LAUNCHLAB_GLOBAL_CONFIG_LEN {
            return Err(BotError::encoding(format!(
                "LaunchLab GlobalConfig account is {} bytes, expected {RAYDIUM_LAUNCHLAB_GLOBAL_CONFIG_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "LaunchLab GlobalConfig");
        let epoch = r.u64()?;
        let curve_type = r.u8()?;
        let index = r.u16()?;
        let migrate_fee = r.u64()?;
        let trade_fee_rate = r.u64()?;
        let max_share_fee_rate = r.u64()?;
        let min_base_supply = r.u64()?;
        let max_lock_rate = r.u64()?;
        let min_base_sell_rate = r.u64()?;
        let min_base_migrate_rate = r.u64()?;
        let min_quote_fund_raising = r.u64()?;
        let quote_mint = r.pubkey()?;
        let protocol_fee_owner = r.pubkey()?;
        let migrate_fee_owner = r.pubkey()?;
        let migrate_to_amm_wallet = r.pubkey()?;
        let migrate_to_cpswap_wallet = r.pubkey()?;
        let requires_platform_auth = r.u8()?;
        r.skip(7)?; // padding_alignment
        r.skip(15 * 8)?; // padding

        if curve_type > RAYDIUM_LAUNCHLAB_CURVE_LINEAR {
            return Err(BotError::encoding(format!(
                "LaunchLab GlobalConfig has an unknown curve_type {curve_type}"
            )));
        }
        if trade_fee_rate >= RAYDIUM_LAUNCHLAB_FEE_RATE_DENOMINATOR {
            return Err(BotError::encoding(format!(
                "LaunchLab GlobalConfig trade_fee_rate {trade_fee_rate} is at or above the denominator"
            )));
        }

        Ok(GlobalConfig {
            epoch,
            curve_type,
            index,
            migrate_fee,
            trade_fee_rate,
            max_share_fee_rate,
            min_base_supply,
            max_lock_rate,
            min_base_sell_rate,
            min_base_migrate_rate,
            min_quote_fund_raising,
            quote_mint,
            protocol_fee_owner,
            migrate_fee_owner,
            migrate_to_amm_wallet,
            migrate_to_cpswap_wallet,
            requires_platform_auth,
        })
    }

    /// The curve type as a typed enum. An unknown value is an error.
    pub fn curve(&self) -> BotResult<crate::venues::raydium_launchlab::curve::CurveType> {
        crate::venues::raydium_launchlab::curve::CurveType::from_u8(self.curve_type)
    }
}

// --------------------------------------------------------------------------
// PlatformConfig
// --------------------------------------------------------------------------

/// The decoded LaunchLab `PlatformConfig` account.
///
/// The layout is version-dependent: the IDL's `PlatformConfig` has a
/// `curve_params: Vec<PlatformCurveParam>` at the end and a `padding`
/// array, while the SDK's older layout has a fixed `padding` of `u64[8]`.
/// This decoder reads the **fixed prefix** (everything up to and including
/// `platform_cp_creator`) and then treats the remainder as opaque padding,
/// because the trailing region is where the two layouts differ and none of it
/// affects the quote maths. The curve parameters live in the separate
/// `PlatformCurveParam` account, which is read by the platform-specific code
/// path, not by this decoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformConfig {
    /// Account update epoch.
    pub epoch: u64,
    /// The platform fee wallet.
    pub platform_fee_wallet: Pubkey,
    /// The platform NFT wallet.
    pub platform_nft_wallet: Pubkey,
    /// The platform's share of the migrated liquidity.
    pub platform_scale: u64,
    /// The creator's share of the migrated liquidity.
    pub creator_scale: u64,
    /// The share of the liquidity that is burned on migration.
    pub burn_scale: u64,
    /// The platform fee rate.
    pub fee_rate: u64,
    /// The platform's creator fee rate (used on the LaunchLab curve).
    pub creator_fee_rate: u64,
    /// The token-2022 transfer-fee extension authority.
    pub transfer_fee_extension_auth: Pubkey,
    /// The platform vesting wallet.
    pub platform_vesting_wallet: Pubkey,
    /// The platform's vesting scale.
    pub platform_vesting_scale: u64,
    /// The creator used for the CPMM pool created at migration.
    pub platform_cp_creator: Pubkey,
    /// The CPMM config the platform selects after migration.
    pub cpswap_config: Pubkey,
}

impl PlatformConfig {
    /// Decode a raw LaunchLab `PlatformConfig` account.
    ///
    /// The minimum length is the fixed prefix plus the discriminator; longer
    /// accounts are accepted (the trailing curve-param vector is
    /// version-dependent) but a shorter one is refused.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(
            data,
            RAYDIUM_LAUNCHLAB_ACC_DISC_PLATFORM_CONFIG,
            "LaunchLab PlatformConfig",
        )?;
        // The fixed prefix, derived from the vendored IDL's `PlatformConfig`
        // type: 8 (disc) + 8 (epoch) + 2 * 32 (wallets) + 3 * 8 (scales)
        // + 8 (fee_rate) + 64 (name) + 256 (web) + 256 (img)
        // + 32 (cpswap_config) + 8 (creator_fee_rate)
        // + 32 (transfer_fee_extension_auth) + 32 (platform_vesting_wallet)
        // + 8 (platform_vesting_scale) + 32 (platform_cp_creator)
        // + 108 (padding) + 4 (the `curve_params` vector length prefix)
        // = 944. Longer accounts carry curve parameters; a shorter one is
        // refused.
        const FIXED_LEN: usize =
            8 + 8 + 2 * 32 + 3 * 8 + 8 + 64 + 256 + 256 + 32 + 8 + 32 + 32 + 8 + 32 + 108 + 4;
        if data.len() < FIXED_LEN {
            return Err(BotError::encoding(format!(
                "LaunchLab PlatformConfig account is {} bytes, expected at least {FIXED_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "LaunchLab PlatformConfig");
        let epoch = r.u64()?;
        let platform_fee_wallet = r.pubkey()?;
        let platform_nft_wallet = r.pubkey()?;
        let platform_scale = r.u64()?;
        let creator_scale = r.u64()?;
        let burn_scale = r.u64()?;
        let fee_rate = r.u64()?;
        r.skip(64)?; // name
        r.skip(256)?; // web
        r.skip(256)?; // img
        let cpswap_config = r.pubkey()?;
        let creator_fee_rate = r.u64()?;
        let transfer_fee_extension_auth = r.pubkey()?;
        let platform_vesting_wallet = r.pubkey()?;
        let platform_vesting_scale = r.u64()?;
        let platform_cp_creator = r.pubkey()?;
        r.skip(108)?; // padding
                      // `curve_params` is a `Vec<PlatformCurveParam>`; its length prefix is
                      // always present, even when the vector is empty. The elements are
                      // not modelled (they only matter to the platform, not to a trade),
                      // but the prefix is validated so a truncated account is refused.
        let curve_param_count = r.u32()?;
        if curve_param_count as usize > (data.len() - r.pos) / 8 {
            return Err(BotError::encoding(format!(
                "LaunchLab PlatformConfig declares {curve_param_count} curve params but the \
                 account has only {} bytes left",
                data.len() - r.pos
            )));
        }

        Ok(PlatformConfig {
            epoch,
            platform_fee_wallet,
            platform_nft_wallet,
            platform_scale,
            creator_scale,
            burn_scale,
            fee_rate,
            creator_fee_rate,
            transfer_fee_extension_auth,
            platform_vesting_wallet,
            platform_vesting_scale,
            platform_cp_creator,
            cpswap_config,
        })
    }
}

// --------------------------------------------------------------------------
// VestingRecord
// --------------------------------------------------------------------------

/// The decoded LaunchLab `VestingRecord` account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VestingAccount {
    /// Account update epoch.
    pub epoch: u64,
    /// The pool the record belongs to.
    pub pool: Pubkey,
    /// The beneficiary.
    pub beneficiary: Pubkey,
    /// The amount already claimed.
    pub claimed_amount: u64,
    /// The amount vested in total.
    pub token_share_amount: u64,
}

impl VestingAccount {
    /// Decode a raw LaunchLab `VestingRecord` account.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(
            data,
            RAYDIUM_LAUNCHLAB_ACC_DISC_VESTING_RECORD,
            "LaunchLab VestingRecord",
        )?;
        if data.len() != RAYDIUM_LAUNCHLAB_VESTING_RECORD_LEN {
            return Err(BotError::encoding(format!(
                "LaunchLab VestingRecord account is {} bytes, expected {RAYDIUM_LAUNCHLAB_VESTING_RECORD_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "LaunchLab VestingRecord");
        let epoch = r.u64()?;
        let pool = r.pubkey()?;
        let beneficiary = r.pubkey()?;
        let claimed_amount = r.u64()?;
        let token_share_amount = r.u64()?;
        r.skip(8 * 8)?; // padding

        Ok(VestingAccount {
            epoch,
            pool,
            beneficiary,
            claimed_amount,
            token_share_amount,
        })
    }

    /// The amount still claimable.
    pub fn remaining(&self) -> u64 {
        self.token_share_amount.saturating_sub(self.claimed_amount)
    }
}

// --------------------------------------------------------------------------
// Synthetic encoders (test fixtures only)
// --------------------------------------------------------------------------

/// A writer that mirrors the byte order the Anchor decoders use.
#[derive(Debug, Default)]
pub struct SyntheticWriter {
    bytes: Vec<u8>,
}

impl SyntheticWriter {
    /// Start a writer with the given Anchor discriminator.
    pub fn with_discriminator(disc: [u8; 8]) -> Self {
        SyntheticWriter {
            bytes: disc.to_vec(),
        }
    }

    /// Append a `u8`.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes.push(v);
        self
    }

    /// Append a `u16`.
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a `u64`.
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a `Pubkey`.
    pub fn pubkey(&mut self, v: &Pubkey) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_bytes());
        self
    }

    /// Append `n` zero bytes.
    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.bytes.extend(std::iter::repeat_n(0u8, n));
        self
    }

    /// Finish and return the raw account bytes.
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// The fields of a synthetic LaunchLab `PoolState` fixture.
///
/// Synthetic by construction: every value here is generated by the test
/// encoder, never captured from mainnet.
#[derive(Debug, Clone)]
pub struct SyntheticPoolState {
    pub epoch: u64,
    pub auth_bump: u8,
    pub status: u8,
    pub base_decimals: u8,
    pub quote_decimals: u8,
    pub migrate_type: u8,
    pub supply: u64,
    pub total_base_sell: u64,
    pub virtual_base: u64,
    pub virtual_quote: u64,
    pub real_base: u64,
    pub real_quote: u64,
    pub total_quote_fund_raising: u64,
    pub quote_protocol_fee: u64,
    pub platform_fee: u64,
    pub migrate_fee: u64,
    pub vesting_schedule: VestingSchedule,
    pub global_config: Pubkey,
    pub platform_config: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub creator: Pubkey,
    pub token_program_flag: u8,
    pub amm_creator_fee_on: u8,
    pub platform_vesting_share: u64,
}

impl Default for SyntheticPoolState {
    fn default() -> Self {
        SyntheticPoolState {
            epoch: 0,
            auth_bump: 254,
            status: RAYDIUM_LAUNCHLAB_STATUS_FUND,
            base_decimals: 6,
            quote_decimals: 9,
            migrate_type: RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
            supply: 1_000_000_000_000_000,
            total_base_sell: 793_100_000_000_000,
            virtual_base: 1_073_471_847_374_405,
            virtual_quote: 30_050_573_465,
            real_base: 0,
            real_quote: 0,
            total_quote_fund_raising: 85_000_000_000,
            quote_protocol_fee: 0,
            platform_fee: 0,
            migrate_fee: 0,
            vesting_schedule: VestingSchedule {
                total_locked_amount: 0,
                cliff_period: 0,
                unlock_period: 0,
                start_time: 0,
                total_allocated_share: 0,
            },
            global_config: Pubkey::new_unique(),
            platform_config: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            base_vault: Pubkey::new_unique(),
            quote_vault: Pubkey::new_unique(),
            creator: Pubkey::new_unique(),
            token_program_flag: 0,
            amm_creator_fee_on: 0,
            platform_vesting_share: 0,
        }
    }
}

impl SyntheticPoolState {
    /// Encode the synthetic pool account bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = SyntheticWriter::with_discriminator(RAYDIUM_LAUNCHLAB_ACC_DISC_POOL_STATE);
        w.u64(self.epoch)
            .u8(self.auth_bump)
            .u8(self.status)
            .u8(self.base_decimals)
            .u8(self.quote_decimals)
            .u8(self.migrate_type)
            .u64(self.supply)
            .u64(self.total_base_sell)
            .u64(self.virtual_base)
            .u64(self.virtual_quote)
            .u64(self.real_base)
            .u64(self.real_quote)
            .u64(self.total_quote_fund_raising)
            .u64(self.quote_protocol_fee)
            .u64(self.platform_fee)
            .u64(self.migrate_fee)
            .u64(self.vesting_schedule.total_locked_amount)
            .u64(self.vesting_schedule.cliff_period)
            .u64(self.vesting_schedule.unlock_period)
            .u64(self.vesting_schedule.start_time)
            .u64(self.vesting_schedule.total_allocated_share)
            .pubkey(&self.global_config)
            .pubkey(&self.platform_config)
            .pubkey(&self.base_mint)
            .pubkey(&self.quote_mint)
            .pubkey(&self.base_vault)
            .pubkey(&self.quote_vault)
            .pubkey(&self.creator)
            .u8(self.token_program_flag)
            .u8(self.amm_creator_fee_on)
            .u64(self.platform_vesting_share)
            .zeros(54);
        w.finish()
    }
}

/// Encode a synthetic LaunchLab `GlobalConfig` account.
pub fn encode_synthetic_global_config(curve_type: u8, quote_mint: &Pubkey) -> Vec<u8> {
    let mut w = SyntheticWriter::with_discriminator(RAYDIUM_LAUNCHLAB_ACC_DISC_GLOBAL_CONFIG);
    w.u64(0)
        .u8(curve_type)
        .u16(0)
        .u64(0)
        .u64(2_500)
        .u64(0)
        .u64(0)
        .u64(0)
        .u64(0)
        .u64(0)
        .u64(0)
        .pubkey(quote_mint)
        .pubkey(&Pubkey::new_unique())
        .pubkey(&Pubkey::new_unique())
        .pubkey(&Pubkey::new_unique())
        .pubkey(&Pubkey::new_unique())
        .u8(0)
        .zeros(7)
        .zeros(15 * 8);
    w.finish()
}

/// Encode a synthetic LaunchLab `VestingRecord` account.
pub fn encode_synthetic_vesting_record(pool: &Pubkey, beneficiary: &Pubkey) -> Vec<u8> {
    let mut w = SyntheticWriter::with_discriminator(RAYDIUM_LAUNCHLAB_ACC_DISC_VESTING_RECORD);
    w.u64(0)
        .pubkey(pool)
        .pubkey(beneficiary)
        .u64(0)
        .u64(1_000_000)
        .zeros(8 * 8);
    w.finish()
}

/// Encode a synthetic LaunchLab `PlatformConfig` account.
pub fn encode_synthetic_platform_config(fee_rate: u64, creator_fee_rate: u64) -> Vec<u8> {
    let mut w = SyntheticWriter::with_discriminator(RAYDIUM_LAUNCHLAB_ACC_DISC_PLATFORM_CONFIG);
    w.u64(0)
        .pubkey(&Pubkey::new_unique())
        .pubkey(&Pubkey::new_unique())
        .u64(0)
        .u64(0)
        .u64(0)
        .u64(fee_rate)
        .zeros(64)
        .zeros(256)
        .zeros(256)
        .pubkey(&Pubkey::new_unique())
        .u64(creator_fee_rate)
        .pubkey(&Pubkey::new_unique())
        .pubkey(&Pubkey::new_unique())
        .u64(0)
        .pubkey(&Pubkey::new_unique())
        .zeros(108)
        .u32(0); // the empty `curve_params` vector
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_state_round_trips_through_the_synthetic_encoder() {
        let s = SyntheticPoolState::default();
        let bytes = s.encode();
        assert_eq!(bytes.len(), RAYDIUM_LAUNCHLAB_POOL_STATE_LEN);
        let p = PoolState::decode(&bytes).unwrap();
        assert_eq!(p.epoch, s.epoch);
        assert_eq!(p.auth_bump, s.auth_bump);
        assert_eq!(p.status, LaunchLabPoolStatus::Fund);
        assert_eq!(p.base_decimals, 6);
        assert_eq!(p.quote_decimals, 9);
        assert_eq!(p.migrate_type, LaunchLabMigrateType::CpSwap);
        assert_eq!(p.supply, s.supply);
        assert_eq!(p.total_base_sell, s.total_base_sell);
        assert_eq!(p.virtual_base, s.virtual_base);
        assert_eq!(p.virtual_quote, s.virtual_quote);
        assert_eq!(p.real_base, 0);
        assert_eq!(p.real_quote, 0);
        assert_eq!(p.total_quote_fund_raising, s.total_quote_fund_raising);
        assert_eq!(p.migrate_fee, 0);
        assert_eq!(p.global_config, s.global_config);
        assert_eq!(p.platform_config, s.platform_config);
        assert_eq!(p.base_mint, s.base_mint);
        assert_eq!(p.quote_mint, s.quote_mint);
        assert_eq!(p.base_vault, s.base_vault);
        assert_eq!(p.quote_vault, s.quote_vault);
        assert_eq!(p.creator, s.creator);
        assert_eq!(p.token_program_flag, 0);
        assert_eq!(p.amm_creator_fee_on, AmmCreatorFeeOn::QuoteToken);
        assert_eq!(p.platform_vesting_share, 0);
        assert_eq!(
            p.vesting_schedule.total_locked_amount,
            s.vesting_schedule.total_locked_amount
        );
    }

    #[test]
    fn pool_state_rejects_a_wrong_length() {
        let s = SyntheticPoolState::default();
        let bytes = s.encode();
        let err = PoolState::decode(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("expected {RAYDIUM_LAUNCHLAB_POOL_STATE_LEN}")),
            "{err}"
        );
        // A buffer of the right length but the wrong discriminator must fail
        // on the discriminator, not on the length.
        let mut wrong_disc = bytes.clone();
        wrong_disc[0] ^= 0xff;
        let err = PoolState::decode(&wrong_disc).unwrap_err();
        assert!(err.to_string().contains("discriminator"), "{err}");
        // A buffer of the right discriminator but the wrong length must fail
        // on the length.
        let mut wrong_len = bytes.clone();
        wrong_len.push(0);
        let err = PoolState::decode(&wrong_len).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("expected {RAYDIUM_LAUNCHLAB_POOL_STATE_LEN}")),
            "{err}"
        );
    }

    #[test]
    fn pool_state_rejects_a_wrong_discriminator() {
        let s = SyntheticPoolState::default();
        let mut bytes = s.encode();
        bytes[7] ^= 0xff;
        let err = PoolState::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("discriminator"), "{err}");
    }

    #[test]
    fn pool_state_rejects_invalid_enum_values() {
        let s = SyntheticPoolState {
            status: 9,
            ..Default::default()
        };
        assert!(PoolState::decode(&s.encode()).is_err());

        let s = SyntheticPoolState {
            migrate_type: 9,
            ..Default::default()
        };
        assert!(PoolState::decode(&s.encode()).is_err());

        let s = SyntheticPoolState {
            amm_creator_fee_on: 9,
            ..Default::default()
        };
        assert!(PoolState::decode(&s.encode()).is_err());
    }

    #[test]
    fn pool_state_rejects_impossible_sell_supply() {
        let base = SyntheticPoolState::default();
        let s = SyntheticPoolState {
            total_base_sell: base.supply + 1,
            ..base
        };
        let err = PoolState::decode(&s.encode()).unwrap_err();
        assert!(err.to_string().contains("exceeds supply"), "{err}");
    }

    #[test]
    fn pool_state_rejects_identical_mints() {
        let base = SyntheticPoolState::default();
        let s = SyntheticPoolState {
            quote_mint: base.base_mint,
            ..base
        };
        assert!(PoolState::decode(&s.encode()).is_err());
    }

    #[test]
    fn token_program_flag_bits_select_the_program() {
        let mut s = SyntheticPoolState {
            token_program_flag: 0b01,
            ..Default::default()
        };
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(p.base_is_token_2022());
        assert!(!p.quote_is_token_2022());
        assert_eq!(p.base_token_program(), *crate::consts::TOKEN_2022_PROGRAM);
        assert_eq!(p.quote_token_program(), *crate::consts::TOKEN_PROGRAM);

        s.token_program_flag = 0b10;
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(!p.base_is_token_2022());
        assert!(p.quote_is_token_2022());
    }

    #[test]
    fn curve_open_follows_the_status() {
        let mut s = SyntheticPoolState {
            status: RAYDIUM_LAUNCHLAB_STATUS_FUND,
            ..Default::default()
        };
        assert!(PoolState::decode(&s.encode()).unwrap().is_curve_open());
        s.status = RAYDIUM_LAUNCHLAB_STATUS_MIGRATE;
        assert!(!PoolState::decode(&s.encode()).unwrap().is_curve_open());
        s.status = RAYDIUM_LAUNCHLAB_STATUS_TRADE;
        assert!(!PoolState::decode(&s.encode()).unwrap().is_curve_open());
    }

    #[test]
    fn remaining_and_migratable_amounts_are_computed() {
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
        // The amount of the sale target that is still to be sold.
        assert_eq!(p.remaining_base(), 600);
        // The amount of quote still to be raised.
        assert_eq!(p.remaining_quote_fund_raising(), 7_500);
        // `supply - total_base_sell - total_locked_amount`, the SDK's
        // `amountMigrate`.
        assert_eq!(p.migratable_base(), 900);
    }

    #[test]
    fn global_config_round_trips_and_validates() {
        let quote = Pubkey::new_unique();
        let bytes = encode_synthetic_global_config(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT, &quote);
        assert_eq!(bytes.len(), RAYDIUM_LAUNCHLAB_GLOBAL_CONFIG_LEN);
        let cfg = GlobalConfig::decode(&bytes).unwrap();
        assert_eq!(cfg.curve_type, RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        assert_eq!(cfg.quote_mint, quote);
        assert_eq!(cfg.trade_fee_rate, 2_500);
        assert_eq!(cfg.requires_platform_auth, 0);

        let bad = encode_synthetic_global_config(9, &quote);
        assert!(GlobalConfig::decode(&bad).is_err());
    }

    #[test]
    fn global_config_rejects_a_wrong_length() {
        let quote = Pubkey::new_unique();
        let bytes = encode_synthetic_global_config(0, &quote);
        assert!(GlobalConfig::decode(&bytes[..100]).is_err());
    }

    #[test]
    fn vesting_record_round_trips() {
        let pool = Pubkey::new_unique();
        let beneficiary = Pubkey::new_unique();
        let bytes = encode_synthetic_vesting_record(&pool, &beneficiary);
        assert_eq!(bytes.len(), RAYDIUM_LAUNCHLAB_VESTING_RECORD_LEN);
        let v = VestingAccount::decode(&bytes).unwrap();
        assert_eq!(v.pool, pool);
        assert_eq!(v.beneficiary, beneficiary);
        assert_eq!(v.claimed_amount, 0);
        assert_eq!(v.token_share_amount, 1_000_000);
        assert_eq!(v.remaining(), 1_000_000);
    }

    #[test]
    fn platform_config_round_trips() {
        let bytes = encode_synthetic_platform_config(500, 1_000);
        let p = PlatformConfig::decode(&bytes).unwrap();
        assert_eq!(p.fee_rate, 500);
        assert_eq!(p.creator_fee_rate, 1_000);
    }
}
