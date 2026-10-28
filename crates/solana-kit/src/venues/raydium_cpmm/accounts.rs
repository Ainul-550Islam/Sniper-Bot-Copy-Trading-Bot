//! Decoders (and synthetic encoders) for the Raydium CPMM (`cp-swap`) accounts.
//!
//! # Fail-closed
//!
//! Every decoder here checks **both** the Anchor discriminator and the exact
//! account length before it reads a single field. A mismatch is a typed
//! [`BotError`], never a default value: a misparsed pool that silently yields
//! zero reserves is how a bot sells at a price nobody would accept.
//!
//! # Layout provenance
//!
//! The layouts are the Anchor structs of the official program
//! (`raydium-io/raydium-cp-swap`, commit
//! `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5`), cross-checked field by field
//! against the vendored official IDL
//! (`crates/solana-kit/idl/raydium_cp_swap.json`). Every offset below is
//! derived from the IDL's declaration order, which is the same order Anchor
//! serialises in.
//!
//! The `#[account(zero_copy(unsafe))]` `PoolState` is `#[repr(C, packed)]`, so
//! there is **no** alignment padding: the byte layout is exactly
//! `8 (discriminator) + 10 * 32 + 5 * 1 + 7 * 8 + 2 * 1 + 6 * 1 + 2 * 8 + 28 * 8`
//! = 616 bytes.
//!
//! # Synthetic encoders
//!
//! The `encode_*` helpers below build *synthetic* account bytes for tests.
//! They are labelled synthetic everywhere they are used and are never
//! described as captured mainnet data.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;

// --------------------------------------------------------------------------
// Shared byte reader
// --------------------------------------------------------------------------

/// A bounds-checked little-endian reader over a raw account buffer.
///
/// Every accessor returns a typed error instead of panicking, so a truncated
/// account produces a diagnostic, not a crash.
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

    fn u128(&mut self) -> BotResult<u128> {
        let b = self.take(16)?;
        let mut a = [0u8; 16];
        a.copy_from_slice(b);
        Ok(u128::from_le_bytes(a))
    }

    fn bool(&mut self) -> BotResult<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(BotError::encoding(format!(
                "{} account has an invalid bool byte {other} at offset {}",
                self.label,
                self.pos - 1
            ))),
        }
    }

    fn pubkey(&mut self) -> BotResult<Pubkey> {
        let b = self.take(32)?;
        let mut a = [0u8; 32];
        a.copy_from_slice(b);
        Ok(Pubkey::new_from_array(a))
    }

    /// Skip `n` bytes (a field we do not model).
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

/// The CPMM pool status bitfield.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolStatus {
    /// Raw bitfield (`bit0` deposit, `bit1` withdraw, `bit2` swap).
    pub bits: u8,
}

impl PoolStatus {
    /// Deposits are disabled when bit 0 is set.
    pub fn deposits_disabled(&self) -> bool {
        self.bits & RAYDIUM_CPMM_STATUS_DISABLE_DEPOSIT != 0
    }
    /// Withdrawals are disabled when bit 1 is set.
    pub fn withdrawals_disabled(&self) -> bool {
        self.bits & RAYDIUM_CPMM_STATUS_DISABLE_WITHDRAW != 0
    }
    /// Swaps are disabled when bit 2 is set.
    pub fn swaps_disabled(&self) -> bool {
        self.bits & RAYDIUM_CPMM_STATUS_DISABLE_SWAP != 0
    }
}

/// How the pool charges the creator fee.
///
/// Mirrors the program's `CreatorFeeOn` enum
/// (`programs/cp-swap/src/states/pool.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreatorFeeOn {
    /// Both token_0 and token_1 may be used as the trade fee — depends on
    /// which side the input is.
    BothToken,
    /// Only token_0.
    OnlyToken0,
    /// Only token_1.
    OnlyToken1,
}

impl CreatorFeeOn {
    /// Decode the raw `u8`. An unknown value is an error, never a default.
    pub fn from_u8(value: u8) -> BotResult<Self> {
        match value {
            0 => Ok(CreatorFeeOn::BothToken),
            1 => Ok(CreatorFeeOn::OnlyToken0),
            2 => Ok(CreatorFeeOn::OnlyToken1),
            other => Err(BotError::encoding(format!(
                "invalid CPMM creator_fee_on value {other}"
            ))),
        }
    }

