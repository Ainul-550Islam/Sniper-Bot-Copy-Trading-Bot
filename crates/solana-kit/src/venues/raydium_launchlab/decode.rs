//! Decoding Raydium LaunchLab (`launchpad`) instructions and events into the
//! venue-neutral event types used elsewhere in `solana-kit`.
//!
//! # What is decoded
//!
//! * [`LaunchLabTradeEvent`] — the program's Anchor `TradeEvent`, decoded
//!   from a `Program data: <base64>` log line. It carries the amounts, the
//!   fee split and the pool status, which is everything a sniper needs to
//!   decide whether the token graduated.
//! * [`LaunchLabPoolCreateEvent`] — the program's Anchor `PoolCreateEvent`,
//!   which fires when a pool is created (the launch signal).
//! * [`LaunchLabPoolInitEvent`] — a pool creation decoded from the
//!   `initialize_v2` *instruction*, for the case where the event log was not
//!   captured.
//!
//! # Graduation detection
//!
//! The LaunchLab pool's `status` moves `Fund → Migrate → Trade`. The
//! graduation signal is the transition out of `Fund`, which is exactly what
//! [`LaunchLabTradeEvent::pool_status`] and
//! [`LaunchLabTradeEvent::is_graduated`] expose.

use base64::Engine;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;
use crate::decode::{DecodedSwap, Side, SwapVenue};
use crate::events::PumpEvent;

/// A tiny forward-only cursor over an event body.
///
/// The LaunchLab event bodies are borsh-encoded, so the fields are read in
/// order. Every read is bounds-checked and fails closed.
struct EventReader<'a> {
    body: &'a [u8],
    pos: usize,
    label: &'static str,
}

impl<'a> EventReader<'a> {
    fn new(body: &'a [u8], label: &'static str) -> Self {
        EventReader {
            body,
            pos: 0,
            label,
        }
    }

    fn take(&mut self, n: usize) -> BotResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| BotError::encoding(format!("{} length overflow", self.label)))?;
        if end > self.body.len() {
            return Err(BotError::encoding(format!(
                "{} body is truncated (needed {n} bytes at offset {})",
                self.label, self.pos
            )));
        }
        let s = &self.body[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> BotResult<u8> {
        let b = self.take(1)?;
        Ok(b[0])
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

    fn string(&mut self) -> BotResult<String> {
        let len = u32::from_le_bytes({
            let b = self.take(4)?;
            let mut a = [0u8; 4];
            a.copy_from_slice(b);
            a
        }) as usize;
        let b = self.take(len)?;
        String::from_utf8(b.to_vec())
            .map_err(|e| BotError::encoding(format!("{} string is not utf-8: {e}", self.label)))
    }
}

/// The LaunchLab trade direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchLabTradeDirection {
    /// A buy: the user spent quote and received base.
    Buy,
    /// A sell: the user spent base and received quote.
    Sell,
}

impl LaunchLabTradeDirection {
    /// Decode the raw enum discriminant. An unknown value is an error.
    pub fn from_u8(value: u8) -> BotResult<Self> {
        match value {
            0 => Ok(LaunchLabTradeDirection::Buy),
            1 => Ok(LaunchLabTradeDirection::Sell),
            other => Err(BotError::encoding(format!(
                "invalid LaunchLab trade_direction {other}"
            ))),
        }
    }

    /// Encode back to the raw discriminant.
    pub fn to_u8(self) -> u8 {
        match self {
            LaunchLabTradeDirection::Buy => 0,
            LaunchLabTradeDirection::Sell => 1,
        }
    }
}

/// The decoded LaunchLab `TradeEvent`.
///
/// Field order and types follow the program's `TradeEvent` struct, as recorded
/// in the vendored IDL's `TradeEvent` type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchLabTradeEvent {
    /// The pool the trade happened in.
    pub pool_state: Pubkey,
    /// The total base sold after the trade.
    pub total_base_sell: u64,
    /// The virtual base parameter after the trade.
    pub virtual_base: u64,
    /// The virtual quote parameter after the trade.
    pub virtual_quote: u64,
    /// The real base amount before the trade.
    pub real_base_before: u64,
    /// The real quote amount before the trade.
    pub real_quote_before: u64,
    /// The real base amount after the trade.
    pub real_base_after: u64,
    /// The real quote amount after the trade.
    pub real_quote_after: u64,
    /// The input amount.
    pub amount_in: u64,
    /// The output amount.
    pub amount_out: u64,
    /// The protocol fee.
    pub protocol_fee: u64,
    /// The platform fee.
    pub platform_fee: u64,
    /// The creator fee.
    pub creator_fee: u64,
    /// The share fee.
    pub share_fee: u64,
    /// The trade direction.
    pub trade_direction: LaunchLabTradeDirection,
    /// The pool status after the trade.
    pub pool_status: u8,
    /// Whether the trade was an exact-in trade.
    pub exact_in: bool,
}

