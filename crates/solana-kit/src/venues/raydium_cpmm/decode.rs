//! Decoding Raydium CPMM (`cp-swap`) instructions and events into the
//! venue-neutral event types used elsewhere in `solana-kit`.
//!
//! # What is decoded
//!
//! * [`CpmmSwapEvent`] — the program's Anchor `SwapEvent`, decoded from a
//!   `Program data: <base64>` log line. It is the only CPMM signal that
//!   carries both mints and both amounts without an extra RPC round trip.
//! * [`CpmmPoolInitEvent`] — a pool creation, decoded from the `initialize`
//!   *instruction* (the program emits no pool-creation event, so the
//!   instruction is the only source).
//!
//! Both are converted into the crate's neutral types:
//! [`crate::decode::DecodedSwap`] for a trade and [`crate::events::PumpEvent`]
//! for a pool creation, so a consumer that already understands the pump
//! venues understands CPMM too without a second dispatch.
//!
//! # Fail-closed
//!
//! A payload that does not match the expected discriminator or length is
//! rejected with a typed error. Nothing is defaulted.

use base64::Engine;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;
use crate::decode::{DecodedSwap, Side, SwapVenue};
use crate::events::PumpEvent;

/// The decoded CPMM `SwapEvent`.
///
/// Field order and types follow the program's `SwapEvent` struct
/// (`programs/cp-swap/src/states/events.rs`) and the vendored IDL's `SwapEvent`
/// type, which agree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpmmSwapEvent {
    /// The pool the swap happened in.
    pub pool_id: Pubkey,
    /// The input vault balance before the swap (excluding trade fees).
    pub input_vault_before: u64,
    /// The output vault balance before the swap (excluding trade fees).
    pub output_vault_before: u64,
    /// The input amount, excluding any Token-2022 transfer fee.
    pub input_amount: u64,
    /// The output amount, excluding any Token-2022 transfer fee.
    pub output_amount: u64,
    /// The Token-2022 transfer fee withheld from the input.
    pub input_transfer_fee: u64,
    /// The Token-2022 transfer fee withheld from the output.
    pub output_transfer_fee: u64,
    /// `true` when the input was the "base" side of the pool.
    pub base_input: bool,
    /// The input mint.
    pub input_mint: Pubkey,
    /// The output mint.
    pub output_mint: Pubkey,
    /// The trading fee taken.
    pub trade_fee: u64,
    /// The creator fee taken.
    pub creator_fee: u64,
    /// Whether the creator fee was charged on the input token.
    pub creator_fee_on_input: bool,
}

