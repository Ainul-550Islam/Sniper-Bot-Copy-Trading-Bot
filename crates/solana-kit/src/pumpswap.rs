//! PumpSwap — the AMM a pump.fun token migrates to once its bonding curve
//! completes.
//!
//! Layouts and account lists come from `pump-fun/pump-public-docs` →
//! `idl/pump_amm.json` (fetched and cross-checked against the discriminators,
//! which are all `sha256("global:<name>")[..8]`).
//!
//! ## The trailing-account problem
//!
//! The published IDL lists **23** accounts for `buy` and **21** for `sell`.
//! The 2026-04-28 upgrade note says every AMM buy/sell must additionally carry
//! `pool-v2`, a fee recipient and that recipient's quote ATA, giving **26/27**
//! for buy and **24/26** for sell. Which one the deployed program wants today
//! depends on its version, and getting it wrong fails with an opaque
//! `AnchorError` rather than something actionable.
//!
//! [`buy_layout_variants`] therefore enumerates every candidate shape and
//! [`PumpSwapLayoutDoctor`] simulates each one, persisting the winner into the
//! [`LayoutStore`]. That is deliberate: a hardcoded guess here has a short
//! shelf life, and the doctor keeps working across upgrades.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::maths;

use crate::consts::*;
use crate::layout::{AccountLayout, LayoutStore};
use crate::rpc::Rpc;

// --------------------------------------------------------------------------
// PDA derivation
// --------------------------------------------------------------------------

/// `pool` PDA. Canonical pump pools use `index = 0`; `creator` is the
/// `pool-authority` PDA for migrated tokens, or the real creator otherwise.
pub fn pool_pda(index: u16, creator: &Pubkey, base_mint: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            PUMPSWAP_SEED_POOL,
            &index.to_le_bytes(),
            creator.as_ref(),
            base_mint.as_ref(),
            quote_mint.as_ref(),
        ],
        &PUMPSWAP_PROGRAM_ID,
    )
    .0
}

pub fn pool_v2_pda(base_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMPSWAP_SEED_POOL_V2, base_mint.as_ref()],
        &PUMPSWAP_PROGRAM_ID,
    )
    .0
}

pub fn lp_mint_pda(pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMPSWAP_SEED_POOL_LP_MINT, pool.as_ref()],
        &PUMPSWAP_PROGRAM_ID,
    )
    .0
}

/// `pool-authority` PDA on the **pump** program — the creator of a migrated
/// pool is this, not the token's original creator.
pub fn pool_authority_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMPSWAP_SEED_POOL_AUTHORITY, mint.as_ref()],
        &PUMP_PROGRAM_ID,
    )
    .0
}

pub fn coin_creator_vault_authority_pda(coin_creator: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMPSWAP_SEED_CREATOR_VAULT, coin_creator.as_ref()],
        &PUMPSWAP_PROGRAM_ID,
    )
    .0
}

pub fn amm_event_authority_pda() -> Pubkey {
    Pubkey::find_program_address(&[PUMPSWAP_SEED_EVENT_AUTHORITY], &PUMPSWAP_PROGRAM_ID).0
}

pub fn amm_global_volume_accumulator_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[PUMPSWAP_SEED_GLOBAL_VOLUME_ACCUMULATOR],
        &PUMPSWAP_PROGRAM_ID,
    )
    .0
}

pub fn amm_user_volume_accumulator_pda(user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMPSWAP_SEED_USER_VOLUME_ACCUMULATOR, user.as_ref()],
        &PUMPSWAP_PROGRAM_ID,
    )
    .0
}

/// The pool's own base/quote token accounts are ATAs **of the pool PDA**.
pub fn pool_base_token_account(
    pool: &Pubkey,
    base_mint: &Pubkey,
    token_program: &Pubkey,
) -> Pubkey {
    spl_associated_token_account::get_associated_token_address_with_program_id(
        pool,
        base_mint,
        token_program,
    )
}

pub fn pool_quote_token_account(
    pool: &Pubkey,
    quote_mint: &Pubkey,
    token_program: &Pubkey,
) -> Pubkey {
    spl_associated_token_account::get_associated_token_address_with_program_id(
        pool,
        quote_mint,
        token_program,
    )
}

// --------------------------------------------------------------------------
// Account state
// --------------------------------------------------------------------------

/// The `Pool` account (271 bytes once fully extended).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolState {
    pub pool_bump: u8,
    pub index: u16,
    pub creator: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub pool_base_token_account: Pubkey,
    pub pool_quote_token_account: Pubkey,
    pub lp_supply: u64,
    pub coin_creator: Pubkey,
    pub is_mayhem_mode: Option<bool>,
    pub is_cashback_coin: Option<bool>,
    pub virtual_quote_reserves: Option<i128>,
    pub creator_fee_bps: Option<u64>,
    pub can_edit_creator_fee: Option<bool>,
    pub is_holder_reward: Option<bool>,
    pub raw_len: usize,
}

impl PoolState {
    pub fn parse(data: &[u8]) -> BotResult<Self> {
        if data.len() < POOL_OFF_COIN_CREATOR + 32 {
            return Err(BotError::encoding(format!(
                "pool account is {} bytes; need at least {}",
                data.len(),
                POOL_OFF_COIN_CREATOR + 32
            )));
        }
        if data[..8] != PUMPSWAP_ACC_DISC_POOL {
            return Err(BotError::solana(format!(
                "pool discriminator mismatch: got {:02x?} want {:02x?}",
                &data[..8],
                PUMPSWAP_ACC_DISC_POOL
            )));
        }
        let u64_at = |off: usize| -> Option<u64> {
            data.get(off..off + 8).map(|b| {
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                u64::from_le_bytes(a)
            })
        };
        let i128_at = |off: usize| -> Option<i128> {
            data.get(off..off + 16).map(|b| {
                let mut a = [0u8; 16];
                a.copy_from_slice(b);
                i128::from_le_bytes(a)
            })
        };
        let pk_at = |off: usize| -> Pubkey {
            let mut a = [0u8; 32];
            a.copy_from_slice(&data[off..off + 32]);
            Pubkey::new_from_array(a)
        };
        let mut index_bytes = [0u8; 2];
        index_bytes.copy_from_slice(&data[POOL_OFF_INDEX..POOL_OFF_INDEX + 2]);

        Ok(PoolState {
            pool_bump: data[POOL_OFF_POOL_BUMP],
            index: u16::from_le_bytes(index_bytes),
            creator: pk_at(POOL_OFF_CREATOR),
            base_mint: pk_at(POOL_OFF_BASE_MINT),
            quote_mint: pk_at(POOL_OFF_QUOTE_MINT),
            lp_mint: pk_at(POOL_OFF_LP_MINT),
            pool_base_token_account: pk_at(POOL_OFF_POOL_BASE_TOKEN_ACCOUNT),
            pool_quote_token_account: pk_at(POOL_OFF_POOL_QUOTE_TOKEN_ACCOUNT),
            lp_supply: u64_at(POOL_OFF_LP_SUPPLY).unwrap_or(0),
            coin_creator: pk_at(POOL_OFF_COIN_CREATOR),
            is_mayhem_mode: data.get(POOL_OFF_IS_MAYHEM_MODE).map(|b| *b != 0),
            is_cashback_coin: data.get(POOL_OFF_IS_CASHBACK_COIN).map(|b| *b != 0),
            virtual_quote_reserves: i128_at(POOL_OFF_VIRTUAL_QUOTE_RESERVES),
            creator_fee_bps: u64_at(POOL_OFF_CREATOR_FEE_BPS),
            can_edit_creator_fee: data.get(POOL_OFF_CAN_EDIT_CREATOR_FEE).map(|b| *b != 0),
            is_holder_reward: data.get(POOL_OFF_IS_HOLDER_REWARD).map(|b| *b != 0),
            raw_len: data.len(),
        })
    }