impl LaunchLabTradeEvent {
    /// Decode the raw Anchor event bytes (discriminator + borsh body).
    pub fn decode(bytes: &[u8]) -> BotResult<Self> {
        if bytes.len() < 8 {
            return Err(BotError::encoding(format!(
                "LaunchLab TradeEvent payload is {} bytes, too short for a discriminator",
                bytes.len()
            )));
        }
        if bytes[..8] != RAYDIUM_LAUNCHLAB_EV_TRADE {
            return Err(BotError::encoding(format!(
                "LaunchLab TradeEvent payload has discriminator {:02x?}, expected {:02x?}",
                &bytes[..8],
                RAYDIUM_LAUNCHLAB_EV_TRADE
            )));
        }
        let body = &bytes[8..];
        // 8 x u64 (64) + pubkey (32) + 4 x u64 (32) + 2 enums (2) + bool (1)
        // = 131 bytes.
        const BODY_LEN: usize = 8 * 8 + 32 + 4 * 8 + 2 + 1;
        if body.len() < BODY_LEN {
            return Err(BotError::encoding(format!(
                "LaunchLab TradeEvent body is {} bytes, expected at least {BODY_LEN}",
                body.len()
            )));
        }
        let mut pos = 0usize;
        let mut take = |n: usize| -> BotResult<&[u8]> {
            if pos + n > body.len() {
                return Err(BotError::encoding("LaunchLab TradeEvent body is truncated"));
            }
            let s = &body[pos..pos + n];
            pos += n;
            Ok(s)
        };
        let u64_at = |bytes: &[u8]| -> u64 {
            let mut a = [0u8; 8];
            a.copy_from_slice(bytes);
            u64::from_le_bytes(a)
        };

        let pool_state = {
            let mut a = [0u8; 32];
            a.copy_from_slice(take(32)?);
            Pubkey::new_from_array(a)
        };
        let total_base_sell = u64_at(take(8)?);
        let virtual_base = u64_at(take(8)?);
        let virtual_quote = u64_at(take(8)?);
        let real_base_before = u64_at(take(8)?);
        let real_quote_before = u64_at(take(8)?);
        let real_base_after = u64_at(take(8)?);
        let real_quote_after = u64_at(take(8)?);
        let amount_in = u64_at(take(8)?);
        let amount_out = u64_at(take(8)?);
        let protocol_fee = u64_at(take(8)?);
        let platform_fee = u64_at(take(8)?);
        let creator_fee = u64_at(take(8)?);
        let share_fee = u64_at(take(8)?);
        let trade_direction = LaunchLabTradeDirection::from_u8(take(1)?[0])?;
        let pool_status = take(1)?[0];
        let exact_in = match take(1)?[0] {
            0 => false,
            1 => true,
            other => {
                return Err(BotError::encoding(format!(
                    "LaunchLab TradeEvent has an invalid bool byte {other}"
                )))
            }
        };

        // The pool status must be one the program defines.
        crate::venues::raydium_launchlab::migration::validate_status(pool_status)?;

        Ok(LaunchLabTradeEvent {
            pool_state,
            total_base_sell,
            virtual_base,
            virtual_quote,
            real_base_before,
            real_quote_before,
            real_base_after,
            real_quote_after,
            amount_in,
            amount_out,
            protocol_fee,
            platform_fee,
            creator_fee,
            share_fee,
            trade_direction,
            pool_status,
            exact_in,
        })
    }

    /// Decode a `Program data: <base64>` log payload.
    pub fn decode_base64(payload: &str) -> BotResult<Self> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| BotError::encoding(format!("LaunchLab TradeEvent base64 payload: {e}")))?;
        Self::decode(&bytes)
    }

    /// Whether the pool has graduated (the status left `Fund`).
    pub fn is_graduated(&self) -> bool {
        self.pool_status != RAYDIUM_LAUNCHLAB_STATUS_FUND
    }

    /// The pool status as a typed value.
    pub fn status(
        &self,
    ) -> BotResult<crate::venues::raydium_launchlab::accounts::LaunchLabPoolStatus> {
        crate::venues::raydium_launchlab::accounts::LaunchLabPoolStatus::from_u8(self.pool_status)
    }
}

/// The decoded LaunchLab `PoolCreateEvent`.
///
/// This is the launch signal: it carries the mint parameters and the curve
/// parameters of the new pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchLabPoolCreateEvent {
    /// The pool account.
    pub pool_state: Pubkey,
    /// The creator of the token.
    pub creator: Pubkey,
    /// The `GlobalConfig` the pool follows.
    pub config: Pubkey,
    /// The base mint's decimals.
    pub base_decimals: u8,
    /// The base mint's name.
    pub base_name: String,
    /// The base mint's symbol.
    pub base_symbol: String,
    /// The base mint's metadata URI.
    pub base_uri: String,
    /// The curve type (`0` constant, `1` fixed, `2` linear).
    pub curve_type: u8,
    /// The curve's supply parameter.
    pub curve_supply: u64,
    /// The curve's total base sell parameter.
    pub curve_total_base_sell: u64,
    /// The curve's total quote fund-raising parameter.
    pub curve_total_quote_fund_raising: u64,
    /// The curve's migration type.
    pub curve_migrate_type: u8,
    /// The vesting schedule's total locked amount.
    pub vesting_total_locked_amount: u64,
    /// The vesting schedule's cliff period.
    pub vesting_cliff_period: u64,
    /// The vesting schedule's unlock period.
    pub vesting_unlock_period: u64,
}

impl LaunchLabPoolCreateEvent {
    /// Decode the raw Anchor event bytes (discriminator + borsh body).
    pub fn decode(bytes: &[u8]) -> BotResult<Self> {
        if bytes.len() < 8 {
            return Err(BotError::encoding(format!(
                "LaunchLab PoolCreateEvent payload is {} bytes, too short for a discriminator",
                bytes.len()
            )));
        }
        if bytes[..8] != RAYDIUM_LAUNCHLAB_EV_POOL_CREATE {
            return Err(BotError::encoding(format!(
                "LaunchLab PoolCreateEvent payload has discriminator {:02x?}, expected {:02x?}",
                &bytes[..8],
                RAYDIUM_LAUNCHLAB_EV_POOL_CREATE
            )));
        }
        let mut r = EventReader::new(&bytes[8..], "LaunchLab PoolCreateEvent");

        let pool_state = r.pubkey()?;
        let creator = r.pubkey()?;
        let config = r.pubkey()?;

        // MintParams: decimals (u8), name (string), symbol (string), uri (string)
        let base_decimals = r.u8()?;
        let base_name = r.string()?;
        let base_symbol = r.string()?;
        let base_uri = r.string()?;

        // CurveParams: an enum with three variants.
        //   Constant { supply, total_base_sell, total_quote_fund_raising, migrate_type }
        //   Fixed    { supply, total_quote_fund_raising, migrate_type }
        //   Linear   { supply, total_quote_fund_raising, migrate_type }
        // The discriminant is a u8 followed by the variant's fields.
        let curve_type = r.u8()?;
        let curve_supply = r.u64()?;
        let (curve_total_base_sell, curve_total_quote_fund_raising) = match curve_type {
            RAYDIUM_LAUNCHLAB_CURVE_CONSTANT => {
                let total_base_sell = r.u64()?;
                let total_quote = r.u64()?;
                (total_base_sell, total_quote)
            }
            RAYDIUM_LAUNCHLAB_CURVE_FIXED | RAYDIUM_LAUNCHLAB_CURVE_LINEAR => {
                let total_quote = r.u64()?;
                (0, total_quote)
            }
            other => {
                return Err(BotError::encoding(format!(
                    "LaunchLab PoolCreateEvent has an unknown curve type {other}"
                )))
            }
        };
        let curve_migrate_type = r.u8()?;

        // VestingParams: total_locked_amount, cliff_period, unlock_period.
        let vesting_total_locked_amount = r.u64()?;
        let vesting_cliff_period = r.u64()?;
        let vesting_unlock_period = r.u64()?;

        // AmmCreatorFeeOn: a one-byte enum.
        let _amm_fee_on = r.u8()?;

        Ok(LaunchLabPoolCreateEvent {
            pool_state,
            creator,
            config,
            base_decimals,
            base_name,
            base_symbol,
            base_uri,
            curve_type,
            curve_supply,
            curve_total_base_sell,
            curve_total_quote_fund_raising,
            curve_migrate_type,
            vesting_total_locked_amount,
            vesting_cliff_period,
            vesting_unlock_period,
        })
    }