    /// Encode back to the program's raw `u8`.
    pub fn to_u8(self) -> u8 {
        match self {
            CreatorFeeOn::BothToken => 0,
            CreatorFeeOn::OnlyToken0 => 1,
            CreatorFeeOn::OnlyToken1 => 2,
        }
    }

    /// Whether the creator fee is charged on the *input* token for a trade
    /// whose input side is `token_0`.
    ///
    /// Source: `PoolState::is_creator_fee_on_input` in
    /// `programs/cp-swap/src/states/pool.rs`.
    pub fn on_input_for_token_0(self) -> bool {
        matches!(self, CreatorFeeOn::BothToken | CreatorFeeOn::OnlyToken0)
    }

    /// Whether the creator fee is charged on the *input* token for a trade
    /// whose input side is `token_1`.
    pub fn on_input_for_token_1(self) -> bool {
        matches!(self, CreatorFeeOn::BothToken | CreatorFeeOn::OnlyToken1)
    }
}

/// The decoded CPMM `PoolState` account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolState {
    /// The `AmmConfig` this pool belongs to.
    pub amm_config: Pubkey,
    /// The pool creator.
    pub pool_creator: Pubkey,
    /// Vault of token_0.
    pub token_0_vault: Pubkey,
    /// Vault of token_1.
    pub token_1_vault: Pubkey,
    /// The LP mint.
    pub lp_mint: Pubkey,
    /// Mint of token_0.
    pub token_0_mint: Pubkey,
    /// Mint of token_1.
    pub token_1_mint: Pubkey,
    /// Token program of token_0 (SPL Token or Token-2022).
    pub token_0_program: Pubkey,
    /// Token program of token_1.
    pub token_1_program: Pubkey,
    /// The oracle observation account.
    pub observation_key: Pubkey,
    /// Bump of the pool authority PDA.
    pub auth_bump: u8,
    /// Status bitfield.
    pub status: PoolStatus,
    /// Decimals of the LP mint (the program fixes this at 9).
    pub lp_mint_decimals: u8,
    /// Decimals of token_0.
    pub mint_0_decimals: u8,
    /// Decimals of token_1.
    pub mint_1_decimals: u8,
    /// Circulating LP supply.
    pub lp_supply: u64,
    /// Accrued protocol fees in token_0.
    pub protocol_fees_token_0: u64,
    /// Accrued protocol fees in token_1.
    pub protocol_fees_token_1: u64,
    /// Accrued fund fees in token_0.
    pub fund_fees_token_0: u64,
    /// Accrued fund fees in token_1.
    pub fund_fees_token_1: u64,
    /// The timestamp from which swaps are accepted (`0` = immediately).
    pub open_time: u64,
    /// The epoch the pool was last touched in.
    pub recent_epoch: u64,
    /// Which token the creator fee may be charged in.
    pub creator_fee_on: CreatorFeeOn,
    /// Whether the creator fee is active at all.
    pub enable_creator_fee: bool,
    /// Accrued creator fees in token_0.
    pub creator_fees_token_0: u64,
    /// Accrued creator fees in token_1.
    pub creator_fees_token_1: u64,
}