    /// The LP mint is Token-2022, which matters for anyone holding LP.
    pub fn lp_token_program(&self) -> Pubkey {
        *TOKEN_2022_PROGRAM
    }
}

/// The `GlobalConfig` account: fee parameters and the protocol fee recipients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmmGlobalConfig {
    pub admin: Pubkey,
    pub lp_fee_basis_points: u64,
    pub protocol_fee_basis_points: u64,
    pub disable_flags: u8,
    pub protocol_fee_recipients: Vec<Pubkey>,
    pub coin_creator_fee_basis_points: u64,
    pub mayhem_mode_enabled: Option<bool>,
    pub is_cashback_enabled: Option<bool>,
    pub buyback_basis_points: Option<u64>,
    pub raw_len: usize,
}

impl AmmGlobalConfig {
    pub fn parse(data: &[u8]) -> BotResult<Self> {
        if data.len() < AGC_OFF_PROTOCOL_FEE_RECIPIENTS + 32 * AGC_PROTOCOL_FEE_RECIPIENTS_LEN {
            return Err(BotError::encoding(format!(
                "global_config is {} bytes; need at least {}",
                data.len(),
                AGC_OFF_PROTOCOL_FEE_RECIPIENTS + 32 * AGC_PROTOCOL_FEE_RECIPIENTS_LEN
            )));
        }
        if data[..8] != PUMPSWAP_ACC_DISC_GLOBAL_CONFIG {
            return Err(BotError::solana(format!(
                "global_config discriminator mismatch: got {:02x?} want {:02x?}",
                &data[..8],
                PUMPSWAP_ACC_DISC_GLOBAL_CONFIG
            )));
        }
        let u64_at = |off: usize| -> u64 {
            let mut a = [0u8; 8];
            a.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(a)
        };
        let pk_at = |off: usize| -> Pubkey {
            let mut a = [0u8; 32];
            a.copy_from_slice(&data[off..off + 32]);
            Pubkey::new_from_array(a)
        };
        let mut admin_bytes = [0u8; 32];
        admin_bytes.copy_from_slice(&data[AGC_OFF_ADMIN..AGC_OFF_ADMIN + 32]);

        let recipients = (0..AGC_PROTOCOL_FEE_RECIPIENTS_LEN)
            .map(|i| pk_at(AGC_OFF_PROTOCOL_FEE_RECIPIENTS + 32 * i))
            .collect();

        Ok(AmmGlobalConfig {
            admin: Pubkey::new_from_array(admin_bytes),
            lp_fee_basis_points: u64_at(AGC_OFF_LP_FEE_BASIS_POINTS),
            protocol_fee_basis_points: u64_at(AGC_OFF_PROTOCOL_FEE_BASIS_POINTS),
            disable_flags: data[AGC_OFF_DISABLE_FLAGS],
            protocol_fee_recipients: recipients,
            coin_creator_fee_basis_points: u64_at(AGC_OFF_COIN_CREATOR_FEE_BASIS_POINTS),
            mayhem_mode_enabled: data.get(AGC_OFF_MAYHEM_MODE_ENABLED).map(|b| *b != 0),
            is_cashback_enabled: data.get(AGC_OFF_IS_CASHBACK_ENABLED).map(|b| *b != 0),
            buyback_basis_points: data
                .get(AGC_OFF_BUYBACK_BASIS_POINTS..AGC_OFF_BUYBACK_BASIS_POINTS + 8)
                .map(|_| u64_at(AGC_OFF_BUYBACK_BASIS_POINTS)),
            raw_len: data.len(),
        })
    }

    /// Total fee taken from a trade, in basis points.
    pub fn total_fee_bps(&self) -> u64 {
        self.lp_fee_basis_points
            .saturating_add(self.protocol_fee_basis_points)
            .saturating_add(self.coin_creator_fee_basis_points)
    }

    /// `disable_flags` is a bitmask; bit 0 disables buys, bit 1 disables sells
    /// (per the program's `DisableFlags`).
    pub fn buys_disabled(&self) -> bool {
        self.disable_flags & 0b01 != 0
    }

    pub fn sells_disabled(&self) -> bool {
        self.disable_flags & 0b10 != 0
    }
}

// --------------------------------------------------------------------------
// Context
// --------------------------------------------------------------------------

/// Everything needed to build a PumpSwap buy or sell for one (pool, user).
#[derive(Debug, Clone)]
pub struct PumpSwapContext {
    pub pool: Pubkey,
    pub pool_state: PoolState,
    pub global_config: Pubkey,
    pub config: AmmGlobalConfig,
    pub user: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_token_program: Pubkey,
    pub quote_token_program: Pubkey,
    pub user_base_token_account: Pubkey,
    pub user_quote_token_account: Pubkey,
    pub pool_base_token_account: Pubkey,
    pub pool_quote_token_account: Pubkey,
    /// The protocol fee recipient chosen for this trade.
    pub protocol_fee_recipient: Pubkey,
    pub protocol_fee_recipient_token_account: Pubkey,
    pub coin_creator_vault_ata: Pubkey,
    pub coin_creator_vault_authority: Pubkey,
    pub global_volume_accumulator: Pubkey,
    pub user_volume_accumulator: Pubkey,
    pub fee_config: Pubkey,
    pub fee_program: Pubkey,
    pub event_authority: Pubkey,
    pub pool_v2: Pubkey,
    pub trailing_fee_recipient: Pubkey,
    pub trailing_fee_recipient_ata: Pubkey,
    /// Base reserve, read from the pool's token account.
    pub base_reserve: u64,
    /// Quote reserve, read from the pool's token account.
    pub quote_reserve: u64,
    pub base_decimals: u8,
    pub quote_decimals: u8,
}

impl PumpSwapContext {
    pub fn named_accounts(&self) -> HashMap<String, Pubkey> {
        let mut m = HashMap::new();
        let ins = [
            ("pool", self.pool),
            ("pool_v2", self.pool_v2),
            ("user", self.user),
            ("global_config", self.global_config),
            ("base_mint", self.base_mint),
            ("quote_mint", self.quote_mint),
            ("lp_mint", self.pool_state.lp_mint),
            ("user_base_token_account", self.user_base_token_account),
            ("user_quote_token_account", self.user_quote_token_account),
            ("pool_base_token_account", self.pool_base_token_account),
            ("pool_quote_token_account", self.pool_quote_token_account),
            ("protocol_fee_recipient", self.protocol_fee_recipient),
            (
                "protocol_fee_recipient_token_account",
                self.protocol_fee_recipient_token_account,
            ),
            ("base_token_program", self.base_token_program),
            ("quote_token_program", self.quote_token_program),
            ("system_program", *SYSTEM_PROGRAM),
            ("associated_token_program", *ASSOCIATED_TOKEN_PROGRAM),
            ("event_authority", self.event_authority),
            ("program", *PUMPSWAP_PROGRAM_ID),
            ("coin_creator_vault_ata", self.coin_creator_vault_ata),
            (
                "coin_creator_vault_authority",
                self.coin_creator_vault_authority,
            ),
            ("global_volume_accumulator", self.global_volume_accumulator),
            ("user_volume_accumulator", self.user_volume_accumulator),
            ("fee_config", self.fee_config),
            ("fee_program", self.fee_program),
            ("trailing_fee_recipient", self.trailing_fee_recipient),
            (
                "trailing_fee_recipient_ata",
                self.trailing_fee_recipient_ata,
            ),
        ];
        for (k, v) in ins {
            m.insert(k.to_string(), v);
        }
        m
    }

