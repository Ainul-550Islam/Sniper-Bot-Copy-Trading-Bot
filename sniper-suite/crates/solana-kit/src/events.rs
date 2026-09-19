//! Anchor event decoding from transaction logs.
//!
//! A `logsSubscribe` notification gives you the log lines but not the
//! instruction data, so the fastest way to learn *what* happened is to decode
//! the `Program data: <base64>` lines Anchor emits for every `emit!`. For a
//! sniper that matters: a `CreateEvent` carries the mint, the creator, the
//! initial reserves and the token program in a single log line, with no extra
//! RPC round trip.
//!
//! ## Telling events apart
//!
//! Both pump programs emit a log line of the form
//! `Program data: <base64>`. The discriminator decides which event it is, and
//! the discriminators never collide between the two programs — but a naive
//! parser can be fooled by *instruction* logs, which Anchor also routes
//! through `Program data:` in the format `<program_name>:<InstructionName>`.
//! Decoding those as base64 yields 4 bytes of garbage (`pump` from
//! `pump_amm:BuyEvent`) rather than an event, so [`decode_data_line`] rejects
//! anything shorter than 8 bytes after the discriminator.

use base64::Engine;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, trace};

use bot_core::error::{BotError, BotResult};

use crate::consts::*;

/// A decoded pump.fun or PumpSwap event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PumpEvent {
    /// `pump::CreateEvent` — a brand-new token launched on the bonding curve.
    /// This is the signal Module 1 acts on.
    Create {
        name: String,
        symbol: String,
        uri: String,
        mint: Pubkey,
        bonding_curve: Pubkey,
        user: Pubkey,
        creator: Pubkey,
        timestamp: i64,
        virtual_token_reserves: u64,
        virtual_sol_reserves: u64,
        real_token_reserves: u64,
        token_total_supply: u64,
        token_program: Pubkey,
        is_mayhem_mode: bool,
        is_cashback_enabled: bool,
        /// Present on the multi-quote version of the event.
        quote_mint: Option<Pubkey>,
        virtual_quote_reserves: Option<u64>,
        creator_fee_bps: Option<u64>,
        is_holder_reward: Option<bool>,
    },
    /// `pump::TradeEvent` — a bonding-curve buy or sell.
    Trade {
        mint: Pubkey,
        sol_amount: u64,
        token_amount: u64,
        is_buy: bool,
        user: Pubkey,
        timestamp: i64,
        virtual_sol_reserves: u64,
        virtual_token_reserves: u64,
        real_sol_reserves: u64,
        real_token_reserves: u64,
        fee_recipient: Pubkey,
        fee_basis_points: u64,
        fee: u64,
        creator: Pubkey,
        creator_fee_basis_points: u64,
        creator_fee: u64,
        ix_name: String,
    },
    /// `pump::CompleteEvent` — the curve filled; migration is now possible.
    Complete {
        user: Pubkey,
        mint: Pubkey,
        bonding_curve: Pubkey,
        timestamp: i64,
    },
    /// `pump::CompletePumpAmmMigrationEvent` — the token graduated to PumpSwap.
    Migration {
        user: Pubkey,
        mint: Pubkey,
        mint_amount: u64,
        sol_amount: u64,
        pool_migration_fee: u64,
        bonding_curve: Pubkey,
        timestamp: i64,
        pool: Pubkey,
    },
    /// `pump_amm::CreatePoolEvent` — a PumpSwap pool was created.
    CreatePool {
        timestamp: i64,
        index: u16,
        creator: Pubkey,
        base_mint: Pubkey,
        quote_mint: Pubkey,
        base_mint_decimals: u8,
        quote_mint_decimals: u8,
        base_amount_in: u64,
        quote_amount_in: u64,
        pool_base_amount: u64,
        pool_quote_amount: u64,
        pool_bump: u8,
        pool: Pubkey,
        lp_mint: Pubkey,
        coin_creator: Pubkey,
    },
    /// `pump_amm::BuyEvent`.
    AmmBuy {
        timestamp: i64,
        base_amount_out: u64,
        quote_amount_in: u64,
        pool: Pubkey,
        user: Pubkey,
        pool_base_token_reserves: u64,
        pool_quote_token_reserves: u64,
        lp_fee: u64,
        protocol_fee: u64,
        coin_creator_fee: u64,
        ix_name: String,
    },
    /// `pump_amm::SellEvent`.
    AmmSell {
        timestamp: i64,
        base_amount_in: u64,
        quote_amount_out: u64,
        pool: Pubkey,
        user: Pubkey,
        pool_base_token_reserves: u64,
        pool_quote_token_reserves: u64,
        lp_fee: u64,
        protocol_fee: u64,
        coin_creator_fee: u64,
    },
    /// An event we recognised the discriminator of but could not fully decode,
    /// or one we do not model. Kept so nothing is silently dropped.
    Unknown {
        discriminator: [u8; 8],
        /// Program that emitted it, when the log line said so.
        program: Option<Pubkey>,
        payload_len: usize,
    },
}