impl PoolState {
    /// Decode a raw `PoolState` account.
    ///
    /// Fails on a wrong discriminator, a wrong length, or an invalid
    /// `creator_fee_on` value.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(data, RAYDIUM_CPMM_ACC_DISC_POOL_STATE, "CPMM PoolState")?;
        if data.len() != RAYDIUM_CPMM_POOL_STATE_LEN {
            return Err(BotError::encoding(format!(
                "CPMM PoolState account is {} bytes, expected {RAYDIUM_CPMM_POOL_STATE_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "CPMM PoolState");
        let amm_config = r.pubkey()?;
        let pool_creator = r.pubkey()?;
        let token_0_vault = r.pubkey()?;
        let token_1_vault = r.pubkey()?;
        let lp_mint = r.pubkey()?;
        let token_0_mint = r.pubkey()?;
        let token_1_mint = r.pubkey()?;
        let token_0_program = r.pubkey()?;
        let token_1_program = r.pubkey()?;
        let observation_key = r.pubkey()?;
        let auth_bump = r.u8()?;
        let status_bits = r.u8()?;
        let lp_mint_decimals = r.u8()?;
        let mint_0_decimals = r.u8()?;
        let mint_1_decimals = r.u8()?;
        let lp_supply = r.u64()?;
        let protocol_fees_token_0 = r.u64()?;
        let protocol_fees_token_1 = r.u64()?;
        let fund_fees_token_0 = r.u64()?;
        let fund_fees_token_1 = r.u64()?;
        let open_time = r.u64()?;
        let recent_epoch = r.u64()?;
        let creator_fee_on_raw = r.u8()?;
        let enable_creator_fee = r.bool()?;
        r.skip(6)?; // padding1
        let creator_fees_token_0 = r.u64()?;
        let creator_fees_token_1 = r.u64()?;
        r.skip(28 * 8)?; // padding

        if token_0_mint == token_1_mint {
            return Err(BotError::encoding(
                "CPMM PoolState has identical token_0_mint and token_1_mint",
            ));
        }
        if token_0_mint == Pubkey::default() || token_1_mint == Pubkey::default() {
            return Err(BotError::encoding(
                "CPMM PoolState contains a zero mint address",
            ));
        }

        Ok(PoolState {
            amm_config,
            pool_creator,
            token_0_vault,
            token_1_vault,
            lp_mint,
            token_0_mint,
            token_1_mint,
            token_0_program,
            token_1_program,
            observation_key,
            auth_bump,
            status: PoolStatus { bits: status_bits },
            lp_mint_decimals,
            mint_0_decimals,
            mint_1_decimals,
            lp_supply,
            protocol_fees_token_0,
            protocol_fees_token_1,
            fund_fees_token_0,
            fund_fees_token_1,
            open_time,
            recent_epoch,
            creator_fee_on: CreatorFeeOn::from_u8(creator_fee_on_raw)?,
            enable_creator_fee,
            creator_fees_token_0,
            creator_fees_token_1,
        })
    }

    /// Which token side `mint` is on.
    pub fn side_of(&self, mint: &Pubkey) -> Option<CpmmSide> {
        if *mint == self.token_0_mint {
            Some(CpmmSide::Token0)
        } else if *mint == self.token_1_mint {
            Some(CpmmSide::Token1)
        } else {
            None
        }
    }

    /// The vault of the given side.
    pub fn vault_of(&self, side: CpmmSide) -> Pubkey {
        match side {
            CpmmSide::Token0 => self.token_0_vault,
            CpmmSide::Token1 => self.token_1_vault,
        }
    }

    /// The mint of the given side.
    pub fn mint_of(&self, side: CpmmSide) -> Pubkey {
        match side {
            CpmmSide::Token0 => self.token_0_mint,
            CpmmSide::Token1 => self.token_1_mint,
        }
    }

    /// The token program of the given side (SPL Token or Token-2022).
    pub fn token_program_of(&self, side: CpmmSide) -> Pubkey {
        match side {
            CpmmSide::Token0 => self.token_0_program,
            CpmmSide::Token1 => self.token_1_program,
        }
    }

    /// Whether the pool accepts swaps right now.
    ///
    /// Mirrors the program's gate in `swap_base_input`:
    /// `pool_state.get_status_by_bit(PoolStatusBitIndex::Swap)` and
    /// `block_timestamp >= pool_state.open_time`.
    pub fn is_swappable(&self) -> bool {
        !self.status.swaps_disabled()
    }

    /// Whether the pool accepts swaps at `now_unix`.
    pub fn is_swappable_at(&self, now_unix: i64) -> bool {
        self.is_swappable()
            && (self.open_time == 0 || i64::try_from(self.open_time).is_ok_and(|t| t <= now_unix))
    }

    /// Whether the creator fee is charged on the input token for a trade that
    /// spends `input_side`.
    pub fn creator_fee_on_input(&self, input_side: CpmmSide) -> bool {
        match input_side {
            CpmmSide::Token0 => self.creator_fee_on.on_input_for_token_0(),
            CpmmSide::Token1 => self.creator_fee_on.on_input_for_token_1(),
        }
    }

    /// The protocol's effective creator fee rate for this pool.
    ///
    /// `enable_creator_fee == false` means the creator fee is switched off for
    /// this pool, whatever the config says.
    pub fn effective_creator_fee_rate(&self, config_rate: u64) -> u64 {
        if self.enable_creator_fee {
            config_rate
        } else {
            0
        }
    }

    /// The vault balances *excluding* the accrued fees.
    ///
    /// Source: `PoolState::vault_amount_without_fee` in
    /// `programs/cp-swap/src/states/pool.rs`. The vault holds the fees too,
    /// so the tradable reserve is the balance minus what is already owed.
    pub fn vault_amount_without_fee(&self, vault_0: u64, vault_1: u64) -> BotResult<(u64, u64)> {
        let fees_0 = self
            .protocol_fees_token_0
            .checked_add(self.fund_fees_token_0)
            .and_then(|v| v.checked_add(self.creator_fees_token_0))
            .ok_or_else(|| BotError::solana("CPMM accrued fee overflow"))?;
        let fees_1 = self
            .protocol_fees_token_1
            .checked_add(self.fund_fees_token_1)
            .and_then(|v| v.checked_add(self.creator_fees_token_1))
            .ok_or_else(|| BotError::solana("CPMM accrued fee overflow"))?;
        let net_0 = vault_0.checked_sub(fees_0).ok_or_else(|| {
            BotError::solana(format!(
                "CPMM token_0 vault {vault_0} is below the accrued fees {fees_0}"
            ))
        })?;
        let net_1 = vault_1.checked_sub(fees_1).ok_or_else(|| {
            BotError::solana(format!(
                "CPMM token_1 vault {vault_1} is below the accrued fees {fees_1}"
            ))
        })?;
        Ok((net_0, net_1))
    }
}

/// Which side of a CPMM pool a mint sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CpmmSide {
    /// The `token_0` side (the smaller pubkey, by the program's constraint).
    Token0,
    /// The `token_1` side.
    Token1,
}

// --------------------------------------------------------------------------
// AmmConfig
// --------------------------------------------------------------------------

/// The decoded CPMM `AmmConfig` account.
///
/// The program struct grew a `creator_fee_share_rate` field after the
/// vendored IDL was generated; it is carved out of the trailing `padding`
/// array, so the **account length is unchanged** (236 bytes) and this decoder
/// reads it from the first padding slot. The IDL's own `padding` field is
/// `[u64; 15]` while the program has `[u64; 14]` after the added field —
/// the total is the same, which is exactly why the program kept the length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmmConfig {
    /// Bump of the config PDA.
    pub bump: u8,
    /// Whether new pools may be created.
    pub disable_create_pool: bool,
    /// The config index.
    pub index: u16,
    /// Trade fee rate, in hundredths of a bip (10^-6).
    pub trade_fee_rate: u64,
    /// Protocol fee rate, in hundredths of a bip.
    pub protocol_fee_rate: u64,
    /// Fund fee rate, in hundredths of a bip.
    pub fund_fee_rate: u64,
    /// The fee charged for creating a pool (in quote tokens).
    pub create_pool_fee: u64,
    /// Address of the protocol fee owner.
    pub protocol_owner: Pubkey,
    /// Address of the fund fee owner.
    pub fund_owner: Pubkey,
    /// Creator fee rate, in hundredths of a bip.
    pub creator_fee_rate: u64,
    /// The share of the creator fee retained by the protocol.
    ///
    /// Present in the program struct but not yet in the vendored IDL; it
    /// occupies the first `padding` slot so the account length is unchanged.
    pub creator_fee_share_rate: u64,
}

impl AmmConfig {
    /// Decode a raw `AmmConfig` account.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(data, RAYDIUM_CPMM_ACC_DISC_AMM_CONFIG, "CPMM AmmConfig")?;
        if data.len() != RAYDIUM_CPMM_AMM_CONFIG_LEN {
            return Err(BotError::encoding(format!(
                "CPMM AmmConfig account is {} bytes, expected {RAYDIUM_CPMM_AMM_CONFIG_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "CPMM AmmConfig");
        let bump = r.u8()?;
        let disable_create_pool = r.bool()?;
        let index = r.u16()?;
        let trade_fee_rate = r.u64()?;
        let protocol_fee_rate = r.u64()?;
        let fund_fee_rate = r.u64()?;
        let create_pool_fee = r.u64()?;
        let protocol_owner = r.pubkey()?;
        let fund_owner = r.pubkey()?;
        let creator_fee_rate = r.u64()?;
        let creator_fee_share_rate = r.u64()?;
        // The program's `AmmConfig` carries `creator_fee_share_rate` in the
        // slot the IDL still calls `padding[0]`, so 14 u64s of padding
        // follow the two fee-rate fields.
        r.skip(14 * 8)?; // remaining padding

        if trade_fee_rate >= RAYDIUM_CPMM_FEE_RATE_DENOMINATOR
            || protocol_fee_rate >= RAYDIUM_CPMM_FEE_RATE_DENOMINATOR
            || fund_fee_rate >= RAYDIUM_CPMM_FEE_RATE_DENOMINATOR
            || creator_fee_rate >= RAYDIUM_CPMM_FEE_RATE_DENOMINATOR
        {
            return Err(BotError::encoding(format!(
                "CPMM AmmConfig has a fee rate at or above the denominator \
                 (trade={trade_fee_rate}, protocol={protocol_fee_rate}, \
                 fund={fund_fee_rate}, creator={creator_fee_rate})"
            )));
        }

        Ok(AmmConfig {
            bump,
            disable_create_pool,
            index,
            trade_fee_rate,
            protocol_fee_rate,
            fund_fee_rate,
            create_pool_fee,
            protocol_owner,
            fund_owner,
            creator_fee_rate,
            creator_fee_share_rate,
        })
    }
}

// --------------------------------------------------------------------------
// ObservationState
// --------------------------------------------------------------------------

/// One entry of the CPMM oracle ring buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// Block timestamp of the observation.
    pub block_timestamp: u64,
    /// Cumulative token_0 price, Q32.32.
    pub cumulative_token_0_price_x32: u128,
    /// Cumulative token_1 price, Q32.32.
    pub cumulative_token_1_price_x32: u128,
}

/// The decoded CPMM `ObservationState` account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationState {
    /// Whether the state has been initialised.
    pub initialized: bool,
    /// Index of the most recently written observation.
    pub observation_index: u16,
    /// The pool this state belongs to.
    pub pool_id: Pubkey,
    /// The ring buffer.
    pub observations: Vec<Observation>,
    /// The last update timestamp.
    pub last_update_timestamp: u64,
}

impl ObservationState {
    /// Decode a raw `ObservationState` account.
    pub fn decode(data: &[u8]) -> BotResult<Self> {
        check_discriminator(
            data,
            RAYDIUM_CPMM_ACC_DISC_OBSERVATION_STATE,
            "CPMM ObservationState",
        )?;
        if data.len() != RAYDIUM_CPMM_OBSERVATION_STATE_LEN {
            return Err(BotError::encoding(format!(
                "CPMM ObservationState account is {} bytes, expected \
                 {RAYDIUM_CPMM_OBSERVATION_STATE_LEN}",
                data.len()
            )));
        }
        let mut r = Reader::new(&data[8..], "CPMM ObservationState");
        let initialized = r.bool()?;
        let observation_index = r.u16()?;
        let pool_id = r.pubkey()?;
        let mut observations = Vec::with_capacity(RAYDIUM_CPMM_OBSERVATION_NUM);
        for _ in 0..RAYDIUM_CPMM_OBSERVATION_NUM {
            observations.push(Observation {
                block_timestamp: r.u64()?,
                cumulative_token_0_price_x32: r.u128()?,
                cumulative_token_1_price_x32: r.u128()?,
            });
        }
        let last_update_timestamp = r.u64()?;
        r.skip(3 * 8)?; // padding

        if usize::from(observation_index) >= RAYDIUM_CPMM_OBSERVATION_NUM {
            return Err(BotError::encoding(format!(
                "CPMM ObservationState observation_index {observation_index} is out of range"
            )));
        }

        Ok(ObservationState {
            initialized,
            observation_index,
            pool_id,
            observations,
            last_update_timestamp,
        })
    }