impl CpmmSwapEvent {
    /// Decode the raw Anchor event bytes (discriminator + borsh body).
    pub fn decode(bytes: &[u8]) -> BotResult<Self> {
        if bytes.len() < 8 {
            return Err(BotError::encoding(format!(
                "CPMM SwapEvent payload is {} bytes, too short for a discriminator",
                bytes.len()
            )));
        }
        if bytes[..8] != RAYDIUM_CPMM_EV_SWAP {
            return Err(BotError::encoding(format!(
                "CPMM SwapEvent payload has discriminator {:02x?}, expected {:02x?}",
                &bytes[..8],
                RAYDIUM_CPMM_EV_SWAP
            )));
        }
        let body = &bytes[8..];
        // pool_id (32) + 4 x u64 (32) + 2 x u64 (16) + bool (1) + 2 x pubkey
        // (64) + 2 x u64 (16) + bool (1) = 162 bytes.
        const BODY_LEN: usize = 32 + 4 * 8 + 2 * 8 + 1 + 2 * 32 + 2 * 8 + 1;
        if body.len() < BODY_LEN {
            return Err(BotError::encoding(format!(
                "CPMM SwapEvent body is {} bytes, expected at least {BODY_LEN}",
                body.len()
            )));
        }
        let mut pos = 0usize;
        let mut take = |n: usize| -> BotResult<&[u8]> {
            if pos + n > body.len() {
                return Err(BotError::encoding("CPMM SwapEvent body is truncated"));
            }
            let s = &body[pos..pos + n];
            pos += n;
            Ok(s)
        };
        let pubkey = |bytes: &[u8]| -> Pubkey {
            let mut a = [0u8; 32];
            a.copy_from_slice(bytes);
            Pubkey::new_from_array(a)
        };
        let u64_at = |bytes: &[u8]| -> u64 {
            let mut a = [0u8; 8];
            a.copy_from_slice(bytes);
            u64::from_le_bytes(a)
        };

        let pool_id = pubkey(take(32)?);
        let input_vault_before = u64_at(take(8)?);
        let output_vault_before = u64_at(take(8)?);
        let input_amount = u64_at(take(8)?);
        let output_amount = u64_at(take(8)?);
        let input_transfer_fee = u64_at(take(8)?);
        let output_transfer_fee = u64_at(take(8)?);
        let base_input = match take(1)?[0] {
            0 => false,
            1 => true,
            other => {
                return Err(BotError::encoding(format!(
                    "CPMM SwapEvent has an invalid bool byte {other}"
                )))
            }
        };
        let input_mint = pubkey(take(32)?);
        let output_mint = pubkey(take(32)?);
        let trade_fee = u64_at(take(8)?);
        let creator_fee = u64_at(take(8)?);
        let creator_fee_on_input = match take(1)?[0] {
            0 => false,
            1 => true,
            other => {
                return Err(BotError::encoding(format!(
                    "CPMM SwapEvent has an invalid bool byte {other}"
                )))
            }
        };

        Ok(CpmmSwapEvent {
            pool_id,
            input_vault_before,
            output_vault_before,
            input_amount,
            output_amount,
            input_transfer_fee,
            output_transfer_fee,
            base_input,
            input_mint,
            output_mint,
            trade_fee,
            creator_fee,
            creator_fee_on_input,
        })
    }

    /// Decode a `Program data: <base64>` log payload.
    pub fn decode_base64(payload: &str) -> BotResult<Self> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| BotError::encoding(format!("CPMM SwapEvent base64 payload: {e}")))?;
        Self::decode(&bytes)
    }
}

/// A CPMM pool creation, decoded from the `initialize` or
/// `initialize_with_permission` instruction.
///
/// The CPMM program emits no event on pool creation, so the instruction is the
/// only source. Account order follows the IDL's `initialize.accounts` (20
/// accounts):
///
/// ```text
///  0 creator            7 creator_token_0   14 token_program
///  1 amm_config         8 creator_token_1   15 token_0_program
///  2 authority          9 creator_lp_token  16 token_1_program
///  3 pool_state        10 token_0_vault     17 associated_token_program
///  4 token_0_mint      11 token_1_vault     18 system_program
///  5 token_1_mint      12 create_pool_fee   19 rent
///  6 lp_mint           13 observation_state
/// ```
///
/// (`initialize_with_permission` prepends `0 payer` and shifts `creator..observation_state`
/// to `1..=14`.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpmmPoolInitEvent {
    /// The pool account.
    pub pool_state: Pubkey,
    /// The `AmmConfig` the pool belongs to.
    pub amm_config: Pubkey,
    /// The creator of the pool.
    pub creator: Pubkey,
    /// The base mint (token_0).
    pub token_0_mint: Pubkey,
    /// The quote mint (token_1).
    pub token_1_mint: Pubkey,
    /// The pool's LP mint.
    pub lp_mint: Pubkey,
    /// The pool's token_0 vault.
    pub token_0_vault: Pubkey,
    /// The pool's token_1 vault.
    pub token_1_vault: Pubkey,
    /// The oracle observation account.
    pub observation_state: Pubkey,
    /// The initial token_0 deposit.
    pub init_amount_0: u64,
    /// The initial token_1 deposit.
    pub init_amount_1: u64,
    /// The timestamp from which swaps are accepted.
    pub open_time: u64,
}