impl PumpEvent {
    /// The mint this event is about, where the event carries one.
    pub fn mint(&self) -> Option<Pubkey> {
        match self {
            PumpEvent::Create { mint, .. } => Some(*mint),
            PumpEvent::Trade { mint, .. } => Some(*mint),
            PumpEvent::Complete { mint, .. } => Some(*mint),
            PumpEvent::Migration { mint, .. } => Some(*mint),
            PumpEvent::CreatePool { base_mint, .. } => Some(*base_mint),
            // AMM buy/sell events carry the pool, not the mint.
            PumpEvent::AmmBuy { .. } | PumpEvent::AmmSell { .. } | PumpEvent::Unknown { .. } => {
                None
            }
        }
    }

    /// The pool address, for AMM events.
    pub fn pool(&self) -> Option<Pubkey> {
        match self {
            PumpEvent::CreatePool { pool, .. }
            | PumpEvent::AmmBuy { pool, .. }
            | PumpEvent::AmmSell { pool, .. }
            | PumpEvent::Migration { pool, .. } => Some(*pool),
            _ => None,
        }
    }

    /// The actor, where the event names one.
    pub fn user(&self) -> Option<Pubkey> {
        match self {
            PumpEvent::Create { user, .. }
            | PumpEvent::Trade { user, .. }
            | PumpEvent::Complete { user, .. }
            | PumpEvent::Migration { user, .. }
            | PumpEvent::AmmBuy { user, .. }
            | PumpEvent::AmmSell { user, .. } => Some(*user),
            PumpEvent::CreatePool { creator, .. } => Some(*creator),
            PumpEvent::Unknown { .. } => None,
        }
    }

    /// A one-line description for logs and Telegram alerts.
    pub fn describe(&self) -> String {
        match self {
            PumpEvent::Create {
                name,
                symbol,
                mint,
                creator,
                ..
            } => {
                format!("launch {symbol} ({name}) mint={mint} creator={creator}")
            }
            PumpEvent::Trade {
                mint,
                is_buy,
                sol_amount,
                token_amount,
                user,
                ..
            } => {
                let side = if *is_buy { "buy" } else { "sell" };
                format!(
                    "{side} mint={mint} sol={} tokens={token_amount} by {user}",
                    sol_amount
                )
            }
            PumpEvent::Complete { mint, .. } => format!("curve complete mint={mint}"),
            PumpEvent::Migration { mint, pool, .. } => {
                format!("migrated mint={mint} -> pool={pool}")
            }
            PumpEvent::CreatePool {
                base_mint,
                quote_mint,
                pool,
                ..
            } => {
                format!("pool created {base_mint}/{quote_mint} pool={pool}")
            }
            PumpEvent::AmmBuy {
                pool,
                user,
                quote_amount_in,
                base_amount_out,
                ..
            } => {
                format!(
                    "amm buy pool={pool} quote_in={quote_amount_in} base_out={base_amount_out} by {user}"
                )
            }
            PumpEvent::AmmSell {
                pool,
                user,
                quote_amount_out,
                base_amount_in,
                ..
            } => {
                format!(
                    "amm sell pool={pool} base_in={base_amount_in} quote_out={quote_amount_out} by {user}"
                )
            }
            PumpEvent::Unknown {
                discriminator,
                payload_len,
                ..
            } => format!(
                "unknown event disc={:02x?} ({payload_len} bytes)",
                discriminator
            ),
        }
    }
}

/// Decode one `Program data: …` payload.
pub fn decode_data_line(b64: &str) -> Option<PumpEvent> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    decode_event(&bytes)
}

/// Decode an event from its raw bytes (discriminator + borsh body).
pub fn decode_event(bytes: &[u8]) -> Option<PumpEvent> {
    if bytes.len() < 8 {
        return None;
    }
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&bytes[..8]);
    let body = &bytes[8..];
    let mut c = Cursor::new(body);

    let event = match disc {
        EV_PUMP_CREATE => decode_create(&mut c).ok()?,
        EV_PUMP_TRADE => decode_trade(&mut c).ok()?,
        EV_PUMP_COMPLETE => decode_complete(&mut c).ok()?,
        EV_PUMP_MIGRATION => decode_migration(&mut c).ok()?,
        EV_AMM_CREATE_POOL => decode_create_pool(&mut c).ok()?,
        EV_AMM_BUY => decode_amm_buy(&mut c).ok()?,
        EV_AMM_SELL => decode_amm_sell(&mut c).ok()?,
        other => {
            trace!(disc = format!("{other:02x?}"), "unmodelled pump event");
            PumpEvent::Unknown {
                discriminator: other,
                program: None,
                payload_len: body.len(),
            }
        }
    };
    Some(event)
}