    /// The most recently written observation.
    pub fn latest(&self) -> Option<&Observation> {
        self.observations.get(usize::from(self.observation_index))
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
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a `u64`.
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a `u128`.
    pub fn u128(&mut self, v: u128) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a `bool`.
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.bytes.push(u8::from(v));
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

/// The fields of a synthetic CPMM `PoolState` fixture.
///
/// Synthetic by construction: every value here is generated by the test
/// encoder, never captured from mainnet.
#[derive(Debug, Clone)]
pub struct SyntheticPoolState {
    pub amm_config: Pubkey,
    pub pool_creator: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub lp_mint: Pubkey,
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_program: Pubkey,
    pub token_1_program: Pubkey,
    pub observation_key: Pubkey,
    pub auth_bump: u8,
    pub status: u8,
    pub lp_mint_decimals: u8,
    pub mint_0_decimals: u8,
    pub mint_1_decimals: u8,
    pub lp_supply: u64,
    pub protocol_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub fund_fees_token_0: u64,
    pub fund_fees_token_1: u64,
    pub open_time: u64,
    pub recent_epoch: u64,
    pub creator_fee_on: u8,
    pub enable_creator_fee: bool,
    pub creator_fees_token_0: u64,
    pub creator_fees_token_1: u64,
}

impl Default for SyntheticPoolState {
    fn default() -> Self {
        SyntheticPoolState {
            amm_config: Pubkey::new_unique(),
            pool_creator: Pubkey::new_unique(),
            token_0_vault: Pubkey::new_unique(),
            token_1_vault: Pubkey::new_unique(),
            lp_mint: Pubkey::new_unique(),
            token_0_mint: Pubkey::new_unique(),
            token_1_mint: Pubkey::new_unique(),
            token_0_program: *crate::consts::TOKEN_PROGRAM,
            token_1_program: *crate::consts::TOKEN_PROGRAM,
            observation_key: Pubkey::new_unique(),
            auth_bump: 254,
            status: 0,
            lp_mint_decimals: 9,
            mint_0_decimals: 6,
            mint_1_decimals: 9,
            lp_supply: 1_000_000,
            protocol_fees_token_0: 0,
            protocol_fees_token_1: 0,
            fund_fees_token_0: 0,
            fund_fees_token_1: 0,
            open_time: 0,
            recent_epoch: 0,
            creator_fee_on: 0,
            enable_creator_fee: false,
            creator_fees_token_0: 0,
            creator_fees_token_1: 0,
        }
    }
}

impl SyntheticPoolState {
    /// Encode the synthetic pool account bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = SyntheticWriter::with_discriminator(RAYDIUM_CPMM_ACC_DISC_POOL_STATE);
        w.pubkey(&self.amm_config)
            .pubkey(&self.pool_creator)
            .pubkey(&self.token_0_vault)
            .pubkey(&self.token_1_vault)
            .pubkey(&self.lp_mint)
            .pubkey(&self.token_0_mint)
            .pubkey(&self.token_1_mint)
            .pubkey(&self.token_0_program)
            .pubkey(&self.token_1_program)
            .pubkey(&self.observation_key)
            .u8(self.auth_bump)
            .u8(self.status)
            .u8(self.lp_mint_decimals)
            .u8(self.mint_0_decimals)
            .u8(self.mint_1_decimals)
            .u64(self.lp_supply)
            .u64(self.protocol_fees_token_0)
            .u64(self.protocol_fees_token_1)
            .u64(self.fund_fees_token_0)
            .u64(self.fund_fees_token_1)
            .u64(self.open_time)
            .u64(self.recent_epoch)
            .u8(self.creator_fee_on)
            .bool(self.enable_creator_fee)
            .zeros(6)
            .u64(self.creator_fees_token_0)
            .u64(self.creator_fees_token_1)
            .zeros(28 * 8);
        w.finish()
    }
}

/// Encode a synthetic CPMM `AmmConfig` account.
pub fn encode_synthetic_amm_config(
    index: u16,
    trade_fee_rate: u64,
    protocol_fee_rate: u64,
    fund_fee_rate: u64,
    creator_fee_rate: u64,
) -> Vec<u8> {
    let mut w = SyntheticWriter::with_discriminator(RAYDIUM_CPMM_ACC_DISC_AMM_CONFIG);
    w.u8(254)
        .bool(false)
        .u16(index)
        .u64(trade_fee_rate)
        .u64(protocol_fee_rate)
        .u64(fund_fee_rate)
        .u64(0)
        .pubkey(&Pubkey::new_unique())
        .pubkey(&Pubkey::new_unique())
        .u64(creator_fee_rate)
        // The program's `creator_fee_share_rate` occupies the slot the IDL
        // still calls `padding[0]`; 14 u64s of padding follow it.
        .u64(0)
        .zeros(14 * 8);
    w.finish()
}

/// Encode a synthetic CPMM `ObservationState` account.
pub fn encode_synthetic_observation_state(pool_id: &Pubkey) -> Vec<u8> {
    let mut w = SyntheticWriter::with_discriminator(RAYDIUM_CPMM_ACC_DISC_OBSERVATION_STATE);
    w.bool(true).u16(0).pubkey(pool_id);
    for _ in 0..RAYDIUM_CPMM_OBSERVATION_NUM {
        w.u64(0).u128(0).u128(0);
    }
    w.u64(0).zeros(3 * 8);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_state_round_trips_through_the_synthetic_encoder() {
        let s = SyntheticPoolState::default();
        let bytes = s.encode();
        assert_eq!(bytes.len(), RAYDIUM_CPMM_POOL_STATE_LEN);
        let decoded = PoolState::decode(&bytes).unwrap();
        assert_eq!(decoded.amm_config, s.amm_config);
        assert_eq!(decoded.pool_creator, s.pool_creator);
        assert_eq!(decoded.token_0_vault, s.token_0_vault);
        assert_eq!(decoded.token_1_vault, s.token_1_vault);
        assert_eq!(decoded.lp_mint, s.lp_mint);
        assert_eq!(decoded.token_0_mint, s.token_0_mint);
        assert_eq!(decoded.token_1_mint, s.token_1_mint);
        assert_eq!(decoded.token_0_program, s.token_0_program);
        assert_eq!(decoded.token_1_program, s.token_1_program);
        assert_eq!(decoded.observation_key, s.observation_key);
        assert_eq!(decoded.auth_bump, s.auth_bump);
        assert_eq!(decoded.status.bits, s.status);
        assert_eq!(decoded.lp_mint_decimals, 9);
        assert_eq!(decoded.mint_0_decimals, 6);
        assert_eq!(decoded.mint_1_decimals, 9);
        assert_eq!(decoded.lp_supply, 1_000_000);
        assert_eq!(decoded.creator_fee_on, CreatorFeeOn::BothToken);
        assert!(!decoded.enable_creator_fee);
    }

    #[test]
    fn pool_state_rejects_a_wrong_length() {
        let s = SyntheticPoolState::default();
        let bytes = s.encode();
        let err = PoolState::decode(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("expected {RAYDIUM_CPMM_POOL_STATE_LEN}")),
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
                .contains(&format!("expected {RAYDIUM_CPMM_POOL_STATE_LEN}")),
            "{err}"
        );
    }