    /// Decode a `Program data: <base64>` log payload.
    pub fn decode_base64(payload: &str) -> BotResult<Self> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| {
                BotError::encoding(format!("LaunchLab PoolCreateEvent base64 payload: {e}"))
            })?;
        Self::decode(&bytes)
    }
}

/// A LaunchLab pool creation, decoded from an `initialize_v2` instruction.
///
/// The LaunchLab program *does* emit a `PoolCreateEvent`, but a
/// `logsSubscribe` consumer may only have the instruction; this decoder is
/// the fallback.
///
/// Account order (the IDL's `initialize` and `initialize_v2` `accounts`, 18 accounts):
///
/// ```text
///  0 payer              6 base_mint         12 quote_token_program
///  1 creator            7 quote_mint        13 metadata_program
///  2 global_config      8 base_vault        14 system_program
///  3 platform_config    9 quote_vault       15 rent_program
///  4 authority         10 metadata_account  16 event_authority
///  5 pool_state        11 base_token_program 17 program
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchLabPoolInitEvent {
    /// The pool account.
    pub pool_state: Pubkey,
    /// The creator of the token.
    pub creator: Pubkey,
    /// The `GlobalConfig`.
    pub global_config: Pubkey,
    /// The `PlatformConfig`.
    pub platform_config: Pubkey,
    /// The base mint.
    pub base_mint: Pubkey,
    /// The quote mint.
    pub quote_mint: Pubkey,
    /// The base vault.
    pub base_vault: Pubkey,
    /// The quote vault.
    pub quote_vault: Pubkey,
    /// The curve type from the `CurveParams` argument.
    pub curve_type: u8,
    /// The curve's supply.
    pub supply: u64,
    /// The curve's total base sell.
    pub total_base_sell: u64,
    /// The curve's total quote fund-raising.
    pub total_quote_fund_raising: u64,
    /// The curve's migration type.
    pub migrate_type: u8,
    /// The base mint's decimals.
    pub decimals: u8,
}

impl LaunchLabPoolInitEvent {
    /// Decode a LaunchLab `initialize` or `initialize_v2` instruction.
    ///
    /// Returns `Ok(None)` for anything that is not a LaunchLab
    /// `initialize` / `initialize_v2`; a malformed one is an **error**.
    pub fn from_instruction(
        program_id: &Pubkey,
        accounts: &[Pubkey],
        data: &[u8],
    ) -> BotResult<Option<Self>> {
        if *program_id != *RAYDIUM_LAUNCHLAB {
            return Ok(None);
        }
        if data.len() < 8
            || (data[..8] != RAYDIUM_LAUNCHLAB_DISC_INITIALIZE
                && data[..8] != RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_V2)
        {
            return Ok(None);
        }
        const ACC_CREATOR: usize = 1;
        const ACC_GLOBAL_CONFIG: usize = 2;
        const ACC_PLATFORM_CONFIG: usize = 3;
        const ACC_POOL_STATE: usize = 5;
        const ACC_BASE_MINT: usize = 6;
        const ACC_QUOTE_MINT: usize = 7;
        const ACC_BASE_VAULT: usize = 8;
        const ACC_QUOTE_VAULT: usize = 9;
        const MIN_ACCOUNTS: usize = ACC_QUOTE_VAULT + 1;
        if accounts.len() < MIN_ACCOUNTS {
            return Err(BotError::encoding(format!(
                "LaunchLab initialize instruction has {} accounts, expected at least {MIN_ACCOUNTS}",
                accounts.len()
            )));
        }
        // The args are: MintParams, CurveParams, VestingParams[, AmmCreatorFeeOn].
        // MintParams starts with the decimals byte, then three length-prefixed
        // strings. We only need the decimals and the curve params, but the
        // strings must be walked to reach them.
        if data.len() < 9 {
            return Err(BotError::encoding(
                "LaunchLab initialize instruction data is truncated",
            ));
        }
        let mut pos = 8usize;
        let decimals = data[pos];
        pos += 1;
        for _ in 0..3 {
            if pos + 4 > data.len() {
                return Err(BotError::encoding(
                    "LaunchLab initialize mint params are truncated",
                ));
            }
            let mut l = [0u8; 4];
            l.copy_from_slice(&data[pos..pos + 4]);
            let len = u32::from_le_bytes(l) as usize;
            pos += 4;
            if pos + len > data.len() {
                return Err(BotError::encoding(
                    "LaunchLab initialize mint params are truncated",
                ));
            }
            pos += len;
        }
        // CurveParams: a u8 discriminant then the variant's fields.
        if pos + 1 > data.len() {
            return Err(BotError::encoding(
                "LaunchLab initialize curve params are truncated",
            ));
        }
        let curve_type = data[pos];
        pos += 1;
        let u64_at = |off: usize| -> BotResult<u64> {
            if off + 8 > data.len() {
                return Err(BotError::encoding(
                    "LaunchLab initialize curve params are truncated",
                ));
            }
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            Ok(u64::from_le_bytes(b))
        };
        let supply = u64_at(pos)?;
        pos += 8;
        let total_base_sell = match curve_type {
            RAYDIUM_LAUNCHLAB_CURVE_CONSTANT => {
                let v = u64_at(pos)?;
                pos += 8;
                v
            }
            RAYDIUM_LAUNCHLAB_CURVE_FIXED | RAYDIUM_LAUNCHLAB_CURVE_LINEAR => 0,
            other => {
                return Err(BotError::encoding(format!(
                    "LaunchLab initialize has an unknown curve type {other}"
                )))
            }
        };
        let total_quote_fund_raising = u64_at(pos)?;
        pos += 8;
        if pos >= data.len() {
            return Err(BotError::encoding(
                "LaunchLab initialize curve params are truncated",
            ));
        }
        let migrate_type = data[pos];

        Ok(Some(LaunchLabPoolInitEvent {
            pool_state: accounts[ACC_POOL_STATE],
            creator: accounts[ACC_CREATOR],
            global_config: accounts[ACC_GLOBAL_CONFIG],
            platform_config: accounts[ACC_PLATFORM_CONFIG],
            base_mint: accounts[ACC_BASE_MINT],
            quote_mint: accounts[ACC_QUOTE_MINT],
            base_vault: accounts[ACC_BASE_VAULT],
            quote_vault: accounts[ACC_QUOTE_VAULT],
            curve_type,
            supply,
            total_base_sell,
            total_quote_fund_raising,
            migrate_type,
            decimals,
        }))
    }
}