    pub fn reverse_names(&self) -> HashMap<Pubkey, String> {
        self.named_accounts()
            .into_iter()
            .map(|(k, v)| (v, k))
            .collect()
    }

    /// Load the pool, the global config and both reserves.
    ///
    /// `pool_address` may be `None`, in which case the canonical pool for a
    /// migrated token is derived (`index = 0`, creator = `pool-authority` PDA).
    pub async fn load(
        rpc: &Rpc,
        base_mint: &Pubkey,
        user: &Pubkey,
        pool_address: Option<Pubkey>,
    ) -> BotResult<Self> {
        let quote_mint = *WSOL_MINT;
        let pool = match pool_address {
            Some(p) => p,
            None => {
                let authority = pool_authority_pda(base_mint);
                pool_pda(0, &authority, base_mint, &quote_mint)
            }
        };

        let accounts = rpc
            .get_multiple_accounts(&[pool, *PUMPSWAP_GLOBAL_CONFIG])
            .await?;
        let pool_data = accounts
            .first()
            .and_then(|a| a.as_ref())
            .ok_or_else(|| {
                BotError::NotFound(format!(
                    "pumpswap pool {pool} for mint {base_mint} does not exist (the token may \
                     still be on its bonding curve, or the pool index is not 0)"
                ))
            })?
            .data
            .clone();
        let config_data = accounts
            .get(1)
            .and_then(|a| a.as_ref())
            .map(|a| a.data.clone())
            .ok_or_else(|| BotError::solana("pumpswap global_config not found"))?;

        let pool_state = PoolState::parse(&pool_data)?;
        let config = AmmGlobalConfig::parse(&config_data)?;

        if pool_state.base_mint != *base_mint {
            return Err(BotError::solana(format!(
                "pool {pool} holds base_mint {}, not {base_mint}",
                pool_state.base_mint
            )));
        }
        if config.buys_disabled() {
            warn!(
                "pumpswap global_config has buys disabled (disable_flags={})",
                config.disable_flags
            );
        }

        let base_token_program = rpc
            .token_program_of(base_mint)
            .await
            .unwrap_or(*TOKEN_PROGRAM);
        let quote_token_program = *TOKEN_PROGRAM;

        // Reserves come from the pool's own token accounts, not the Pool
        // struct: the struct has no base reserve field at all.
        let reserve_accounts = rpc
            .get_multiple_accounts(&[
                pool_state.pool_base_token_account,
                pool_state.pool_quote_token_account,
            ])
            .await?;
        let amount_of = |a: &Option<solana_sdk::account::Account>| {
            a.as_ref()
                .and_then(|acc| acc.data.get(64..72))
                .map(|b| {
                    let mut x = [0u8; 8];
                    x.copy_from_slice(b);
                    u64::from_le_bytes(x)
                })
                .unwrap_or(0)
        };
        let base_reserve = reserve_accounts.first().map(&amount_of).unwrap_or(0);
        let quote_reserve = reserve_accounts.get(1).map(amount_of).unwrap_or(0);

        let base_decimals = rpc.token_decimals(base_mint).await.unwrap_or(6);
        let quote_decimals = rpc.token_decimals(&quote_mint).await.unwrap_or(9);

        // The protocol fee recipient rotates; any of the eight is accepted.
        let protocol_fee_recipient = config
            .protocol_fee_recipients
            .first()
            .copied()
            .filter(|p| *p != Pubkey::default())
            .unwrap_or(*PUMP_FEE_RECIPIENT_FALLBACK);

        let trailing_fee_recipient = pick_breaking_fee_recipient();
        let creator_vault_authority = coin_creator_vault_authority_pda(&pool_state.coin_creator);

        Ok(PumpSwapContext {
            pool,
            pool_state: pool_state.clone(),
            global_config: *PUMPSWAP_GLOBAL_CONFIG,
            config,
            user: *user,
            base_mint: *base_mint,
            quote_mint: pool_state.quote_mint,
            base_token_program,
            quote_token_program,
            user_base_token_account: ata(user, &base_token_program, base_mint),
            user_quote_token_account: ata(user, &quote_token_program, &pool_state.quote_mint),
            pool_base_token_account: pool_state.pool_base_token_account,
            pool_quote_token_account: pool_state.pool_quote_token_account,
            protocol_fee_recipient,
            protocol_fee_recipient_token_account: ata(
                &protocol_fee_recipient,
                &quote_token_program,
                &pool_state.quote_mint,
            ),
            coin_creator_vault_ata: ata(
                &creator_vault_authority,
                &quote_token_program,
                &pool_state.quote_mint,
            ),
            coin_creator_vault_authority: creator_vault_authority,
            global_volume_accumulator: amm_global_volume_accumulator_pda(),
            user_volume_accumulator: amm_user_volume_accumulator_pda(user),
            fee_config: *PUMPSWAP_FEE_CONFIG,
            fee_program: *PUMP_FEES_PROGRAM_ID,
            event_authority: amm_event_authority_pda(),
            pool_v2: pool_v2_pda(base_mint),
            trailing_fee_recipient,
            trailing_fee_recipient_ata: ata(
                &trailing_fee_recipient,
                &TOKEN_PROGRAM,
                &pool_state.quote_mint,
            ),
            base_reserve,
            quote_reserve,
            base_decimals,
            quote_decimals,
        })
    }

    /// Constant-product quote for spending `amount_in` of the quote asset.
    pub fn quote_buy(&self, quote_amount_in: u64) -> BotResult<u64> {
        self.quote(self.quote_reserve, self.base_reserve, quote_amount_in)
    }

    /// Constant-product quote for selling `base_amount_in`.
    pub fn quote_sell(&self, base_amount_in: u64) -> BotResult<u64> {
        self.quote(self.base_reserve, self.quote_reserve, base_amount_in)
    }

    fn quote(&self, reserve_in: u64, reserve_out: u64, amount_in: u64) -> BotResult<u64> {
        if reserve_in == 0 || reserve_out == 0 {
            return Err(BotError::solana(format!(
                "pool {} has empty reserves (in={reserve_in}, out={reserve_out})",
                self.pool
            )));
        }
        // The LP fee stays in the pool; the protocol and creator fees are paid
        // on top of the swap, so only the LP fee reduces the swapped amount.
        let out = maths::constant_product_out(
            amount_in,
            reserve_in,
            reserve_out,
            self.config.lp_fee_basis_points,
            maths::BPS_DENOM,
        );
        if out == 0 {
            return Err(BotError::solana(
                "pumpswap quotes 0 out — the trade is too small for this pool",
            ));
        }
        Ok(out)
    }

    /// Price of one base token in quote units, in human decimals.
    pub fn price(&self) -> f64 {
        if self.base_reserve == 0 {
            return 0.0;
        }
        let base = maths::from_raw_amount(self.base_reserve, self.base_decimals);
        let quote = maths::from_raw_amount(self.quote_reserve, self.quote_decimals);
        if base == 0.0 {
            return 0.0;
        }
        quote / base
    }
}