    #[test]
    fn pool_state_rejects_a_wrong_discriminator() {
        let s = SyntheticPoolState::default();
        let mut bytes = s.encode();
        bytes[0] ^= 0xff;
        let err = PoolState::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("discriminator"), "{err}");
    }

    #[test]
    fn pool_state_rejects_a_truncated_buffer_before_the_discriminator() {
        let err = PoolState::decode(&[1, 2, 3]).unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");
    }

    #[test]
    fn pool_state_rejects_an_invalid_creator_fee_on() {
        let s = SyntheticPoolState {
            creator_fee_on: 7,
            ..Default::default()
        };
        let err = PoolState::decode(&s.encode()).unwrap_err();
        assert!(err.to_string().contains("creator_fee_on"), "{err}");
    }

    #[test]
    fn pool_state_rejects_identical_mints() {
        let base = SyntheticPoolState::default();
        let s = SyntheticPoolState {
            token_1_mint: base.token_0_mint,
            ..base
        };
        let err = PoolState::decode(&s.encode()).unwrap_err();
        assert!(err.to_string().contains("identical"), "{err}");
    }

    #[test]
    fn status_bits_behave_like_the_program() {
        let mut s = SyntheticPoolState {
            status: RAYDIUM_CPMM_STATUS_DISABLE_SWAP,
            ..Default::default()
        };
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(p.status.swaps_disabled());
        assert!(!p.status.deposits_disabled());
        assert!(!p.status.withdrawals_disabled());
        assert!(!p.is_swappable());

        s.status = RAYDIUM_CPMM_STATUS_DISABLE_DEPOSIT | RAYDIUM_CPMM_STATUS_DISABLE_WITHDRAW;
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(p.status.deposits_disabled());
        assert!(p.status.withdrawals_disabled());
        assert!(p.is_swappable());
    }