/// Decoded LaunchLab trade instruction (`buy_exact_in`, `buy_exact_out`,
/// `sell_exact_in`, `sell_exact_out`).
///
/// Account order follows the IDL prefix (15 accounts, `0..=14`):
/// `0 payer, 1 authority, 2 global_config, 3 platform_config, 4 pool_state,
/// 5 user_base_token, 6 user_quote_token, 7 base_vault, 8 quote_vault,
/// 9 base_token_mint, 10 quote_token_mint, 11 base_token_program,
/// 12 quote_token_program, 13 event_authority, 14 program`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchLabSwapInstruction {
    /// Trade direction (`Buy` or `Sell`).
    pub trade_direction: LaunchLabTradeDirection,
    /// `true` for `*_exact_in`, `false` for `*_exact_out`.
    pub exact_in: bool,
    /// Signing trader (`payer`).
    pub payer: Pubkey,
    /// Pool state account.
    pub pool_state: Pubkey,
    /// GlobalConfig account.
    pub global_config: Pubkey,
    /// PlatformConfig account.
    pub platform_config: Pubkey,
    /// Base token mint.
    pub base_mint: Pubkey,
    /// Quote token mint.
    pub quote_mint: Pubkey,
    /// First `u64` argument (`amount_in` for exact-in, `amount_out` for exact-out).
    pub primary_amount: u64,
    /// Second `u64` argument (`minimum_amount_out` for exact-in, `maximum_amount_in` for exact-out).
    pub limit_amount: u64,
    /// Third `u64` argument (`share_fee_rate`).
    pub share_fee_rate: u64,
}

impl LaunchLabSwapInstruction {
    /// Decode one of the four LaunchLab trade instructions.
    pub fn from_instruction(
        program_id: &Pubkey,
        accounts: &[Pubkey],
        data: &[u8],
    ) -> BotResult<Option<Self>> {
        if *program_id != *RAYDIUM_LAUNCHLAB || data.len() < 8 {
            return Ok(None);
        }
        let (trade_direction, exact_in) = if data[..8] == RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN {
            (LaunchLabTradeDirection::Buy, true)
        } else if data[..8] == RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT {
            (LaunchLabTradeDirection::Buy, false)
        } else if data[..8] == RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN {
            (LaunchLabTradeDirection::Sell, true)
        } else if data[..8] == RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT {
            (LaunchLabTradeDirection::Sell, false)
        } else {
            return Ok(None);
        };
        const MIN_ACCOUNTS: usize = 15;
        if accounts.len() < MIN_ACCOUNTS {
            return Err(BotError::encoding(format!(
                "LaunchLab trade instruction has {} accounts, expected at least {MIN_ACCOUNTS}",
                accounts.len()
            )));
        }
        if data.len() < 32 {
            return Err(BotError::encoding(format!(
                "LaunchLab trade instruction data is {} bytes, expected at least 32",
                data.len()
            )));
        }
        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(b)
        };
        Ok(Some(LaunchLabSwapInstruction {
            trade_direction,
            exact_in,
            payer: accounts[0],
            global_config: accounts[2],
            platform_config: accounts[3],
            pool_state: accounts[4],
            base_mint: accounts[9],
            quote_mint: accounts[10],
            primary_amount: u64_at(8),
            limit_amount: u64_at(16),
            share_fee_rate: u64_at(24),
        }))
    }
}

/// Decoded LaunchLab graduation instruction (`migrate_to_amm` or
/// `migrate_to_cpswap`).
///
/// Carries the graduated LaunchLab `pool_state` and the newly created
/// `destination_pool` (`amm_pool` at index 13 for `migrate_to_amm`, or
/// `cpswap_pool` at index 5 for `migrate_to_cpswap`) from the vendored IDL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchLabMigrationEvent {
    /// `0` (`RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM`) or `1` (`RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP`).
    pub migrate_type: u8,
    /// Signing caller (`payer`).
    pub payer: Pubkey,
    /// Graduating LaunchLab `pool_state`.
    pub pool_state: Pubkey,
    /// Newly created AMM v4 (`amm_pool`) or CPMM (`cpswap_pool`) destination pool.
    pub destination_pool: Pubkey,
    /// Base token mint.
    pub base_mint: Pubkey,
    /// Quote token mint.
    pub quote_mint: Pubkey,
    /// LaunchLab `global_config`.
    pub global_config: Pubkey,
}