fn ata(owner: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
    spl_associated_token_account::get_associated_token_address_with_program_id(
        owner,
        mint,
        token_program,
    )
}

// --------------------------------------------------------------------------
// Layouts
// --------------------------------------------------------------------------

/// The `buy` account list exactly as the official IDL states it: 23 accounts.
pub fn default_buy_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*PUMPSWAP_PROGRAM_ID, "buy", Vec::new());
    l.push_named("pool", true, false);
    l.push_named("user", true, true);
    l.push_named("global_config", false, false);
    l.push_named("base_mint", false, false);
    l.push_named("quote_mint", false, false);
    l.push_named("user_base_token_account", true, false);
    l.push_named("user_quote_token_account", true, false);
    l.push_named("pool_base_token_account", true, false);
    l.push_named("pool_quote_token_account", true, false);
    l.push_named("protocol_fee_recipient", false, false);
    l.push_named("protocol_fee_recipient_token_account", true, false);
    l.push_named("base_token_program", false, false);
    l.push_named("quote_token_program", false, false);
    l.push_named("system_program", false, false);
    l.push_named("associated_token_program", false, false);
    l.push_named("event_authority", false, false);
    l.push_named("program", false, false);
    l.push_named("coin_creator_vault_ata", true, false);
    l.push_named("coin_creator_vault_authority", false, false);
    l.push_named("global_volume_accumulator", false, false);
    l.push_named("user_volume_accumulator", true, false);
    l.push_named("fee_config", false, false);
    l.push_named("fee_program", false, false);
    l
}

/// The `sell` account list per the IDL: 21 accounts (no volume accumulators).
pub fn default_sell_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*PUMPSWAP_PROGRAM_ID, "sell", Vec::new());
    l.push_named("pool", true, false);
    l.push_named("user", true, true);
    l.push_named("global_config", false, false);
    l.push_named("base_mint", false, false);
    l.push_named("quote_mint", false, false);
    l.push_named("user_base_token_account", true, false);
    l.push_named("user_quote_token_account", true, false);
    l.push_named("pool_base_token_account", true, false);
    l.push_named("pool_quote_token_account", true, false);
    l.push_named("protocol_fee_recipient", false, false);
    l.push_named("protocol_fee_recipient_token_account", true, false);
    l.push_named("base_token_program", false, false);
    l.push_named("quote_token_program", false, false);
    l.push_named("system_program", false, false);
    l.push_named("associated_token_program", false, false);
    l.push_named("event_authority", false, false);
    l.push_named("program", false, false);
    l.push_named("coin_creator_vault_ata", true, false);
    l.push_named("coin_creator_vault_authority", false, false);
    l.push_named("fee_config", false, false);
    l.push_named("fee_program", false, false);
    l
}

/// Candidate `buy` layouts, in probe order.
///
/// 1. IDL (23) — authoritative as published
/// 2. IDL + `pool-v2` (24)
/// 3. IDL + `pool-v2` + trailing fee recipient + its quote ATA (26) — the shape
///    the 2026-04-28 upgrade note describes
/// 4. IDL + trailing fee recipient + its quote ATA (25)
pub fn buy_layout_variants() -> Vec<AccountLayout> {
    let base = default_buy_layout();

    let mut with_v2 = base.clone();
    with_v2.push_named("pool_v2", false, false);

    let mut full = with_v2.clone();
    full.push_named("trailing_fee_recipient", false, false);
    full.push_named("trailing_fee_recipient_ata", true, false);

    let mut no_v2 = base.clone();
    no_v2.push_named("trailing_fee_recipient", false, false);
    no_v2.push_named("trailing_fee_recipient_ata", true, false);

    vec![base, with_v2, full, no_v2]
}

/// Candidate `sell` layouts, in probe order.
pub fn sell_layout_variants() -> Vec<AccountLayout> {
    let base = default_sell_layout();

    let mut with_v2 = base.clone();
    with_v2.push_named("pool_v2", false, false);

    let mut full = with_v2.clone();
    full.push_named("trailing_fee_recipient", false, false);
    full.push_named("trailing_fee_recipient_ata", true, false);

    let mut no_v2 = base.clone();
    no_v2.push_named("trailing_fee_recipient", false, false);
    no_v2.push_named("trailing_fee_recipient_ata", true, false);

    vec![base, with_v2, full, no_v2]
}

/// Choose a layout: a trusted learned one wins, otherwise the IDL default.
fn effective_layout(
    store: &LayoutStore,
    instruction: &str,
    default: AccountLayout,
) -> AccountLayout {
    if let Some(learned) = store.get(&PUMPSWAP_PROGRAM_ID, instruction) {
        if learned.trusted {
            debug!(
                instruction,
                accounts = learned.len(),
                "using learned pumpswap layout"
            );
            return learned.clone();
        }
    }
    default
}

// --------------------------------------------------------------------------
// Instruction builders
// --------------------------------------------------------------------------

/// Which buy instruction to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuyKind {
    /// Pins the token amount out, ceilings the quote in.
    ExactBaseOut,
    /// Pins the quote in, floors the token amount out. This is what most
    /// clients use because the SOL budget is the thing you actually control.
    #[default]
    ExactQuoteIn,
}

/// Build a PumpSwap buy.
pub fn build_buy_ix(
    ctx: &PumpSwapContext,
    store: &LayoutStore,
    kind: BuyKind,
    amount: u64,
    limit: u64,
    track_volume: bool,
) -> BotResult<Instruction> {
    let (discriminator, instruction) = match kind {
        BuyKind::ExactBaseOut => (PUMPSWAP_DISC_BUY, "buy"),
        BuyKind::ExactQuoteIn => (PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN, "buy_exact_quote_in"),
    };

    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&discriminator);
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&limit.to_le_bytes());
    if kind == BuyKind::ExactBaseOut {
        // `buy` carries a third `track_volume: OptionBool` argument;
        // `buy_exact_quote_in` does too, so append it for both.
        data.push(if track_volume { 1 } else { 0 });
    } else {
        data.push(if track_volume { 1 } else { 0 });
    }

    let default = match kind {
        BuyKind::ExactBaseOut => default_buy_layout(),
        // Same account list, different discriminator.
        BuyKind::ExactQuoteIn => {
            let mut l = default_buy_layout();
            l.instruction = "buy_exact_quote_in".to_string();
            l
        }
    };
    let layout = effective_layout(store, instruction, default);
    let accounts = layout.build(&ctx.named_accounts())?;

    Ok(Instruction {
        program_id: *PUMPSWAP_PROGRAM_ID,
        accounts,
        data,
    })
}

/// Build a PumpSwap sell.
pub fn build_sell_ix(
    ctx: &PumpSwapContext,
    store: &LayoutStore,
    base_amount_in: u64,
    min_quote_amount_out: u64,
) -> BotResult<Instruction> {
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMPSWAP_DISC_SELL);
    data.extend_from_slice(&base_amount_in.to_le_bytes());
    data.extend_from_slice(&min_quote_amount_out.to_le_bytes());

    let layout = effective_layout(store, "sell", default_sell_layout());
    let accounts = layout.build(&ctx.named_accounts())?;

    Ok(Instruction {
        program_id: *PUMPSWAP_PROGRAM_ID,
        accounts,
        data,
    })
}