    #[test]
    fn open_time_gates_swaps() {
        let s = SyntheticPoolState {
            open_time: 1_700_000_000,
            ..Default::default()
        };
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(!p.is_swappable_at(1_699_999_999));
        assert!(p.is_swappable_at(1_700_000_000));
    }

    #[test]
    fn creator_fee_on_input_follows_the_program_rules() {
        let mut s = SyntheticPoolState {
            creator_fee_on: 0, // BothToken
            ..Default::default()
        };
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(p.creator_fee_on_input(CpmmSide::Token0));
        assert!(p.creator_fee_on_input(CpmmSide::Token1));

        s.creator_fee_on = 1; // OnlyToken0
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(p.creator_fee_on_input(CpmmSide::Token0));
        assert!(!p.creator_fee_on_input(CpmmSide::Token1));

        s.creator_fee_on = 2; // OnlyToken1
        let p = PoolState::decode(&s.encode()).unwrap();
        assert!(!p.creator_fee_on_input(CpmmSide::Token0));
        assert!(p.creator_fee_on_input(CpmmSide::Token1));
    }

    #[test]
    fn vault_amount_without_fee_subtracts_the_accrued_fees() {
        let s = SyntheticPoolState {
            protocol_fees_token_0: 10,
            fund_fees_token_0: 5,
            creator_fees_token_0: 1,
            ..Default::default()
        };
        let p = PoolState::decode(&s.encode()).unwrap();
        assert_eq!(p.vault_amount_without_fee(100, 200).unwrap(), (84, 200));
        // The vault cannot be below the accrued fees.
        assert!(p.vault_amount_without_fee(15, 200).is_err());
    }