impl LaunchLabMigrationEvent {
    /// Decode a `migrate_to_amm` or `migrate_to_cpswap` instruction.
    pub fn from_instruction(
        program_id: &Pubkey,
        accounts: &[Pubkey],
        data: &[u8],
    ) -> BotResult<Option<Self>> {
        if *program_id != *RAYDIUM_LAUNCHLAB || data.len() < 8 {
            return Ok(None);
        }
        if data[..8] == RAYDIUM_LAUNCHLAB_DISC_MIGRATE_TO_AMM {
            // IDL `migrate_to_amm`: 32 accounts, args: (u64, u64, u8) = 17 bytes.
            const MIN_ACCOUNTS: usize = 25;
            const MIN_DATA: usize = 8 + 17;
            if accounts.len() < MIN_ACCOUNTS {
                return Err(BotError::encoding(format!(
                    "LaunchLab migrate_to_amm instruction has {} accounts, expected at least {MIN_ACCOUNTS}",
                    accounts.len()
                )));
            }
            if data.len() < MIN_DATA {
                return Err(BotError::encoding(format!(
                    "LaunchLab migrate_to_amm data is {} bytes, expected at least {MIN_DATA}",
                    data.len()
                )));
            }
            Ok(Some(LaunchLabMigrationEvent {
                migrate_type: RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM,
                payer: accounts[0],
                base_mint: accounts[1],
                quote_mint: accounts[2],
                destination_pool: accounts[13],
                pool_state: accounts[23],
                global_config: accounts[24],
            }))
        } else if data[..8] == RAYDIUM_LAUNCHLAB_DISC_MIGRATE_TO_CPSWAP {
            // IDL `migrate_to_cpswap`: 28 accounts, args: [] = 0 bytes.
            const MIN_ACCOUNTS: usize = 19;
            if accounts.len() < MIN_ACCOUNTS {
                return Err(BotError::encoding(format!(
                    "LaunchLab migrate_to_cpswap instruction has {} accounts, expected at least {MIN_ACCOUNTS}",
                    accounts.len()
                )));
            }
            Ok(Some(LaunchLabMigrationEvent {
                migrate_type: RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
                payer: accounts[0],
                base_mint: accounts[1],
                quote_mint: accounts[2],
                destination_pool: accounts[5],
                pool_state: accounts[17],
                global_config: accounts[18],
            }))
        } else {
            Ok(None)
        }
    }
}

/// Convert a LaunchLab trade event into the crate's neutral [`DecodedSwap`].
///
/// The LaunchLab curve always trades base against quote, so the base mint is
/// the token the pool sells and the quote mint is the token it accepts.
#[allow(clippy::too_many_arguments)]
pub fn to_decoded_swap(
    event: &LaunchLabTradeEvent,
    signature: &str,
    slot: u64,
    block_time: Option<i64>,
    wallet: &Pubkey,
    base_mint: &Pubkey,
    base_decimals: u8,
    quote_mint: &Pubkey,
    quote_decimals: u8,
) -> BotResult<DecodedSwap> {
    let (side, base_amount, quote_amount) = match event.trade_direction {
        LaunchLabTradeDirection::Buy => (Side::Buy, event.amount_out, event.amount_in),
        LaunchLabTradeDirection::Sell => (Side::Sell, event.amount_in, event.amount_out),
    };
    Ok(DecodedSwap {
        signature: signature.to_string(),
        slot,
        block_time,
        wallet: wallet.to_string(),
        side,
        // `SwapVenue` is a pre-existing public enum that `module-copy` matches
        // exhaustively, and `module-copy` is owned by another batch. Adding a
        // `RaydiumLaunchLab` variant here would break that crate's build, so
        // the neutral conversion reports the generic `Other` venue instead.
        // The LaunchLab-specific detail is preserved in
        // [`LaunchLabTradeEvent`], which carries the pool status, the fee
        // split and the amounts.
        venue: SwapVenue::Other,
        base_mint: base_mint.to_string(),
        quote_mint: quote_mint.to_string(),
        base_amount,
        quote_amount,
        base_decimals,
        quote_decimals,
        fee_lamports: 0,
        price_quote_per_base: if base_amount == 0 {
            0.0
        } else {
            let base = bot_core::maths::from_raw_amount(base_amount, base_decimals);
            let quote = bot_core::maths::from_raw_amount(quote_amount, quote_decimals);
            if base == 0.0 {
                0.0
            } else {
                quote / base
            }
        },
        programs: vec![RAYDIUM_LAUNCHLAB.to_string()],
        events: Vec::new(),
        logs: Vec::new(),
        succeeded: true,
        error: None,
    })
}

/// Convert a decoded LaunchLab trade instruction into the crate's neutral
/// [`DecodedSwap`] when the CPI `TradeEvent` log is unavailable.
#[allow(clippy::too_many_arguments)]
pub fn to_decoded_swap_from_instruction(
    ix: &LaunchLabSwapInstruction,
    signature: &str,
    slot: u64,
    block_time: Option<i64>,
    base_decimals: u8,
    quote_decimals: u8,
) -> BotResult<DecodedSwap> {
    let (amount_in, amount_out) = if ix.exact_in {
        (ix.primary_amount, ix.limit_amount)
    } else {
        (ix.limit_amount, ix.primary_amount)
    };
    let synthetic_event = LaunchLabTradeEvent {
        pool_state: ix.pool_state,
        total_base_sell: 0,
        virtual_base: 0,
        virtual_quote: 0,
        real_base_before: 0,
        real_quote_before: 0,
        real_base_after: 0,
        real_quote_after: 0,
        amount_in,
        amount_out,
        protocol_fee: 0,
        platform_fee: 0,
        creator_fee: 0,
        share_fee: 0,
        trade_direction: ix.trade_direction,
        pool_status: RAYDIUM_LAUNCHLAB_STATUS_FUND,
        exact_in: ix.exact_in,
    };
    to_decoded_swap(
        &synthetic_event,
        signature,
        slot,
        block_time,
        &ix.payer,
        &ix.base_mint,
        base_decimals,
        &ix.quote_mint,
        quote_decimals,
    )
}