impl CpmmPoolInitEvent {
    /// Decode a CPMM `initialize` or `initialize_with_permission` instruction.
    ///
    /// Returns `None` when the instruction is not a CPMM pool-init instruction
    /// (a different program, or a different instruction on the same
    /// program); a malformed CPMM `initialize` is an **error**, not `None`.
    pub fn from_instruction(
        program_id: &Pubkey,
        accounts: &[Pubkey],
        data: &[u8],
    ) -> BotResult<Option<Self>> {
        if *program_id != *RAYDIUM_CPMM {
            return Ok(None);
        }
        if data.len() < 8 {
            return Ok(None);
        }
        let disc = &data[..8];
        let shift = if disc == RAYDIUM_CPMM_DISC_INITIALIZE {
            0usize
        } else if disc == RAYDIUM_CPMM_DISC_INITIALIZE_WITH_PERMISSION {
            1usize
        } else {
            return Ok(None);
        };
        // 20 accounts per IDL `initialize` (21 for `initialize_with_permission`).
        let acc_creator = shift;
        let acc_amm_config = 1 + shift;
        let acc_pool_state = 3 + shift;
        let acc_token_0_mint = 4 + shift;
        let acc_token_1_mint = 5 + shift;
        let acc_lp_mint = 6 + shift;
        let acc_token_0_vault = 10 + shift;
        let acc_token_1_vault = 11 + shift;
        let acc_observation = 13 + shift;
        let min_accounts = acc_observation + 1;
        if accounts.len() < min_accounts {
            return Err(BotError::encoding(format!(
                "CPMM initialize instruction has {} accounts, expected at least {min_accounts}",
                accounts.len()
            )));
        }
        // discriminator + 3 x u64 (+ 1 byte CreatorFeeOn for initialize_with_permission)
        let min_data = 8 + 24 + shift;
        if data.len() < min_data {
            return Err(BotError::encoding(format!(
                "CPMM initialize instruction data is {} bytes, expected at least {min_data}",
                data.len()
            )));
        }
        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(b)
        };
        Ok(Some(CpmmPoolInitEvent {
            pool_state: accounts[acc_pool_state],
            amm_config: accounts[acc_amm_config],
            creator: accounts[acc_creator],
            token_0_mint: accounts[acc_token_0_mint],
            token_1_mint: accounts[acc_token_1_mint],
            lp_mint: accounts[acc_lp_mint],
            token_0_vault: accounts[acc_token_0_vault],
            token_1_vault: accounts[acc_token_1_vault],
            observation_state: accounts[acc_observation],
            init_amount_0: u64_at(8),
            init_amount_1: u64_at(16),
            open_time: u64_at(24),
        }))
    }
}

/// A decoded CPMM `swap_base_input` or `swap_base_output` instruction.
///
/// Account order follows the IDL's `swap_base_input` / `swap_base_output` (13 accounts):
/// `0 payer, 1 authority, 2 amm_config, 3 pool_state, 4 input_token_account,
/// 5 output_token_account, 6 input_vault, 7 output_vault, 8 input_token_program,
/// 9 output_token_program, 10 input_token_mint, 11 output_token_mint,
/// 12 observation_state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpmmSwapInstruction {
    /// `true` for `swap_base_input`, `false` for `swap_base_output`.
    pub is_base_input: bool,
    /// Signing trader (`payer`).
    pub payer: Pubkey,
    /// `AmmConfig` account.
    pub amm_config: Pubkey,
    /// Pool state account.
    pub pool_state: Pubkey,
    /// Input vault account.
    pub input_vault: Pubkey,
    /// Output vault account.
    pub output_vault: Pubkey,
    /// Input token mint.
    pub input_mint: Pubkey,
    /// Output token mint.
    pub output_mint: Pubkey,
    /// Exact input (`swap_base_input`) or maximum input (`swap_base_output`).
    pub input_amount: u64,
    /// Minimum output (`swap_base_input`) or exact output (`swap_base_output`).
    pub output_amount: u64,
}