/// Scan a transaction's log lines and return every pump event in them.
///
/// Handles both `Program data: <base64>` (the emitted event) and the
/// `Program <id> invoke [1]` lines that precede it, so events can be attributed
/// to the program that emitted them.
pub fn parse_logs(logs: &[String]) -> Vec<PumpEvent> {
    let mut out = Vec::new();
    let mut current_program: Option<Pubkey> = None;

    for line in logs {
        // NB: "Program data: <b64>" also starts with "Program ", so the
        // more-specific data prefix MUST be tested first or event payloads get
        // mistaken for invoke/success lines and dropped.
        if let Some(payload) = line.strip_prefix(ANCHOR_DATA_LOG_PREFIX) {
            match decode_data_line(payload) {
                Some(mut event) => {
                    if let PumpEvent::Unknown { program, .. } = &mut event {
                        *program = current_program;
                    }
                    out.push(event);
                }
                None => {
                    // Instruction-name logs (`pump:Buy`) and anything else land here.
                    debug!(payload = %payload.chars().take(40).collect::<String>(), "not an event payload");
                }
            }
            continue;
        }
        if let Some(id) = line.strip_prefix("Program ") {
            // "Program <id> invoke [1]" / "Program <id> success"
            if let Some(end) = id.find(' ') {
                if let Ok(pk) = Pubkey::try_from(id[..end].trim()) {
                    current_program = Some(pk);
                }
            }
            continue;
        }
    }
    out
}

/// Scan logs for the specific signal that a token just launched.
pub fn find_launch(logs: &[String]) -> Option<PumpEvent> {
    parse_logs(logs)
        .into_iter()
        .find(|e| matches!(e, PumpEvent::Create { .. }))
}

/// Scan logs for a graduation (curve complete or migration).
pub fn find_graduation(logs: &[String]) -> Option<PumpEvent> {
    parse_logs(logs).into_iter().find(|e| {
        matches!(
            e,
            PumpEvent::Complete { .. } | PumpEvent::Migration { .. } | PumpEvent::CreatePool { .. }
        )
    })
}

// --------------------------------------------------------------------------
// Borsh reader
// --------------------------------------------------------------------------

/// Minimal borsh reader. Borsh is little-endian with no framing: fixed-width
/// ints inline, `bool` as one byte, `Pubkey` as 32 bytes, `string` and `vec`
/// prefixed by a `u32` length.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