/// `extend_account` on the AMM program — 5 accounts.
pub fn build_extend_account_ix(ctx: &PumpSwapContext, account: Pubkey) -> Instruction {
    Instruction {
        program_id: *PUMPSWAP_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(account, false),
            AccountMeta::new(ctx.user, true),
            AccountMeta::new_readonly(*SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(ctx.event_authority, false),
            AccountMeta::new_readonly(*PUMPSWAP_PROGRAM_ID, false),
        ],
        data: PUMPSWAP_DISC_EXTEND_ACCOUNT.to_vec(),
    }
}

/// Plan a buy from a SOL budget: returns `(quote_in, base_out, max_quote_in)`.
pub fn plan_buy(
    ctx: &PumpSwapContext,
    sol_budget: u64,
    slippage_pct: f64,
) -> BotResult<(u64, u64, u64)> {
    if sol_budget == 0 {
        return Err(BotError::invalid("sol_budget must be > 0"));
    }
    let expected_out = ctx.quote_buy(sol_budget)?;
    let max_quote_in = maths::apply_pct_u64(sol_budget, slippage_pct);
    let min_base_out = maths::minus_pct_u64(expected_out, slippage_pct);
    if min_base_out == 0 {
        return Err(BotError::solana(
            "slippage tolerance would allow zero tokens out",
        ));
    }
    Ok((sol_budget, min_base_out, max_quote_in))
}

/// Plan a sell of the whole base balance.
pub fn plan_sell(
    ctx: &PumpSwapContext,
    base_amount_in: u64,
    slippage_pct: f64,
) -> BotResult<(u64, u64)> {
    if base_amount_in == 0 {
        return Err(BotError::invalid("base_amount_in must be > 0"));
    }
    let expected = ctx.quote_sell(base_amount_in)?;
    Ok((base_amount_in, maths::minus_pct_u64(expected, slippage_pct)))
}

/// Find every PumpSwap pool whose base mint is `base_mint` and quote is WSOL.
/// Two `memcmp` filters do the work server-side.
pub async fn find_pools_for_mint(rpc: &Rpc, base_mint: &Pubkey) -> BotResult<Vec<Pubkey>> {
    use solana_account_decoder::UiAccountEncoding;
    use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
    use solana_client::rpc_filter::{Memcmp, MemcmpEncodedBytes, RpcFilterType};

    let config = RpcProgramAccountsConfig {
        filters: Some(vec![
            RpcFilterType::Memcmp(Memcmp::new(
                POOL_OFF_BASE_MINT,
                MemcmpEncodedBytes::Bytes(base_mint.to_bytes().to_vec()),
            )),
            RpcFilterType::Memcmp(Memcmp::new(
                POOL_OFF_QUOTE_MINT,
                MemcmpEncodedBytes::Bytes(WSOL_MINT.to_bytes().to_vec()),
            )),
        ]),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: None,
            commitment: Some(rpc.commitment()),
            min_context_slot: None,
        },
        with_context: Some(false),
        sort_results: None,
    };

    let accounts = rpc
        .raw()
        .get_program_accounts_with_config(&PUMPSWAP_PROGRAM_ID, config)
        .await
        .map_err(|e| BotError::rpc(format!("getProgramAccounts for pumpswap pools: {e}")))?;
    let pools: Vec<Pubkey> = accounts.into_iter().map(|(pk, _)| pk).collect();
    debug!(%base_mint, count = pools.len(), "discovered pumpswap pools");
    Ok(pools)
}

/// Learn a layout from a confirmed transaction's account list.
pub fn layout_from_accounts(
    accounts: &[AccountMeta],
    names: &HashMap<Pubkey, String>,
    instruction: &str,
) -> Option<AccountLayout> {
    if accounts.is_empty() {
        return None;
    }
    let mut layout = AccountLayout::new(*PUMPSWAP_PROGRAM_ID, instruction, Vec::new());
    layout.learn(accounts, names, None);
    Some(layout)
}

/// Summary for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PumpSwapSummary {
    pub pool: String,
    pub base_mint: String,
    pub quote_mint: String,
    pub base_reserve: f64,
    pub quote_reserve: f64,
    pub price: f64,
    pub lp_fee_bps: u64,
    pub protocol_fee_bps: u64,
    pub creator_fee_bps: u64,
    pub buys_disabled: bool,
    pub sells_disabled: bool,
}

impl From<&PumpSwapContext> for PumpSwapSummary {
    fn from(c: &PumpSwapContext) -> Self {
        PumpSwapSummary {
            pool: c.pool.to_string(),
            base_mint: c.base_mint.to_string(),
            quote_mint: c.quote_mint.to_string(),
            base_reserve: maths::from_raw_amount(c.base_reserve, c.base_decimals),
            quote_reserve: maths::from_raw_amount(c.quote_reserve, c.quote_decimals),
            price: c.price(),
            lp_fee_bps: c.config.lp_fee_basis_points,
            protocol_fee_bps: c.config.protocol_fee_basis_points,
            creator_fee_bps: c.config.coin_creator_fee_basis_points,
            buys_disabled: c.config.buys_disabled(),
            sells_disabled: c.config.sells_disabled(),
        }
    }
}

/// Probe every candidate layout by simulation and persist the first one that
/// works.
///
/// Run at startup (and on demand from the dashboard) so an on-chain account
/// change does not need a code deploy. Simulation is free and does not touch
/// funds.
pub struct PumpSwapLayoutDoctor;

impl PumpSwapLayoutDoctor {
    /// Returns the number of accounts in the layout that simulated cleanly, or
    /// `None` if every candidate failed.
    pub async fn probe_buy(
        rpc: &Rpc,
        ctx: &PumpSwapContext,
        store: &mut LayoutStore,
        signer: &dyn solana_sdk::signer::Signer,
        quote_in: u64,
    ) -> BotResult<Option<usize>> {
        Self::probe(
            rpc,
            ctx,
            store,
            signer,
            buy_layout_variants(),
            "buy_exact_quote_in",
            PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN,
            quote_in,
            1,
        )
        .await
    }