impl CpmmSwapInstruction {
    /// Decode a CPMM `swap_base_input` or `swap_base_output` instruction.
    ///
    /// Returns `Ok(None)` for unrelated programs/discriminators and fails closed
    /// with a typed error when a CPMM swap instruction has truncated accounts or args.
    pub fn from_instruction(
        program_id: &Pubkey,
        accounts: &[Pubkey],
        data: &[u8],
    ) -> BotResult<Option<Self>> {
        if *program_id != *RAYDIUM_CPMM {
            return Ok(None);
        }
        if data.len() < 8 {
            return Ok(None);
        }
        let is_base_input = if data[..8] == RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT {
            true
        } else if data[..8] == RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT {
            false
        } else {
            return Ok(None);
        };
        const MIN_ACCOUNTS: usize = 13;
        if accounts.len() < MIN_ACCOUNTS {
            return Err(BotError::encoding(format!(
                "CPMM swap instruction has {} accounts, expected at least {MIN_ACCOUNTS}",
                accounts.len()
            )));
        }
        if data.len() < 24 {
            return Err(BotError::encoding(format!(
                "CPMM swap instruction data is {} bytes, expected at least 24",
                data.len()
            )));
        }
        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(b)
        };
        Ok(Some(CpmmSwapInstruction {
            is_base_input,
            payer: accounts[0],
            amm_config: accounts[2],
            pool_state: accounts[3],
            input_vault: accounts[6],
            output_vault: accounts[7],
            input_mint: accounts[10],
            output_mint: accounts[11],
            input_amount: u64_at(8),
            output_amount: u64_at(16),
        }))
    }
}

/// The trade direction of a CPMM swap event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CpmmTradeSide {
    /// The user bought the output mint with the input mint.
    Buy,
    /// The user sold the input mint for the output mint.
    Sell,
}

impl CpmmTradeSide {
    /// Decide the side from the event.
    ///
    /// The event does not carry a "buy" flag: it carries the input and output
    /// mints. The caller supplies which mint it considers the *base* (the
    /// token being traded); the side follows from whether the base was the
    /// input or the output.
    pub fn from_event(event: &CpmmSwapEvent, base_mint: &Pubkey) -> Option<CpmmTradeSide> {
        if event.input_mint == *base_mint {
            Some(CpmmTradeSide::Sell)
        } else if event.output_mint == *base_mint {
            Some(CpmmTradeSide::Buy)
        } else {
            None
        }
    }
}

/// Convert a CPMM swap event into the crate's neutral [`DecodedSwap`].
///
/// `base_mint` is the token being traded; the other mint is the quote.
/// `decimals` are the base and quote decimals.
#[allow(clippy::too_many_arguments)]
pub fn to_decoded_swap(
    event: &CpmmSwapEvent,
    signature: &str,
    slot: u64,
    block_time: Option<i64>,
    wallet: &Pubkey,
    base_mint: &Pubkey,
    base_decimals: u8,
    quote_decimals: u8,
) -> BotResult<DecodedSwap> {
    let side = CpmmTradeSide::from_event(event, base_mint).ok_or_else(|| {
        BotError::invalid(format!(
            "base mint {base_mint} is neither the input ({}) nor the output ({}) of the CPMM swap",
            event.input_mint, event.output_mint
        ))
    })?;
    let (base_amount, quote_amount) = match side {
        CpmmTradeSide::Buy => (event.output_amount, event.input_amount),
        CpmmTradeSide::Sell => (event.input_amount, event.output_amount),
    };
    let quote_mint = match side {
        CpmmTradeSide::Buy => event.input_mint,
        CpmmTradeSide::Sell => event.output_mint,
    };
    Ok(DecodedSwap {
        signature: signature.to_string(),
        slot,
        block_time,
        wallet: wallet.to_string(),
        side: match side {
            CpmmTradeSide::Buy => Side::Buy,
            CpmmTradeSide::Sell => Side::Sell,
        },
        venue: SwapVenue::RaydiumCpmm,
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
        programs: vec![RAYDIUM_CPMM.to_string()],
        events: Vec::new(),
        logs: Vec::new(),
        succeeded: true,
        error: None,
    })
}

/// Convert a decoded CPMM swap instruction into the crate's neutral [`DecodedSwap`]
/// when the CPI `SwapEvent` log is unavailable.
#[allow(clippy::too_many_arguments)]
pub fn to_decoded_swap_from_instruction(
    ix: &CpmmSwapInstruction,
    signature: &str,
    slot: u64,
    block_time: Option<i64>,
    base_mint: &Pubkey,
    base_decimals: u8,
    quote_decimals: u8,
) -> BotResult<DecodedSwap> {
    let synthetic_event = CpmmSwapEvent {
        pool_id: ix.pool_state,
        input_vault_before: 0,
        output_vault_before: 0,
        input_amount: ix.input_amount,
        output_amount: ix.output_amount,
        input_transfer_fee: 0,
        output_transfer_fee: 0,
        base_input: ix.is_base_input,
        input_mint: ix.input_mint,
        output_mint: ix.output_mint,
        trade_fee: 0,
        creator_fee: 0,
        creator_fee_on_input: false,
    };
    to_decoded_swap(
        &synthetic_event,
        signature,
        slot,
        block_time,
        &ix.payer,
        base_mint,
        base_decimals,
        quote_decimals,
    )
}