#[allow(dead_code)] // complete borsh reader; not every accessor is exercised yet
impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> BotResult<&'a [u8]> {
        if self.remaining() < n {
            return Err(BotError::encoding(format!(
                "event truncated: wanted {n} bytes at offset {}, have {}",
                self.pos,
                self.remaining()
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

    fn i64(&mut self) -> BotResult<i64> {
        Ok(self.u64()? as i64)
    }

    fn i128(&mut self) -> BotResult<i128> {
        let b = self.take(16)?;
        let mut a = [0u8; 16];
        a.copy_from_slice(b);
        Ok(i128::from_le_bytes(a))
    }

    fn bool(&mut self) -> BotResult<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(BotError::encoding(format!(
                "invalid bool byte {other} at offset {}",
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

    fn string(&mut self) -> BotResult<String> {
        let len = self.u32()? as usize;
        // A bogus length is the usual symptom of a layout mismatch; refuse
        // rather than allocate.
        if len > self.remaining() {
            return Err(BotError::encoding(format!(
                "string length {len} exceeds the {} bytes left",
                self.remaining()
            )));
        }
        let b = self.take(len)?;
        String::from_utf8(b.to_vec())
            .map_err(|e| BotError::encoding(format!("string is not utf-8: {e}")))
    }

    fn u32(&mut self) -> BotResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Skip `n` bytes (a field we do not model).
    fn skip(&mut self, n: usize) -> BotResult<()> {
        self.take(n)?;
        Ok(())
    }

    /// Skip a `vec<T>` whose element size is fixed.
    fn skip_vec(&mut self, elem_size: usize) -> BotResult<u32> {
        let len = self.u32()?;
        let total = (len as usize)
            .checked_mul(elem_size)
            .ok_or_else(|| BotError::encoding("vec length overflow"))?;
        if total > self.remaining() {
            return Err(BotError::encoding(format!(
                "vec of {len}×{elem_size} exceeds the {} bytes left",
                self.remaining()
            )));
        }
        self.skip(total)?;
        Ok(len)
    }
}

fn decode_create(c: &mut Cursor) -> BotResult<PumpEvent> {
    let name = c.string()?;
    let symbol = c.string()?;
    let uri = c.string()?;
    let mint = c.pubkey()?;
    let bonding_curve = c.pubkey()?;
    let user = c.pubkey()?;
    let creator = c.pubkey()?;
    let timestamp = c.i64()?;
    let virtual_token_reserves = c.u64()?;
    let virtual_sol_reserves = c.u64()?;
    let real_token_reserves = c.u64()?;
    let token_total_supply = c.u64()?;
    let token_program = c.pubkey()?;
    let is_mayhem_mode = c.bool()?;
    let is_cashback_enabled = c.bool()?;

    // The multi-quote upgrade appended these; older events stop above.
    let quote_mint = if c.remaining() >= 32 {
        Some(c.pubkey()?)
    } else {
        None
    };
    let virtual_quote_reserves = if c.remaining() >= 8 {
        Some(c.u64()?)
    } else {
        None
    };
    let creator_fee_bps = if c.remaining() >= 8 {
        Some(c.u64()?)
    } else {
        None
    };
    let is_holder_reward = if c.remaining() >= 1 {
        Some(c.bool()?)
    } else {
        None
    };

    Ok(PumpEvent::Create {
        name,
        symbol,
        uri,
        mint,
        bonding_curve,
        user,
        creator,
        timestamp,
        virtual_token_reserves,
        virtual_sol_reserves,
        real_token_reserves,
        token_total_supply,
        token_program,
        is_mayhem_mode,
        is_cashback_enabled,
        quote_mint,
        virtual_quote_reserves,
        creator_fee_bps,
        is_holder_reward,
    })
}

fn decode_trade(c: &mut Cursor) -> BotResult<PumpEvent> {
    let mint = c.pubkey()?;
    let sol_amount = c.u64()?;
    let token_amount = c.u64()?;
    let is_buy = c.bool()?;
    let user = c.pubkey()?;
    let timestamp = c.i64()?;
    let virtual_sol_reserves = c.u64()?;
    let virtual_token_reserves = c.u64()?;
    let real_sol_reserves = c.u64()?;
    let real_token_reserves = c.u64()?;
    let fee_recipient = c.pubkey()?;
    let fee_basis_points = c.u64()?;
    let fee = c.u64()?;
    let creator = c.pubkey()?;
    let creator_fee_basis_points = c.u64()?;
    let creator_fee = c.u64()?;

    // Everything below is version-dependent; read what is there.
    let track_volume = if c.remaining() >= 1 { c.bool()? } else { false };
    let _ = track_volume;
    if c.remaining() >= 8 * 4 {
        c.skip(8)?; // total_unclaimed_tokens
        c.skip(8)?; // total_claimed_tokens
        c.skip(8)?; // current_sol_volume
        c.skip(8)?; // last_update_timestamp
    }
    let ix_name = if c.remaining() >= 4 {
        c.string().unwrap_or_default()
    } else {
        String::new()
    };

    Ok(PumpEvent::Trade {
        mint,
        sol_amount,
        token_amount,
        is_buy,
        user,
        timestamp,
        virtual_sol_reserves,
        virtual_token_reserves,
        real_sol_reserves,
        real_token_reserves,
        fee_recipient,
        fee_basis_points,
        fee,
        creator,
        creator_fee_basis_points,
        creator_fee,
        ix_name,
    })
}

fn decode_complete(c: &mut Cursor) -> BotResult<PumpEvent> {
    let user = c.pubkey()?;
    let mint = c.pubkey()?;
    let bonding_curve = c.pubkey()?;
    let timestamp = c.i64()?;
    // `quote_mint` was appended by the multi-quote upgrade.
    if c.remaining() >= 32 {
        c.skip(32)?;
    }
    Ok(PumpEvent::Complete {
        user,
        mint,
        bonding_curve,
        timestamp,
    })
}

fn decode_migration(c: &mut Cursor) -> BotResult<PumpEvent> {
    let user = c.pubkey()?;
    let mint = c.pubkey()?;
    let mint_amount = c.u64()?;
    let sol_amount = c.u64()?;
    let pool_migration_fee = c.u64()?;
    let bonding_curve = c.pubkey()?;
    let timestamp = c.i64()?;
    let pool = c.pubkey()?;
    if c.remaining() >= 32 {
        c.skip(32)?; // quote_mint
    }
    Ok(PumpEvent::Migration {
        user,
        mint,
        mint_amount,
        sol_amount,
        pool_migration_fee,
        bonding_curve,
        timestamp,
        pool,
    })
}

fn decode_create_pool(c: &mut Cursor) -> BotResult<PumpEvent> {
    let timestamp = c.i64()?;
    let index = c.u16()?;
    let creator = c.pubkey()?;
    let base_mint = c.pubkey()?;
    let quote_mint = c.pubkey()?;
    let base_mint_decimals = c.u8()?;
    let quote_mint_decimals = c.u8()?;
    let base_amount_in = c.u64()?;
    let quote_amount_in = c.u64()?;
    let pool_base_amount = c.u64()?;
    let pool_quote_amount = c.u64()?;
    let minimum_liquidity = c.u64()?;
    let initial_liquidity = c.u64()?;
    let lp_token_amount_out = c.u64()?;
    let pool_bump = c.u8()?;
    let pool = c.pubkey()?;
    let lp_mint = c.pubkey()?;
    // user_base_token_account, user_quote_token_account
    c.skip(64)?;
    let coin_creator = c.pubkey()?;

    let _ = (minimum_liquidity, initial_liquidity, lp_token_amount_out);
    Ok(PumpEvent::CreatePool {
        timestamp,
        index,
        creator,
        base_mint,
        quote_mint,
        base_mint_decimals,
        quote_mint_decimals,
        base_amount_in,
        quote_amount_in,
        pool_base_amount,
        pool_quote_amount,
        pool_bump,
        pool,
        lp_mint,
        coin_creator,
    })
}

fn decode_amm_buy(c: &mut Cursor) -> BotResult<PumpEvent> {
    let timestamp = c.i64()?;
    let base_amount_out = c.u64()?;
    let max_quote_amount_in = c.u64()?;
    // user_base_token_reserves, user_quote_token_reserves
    c.skip(16)?;
    let pool_base_token_reserves = c.u64()?;
    let pool_quote_token_reserves = c.u64()?;
    let quote_amount_in = c.u64()?;
    let lp_fee_basis_points = c.u64()?;
    let lp_fee = c.u64()?;
    let protocol_fee_basis_points = c.u64()?;
    let protocol_fee = c.u64()?;
    // quote_amount_in_with_lp_fee, user_quote_amount_in
    c.skip(16)?;
    let pool = c.pubkey()?;
    let user = c.pubkey()?;
    // user_base_token_account, user_quote_token_account, protocol_fee_recipient,
    // protocol_fee_recipient_token_account
    c.skip(128)?;
    let coin_creator = c.pubkey()?;
    let coin_creator_fee_basis_points = c.u64()?;
    let coin_creator_fee = c.u64()?;

    // Volume-tracking and later fields are version dependent.
    let mut ix_name = String::new();
    if c.remaining() > 8 * 4 {
        c.skip(1)?; // track_volume
        c.skip(32)?; // unclaimed / claimed / volume / timestamp
        if c.remaining() >= 4 {
            ix_name = c.string().unwrap_or_default();
        }
    }

    let _ = (
        max_quote_amount_in,
        lp_fee_basis_points,
        protocol_fee_basis_points,
        coin_creator,
        coin_creator_fee_basis_points,
    );
    Ok(PumpEvent::AmmBuy {
        timestamp,
        base_amount_out,
        quote_amount_in,
        pool,
        user,
        pool_base_token_reserves,
        pool_quote_token_reserves,
        lp_fee,
        protocol_fee,
        coin_creator_fee,
        ix_name,
    })
}

fn decode_amm_sell(c: &mut Cursor) -> BotResult<PumpEvent> {
    let timestamp = c.i64()?;
    let base_amount_in = c.u64()?;
    let min_quote_amount_out = c.u64()?;
    c.skip(16)?; // user reserves
    let pool_base_token_reserves = c.u64()?;
    let pool_quote_token_reserves = c.u64()?;
    let quote_amount_out = c.u64()?;
    let lp_fee_basis_points = c.u64()?;
    let lp_fee = c.u64()?;
    let protocol_fee_basis_points = c.u64()?;
    let protocol_fee = c.u64()?;
    c.skip(16)?; // quote_amount_out_without_lp_fee, user_quote_amount_out
    let pool = c.pubkey()?;
    let user = c.pubkey()?;
    c.skip(128)?;
    let coin_creator = c.pubkey()?;
    let coin_creator_fee_basis_points = c.u64()?;
    let coin_creator_fee = c.u64()?;

    let _ = (
        min_quote_amount_out,
        lp_fee_basis_points,
        protocol_fee_basis_points,
        coin_creator,
        coin_creator_fee_basis_points,
    );
    Ok(PumpEvent::AmmSell {
        timestamp,
        base_amount_in,
        quote_amount_out,
        pool,
        user,
        pool_base_token_reserves,
        pool_quote_token_reserves,
        lp_fee,
        protocol_fee,
        coin_creator_fee,
    })
}

/// Re-encode an event body for tests: borsh writer for the subset we decode.
#[cfg(test)]
mod writer {
    pub struct Buf(pub Vec<u8>);

    impl Buf {
        pub fn new(disc: [u8; 8]) -> Self {
            Buf(disc.to_vec())
        }
        pub fn u8(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        pub fn u16(&mut self, v: u16) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn u64(&mut self, v: u64) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn i64(&mut self, v: i64) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn bool(&mut self, v: bool) -> &mut Self {
            self.0.push(v as u8);
            self
        }
        pub fn pubkey(&mut self, v: solana_sdk::pubkey::Pubkey) -> &mut Self {
            self.0.extend_from_slice(&v.to_bytes());
            self
        }
        pub fn string(&mut self, v: &str) -> &mut Self {
            self.0.extend_from_slice(&(v.len() as u32).to_le_bytes());
            self.0.extend_from_slice(v.as_bytes());
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use writer::Buf;

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    #[test]
    fn decodes_a_create_event_with_every_field() {
        let mint = Pubkey::new_unique();
        let bc = Pubkey::new_unique();
        let user = Pubkey::new_unique();
        let creator = Pubkey::new_unique();
        let quote = *WSOL_MINT;

        let mut b = Buf::new(EV_PUMP_CREATE);
        b.string("Dog Wif Hat");
        b.string("WIF");
        b.string("https://ipfs.io/ipfs/QmTest");
        b.pubkey(mint);
        b.pubkey(bc);
        b.pubkey(user);
        b.pubkey(creator);
        b.i64(1_750_000_000);
        b.u64(1_073_000_000);
        b.u64(30_000_000_000);
        b.u64(793_100_000);
        b.u64(1_000_000_000);
        b.pubkey(*TOKEN_PROGRAM);
        b.bool(false);
        b.bool(true);
        b.pubkey(quote);
        b.u64(30_000_000_000);
        b.u64(100);
        b.bool(false);

        match decode_event(&b.0).expect("must decode") {
            PumpEvent::Create {
                name,
                symbol,
                uri,
                mint: m,
                bonding_curve,
                creator: cr,
                timestamp,
                virtual_token_reserves,
                virtual_sol_reserves,
                real_token_reserves,
                token_total_supply,
                token_program,
                is_cashback_enabled,
                quote_mint,
                creator_fee_bps,
                ..
            } => {
                assert_eq!(name, "Dog Wif Hat");
                assert_eq!(symbol, "WIF");
                assert_eq!(uri, "https://ipfs.io/ipfs/QmTest");
                assert_eq!(m, mint);
                assert_eq!(bonding_curve, bc);
                assert_eq!(cr, creator);
                assert_eq!(timestamp, 1_750_000_000);
                assert_eq!(virtual_token_reserves, 1_073_000_000);
                assert_eq!(virtual_sol_reserves, 30_000_000_000);
                assert_eq!(real_token_reserves, 793_100_000);
                assert_eq!(token_total_supply, 1_000_000_000);
                assert_eq!(token_program, *TOKEN_PROGRAM);
                assert!(is_cashback_enabled);
                assert_eq!(quote_mint, Some(quote));
                assert_eq!(creator_fee_bps, Some(100));
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn decodes_a_pre_multi_quote_create_event() {
        // Older events stop after `is_cashback_enabled`; the trailing Option
        // fields must come back as None instead of failing the decode.
        let mut b = Buf::new(EV_PUMP_CREATE);
        b.string("n");
        b.string("S");
        b.string("u");
        b.pubkey(Pubkey::new_unique());
        b.pubkey(Pubkey::new_unique());
        b.pubkey(Pubkey::new_unique());
        b.pubkey(Pubkey::new_unique());
        b.i64(1);
        b.u64(1);
        b.u64(1);
        b.u64(1);
        b.u64(1);
        b.pubkey(*TOKEN_PROGRAM);
        b.bool(false);
        b.bool(false);

        match decode_event(&b.0).expect("short create must still decode") {
            PumpEvent::Create {
                quote_mint,
                virtual_quote_reserves,
                creator_fee_bps,
                is_holder_reward,
                ..
            } => {
                assert!(quote_mint.is_none());
                assert!(virtual_quote_reserves.is_none());
                assert!(creator_fee_bps.is_none());
                assert!(is_holder_reward.is_none());
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn decodes_a_trade_event() {
        let mint = Pubkey::new_unique();
        let user = Pubkey::new_unique();
        let mut b = Buf::new(EV_PUMP_TRADE);
        b.pubkey(mint);
        b.u64(500_000_000); // 0.5 SOL
        b.u64(15_000_000); // tokens
        b.bool(true); // is_buy
        b.pubkey(user);
        b.i64(1_750_000_001);
        b.u64(30_500_000_000);
        b.u64(1_058_000_000);
        b.u64(793_100_000);
        b.u64(985_000_000);
        b.pubkey(Pubkey::new_unique()); // fee_recipient
        b.u64(100);
        b.u64(500_000);
        b.pubkey(Pubkey::new_unique()); // creator
        b.u64(50);
        b.u64(250_000);
        b.bool(true); // track_volume
        b.u64(0);
        b.u64(0);
        b.u64(0);
        b.i64(0);
        b.string("buy_exact_sol_in");

        match decode_event(&b.0).expect("must decode") {
            PumpEvent::Trade {
                mint: m,
                sol_amount,
                token_amount,
                is_buy,
                user: u,
                fee,
                creator_fee,
                ix_name,
                ..
            } => {
                assert_eq!(m, mint);
                assert_eq!(sol_amount, 500_000_000);
                assert_eq!(token_amount, 15_000_000);
                assert!(is_buy);
                assert_eq!(u, user);
                assert_eq!(fee, 500_000);
                assert_eq!(creator_fee, 250_000);
                assert_eq!(ix_name, "buy_exact_sol_in");
            }
            other => panic!("expected Trade, got {other:?}"),
        }
    }

    #[test]
    fn decodes_complete_and_migration() {
        let mint = Pubkey::new_unique();
        let pool = Pubkey::new_unique();

        let mut c = Buf::new(EV_PUMP_COMPLETE);
        c.pubkey(Pubkey::new_unique());
        c.pubkey(mint);
        c.pubkey(Pubkey::new_unique());
        c.i64(99);
        c.pubkey(*WSOL_MINT);
        match decode_event(&c.0).unwrap() {
            PumpEvent::Complete {
                mint: m, timestamp, ..
            } => {
                assert_eq!(m, mint);
                assert_eq!(timestamp, 99);
            }
            other => panic!("expected Complete, got {other:?}"),
        }

        let mut m = Buf::new(EV_PUMP_MIGRATION);
        m.pubkey(Pubkey::new_unique());
        m.pubkey(mint);
        m.u64(1_000);
        m.u64(2_000);
        m.u64(3_000);
        m.pubkey(Pubkey::new_unique());
        m.i64(100);
        m.pubkey(pool);
        m.pubkey(*WSOL_MINT);
        match decode_event(&m.0).unwrap() {
            PumpEvent::Migration {
                mint: mm,
                pool: pp,
                pool_migration_fee,
                ..
            } => {
                assert_eq!(mm, mint);
                assert_eq!(pp, pool);
                assert_eq!(pool_migration_fee, 3_000);
            }
            other => panic!("expected Migration, got {other:?}"),
        }
    }

    #[test]
    fn decodes_an_amm_create_pool_event() {
        let base = Pubkey::new_unique();
        let pool = Pubkey::new_unique();
        let mut b = Buf::new(EV_AMM_CREATE_POOL);
        b.i64(1_750_000_100);
        b.u16(0);
        b.pubkey(Pubkey::new_unique()); // creator
        b.pubkey(base);
        b.pubkey(*WSOL_MINT);
        b.u8(6);
        b.u8(9);
        b.u64(793_100_000);
        b.u64(85_000_000_000);
        b.u64(793_100_000);
        b.u64(85_000_000_000);
        b.u64(100); // minimum_liquidity
        b.u64(1_000); // initial_liquidity
        b.u64(900); // lp_token_amount_out
        b.u8(254); // pool_bump
        b.pubkey(pool);
        b.pubkey(Pubkey::new_unique()); // lp_mint
        b.pubkey(Pubkey::new_unique()); // user_base_token_account
        b.pubkey(Pubkey::new_unique()); // user_quote_token_account
        b.pubkey(Pubkey::new_unique()); // coin_creator

        match decode_event(&b.0).expect("must decode") {
            PumpEvent::CreatePool {
                index,
                base_mint,
                quote_mint,
                base_mint_decimals,
                quote_mint_decimals,
                pool_base_amount,
                pool_bump,
                pool: p,
                ..
            } => {
                assert_eq!(index, 0);
                assert_eq!(base_mint, base);
                assert_eq!(quote_mint, *WSOL_MINT);
                assert_eq!(base_mint_decimals, 6);
                assert_eq!(quote_mint_decimals, 9);
                assert_eq!(pool_base_amount, 793_100_000);
                assert_eq!(pool_bump, 254);
                assert_eq!(p, pool);
            }
            other => panic!("expected CreatePool, got {other:?}"),
        }
    }

    #[test]
    fn decodes_an_amm_buy_event() {
        let pool = Pubkey::new_unique();
        let user = Pubkey::new_unique();
        let mut b = Buf::new(EV_AMM_BUY);
        b.i64(1_750_000_200);
        b.u64(1_000_000); // base_amount_out
        b.u64(200_000_000); // max_quote_amount_in
        b.u64(0); // user_base_token_reserves
        b.u64(0); // user_quote_token_reserves
        b.u64(500_000_000); // pool_base
        b.u64(90_000_000_000); // pool_quote
        b.u64(150_000_000); // quote_amount_in
        b.u64(20); // lp_fee_bps
        b.u64(300_000); // lp_fee
        b.u64(5); // protocol_fee_bps
        b.u64(75_000); // protocol_fee
        b.u64(150_300_000); // quote_amount_in_with_lp_fee
        b.u64(150_375_000); // user_quote_amount_in
        b.pubkey(pool);
        b.pubkey(user);
        b.pubkey(Pubkey::new_unique()); // user_base_token_account
        b.pubkey(Pubkey::new_unique()); // user_quote_token_account
        b.pubkey(Pubkey::new_unique()); // protocol_fee_recipient
        b.pubkey(Pubkey::new_unique()); // protocol_fee_recipient_token_account
        b.pubkey(Pubkey::new_unique()); // coin_creator
        b.u64(0); // coin_creator_fee_bps
        b.u64(0); // coin_creator_fee
        b.bool(true); // track_volume
        b.u64(0);
        b.u64(0);
        b.u64(0);
        b.i64(0);
        b.string("buy_exact_quote_in");

        match decode_event(&b.0).expect("must decode") {
            PumpEvent::AmmBuy {
                base_amount_out,
                quote_amount_in,
                pool: p,
                user: u,
                lp_fee,
                protocol_fee,
                ix_name,
                ..
            } => {
                assert_eq!(base_amount_out, 1_000_000);
                assert_eq!(quote_amount_in, 150_000_000);
                assert_eq!(p, pool);
                assert_eq!(u, user);
                assert_eq!(lp_fee, 300_000);
                assert_eq!(protocol_fee, 75_000);
                assert_eq!(ix_name, "buy_exact_quote_in");
            }
            other => panic!("expected AmmBuy, got {other:?}"),
        }
    }

    #[test]
    fn unknown_discriminator_is_preserved_not_dropped() {
        let mut bytes = vec![9u8; 8];
        bytes.extend_from_slice(&[1, 2, 3]);
        match decode_event(&bytes).unwrap() {
            PumpEvent::Unknown {
                discriminator,
                payload_len,
                ..
            } => {
                assert_eq!(discriminator, [9u8; 8]);
                assert_eq!(payload_len, 3);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn instruction_name_logs_are_not_mistaken_for_events() {
        // Anchor instruction-name logs such as `pump:Buy` are not valid base64
        // (`:` is outside the alphabet), so they never decode as an event.
        assert!(
            decode_data_line("pump:Buy").is_none(),
            "an instruction-name line must not decode as an event"
        );
        // A short-but-valid base64 payload decodes to <8 bytes and cannot
        // carry a discriminator.
        assert!(
            decode_data_line(&b64(b"buy")).is_none(),
            "a payload shorter than 8 bytes must not decode"
        );
        // Anything under 8 bytes cannot carry a discriminator.
        assert!(decode_event(&[1, 2, 3]).is_none());
        assert!(decode_data_line("!!!not base64!!!").is_none());
    }

    #[test]
    fn parse_logs_finds_the_launch_among_noise() {
        let mint = Pubkey::new_unique();
        let mut b = Buf::new(EV_PUMP_CREATE);
        b.string("n");
        b.string("S");
        b.string("u");
        b.pubkey(mint);
        b.pubkey(Pubkey::new_unique());
        b.pubkey(Pubkey::new_unique());
        b.pubkey(Pubkey::new_unique());
        b.i64(1);
        b.u64(1);
        b.u64(1);
        b.u64(1);
        b.u64(1);
        b.pubkey(*TOKEN_PROGRAM);
        b.bool(false);
        b.bool(false);

        let logs = vec![
            format!("Program {} invoke [1]", *PUMP_PROGRAM_ID),
            "Program log: Instruction: Create".to_string(),
            "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [2]".to_string(),
            "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA success".to_string(),
            format!("Program data: {}", b64(&b.0)),
            // A real instruction-name log is the raw token, which is not valid
            // base64 (':' is outside the alphabet), so it must not decode.
            "Program data: pump:Create".to_string(),
            format!("Program {} success", *PUMP_PROGRAM_ID),
        ];

        let events = parse_logs(&logs);
        assert_eq!(events.len(), 1, "the instruction-name line must not decode");
        assert_eq!(events[0].mint(), Some(mint));
        assert!(find_launch(&logs).is_some());
        assert!(find_graduation(&logs).is_none());
        assert!(events[0].describe().contains("launch"));
    }

    #[test]
    fn parse_logs_attributes_unknown_events_to_the_emitting_program() {
        let mut bytes = vec![7u8; 8];
        bytes.extend_from_slice(&[0u8; 16]);
        let logs = vec![
            format!("Program {} invoke [1]", *PUMPSWAP_PROGRAM_ID),
            format!("Program data: {}", b64(&bytes)),
        ];
        let events = parse_logs(&logs);
        assert_eq!(events.len(), 1);
        match &events[0] {
            PumpEvent::Unknown { program, .. } => {
                assert_eq!(*program, Some(*PUMPSWAP_PROGRAM_ID));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn truncated_events_error_instead_of_panicking() {
        // A Create event cut off mid-pubkey.
        let mut b = Buf::new(EV_PUMP_CREATE);
        b.string("name");
        b.0.truncate(20);
        assert!(decode_event(&b.0).is_none());

        // A bogus string length must not cause a huge allocation.
        let mut bytes = EV_PUMP_CREATE.to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_event(&bytes).is_none());
    }

    #[test]
    fn cursor_skip_vec_bounds_are_checked() {
        let data = vec![1u8, 0, 0, 0, 5, 6];
        let mut c = Cursor::new(&data);
        assert_eq!(c.skip_vec(1).unwrap(), 1);
        // 4 length bytes + 1 element consumed from 6 leaves exactly 1.
        assert_eq!(c.remaining(), 1);

        let bad = vec![255u8, 255, 255, 255];
        let mut c2 = Cursor::new(&bad);
        assert!(
            c2.skip_vec(8).is_err(),
            "a huge vec length must be rejected"
        );
    }

    #[test]
    fn cursor_rejects_an_invalid_bool() {
        let data = vec![2u8];
        let mut c = Cursor::new(&data);
        assert!(c.bool().is_err());
    }
}