/// Convert a LaunchLab pool creation into the crate's neutral pool-creation
/// event.
pub fn to_pool_creation_event(init: &LaunchLabPoolInitEvent) -> PumpEvent {
    PumpEvent::CreatePool {
        timestamp: 0,
        index: 0,
        creator: init.creator,
        base_mint: init.base_mint,
        quote_mint: init.quote_mint,
        base_mint_decimals: init.decimals,
        quote_mint_decimals: 0,
        base_amount_in: 0,
        quote_amount_in: init.total_quote_fund_raising,
        pool_base_amount: init.total_base_sell,
        pool_quote_amount: init.total_quote_fund_raising,
        pool_bump: 0,
        pool: init.pool_state,
        lp_mint: Pubkey::default(),
        coin_creator: init.creator,
    }
}

/// Convert a decoded LaunchLab `migrate_to_amm` / `migrate_to_cpswap`
/// graduation instruction into the crate's neutral [`PumpEvent::Migration`].
pub fn to_migration_event(ev: &LaunchLabMigrationEvent) -> PumpEvent {
    PumpEvent::Migration {
        user: ev.payer,
        mint: ev.base_mint,
        mint_amount: 0,
        sol_amount: 0,
        pool_migration_fee: 0,
        bonding_curve: ev.pool_state,
        timestamp: 0,
        pool: ev.destination_pool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_trade_event(pool_status: u8) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&RAYDIUM_LAUNCHLAB_EV_TRADE);
        b.extend_from_slice(&[1u8; 32]); // pool_state
        b.extend_from_slice(&100u64.to_le_bytes()); // total_base_sell
        b.extend_from_slice(&200u64.to_le_bytes()); // virtual_base
        b.extend_from_slice(&300u64.to_le_bytes()); // virtual_quote
        b.extend_from_slice(&400u64.to_le_bytes()); // real_base_before
        b.extend_from_slice(&500u64.to_le_bytes()); // real_quote_before
        b.extend_from_slice(&600u64.to_le_bytes()); // real_base_after
        b.extend_from_slice(&700u64.to_le_bytes()); // real_quote_after
        b.extend_from_slice(&10u64.to_le_bytes()); // amount_in
        b.extend_from_slice(&20u64.to_le_bytes()); // amount_out
        b.extend_from_slice(&1u64.to_le_bytes()); // protocol_fee
        b.extend_from_slice(&2u64.to_le_bytes()); // platform_fee
        b.extend_from_slice(&3u64.to_le_bytes()); // creator_fee
        b.extend_from_slice(&4u64.to_le_bytes()); // share_fee
        b.push(0); // trade_direction = Buy
        b.push(pool_status);
        b.push(1); // exact_in
        b
    }

    #[test]
    fn trade_event_decodes_every_field() {
        let ev = LaunchLabTradeEvent::decode(&encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_FUND))
            .unwrap();
        assert_eq!(ev.pool_state, Pubkey::new_from_array([1u8; 32]));
        assert_eq!(ev.total_base_sell, 100);
        assert_eq!(ev.virtual_base, 200);
        assert_eq!(ev.virtual_quote, 300);
        assert_eq!(ev.real_base_before, 400);
        assert_eq!(ev.real_quote_before, 500);
        assert_eq!(ev.real_base_after, 600);
        assert_eq!(ev.real_quote_after, 700);
        assert_eq!(ev.amount_in, 10);
        assert_eq!(ev.amount_out, 20);
        assert_eq!(ev.protocol_fee, 1);
        assert_eq!(ev.platform_fee, 2);
        assert_eq!(ev.creator_fee, 3);
        assert_eq!(ev.share_fee, 4);
        assert_eq!(ev.trade_direction, LaunchLabTradeDirection::Buy);
        assert_eq!(ev.pool_status, RAYDIUM_LAUNCHLAB_STATUS_FUND);
        assert!(ev.exact_in);
    }

    #[test]
    fn trade_event_detects_the_graduation() {
        let ev = LaunchLabTradeEvent::decode(&encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_FUND))
            .unwrap();
        assert!(!ev.is_graduated());
        let ev = LaunchLabTradeEvent::decode(&encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_MIGRATE))
            .unwrap();
        assert!(ev.is_graduated());
        let ev = LaunchLabTradeEvent::decode(&encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_TRADE))
            .unwrap();
        assert!(ev.is_graduated());
    }

    #[test]
    fn trade_event_rejects_a_wrong_discriminator() {
        let mut bytes = encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_FUND);
        bytes[0] ^= 0xff;
        let err = LaunchLabTradeEvent::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("discriminator"), "{err}");
    }

    #[test]
    fn trade_event_rejects_a_truncated_body_and_bad_values() {
        let bytes = encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_FUND);
        assert!(LaunchLabTradeEvent::decode(&bytes[..40]).is_err());
        assert!(LaunchLabTradeEvent::decode(&bytes[..4]).is_err());

        // An invalid trade direction. The body is one pubkey followed by
        // thirteen u64 fields, then the direction, the status and the
        // exact-in flag.
        let mut bad = bytes.clone();
        let dir_off = 8 + 32 + 13 * 8;
        bad[dir_off] = 9;
        assert!(LaunchLabTradeEvent::decode(&bad).is_err());

        // An invalid pool status.
        let mut bad = bytes.clone();
        bad[dir_off + 1] = 9;
        let err = LaunchLabTradeEvent::decode(&bad).unwrap_err();
        assert!(err.to_string().contains("status"), "{err}");
    }

    #[test]
    fn trade_event_decodes_from_base64() {
        use base64::Engine;
        let bytes = encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_FUND);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let ev = LaunchLabTradeEvent::decode_base64(&b64).unwrap();
        assert_eq!(ev.amount_in, 10);
        assert!(LaunchLabTradeEvent::decode_base64("!!!").is_err());
    }

    fn encode_pool_create_event(curve_type: u8) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&RAYDIUM_LAUNCHLAB_EV_POOL_CREATE);
        b.extend_from_slice(&[2u8; 32]); // pool_state
        b.extend_from_slice(&[3u8; 32]); // creator
        b.extend_from_slice(&[4u8; 32]); // config
        b.push(6); // decimals
        for s in ["Token", "TKN", "https://example.test/meta.json"] {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        }
        b.push(curve_type);
        b.extend_from_slice(&1_000_000_000_000u64.to_le_bytes()); // supply
        if curve_type == RAYDIUM_LAUNCHLAB_CURVE_CONSTANT {
            b.extend_from_slice(&793_100_000_000_000u64.to_le_bytes()); // total_base_sell
        }
        b.extend_from_slice(&85_000_000_000u64.to_le_bytes()); // total_quote
        b.push(1); // migrate_type
        b.extend_from_slice(&0u64.to_le_bytes()); // total_locked_amount
        b.extend_from_slice(&0u64.to_le_bytes()); // cliff_period
        b.extend_from_slice(&0u64.to_le_bytes()); // unlock_period
        b.push(0); // amm_fee_on
        b
    }

    #[test]
    fn pool_create_event_decodes_the_constant_curve() {
        let ev = LaunchLabPoolCreateEvent::decode(&encode_pool_create_event(
            RAYDIUM_LAUNCHLAB_CURVE_CONSTANT,
        ))
        .unwrap();
        assert_eq!(ev.pool_state, Pubkey::new_from_array([2u8; 32]));
        assert_eq!(ev.creator, Pubkey::new_from_array([3u8; 32]));
        assert_eq!(ev.config, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(ev.base_decimals, 6);
        assert_eq!(ev.base_name, "Token");
        assert_eq!(ev.base_symbol, "TKN");
        assert_eq!(ev.base_uri, "https://example.test/meta.json");
        assert_eq!(ev.curve_type, RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        assert_eq!(ev.curve_supply, 1_000_000_000_000);
        assert_eq!(ev.curve_total_base_sell, 793_100_000_000_000);
        assert_eq!(ev.curve_total_quote_fund_raising, 85_000_000_000);
        assert_eq!(ev.curve_migrate_type, 1);
    }

    #[test]
    fn pool_create_event_decodes_the_fixed_and_linear_curves() {
        for curve in [
            RAYDIUM_LAUNCHLAB_CURVE_FIXED,
            RAYDIUM_LAUNCHLAB_CURVE_LINEAR,
        ] {
            let ev = LaunchLabPoolCreateEvent::decode(&encode_pool_create_event(curve)).unwrap();
            assert_eq!(ev.curve_type, curve);
            assert_eq!(ev.curve_total_base_sell, 0);
            assert_eq!(ev.curve_total_quote_fund_raising, 85_000_000_000);
        }
    }

    #[test]
    fn pool_create_event_rejects_an_unknown_curve() {
        let mut bytes = encode_pool_create_event(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        // The curve discriminant sits after the three length-prefixed
        // strings. The offset is derived from the encoded bytes rather than
        // hard-coded, so the test cannot drift from the encoder.
        let mut off = 8 + 3 * 32 + 1; // discriminator + pubkeys + decimals
        for s in ["Token", "TKN", "https://example.test/meta.json"] {
            off += 4 + s.len();
        }
        bytes[off] = 9;
        let err = LaunchLabPoolCreateEvent::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("curve type"), "{err}");
    }

    fn init_v2_data(curve_type: u8) -> Vec<u8> {
        let mut d = RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_V2.to_vec();
        d.push(6); // decimals
        for s in ["Token", "TKN", "https://example.test/meta.json"] {
            d.extend_from_slice(&(s.len() as u32).to_le_bytes());
            d.extend_from_slice(s.as_bytes());
        }
        d.push(curve_type);
        d.extend_from_slice(&1_000_000_000_000u64.to_le_bytes()); // supply
        if curve_type == RAYDIUM_LAUNCHLAB_CURVE_CONSTANT {
            d.extend_from_slice(&793_100_000_000_000u64.to_le_bytes());
        }
        d.extend_from_slice(&85_000_000_000u64.to_le_bytes()); // total_quote
        d.push(1); // migrate_type
                   // VestingParams.
        d.extend_from_slice(&0u64.to_le_bytes());
        d.extend_from_slice(&0u64.to_le_bytes());
        d.extend_from_slice(&0u64.to_le_bytes());
        // AmmCreatorFeeOn.
        d.push(0);
        d
    }

    #[test]
    fn pool_init_event_decodes_the_idl_account_order() {
        let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
        let ev = LaunchLabPoolInitEvent::from_instruction(
            &RAYDIUM_LAUNCHLAB,
            &accounts,
            &init_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT),
        )
        .unwrap()
        .expect("an initialize_v2 decodes");
        assert_eq!(ev.pool_state, accounts[5]);
        assert_eq!(ev.creator, accounts[1]);
        assert_eq!(ev.global_config, accounts[2]);
        assert_eq!(ev.platform_config, accounts[3]);
        assert_eq!(ev.base_mint, accounts[6]);
        assert_eq!(ev.quote_mint, accounts[7]);
        assert_eq!(ev.base_vault, accounts[8]);
        assert_eq!(ev.quote_vault, accounts[9]);
        assert_eq!(ev.decimals, 6);
        assert_eq!(ev.curve_type, RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        assert_eq!(ev.supply, 1_000_000_000_000);
        assert_eq!(ev.total_base_sell, 793_100_000_000_000);
        assert_eq!(ev.total_quote_fund_raising, 85_000_000_000);
        assert_eq!(ev.migrate_type, 1);

        // Legacy `initialize` shares the same first 18 accounts and MintParams + CurveParams prefix.
        let mut init_v1 = init_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        init_v1[..8].copy_from_slice(&RAYDIUM_LAUNCHLAB_DISC_INITIALIZE);
        init_v1.pop(); // no trailing AmmCreatorFeeOn byte
        let ev1 = LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts, &init_v1)
            .unwrap()
            .expect("initialize decodes");
        assert_eq!(ev1, ev);
    }

    #[test]
    fn trade_and_migration_instructions_decode_and_convert() {
        let trade_accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
        for (disc, expected_dir, expected_exact_in, expected_side) in [
            (
                RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN,
                LaunchLabTradeDirection::Buy,
                true,
                Side::Buy,
            ),
            (
                RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT,
                LaunchLabTradeDirection::Buy,
                false,
                Side::Buy,
            ),
            (
                RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN,
                LaunchLabTradeDirection::Sell,
                true,
                Side::Sell,
            ),
            (
                RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT,
                LaunchLabTradeDirection::Sell,
                false,
                Side::Sell,
            ),
        ] {
            let mut d = disc.to_vec();
            d.extend_from_slice(&1_000u64.to_le_bytes());
            d.extend_from_slice(&900u64.to_le_bytes());
            d.extend_from_slice(&50u64.to_le_bytes());
            let ix =
                LaunchLabSwapInstruction::from_instruction(&RAYDIUM_LAUNCHLAB, &trade_accounts, &d)
                    .unwrap()
                    .expect("trade ix decodes");
            assert_eq!(ix.trade_direction, expected_dir);
            assert_eq!(ix.exact_in, expected_exact_in);
            assert_eq!(ix.pool_state, trade_accounts[4]);
            assert_eq!(ix.base_mint, trade_accounts[9]);
            assert_eq!(ix.quote_mint, trade_accounts[10]);
            let swap = to_decoded_swap_from_instruction(&ix, "sig", 1, None, 6, 9).unwrap();
            assert_eq!(swap.side, expected_side);
            assert_eq!(swap.venue, SwapVenue::Other);
        }

        let amm_accounts: Vec<Pubkey> = (0..32).map(|_| Pubkey::new_unique()).collect();
        let mut amm_data = RAYDIUM_LAUNCHLAB_DISC_MIGRATE_TO_AMM.to_vec();
        amm_data.extend_from_slice(&100u64.to_le_bytes());
        amm_data.extend_from_slice(&200u64.to_le_bytes());
        amm_data.push(1);
        let mig_amm =
            LaunchLabMigrationEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &amm_accounts, &amm_data)
                .unwrap()
                .expect("migrate_to_amm decodes");
        assert_eq!(mig_amm.migrate_type, RAYDIUM_LAUNCHLAB_MIGRATE_TO_AMM);
        assert_eq!(mig_amm.destination_pool, amm_accounts[13]);
        assert_eq!(mig_amm.pool_state, amm_accounts[23]);

        let cpswap_accounts: Vec<Pubkey> = (0..28).map(|_| Pubkey::new_unique()).collect();
        let mig_cp = LaunchLabMigrationEvent::from_instruction(
            &RAYDIUM_LAUNCHLAB,
            &cpswap_accounts,
            &RAYDIUM_LAUNCHLAB_DISC_MIGRATE_TO_CPSWAP,
        )
        .unwrap()
        .expect("migrate_to_cpswap decodes");
        assert_eq!(mig_cp.migrate_type, RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP);
        assert_eq!(mig_cp.destination_pool, cpswap_accounts[5]);
        assert_eq!(mig_cp.pool_state, cpswap_accounts[17]);
        let pump_ev = to_migration_event(&mig_cp);
        match pump_ev {
            PumpEvent::Migration {
                bonding_curve,
                pool,
                mint,
                ..
            } => {
                assert_eq!(pool, cpswap_accounts[5]);
                assert_eq!(bonding_curve, cpswap_accounts[17]);
                assert_eq!(mint, cpswap_accounts[1]);
            }
            other => panic!("expected PumpEvent::Migration, got {other:?}"),
        }
    }

    #[test]
    fn pool_init_event_ignores_other_programs_and_instructions() {
        let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
        let data = init_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        assert!(
            LaunchLabPoolInitEvent::from_instruction(&Pubkey::new_unique(), &accounts, &data)
                .unwrap()
                .is_none()
        );
        let mut wrong = data.clone();
        wrong[0] ^= 0xff;
        assert!(
            LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts, &wrong)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pool_init_event_errors_on_a_malformed_instruction() {
        let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
        assert!(LaunchLabPoolInitEvent::from_instruction(
            &RAYDIUM_LAUNCHLAB,
            &accounts[..5],
            &init_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT)
        )
        .is_err());
        assert!(LaunchLabPoolInitEvent::from_instruction(
            &RAYDIUM_LAUNCHLAB,
            &accounts,
            &init_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT)[..20]
        )
        .is_err());
    }

    #[test]
    fn to_decoded_swap_maps_the_sides() {
        let ev = LaunchLabTradeEvent::decode(&encode_trade_event(RAYDIUM_LAUNCHLAB_STATUS_FUND))
            .unwrap();
        let wallet = Pubkey::new_unique();
        let base = Pubkey::new_unique();
        let quote = Pubkey::new_unique();
        let buy = to_decoded_swap(&ev, "sig", 1, None, &wallet, &base, 6, &quote, 9).unwrap();
        assert_eq!(buy.side, Side::Buy);
        assert_eq!(buy.base_amount, 20);
        assert_eq!(buy.quote_amount, 10);
        assert_eq!(buy.base_mint, base.to_string());
        assert_eq!(buy.quote_mint, quote.to_string());
    }

    #[test]
    fn pool_creation_event_carries_the_pool() {
        let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
        let init = LaunchLabPoolInitEvent::from_instruction(
            &RAYDIUM_LAUNCHLAB,
            &accounts,
            &init_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT),
        )
        .unwrap()
        .unwrap();
        let ev = to_pool_creation_event(&init);
        assert_eq!(ev.pool(), Some(init.pool_state));
        assert_eq!(ev.user(), Some(init.creator));
    }
}