/// Convert a CPMM pool creation into the crate's neutral pool-creation event.
///
/// The neutral event type used by `crate::events` is pump-specific, so the
/// CPMM creation is expressed as a `CreatePool` event with the CPMM pool in
/// the `pool` field. That keeps a single event stream for the sniper.
pub fn to_pool_creation_event(init: &CpmmPoolInitEvent) -> PumpEvent {
    PumpEvent::CreatePool {
        timestamp: 0,
        index: 0,
        creator: init.creator,
        base_mint: init.token_0_mint,
        quote_mint: init.token_1_mint,
        base_mint_decimals: 0,
        quote_mint_decimals: 0,
        base_amount_in: init.init_amount_0,
        quote_amount_in: init.init_amount_1,
        pool_base_amount: init.init_amount_0,
        pool_quote_amount: init.init_amount_1,
        pool_bump: 0,
        pool: init.pool_state,
        lp_mint: init.lp_mint,
        coin_creator: init.creator,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_swap_event() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&RAYDIUM_CPMM_EV_SWAP);
        b.extend_from_slice(&[7u8; 32]); // pool_id
        b.extend_from_slice(&1_000u64.to_le_bytes()); // input_vault_before
        b.extend_from_slice(&2_000u64.to_le_bytes()); // output_vault_before
        b.extend_from_slice(&100u64.to_le_bytes()); // input_amount
        b.extend_from_slice(&200u64.to_le_bytes()); // output_amount
        b.extend_from_slice(&3u64.to_le_bytes()); // input_transfer_fee
        b.extend_from_slice(&4u64.to_le_bytes()); // output_transfer_fee
        b.push(1); // base_input
        b.extend_from_slice(&[8u8; 32]); // input_mint
        b.extend_from_slice(&[9u8; 32]); // output_mint
        b.extend_from_slice(&5u64.to_le_bytes()); // trade_fee
        b.extend_from_slice(&6u64.to_le_bytes()); // creator_fee
        b.push(0); // creator_fee_on_input
        b
    }

    #[test]
    fn swap_event_decodes_every_field() {
        let ev = CpmmSwapEvent::decode(&encode_swap_event()).unwrap();
        assert_eq!(ev.pool_id, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(ev.input_vault_before, 1_000);
        assert_eq!(ev.output_vault_before, 2_000);
        assert_eq!(ev.input_amount, 100);
        assert_eq!(ev.output_amount, 200);
        assert_eq!(ev.input_transfer_fee, 3);
        assert_eq!(ev.output_transfer_fee, 4);
        assert!(ev.base_input);
        assert_eq!(ev.input_mint, Pubkey::new_from_array([8u8; 32]));
        assert_eq!(ev.output_mint, Pubkey::new_from_array([9u8; 32]));
        assert_eq!(ev.trade_fee, 5);
        assert_eq!(ev.creator_fee, 6);
        assert!(!ev.creator_fee_on_input);
    }

    #[test]
    fn swap_event_rejects_a_wrong_discriminator() {
        let mut bytes = encode_swap_event();
        bytes[0] ^= 0xff;
        let err = CpmmSwapEvent::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("discriminator"), "{err}");
    }

    #[test]
    fn swap_event_rejects_a_truncated_body() {
        let bytes = encode_swap_event();
        let err = CpmmSwapEvent::decode(&bytes[..40]).unwrap_err();
        assert!(err.to_string().contains("expected at least"), "{err}");
        let err = CpmmSwapEvent::decode(&bytes[..4]).unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");
    }

    #[test]
    fn swap_event_rejects_an_invalid_bool() {
        let mut bytes = encode_swap_event();
        // The `base_input` bool sits right after 4 + 6 u64 fields.
        let bool_off = 8 + 32 + 6 * 8;
        bytes[bool_off] = 9;
        let err = CpmmSwapEvent::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("invalid bool"), "{err}");
    }

    #[test]
    fn swap_event_decodes_from_base64() {
        use base64::Engine;
        let bytes = encode_swap_event();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let ev = CpmmSwapEvent::decode_base64(&b64).unwrap();
        assert_eq!(ev.input_amount, 100);
        assert!(CpmmSwapEvent::decode_base64("!!!").is_err());
    }

    fn init_accounts() -> Vec<Pubkey> {
        (0..20).map(|_| Pubkey::new_unique()).collect()
    }

    fn init_data() -> Vec<u8> {
        let mut d = RAYDIUM_CPMM_DISC_INITIALIZE.to_vec();
        d.extend_from_slice(&111u64.to_le_bytes());
        d.extend_from_slice(&222u64.to_le_bytes());
        d.extend_from_slice(&333u64.to_le_bytes());
        d
    }

    #[test]
    fn pool_init_event_decodes_the_idl_account_order() {
        let accounts = init_accounts();
        let ev = CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM, &accounts, &init_data())
            .unwrap()
            .expect("a CPMM initialize decodes");
        assert_eq!(ev.pool_state, accounts[3]);
        assert_eq!(ev.amm_config, accounts[1]);
        assert_eq!(ev.creator, accounts[0]);
        assert_eq!(ev.token_0_mint, accounts[4]);
        assert_eq!(ev.token_1_mint, accounts[5]);
        assert_eq!(ev.lp_mint, accounts[6]);
        assert_eq!(ev.token_0_vault, accounts[10]);
        assert_eq!(ev.token_1_vault, accounts[11]);
        assert_eq!(ev.observation_state, accounts[13]);
        assert_eq!(ev.init_amount_0, 111);
        assert_eq!(ev.init_amount_1, 222);
        assert_eq!(ev.open_time, 333);
    }

    #[test]
    fn pool_init_with_permission_decodes_shifted_idl_order() {
        let accounts: Vec<Pubkey> = (0..21).map(|_| Pubkey::new_unique()).collect();
        let mut d = RAYDIUM_CPMM_DISC_INITIALIZE_WITH_PERMISSION.to_vec();
        d.extend_from_slice(&444u64.to_le_bytes());
        d.extend_from_slice(&555u64.to_le_bytes());
        d.extend_from_slice(&666u64.to_le_bytes());
        d.push(0); // CreatorFeeOn::BothToken
        let ev = CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM, &accounts, &d)
            .unwrap()
            .expect("initialize_with_permission decodes");
        assert_eq!(ev.creator, accounts[1]);
        assert_eq!(ev.amm_config, accounts[2]);
        assert_eq!(ev.pool_state, accounts[4]);
        assert_eq!(ev.token_0_mint, accounts[5]);
        assert_eq!(ev.token_1_mint, accounts[6]);
        assert_eq!(ev.lp_mint, accounts[7]);
        assert_eq!(ev.token_0_vault, accounts[11]);
        assert_eq!(ev.token_1_vault, accounts[12]);
        assert_eq!(ev.observation_state, accounts[14]);
        assert_eq!(ev.init_amount_0, 444);
        assert_eq!(ev.init_amount_1, 555);
        assert_eq!(ev.open_time, 666);
    }

    #[test]
    fn swap_instruction_decodes_base_input_and_base_output() {
        let accounts: Vec<Pubkey> = (0..13).map(|_| Pubkey::new_unique()).collect();
        let mut d_in = RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT.to_vec();
        d_in.extend_from_slice(&1_000u64.to_le_bytes());
        d_in.extend_from_slice(&950u64.to_le_bytes());
        let ix_in = CpmmSwapInstruction::from_instruction(&RAYDIUM_CPMM, &accounts, &d_in)
            .unwrap()
            .expect("swap_base_input decodes");
        assert!(ix_in.is_base_input);
        assert_eq!(ix_in.payer, accounts[0]);
        assert_eq!(ix_in.pool_state, accounts[3]);
        assert_eq!(ix_in.input_mint, accounts[10]);
        assert_eq!(ix_in.output_mint, accounts[11]);
        assert_eq!(ix_in.input_amount, 1_000);
        assert_eq!(ix_in.output_amount, 950);

        let swap =
            to_decoded_swap_from_instruction(&ix_in, "sig", 7, None, &accounts[11], 6, 9).unwrap();
        assert_eq!(swap.venue, SwapVenue::RaydiumCpmm);
        assert_eq!(swap.side, Side::Buy);
        assert_eq!(swap.base_amount, 950);
        assert_eq!(swap.quote_amount, 1_000);

        let mut d_out = RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT.to_vec();
        d_out.extend_from_slice(&1_200u64.to_le_bytes());
        d_out.extend_from_slice(&900u64.to_le_bytes());
        let ix_out = CpmmSwapInstruction::from_instruction(&RAYDIUM_CPMM, &accounts, &d_out)
            .unwrap()
            .expect("swap_base_output decodes");
        assert!(!ix_out.is_base_input);
        assert_eq!(ix_out.input_amount, 1_200);
        assert_eq!(ix_out.output_amount, 900);
    }

    #[test]
    fn pool_init_event_ignores_other_programs_and_instructions() {
        let accounts = init_accounts();
        // Wrong program.
        assert!(CpmmPoolInitEvent::from_instruction(
            &Pubkey::new_unique(),
            &accounts,
            &init_data()
        )
        .unwrap()
        .is_none());
        // Wrong instruction on the right program.
        let mut data = init_data();
        data[0] ^= 0xff;
        assert!(
            CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM, &accounts, &data)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pool_init_event_errors_on_a_malformed_instruction() {
        let accounts = init_accounts();
        // Too few accounts.
        assert!(
            CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM, &accounts[..5], &init_data())
                .is_err()
        );
        // Truncated data.
        assert!(
            CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM, &accounts, &init_data()[..16])
                .is_err()
        );
    }

    #[test]
    fn trade_side_follows_the_base_mint() {
        let ev = CpmmSwapEvent::decode(&encode_swap_event()).unwrap();
        let input = ev.input_mint;
        let output = ev.output_mint;
        assert_eq!(
            CpmmTradeSide::from_event(&ev, &input),
            Some(CpmmTradeSide::Sell)
        );
        assert_eq!(
            CpmmTradeSide::from_event(&ev, &output),
            Some(CpmmTradeSide::Buy)
        );
        assert_eq!(CpmmTradeSide::from_event(&ev, &Pubkey::new_unique()), None);
    }

    #[test]
    fn to_decoded_swap_maps_the_sides() {
        let ev = CpmmSwapEvent::decode(&encode_swap_event()).unwrap();
        let wallet = Pubkey::new_unique();
        let buy =
            to_decoded_swap(&ev, "sig", 10, Some(1234), &wallet, &ev.output_mint, 6, 9).unwrap();
        assert_eq!(buy.venue, SwapVenue::RaydiumCpmm);
        assert_eq!(buy.side, Side::Buy);
        assert_eq!(buy.base_amount, 200);
        assert_eq!(buy.quote_amount, 100);
        assert_eq!(buy.base_mint, ev.output_mint.to_string());
        assert_eq!(buy.quote_mint, ev.input_mint.to_string());

        let sell =
            to_decoded_swap(&ev, "sig", 10, Some(1234), &wallet, &ev.input_mint, 6, 9).unwrap();
        assert_eq!(sell.side, Side::Sell);
        assert_eq!(sell.base_amount, 100);
        assert_eq!(sell.quote_amount, 200);
    }

    #[test]
    fn to_decoded_swap_refuses_an_unrelated_base_mint() {
        let ev = CpmmSwapEvent::decode(&encode_swap_event()).unwrap();
        let err = to_decoded_swap(
            &ev,
            "sig",
            1,
            None,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            6,
            9,
        )
        .unwrap_err();
        assert!(err.to_string().contains("neither the input"), "{err}");
    }

    #[test]
    fn pool_creation_event_carries_the_pool() {
        let accounts = init_accounts();
        let init = CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM, &accounts, &init_data())
            .unwrap()
            .unwrap();
        let ev = to_pool_creation_event(&init);
        assert_eq!(ev.pool(), Some(init.pool_state));
        assert_eq!(ev.user(), Some(init.creator));
    }
}