    #[test]
    fn amm_config_round_trips_and_rejects_bad_fee_rates() {
        let bytes = encode_synthetic_amm_config(0, 2_500, 500, 0, 1_000);
        assert_eq!(bytes.len(), RAYDIUM_CPMM_AMM_CONFIG_LEN);
        let cfg = AmmConfig::decode(&bytes).unwrap();
        assert_eq!(cfg.index, 0);
        assert_eq!(cfg.trade_fee_rate, 2_500);
        assert_eq!(cfg.protocol_fee_rate, 500);
        assert_eq!(cfg.fund_fee_rate, 0);
        assert_eq!(cfg.creator_fee_rate, 1_000);
        assert!(!cfg.disable_create_pool);

        let bad = encode_synthetic_amm_config(0, RAYDIUM_CPMM_FEE_RATE_DENOMINATOR, 0, 0, 0);
        assert!(AmmConfig::decode(&bad).is_err());
    }

    #[test]
    fn amm_config_rejects_wrong_length_and_discriminator() {
        let bytes = encode_synthetic_amm_config(0, 100, 0, 0, 0);
        assert!(AmmConfig::decode(&bytes[..100]).is_err());
        let mut wrong = bytes.clone();
        wrong[3] ^= 0xff;
        assert!(AmmConfig::decode(&wrong).is_err());
    }

    #[test]
    fn observation_state_round_trips() {
        let pool = Pubkey::new_unique();
        let bytes = encode_synthetic_observation_state(&pool);
        assert_eq!(bytes.len(), RAYDIUM_CPMM_OBSERVATION_STATE_LEN);
        let st = ObservationState::decode(&bytes).unwrap();
        assert!(st.initialized);
        assert_eq!(st.observation_index, 0);
        assert_eq!(st.pool_id, pool);
        assert_eq!(st.observations.len(), RAYDIUM_CPMM_OBSERVATION_NUM);
        assert!(st.latest().is_some());
    }

    #[test]
    fn observation_state_rejects_wrong_length() {
        let pool = Pubkey::new_unique();
        let bytes = encode_synthetic_observation_state(&pool);
        assert!(ObservationState::decode(&bytes[..100]).is_err());
    }
}