    pub async fn probe_sell(
        rpc: &Rpc,
        ctx: &PumpSwapContext,
        store: &mut LayoutStore,
        signer: &dyn solana_sdk::signer::Signer,
        base_in: u64,
    ) -> BotResult<Option<usize>> {
        Self::probe(
            rpc,
            ctx,
            store,
            signer,
            sell_layout_variants(),
            "sell",
            PUMPSWAP_DISC_SELL,
            base_in,
            1,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn probe(
        rpc: &Rpc,
        ctx: &PumpSwapContext,
        store: &mut LayoutStore,
        signer: &dyn solana_sdk::signer::Signer,
        variants: Vec<AccountLayout>,
        instruction: &str,
        discriminator: [u8; 8],
        amount: u64,
        limit: u64,
    ) -> BotResult<Option<usize>> {
        let names = ctx.named_accounts();
        let blockhash = rpc.latest_blockhash(true).await?.blockhash;

        for layout in &variants {
            let Ok(accounts) = layout.build(&names) else {
                debug!(
                    instruction,
                    accounts = layout.len(),
                    "candidate layout has unresolvable slots, skipping"
                );
                continue;
            };
            let mut data = Vec::with_capacity(24);
            data.extend_from_slice(&discriminator);
            data.extend_from_slice(&amount.to_le_bytes());
            data.extend_from_slice(&limit.to_le_bytes());

            let ix = Instruction {
                program_id: *PUMPSWAP_PROGRAM_ID,
                accounts,
                data,
            };
            let Ok(message) = solana_sdk::message::v0::Message::try_compile(
                &signer.pubkey(),
                &[ix],
                &[],
                blockhash,
            ) else {
                continue;
            };
            let Ok(tx) = solana_sdk::transaction::VersionedTransaction::try_new(
                solana_sdk::message::VersionedMessage::V0(message),
                &[signer],
            ) else {
                continue;
            };

            match rpc.simulate(&tx).await {
                Ok(sim) => {
                    let v = sim.value;
                    if v.err.is_none() {
                        info!(
                            instruction,
                            accounts = layout.len(),
                            "pumpswap layout probe succeeded"
                        );
                        let mut winner = layout.clone();
                        winner.trusted = true;
                        winner.instruction = instruction.to_string();
                        store.insert(winner);
                        return Ok(Some(layout.len()));
                    } else {
                        debug!(
                            instruction,
                            accounts = layout.len(),
                            error = ?v.err,
                            logs = ?v.logs.as_ref().and_then(|l| l.last().cloned()),
                            "candidate layout rejected"
                        );
                    }
                }
                Err(e) => {
                    warn!(instruction, error = %e, "layout probe simulation call failed");
                }
            }
        }

        warn!(
            instruction,
            candidates = variants.len(),
            "no pumpswap layout simulated cleanly — check the RPC endpoint and the pool state"
        );
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_data(base_mint: Pubkey, quote_mint: Pubkey) -> Vec<u8> {
        let mut d = vec![0u8; POOL_LEN];
        d[..8].copy_from_slice(&PUMPSWAP_ACC_DISC_POOL);
        d[POOL_OFF_POOL_BUMP] = 253;
        d[POOL_OFF_INDEX..POOL_OFF_INDEX + 2].copy_from_slice(&0u16.to_le_bytes());
        let pk = |d: &mut Vec<u8>, off: usize, v: Pubkey| {
            d[off..off + 32].copy_from_slice(&v.to_bytes())
        };
        pk(&mut d, POOL_OFF_CREATOR, Pubkey::new_unique());
        pk(&mut d, POOL_OFF_BASE_MINT, base_mint);
        pk(&mut d, POOL_OFF_QUOTE_MINT, quote_mint);
        pk(&mut d, POOL_OFF_LP_MINT, Pubkey::new_unique());
        pk(
            &mut d,
            POOL_OFF_POOL_BASE_TOKEN_ACCOUNT,
            Pubkey::new_unique(),
        );
        pk(
            &mut d,
            POOL_OFF_POOL_QUOTE_TOKEN_ACCOUNT,
            Pubkey::new_unique(),
        );
        d[POOL_OFF_LP_SUPPLY..POOL_OFF_LP_SUPPLY + 8].copy_from_slice(&1_000_000u64.to_le_bytes());
        pk(&mut d, POOL_OFF_COIN_CREATOR, Pubkey::new_unique());
        d[POOL_OFF_IS_MAYHEM_MODE] = 0;
        d[POOL_OFF_IS_CASHBACK_COIN] = 1;
        d[POOL_OFF_CREATOR_FEE_BPS..POOL_OFF_CREATOR_FEE_BPS + 8]
            .copy_from_slice(&50u64.to_le_bytes());
        d
    }

    fn global_config_data(lp_bps: u64, proto_bps: u64, creator_bps: u64, flags: u8) -> Vec<u8> {
        let mut d = vec![0u8; AMM_GLOBAL_CONFIG_LEN];
        d[..8].copy_from_slice(&PUMPSWAP_ACC_DISC_GLOBAL_CONFIG);
        d[AGC_OFF_LP_FEE_BASIS_POINTS..AGC_OFF_LP_FEE_BASIS_POINTS + 8]
            .copy_from_slice(&lp_bps.to_le_bytes());
        d[AGC_OFF_PROTOCOL_FEE_BASIS_POINTS..AGC_OFF_PROTOCOL_FEE_BASIS_POINTS + 8]
            .copy_from_slice(&proto_bps.to_le_bytes());
        d[AGC_OFF_COIN_CREATOR_FEE_BASIS_POINTS..AGC_OFF_COIN_CREATOR_FEE_BASIS_POINTS + 8]
            .copy_from_slice(&creator_bps.to_le_bytes());
        d[AGC_OFF_DISABLE_FLAGS] = flags;
        for i in 0..AGC_PROTOCOL_FEE_RECIPIENTS_LEN {
            let pk = Pubkey::new_unique();
            d[AGC_OFF_PROTOCOL_FEE_RECIPIENTS + 32 * i
                ..AGC_OFF_PROTOCOL_FEE_RECIPIENTS + 32 * i + 32]
                .copy_from_slice(&pk.to_bytes());
        }
        d
    }

    #[test]
    fn pool_offsets_match_the_idl() {
        let base = Pubkey::new_unique();
        let d = pool_data(base, *WSOL_MINT);
        let p = PoolState::parse(&d).unwrap();
        assert_eq!(p.pool_bump, 253);
        assert_eq!(p.index, 0);
        assert_eq!(p.base_mint, base);
        assert_eq!(p.quote_mint, *WSOL_MINT);
        assert_eq!(p.lp_supply, 1_000_000);
        assert_eq!(p.is_cashback_coin, Some(true));
        assert_eq!(p.is_mayhem_mode, Some(false));
        assert_eq!(p.creator_fee_bps, Some(50));
        assert_eq!(p.raw_len, POOL_LEN);
        assert_eq!(p.lp_token_program(), *TOKEN_2022_PROGRAM);
    }

    #[test]
    fn pool_base_mint_offset_is_43() {
        // Independently corroborated by a third party using GPA memcmp filters.
        assert_eq!(POOL_OFF_BASE_MINT, 43);
        assert_eq!(POOL_OFF_QUOTE_MINT, 75);
    }

    #[test]
    fn pool_parse_rejects_a_bad_discriminator() {
        let mut d = pool_data(Pubkey::new_unique(), *WSOL_MINT);
        d[0] ^= 0xff;
        let err = PoolState::parse(&d).unwrap_err();
        assert!(err.to_string().contains("discriminator mismatch"), "{err}");
    }

    #[test]
    fn pool_parse_rejects_a_short_account() {
        let err = PoolState::parse(&[0u8; 40]).unwrap_err();
        assert!(err.to_string().contains("need at least"), "{err}");
    }

    #[test]
    fn global_config_parses_fees_and_recipients() {
        let d = global_config_data(20, 5, 100, 0);
        let c = AmmGlobalConfig::parse(&d).unwrap();
        assert_eq!(c.lp_fee_basis_points, 20);
        assert_eq!(c.protocol_fee_basis_points, 5);
        assert_eq!(c.coin_creator_fee_basis_points, 100);
        assert_eq!(c.total_fee_bps(), 125);
        assert_eq!(c.protocol_fee_recipients.len(), 8);
        assert!(!c.buys_disabled());
        assert!(!c.sells_disabled());
        assert!(c
            .protocol_fee_recipients
            .iter()
            .all(|p| *p != Pubkey::default()));
    }

    #[test]
    fn disable_flags_are_a_bitmask() {
        assert!(AmmGlobalConfig::parse(&global_config_data(0, 0, 0, 0b01))
            .unwrap()
            .buys_disabled());
        assert!(AmmGlobalConfig::parse(&global_config_data(0, 0, 0, 0b10))
            .unwrap()
            .sells_disabled());
        let both = AmmGlobalConfig::parse(&global_config_data(0, 0, 0, 0b11)).unwrap();
        assert!(both.buys_disabled() && both.sells_disabled());
    }

    #[test]
    fn discriminators_match_the_published_idl() {
        // Every one of these was read out of idl/pump_amm.json.
        assert_eq!(PUMPSWAP_DISC_BUY, [102, 6, 61, 18, 1, 218, 235, 234]);
        assert_eq!(PUMPSWAP_DISC_SELL, [51, 230, 133, 164, 1, 127, 131, 173]);
        assert_eq!(
            PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN,
            [198, 46, 21, 82, 180, 217, 232, 112]
        );
        assert_eq!(
            PUMPSWAP_DISC_CREATE_POOL,
            [233, 146, 209, 142, 207, 104, 64, 188]
        );
        assert_eq!(
            PUMPSWAP_DISC_DEPOSIT,
            [242, 35, 198, 137, 82, 225, 242, 182]
        );
        assert_eq!(
            PUMPSWAP_DISC_WITHDRAW,
            [183, 18, 70, 156, 148, 109, 161, 34]
        );
        assert_eq!(
            PUMPSWAP_ACC_DISC_POOL,
            [241, 154, 109, 4, 17, 177, 109, 188]
        );
        assert_eq!(
            PUMPSWAP_ACC_DISC_GLOBAL_CONFIG,
            [149, 8, 156, 202, 160, 252, 176, 217]
        );
    }

    #[test]
    fn discriminators_are_sighashes_of_their_names() {
        use sha2::{Digest, Sha256};
        let sighash = |prefix: &str, name: &str| -> [u8; 8] {
            let mut h = Sha256::new();
            h.update(format!("{prefix}:{name}").as_bytes());
            let out = h.finalize();
            let mut a = [0u8; 8];
            a.copy_from_slice(&out[..8]);
            a
        };
        assert_eq!(sighash("global", "buy"), PUMPSWAP_DISC_BUY);
        assert_eq!(sighash("global", "sell"), PUMPSWAP_DISC_SELL);
        assert_eq!(
            sighash("global", "buy_exact_quote_in"),
            PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN
        );
        assert_eq!(
            sighash("global", "sell_exact_base_in"),
            PUMPSWAP_DISC_SELL_EXACT_BASE_IN
        );
        assert_eq!(sighash("account", "Pool"), PUMPSWAP_ACC_DISC_POOL);
        assert_eq!(
            sighash("account", "GlobalConfig"),
            PUMPSWAP_ACC_DISC_GLOBAL_CONFIG
        );
        assert_eq!(sighash("event", "CreatePoolEvent"), EV_AMM_CREATE_POOL);
        assert_eq!(sighash("event", "BuyEvent"), EV_AMM_BUY);
        assert_eq!(sighash("event", "SellEvent"), EV_AMM_SELL);
    }

    #[test]
    fn layout_variants_have_the_documented_account_counts() {
        let counts: Vec<usize> = buy_layout_variants().iter().map(|l| l.len()).collect();
        assert_eq!(counts, vec![23, 24, 26, 25], "buy candidates");

        let counts: Vec<usize> = sell_layout_variants().iter().map(|l| l.len()).collect();
        assert_eq!(counts, vec![21, 22, 24, 23], "sell candidates");

        // The IDL default must be exactly 23/21.
        assert_eq!(default_buy_layout().len(), 23);
        assert_eq!(default_sell_layout().len(), 21);
    }

    #[test]
    fn sell_layout_omits_the_volume_accumulators() {
        let names: Vec<String> = default_sell_layout()
            .slots
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        assert!(!names.contains(&"user_volume_accumulator".to_string()));
        assert!(!names.contains(&"global_volume_accumulator".to_string()));
        // Buy does include them.
        let buy: Vec<String> = default_buy_layout()
            .slots
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        assert!(buy.contains(&"user_volume_accumulator".to_string()));
        assert!(buy.contains(&"global_volume_accumulator".to_string()));
    }

    #[test]
    fn buy_layout_flag_order_matches_the_idl() {
        let l = default_buy_layout();
        let flags: Vec<(String, bool, bool)> = l
            .slots
            .iter()
            .map(|s| (s.name().to_string(), s.writable(), s.signer()))
            .collect();
        assert_eq!(flags[0], ("pool".into(), true, false));
        assert_eq!(flags[1], ("user".into(), true, true));
        assert_eq!(flags[2], ("global_config".into(), false, false));
        assert_eq!(flags[10].0, "protocol_fee_recipient_token_account");
        assert!(flags[10].1, "the recipient ATA is writable");
        assert_eq!(flags[13].0, "system_program");
        assert!(!flags[13].1);
        assert_eq!(flags[16].0, "program");
        assert_eq!(flags[20].0, "user_volume_accumulator");
        assert!(flags[20].1);
        assert_eq!(flags[22].0, "fee_program");
        // Exactly one signer.
        assert_eq!(flags.iter().filter(|f| f.2).count(), 1);
    }

    fn test_ctx(base_reserve: u64, quote_reserve: u64) -> PumpSwapContext {
        let base_mint = Pubkey::new_unique();
        let quote_mint = *WSOL_MINT;
        let pool = pool_pda(0, &Pubkey::new_unique(), &base_mint, &quote_mint);
        PumpSwapContext {
            pool,
            pool_state: PoolState::parse(&pool_data(base_mint, quote_mint)).unwrap(),
            global_config: *PUMPSWAP_GLOBAL_CONFIG,
            config: AmmGlobalConfig::parse(&global_config_data(20, 5, 0, 0)).unwrap(),
            user: Pubkey::new_unique(),
            base_mint,
            quote_mint,
            base_token_program: *TOKEN_PROGRAM,
            quote_token_program: *TOKEN_PROGRAM,
            user_base_token_account: Pubkey::new_unique(),
            user_quote_token_account: Pubkey::new_unique(),
            pool_base_token_account: Pubkey::new_unique(),
            pool_quote_token_account: Pubkey::new_unique(),
            protocol_fee_recipient: Pubkey::new_unique(),
            protocol_fee_recipient_token_account: Pubkey::new_unique(),
            coin_creator_vault_ata: Pubkey::new_unique(),
            coin_creator_vault_authority: Pubkey::new_unique(),
            global_volume_accumulator: amm_global_volume_accumulator_pda(),
            user_volume_accumulator: amm_user_volume_accumulator_pda(&Pubkey::new_unique()),
            fee_config: *PUMPSWAP_FEE_CONFIG,
            fee_program: *PUMP_FEES_PROGRAM_ID,
            event_authority: amm_event_authority_pda(),
            pool_v2: pool_v2_pda(&base_mint),
            trailing_fee_recipient: pick_breaking_fee_recipient(),
            trailing_fee_recipient_ata: Pubkey::new_unique(),
            base_reserve,
            quote_reserve,
            base_decimals: 6,
            quote_decimals: 9,
        }
    }

    #[test]
    fn build_buy_ix_uses_the_effective_layout_and_correct_data() {
        let ctx = test_ctx(1_000_000_000, 100_000_000_000);
        let store = LayoutStore::default();

        let ix = build_buy_ix(
            &ctx,
            &store,
            BuyKind::ExactQuoteIn,
            500_000,
            1_000_000,
            true,
        )
        .unwrap();
        assert_eq!(ix.program_id, *PUMPSWAP_PROGRAM_ID);
        assert_eq!(ix.accounts.len(), 23, "IDL default");
        assert_eq!(&ix.data[..8], &PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN);
        assert_eq!(
            u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
            500_000
        );
        assert_eq!(
            u64::from_le_bytes(ix.data[16..24].try_into().unwrap()),
            1_000_000
        );
        assert_eq!(ix.data[24], 1, "track_volume = true");
        assert_eq!(ix.data.len(), 25);

        let ix2 = build_buy_ix(&ctx, &store, BuyKind::ExactBaseOut, 10, 20, false).unwrap();
        assert_eq!(&ix2.data[..8], &PUMPSWAP_DISC_BUY);
        assert_eq!(ix2.data[24], 0);
    }

    #[test]
    fn build_sell_ix_data_is_disc_plus_two_u64() {
        let ctx = test_ctx(1_000_000_000, 100_000_000_000);
        let store = LayoutStore::default();
        let ix = build_sell_ix(&ctx, &store, 123, 456).unwrap();
        assert_eq!(ix.accounts.len(), 21);
        assert_eq!(ix.data.len(), 24);
        assert_eq!(&ix.data[..8], &PUMPSWAP_DISC_SELL);
        assert_eq!(u64::from_le_bytes(ix.data[8..16].try_into().unwrap()), 123);
        assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 456);
    }

    #[test]
    fn a_learned_layout_overrides_the_default() {
        let ctx = test_ctx(1_000_000_000, 100_000_000_000);
        let mut store = LayoutStore::default();

        // Simulate what the doctor stores after the 26-account variant wins.
        let mut winner = buy_layout_variants().remove(2);
        winner.instruction = "buy_exact_quote_in".to_string();
        winner.trusted = true;
        store.insert(winner);

        let ix = build_buy_ix(&ctx, &store, BuyKind::ExactQuoteIn, 1, 2, false).unwrap();
        assert_eq!(ix.accounts.len(), 26, "the trusted learned layout must win");

        // An untrusted layout must NOT win.
        let mut store2 = LayoutStore::default();
        let mut untrusted = buy_layout_variants().remove(2);
        untrusted.instruction = "buy_exact_quote_in".to_string();
        untrusted.trusted = false;
        store2.insert(untrusted);
        let ix2 = build_buy_ix(&ctx, &store2, BuyKind::ExactQuoteIn, 1, 2, false).unwrap();
        assert_eq!(ix2.accounts.len(), 23);
    }

    #[test]
    fn quotes_apply_the_lp_fee_and_constant_product() {
        let ctx = test_ctx(1_000_000_000, 100_000_000_000);
        let out = ctx.quote_buy(1_000_000_000).unwrap();
        assert!(out > 0);
        // 20 bps LP fee: the output must be lower than the fee-free result.
        let fee_free =
            maths::constant_product_out(1_000_000_000, 100_000_000_000, 1_000_000_000, 0, 0);
        assert!(out < fee_free, "{out} should be below {fee_free}");

        let back = ctx.quote_sell(out).unwrap();
        assert!(
            back < 1_000_000_000,
            "a round trip must lose value to fees and impact, got {back}"
        );

        // Empty reserves error instead of dividing by zero.
        let empty = test_ctx(0, 100_000_000_000);
        assert!(empty.quote_buy(1).is_err());
    }

    #[test]
    fn plan_buy_returns_a_min_out_below_the_spot_quote() {
        let ctx = test_ctx(1_000_000_000, 100_000_000_000);
        let (quote_in, min_out, max_in) = plan_buy(&ctx, 1_000_000_000, 5.0).unwrap();
        assert_eq!(quote_in, 1_000_000_000);
        assert!(
            max_in > quote_in,
            "max_quote_in carries the slippage headroom"
        );
        let spot = ctx.quote_buy(1_000_000_000).unwrap();
        assert!(min_out < spot, "min_base_out must be discounted");
        assert!(plan_buy(&ctx, 0, 5.0).is_err());
    }

    #[test]
    fn plan_sell_discounts_the_expected_output() {
        let ctx = test_ctx(1_000_000_000, 100_000_000_000);
        let (base_in, min_out) = plan_sell(&ctx, 100_000_000, 10.0).unwrap();
        assert_eq!(base_in, 100_000_000);
        let spot = ctx.quote_sell(100_000_000).unwrap();
        assert!(min_out < spot);
        assert!(plan_sell(&ctx, 0, 1.0).is_err());
    }

    #[test]
    fn price_is_quote_per_base_in_human_units() {
        let ctx = test_ctx(1_000_000_000, 10_000_000_000);
        // 1000 base (6dp) vs 10 SOL (9dp) => 0.01
        let p = ctx.price();
        assert!((p - 0.01).abs() < 1e-9, "got {p}");
    }

    #[test]
    fn pool_pda_derivation_is_deterministic() {
        let creator = Pubkey::new_unique();
        let base = Pubkey::new_unique();
        let a = pool_pda(0, &creator, &base, &WSOL_MINT);
        let b = pool_pda(0, &creator, &base, &WSOL_MINT);
        assert_eq!(a, b);
        assert_ne!(a, pool_pda(1, &creator, &base, &WSOL_MINT));
        assert_ne!(a, pool_v2_pda(&base));
    }

    #[test]
    fn named_accounts_covers_every_slot_in_every_variant() {
        let ctx = test_ctx(1, 1);
        let names = ctx.named_accounts();
        for layout in buy_layout_variants()
            .into_iter()
            .chain(sell_layout_variants())
        {
            let unresolved: Vec<String> = layout
                .slots
                .iter()
                .filter(|s| !names.contains_key(s.name()))
                .map(|s| s.name().to_string())
                .collect();
            assert!(
                unresolved.is_empty(),
                "{} has slots that named_accounts cannot resolve: {unresolved:?}",
                layout.instruction
            );
            // And the whole layout must build.
            assert!(layout.build(&names).is_ok());
        }
    }

    #[test]
    fn layout_from_accounts_pins_unnamed_pubkeys() {
        let ctx = test_ctx(1, 1);
        let names = ctx.reverse_names();
        let unknown = Pubkey::new_unique();
        let metas = vec![
            AccountMeta::new(ctx.pool, false),
            AccountMeta::new(unknown, false),
        ];
        let l = layout_from_accounts(&metas, &names, "buy").unwrap();
        assert_eq!(l.len(), 2);
        assert!(matches!(&l.slots[0], crate::layout::Slot::Named { name, .. } if name == "pool"));
        assert!(matches!(&l.slots[1], crate::layout::Slot::Fixed { .. }));
        assert!(layout_from_accounts(&[], &names, "buy").is_none());
    }

    #[test]
    fn extend_account_has_five_accounts() {
        let ctx = test_ctx(1, 1);
        let ix = build_extend_account_ix(&ctx, ctx.pool);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(ix.data, PUMPSWAP_DISC_EXTEND_ACCOUNT.to_vec());
        assert_eq!(ix.accounts[0].pubkey, ctx.pool);
        assert!(ix.accounts[1].is_signer);
        assert_eq!(ix.accounts[3].pubkey, ctx.event_authority);
        assert_eq!(ix.accounts[4].pubkey, *PUMPSWAP_PROGRAM_ID);
    }

    #[test]
    fn summary_is_serialisable() {
        let ctx = test_ctx(1_000_000_000, 10_000_000_000);
        let s = PumpSwapSummary::from(&ctx);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"lp_fee_bps\":20"));
        assert_eq!(s.quote_reserve, 10.0);
        assert!(!s.buys_disabled);
    }
}
