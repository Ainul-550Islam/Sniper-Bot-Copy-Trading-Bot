//! Pump.fun bonding curve: accounts, PDAs, pricing and instruction builders.
//!
//! ## Account layout (verified against the post-cashback-upgrade program)
//!
//! `buy` takes **17** accounts:
//!
//! ```text
//!  0 global                      ro     9 creator_vault            w
//!  1 fee_recipient               w     10 event_authority          ro
//!  2 mint                        ro    11 program                  ro
//!  3 bonding_curve               w     12 global_volume_accumulator ro
//!  4 associated_bonding_curve    w     13 user_volume_accumulator  w
//!  5 associated_user             w     14 fee_config               ro
//!  6 user                        w/s   15 fee_program              ro
//!  7 system_program              ro    16 bonding_curve_v2         ro
//!  8 token_program               ro
//! ```
//!
//! `sell` takes **15** accounts for non-cashback tokens and **16** for
//! cashback-enabled tokens (the extra one is `user_volume_accumulator`,
//! inserted before `bonding_curve_v2`). The cashback flag is byte 82 of the
//! bonding-curve account — it is *not* implied by the token being Token-2022.
//!
//! `buy` args: `amount: u64` (tokens out), `max_sol_cost: u64` (lamports),
//! `track_volume: OptionBool` (1 byte).
//! `sell` args: `amount: u64` (tokens in), `min_sol_output: u64` (lamports).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_system_interface::program as system_program;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use tracing::{debug, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::maths;

use crate::consts::*;
use crate::layout::{AccountLayout, LayoutStore, Slot};
use crate::rpc::Rpc;

// --------------------------------------------------------------------------
// PDA derivation
// --------------------------------------------------------------------------

pub fn bonding_curve_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PUMP_SEED_BONDING_CURVE, mint.as_ref()], &PUMP_PROGRAM_ID).0
}

pub fn bonding_curve_v2_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMP_SEED_BONDING_CURVE_V2, mint.as_ref()],
        &PUMP_PROGRAM_ID,
    )
    .0
}

pub fn creator_vault_pda(creator: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMP_SEED_CREATOR_VAULT, creator.as_ref()],
        &PUMP_PROGRAM_ID,
    )
    .0
}

pub fn event_authority_pda() -> Pubkey {
    Pubkey::find_program_address(&[PUMP_SEED_EVENT_AUTHORITY], &PUMP_PROGRAM_ID).0
}

pub fn global_volume_accumulator_pda() -> Pubkey {
    Pubkey::find_program_address(&[PUMP_SEED_GLOBAL_VOLUME_ACCUMULATOR], &PUMP_PROGRAM_ID).0
}

pub fn user_volume_accumulator_pda(user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[PUMP_SEED_USER_VOLUME_ACCUMULATOR, user.as_ref()],
        &PUMP_PROGRAM_ID,
    )
    .0
}

/// `fee_config` lives on the **fee program**, seeded with the pump program id.
pub fn fee_config_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[PUMP_SEED_FEE_CONFIG, PUMP_PROGRAM_ID.as_ref()],
        &PUMP_FEES_PROGRAM_ID,
    )
    .0
}

/// `sharing-config` PDA, required by the v2 instructions. Readonly, and it may
/// not exist for a token that has no revenue sharing configured — the program
/// accepts the derived address either way.
pub fn sharing_config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PUMP_SEED_SHARING_CONFIG, mint.as_ref()], &PUMP_PROGRAM_ID).0
}

/// `mint-authority` PDA, used by `create`/`create_v2`.
pub fn mint_authority_pda() -> Pubkey {
    Pubkey::find_program_address(&[PUMP_SEED_MINT_AUTHORITY], &PUMP_PROGRAM_ID).0
}

/// ATA of the bonding curve for `mint` (allow off-curve).
pub fn associated_bonding_curve(mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let bc = bonding_curve_pda(mint);
    get_associated_token_address_with_program_id(&bc, mint, token_program)
}

/// The trader's ATA for `mint`.
pub fn associated_user(mint: &Pubkey, user: &Pubkey, token_program: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(user, mint, token_program)
}

// --------------------------------------------------------------------------
// Account state
// --------------------------------------------------------------------------

/// Decoded `Global` account.
///
/// Layout source: `pump-fun/pump-public-docs` → `idl/pump.json`. The account
/// has grown repeatedly (fee_recipients, mayhem, cashback, buyback, holder
/// rewards), so every field after the core reserves is read defensively: a
/// short account simply yields `None`/defaults rather than an error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalState {
    pub initialized: bool,
    pub authority: Pubkey,
    /// The protocol fee recipient in slot 0. Trades pay this one.
    pub fee_recipient: Pubkey,
    pub initial_virtual_token_reserves: u64,
    pub initial_virtual_sol_reserves: u64,
    pub initial_real_token_reserves: u64,
    pub token_total_supply: u64,
    pub fee_basis_points: u64,
    pub withdraw_authority: Pubkey,
    pub enable_migrate: bool,
    pub pool_migration_fee: u64,
    pub creator_fee_basis_points: u64,
    /// The seven additional protocol fee recipients (`fee_recipients[0..7]`).
    pub fee_recipients: Vec<Pubkey>,
    pub create_v2_enabled: Option<bool>,
    pub mayhem_mode_enabled: Option<bool>,
    pub is_cashback_enabled: Option<bool>,
    pub buyback_basis_points: Option<u64>,
    pub initial_virtual_quote_reserves: Option<u64>,
    pub is_holder_reward_enabled: Option<bool>,
    /// True when the account did not match the canonical layout and the fee
    /// recipient was recovered heuristically.
    pub heuristic: bool,
    pub raw_len: usize,
}

impl GlobalState {
    /// Parse the `Global` account, verifying the Anchor discriminator first.
    ///
    /// If the layout has drifted we fall back to picking the first pubkey in
    /// the account body that is neither the authority nor a program id; that is
    /// flagged with `heuristic = true` so the caller can log a loud warning.
    pub fn parse(data: &[u8]) -> BotResult<Self> {
        if data.len() < 8 {
            return Err(BotError::solana(format!(
                "global account too short: {} bytes",
                data.len()
            )));
        }
        let disc_ok = data[..8] == PUMP_ACC_DISC_GLOBAL;
        if !disc_ok {
            warn!(
                got = format!("{:02x?}", &data[..8]),
                want = format!("{:02x?}", PUMP_ACC_DISC_GLOBAL),
                "global account discriminator mismatch"
            );
        }

        // Canonical layout: needs at least through fee_basis_points (105..113).
        if data.len() >= GLOBAL_OFF_WITHDRAW_AUTHORITY {
            let initial_virtual_token_reserves =
                read_u64(data, GLOBAL_OFF_INITIAL_VIRTUAL_TOKEN_RESERVES);
            let initial_virtual_sol_reserves =
                read_u64(data, GLOBAL_OFF_INITIAL_VIRTUAL_SOL_RESERVES);
            let initial_real_token_reserves =
                read_u64(data, GLOBAL_OFF_INITIAL_REAL_TOKEN_RESERVES);
            let token_total_supply = read_u64(data, GLOBAL_OFF_TOKEN_TOTAL_SUPPLY);
            let fee_basis_points = read_u64(data, GLOBAL_OFF_FEE_BASIS_POINTS);

            // Sanity: the documented defaults are 1.073e9 tokens (6dp) and
            // 30 SOL. If they are wildly off, the layout moved and the fee
            // recipient we read cannot be trusted either.
            let sane = initial_virtual_sol_reserves > 0
                && initial_virtual_sol_reserves < 10_000 * maths::LAMPORTS_PER_SOL
                && initial_virtual_token_reserves > 0
                && token_total_supply > 0
                && fee_basis_points < maths::BPS_DENOM;
            if sane && disc_ok {
                let mut fee_recipients = Vec::with_capacity(GLOBAL_FEE_RECIPIENTS_LEN);
                if data.len() >= GLOBAL_OFF_FEE_RECIPIENTS + 32 * GLOBAL_FEE_RECIPIENTS_LEN {
                    for i in 0..GLOBAL_FEE_RECIPIENTS_LEN {
                        fee_recipients.push(read_pubkey(data, GLOBAL_OFF_FEE_RECIPIENTS + 32 * i));
                    }
                }
                return Ok(GlobalState {
                    initialized: data[GLOBAL_OFF_INITIALIZED] != 0,
                    authority: read_pubkey(data, GLOBAL_OFF_AUTHORITY),
                    fee_recipient: read_pubkey(data, GLOBAL_OFF_FEE_RECIPIENT),
                    initial_virtual_token_reserves,
                    initial_virtual_sol_reserves,
                    initial_real_token_reserves,
                    token_total_supply,
                    fee_basis_points,
                    withdraw_authority: read_pubkey(data, GLOBAL_OFF_WITHDRAW_AUTHORITY),
                    enable_migrate: data
                        .get(GLOBAL_OFF_ENABLE_MIGRATE)
                        .map(|b| *b != 0)
                        .unwrap_or(false),
                    pool_migration_fee: read_u64(data, GLOBAL_OFF_POOL_MIGRATION_FEE),
                    creator_fee_basis_points: read_u64(data, GLOBAL_OFF_CREATOR_FEE_BASIS_POINTS),
                    fee_recipients,
                    create_v2_enabled: data.get(GLOBAL_OFF_CREATE_V2_ENABLED).map(|b| *b != 0),
                    mayhem_mode_enabled: data.get(GLOBAL_OFF_MAYHEM_MODE_ENABLED).map(|b| *b != 0),
                    is_cashback_enabled: data.get(GLOBAL_OFF_IS_CASHBACK_ENABLED).map(|b| *b != 0),
                    buyback_basis_points: data
                        .get(GLOBAL_OFF_BUYBACK_BASIS_POINTS..GLOBAL_OFF_BUYBACK_BASIS_POINTS + 8)
                        .map(|_| read_u64(data, GLOBAL_OFF_BUYBACK_BASIS_POINTS)),
                    initial_virtual_quote_reserves: data
                        .get(
                            GLOBAL_OFF_INITIAL_VIRTUAL_QUOTE_RESERVES
                                ..GLOBAL_OFF_INITIAL_VIRTUAL_QUOTE_RESERVES + 8,
                        )
                        .map(|_| read_u64(data, GLOBAL_OFF_INITIAL_VIRTUAL_QUOTE_RESERVES)),
                    is_holder_reward_enabled: data
                        .get(GLOBAL_OFF_IS_HOLDER_REWARD_ENABLED)
                        .map(|b| *b != 0),
                    heuristic: false,
                    raw_len: data.len(),
                });
            }
        }

        // Heuristic fallback: scan the account body for the fee recipient.
        let authority = read_pubkey(
            data,
            GLOBAL_OFF_AUTHORITY.min(data.len().saturating_sub(32)),
        );
        for off in 9..data.len().saturating_sub(31) {
            let candidate = read_pubkey(data, off);
            if candidate == authority
                || candidate == Pubkey::default()
                || candidate == *PUMP_PROGRAM_ID
                || candidate == *SYSTEM_PROGRAM
            {
                continue;
            }
            return Ok(GlobalState {
                initialized: false,
                authority,
                fee_recipient: candidate,
                initial_virtual_token_reserves: maths::PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES,
                initial_virtual_sol_reserves: maths::PUMP_INITIAL_VIRTUAL_SOL_RESERVES,
                initial_real_token_reserves: maths::PUMP_INITIAL_REAL_TOKEN_RESERVES,
                token_total_supply: maths::PUMP_TOKEN_TOTAL_SUPPLY,
                fee_basis_points: 100,
                withdraw_authority: Pubkey::default(),
                enable_migrate: true,
                pool_migration_fee: 0,
                creator_fee_basis_points: 0,
                fee_recipients: Vec::new(),
                create_v2_enabled: None,
                mayhem_mode_enabled: None,
                is_cashback_enabled: None,
                buyback_basis_points: None,
                initial_virtual_quote_reserves: None,
                is_holder_reward_enabled: None,
                heuristic: true,
                raw_len: data.len(),
            });
        }

        Err(BotError::solana(
            "could not parse the pump Global account (layout changed?) — set an explicit fee_recipient override",
        ))
    }
}

/// Decoded `BondingCurve` account.
///
/// Two on-chain versions coexist:
///   * **v1** (83 bytes) — pre-multi-quote. Ends after `is_cashback_coin`.
///   * **v2** (125 bytes) — adds `quote_mint`, `creator_fee_bps`,
///     `can_edit_creator_fee`, `is_holder_reward`.
///
/// Offsets 8..83 are identical in both, so a SOL-quoted curve parses the same
/// either way; `quote_mint` is `None` on a v1 account, which callers must treat
/// as WSOL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BondingCurveState {
    pub virtual_token_reserves: u64,
    /// Called `virtual_quote_reserves` in the current IDL; same offset and the
    /// same value as `virtual_sol_reserves` for a SOL-quoted curve.
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
    pub creator: Pubkey,
    pub is_mayhem_mode: Option<bool>,
    /// Byte 82: decides the `sell` account layout. `None` when the account is
    /// the older 81-byte version and has not been extended yet.
    pub cashback_enabled: Option<bool>,
    /// `None` on a v1 account, which is always SOL-quoted.
    pub quote_mint: Option<Pubkey>,
    pub creator_fee_bps: Option<u64>,
    pub can_edit_creator_fee: Option<bool>,
    pub is_holder_reward: Option<bool>,
    pub raw_len: usize,
}

impl BondingCurveState {
    pub fn parse(data: &[u8]) -> BotResult<Self> {
        if data.len() < BONDING_CURVE_MIN_LEN {
            return Err(BotError::solana(format!(
                "bonding curve account too short: {} bytes (need {BONDING_CURVE_MIN_LEN})",
                data.len()
            )));
        }
        if data[..8] != PUMP_ACC_DISC_BONDING_CURVE {
            return Err(BotError::solana(format!(
                "bonding curve discriminator mismatch: got {:02x?} want {:02x?}",
                &data[..8],
                PUMP_ACC_DISC_BONDING_CURVE
            )));
        }
        let at = |off: usize| -> Option<u8> { data.get(off).copied() };
        Ok(BondingCurveState {
            virtual_token_reserves: read_u64(data, BC_OFF_VIRTUAL_TOKEN_RESERVES),
            virtual_sol_reserves: read_u64(data, BC_OFF_VIRTUAL_SOL_RESERVES),
            real_token_reserves: read_u64(data, BC_OFF_REAL_TOKEN_RESERVES),
            real_sol_reserves: read_u64(data, BC_OFF_REAL_SOL_RESERVES),
            token_total_supply: read_u64(data, BC_OFF_TOKEN_TOTAL_SUPPLY),
            complete: data[BC_OFF_COMPLETE] != 0,
            creator: read_pubkey(data, BC_OFF_CREATOR),
            is_mayhem_mode: at(BC_OFF_IS_MAYHEM_MODE).map(|b| b != 0),
            cashback_enabled: at(BC_OFF_IS_CASHBACK_COIN).map(|b| b != 0),
            quote_mint: data
                .get(BC_OFF_QUOTE_MINT..BC_OFF_QUOTE_MINT + 32)
                .map(|_| read_pubkey(data, BC_OFF_QUOTE_MINT)),
            creator_fee_bps: data
                .get(BC_OFF_CREATOR_FEE_BPS..BC_OFF_CREATOR_FEE_BPS + 8)
                .map(|_| read_u64(data, BC_OFF_CREATOR_FEE_BPS)),
            can_edit_creator_fee: at(BC_OFF_CAN_EDIT_CREATOR_FEE).map(|b| b != 0),
            is_holder_reward: at(BC_OFF_IS_HOLDER_REWARD).map(|b| b != 0),
            raw_len: data.len(),
        })
    }

    /// The quote mint, defaulting to WSOL for a v1 account.
    pub fn quote_mint_or_wsol(&self) -> Pubkey {
        self.quote_mint.unwrap_or(*WSOL_MINT)
    }

    /// True when this is the extended (v2) account layout.
    pub fn is_v2(&self) -> bool {
        self.raw_len >= BONDING_CURVE_V2_LEN
    }

    /// Spot price of one whole token, in SOL.
    pub fn spot_price_sol(&self) -> f64 {
        maths::pump_spot_price_sol(self.virtual_sol_reserves, self.virtual_token_reserves)
    }

    /// Implied market cap in SOL.
    pub fn market_cap_sol(&self) -> f64 {
        maths::pump_market_cap_sol(self.virtual_sol_reserves, self.virtual_token_reserves)
    }

    /// How many lamports it costs to buy `tokens_out`.
    pub fn buy_cost(&self, tokens_out: u64) -> Option<u64> {
        maths::pump_get_buy_cost(
            tokens_out,
            self.virtual_sol_reserves,
            self.virtual_token_reserves,
        )
    }

    /// How many tokens `sol_in` lamports buys.
    pub fn tokens_for_sol(&self, sol_in: u64) -> Option<u64> {
        maths::pump_get_tokens_for_sol(
            sol_in,
            self.virtual_sol_reserves,
            self.virtual_token_reserves,
        )
    }

    /// How many lamports selling `tokens_in` returns.
    pub fn sol_for_tokens(&self, tokens_in: u64) -> Option<u64> {
        maths::pump_get_sol_for_tokens(
            tokens_in,
            self.virtual_sol_reserves,
            self.virtual_token_reserves,
        )
    }

    /// Fraction of the real token reserve already sold (0.0 .. 1.0).
    pub fn progress(&self) -> f64 {
        if self.token_total_supply == 0 {
            return 0.0;
        }
        let sold = self
            .token_total_supply
            .saturating_sub(self.real_token_reserves);
        sold as f64 / self.token_total_supply as f64
    }

    /// True when a migration has already run: reserves are zeroed and the
    /// token trades on PumpSwap instead. Any price computed from this account
    /// would be a division by zero, so callers must check this first.
    pub fn is_graduated(&self) -> bool {
        self.complete || (self.virtual_token_reserves == 0 && self.virtual_sol_reserves == 0)
    }

    /// True when the account still needs `extend_account` before trading
    /// (accounts created before the cashback upgrade are shorter than 83 B).
    pub fn needs_extend(&self) -> bool {
        self.raw_len < BONDING_CURVE_FULL_LEN
    }
}

fn read_u64(data: &[u8], off: usize) -> u64 {
    match data.get(off..off + 8) {
        Some(b) => u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        None => 0,
    }
}

fn read_pubkey(data: &[u8], off: usize) -> Pubkey {
    match data.get(off..off + 32) {
        Some(b) => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(b);
            Pubkey::new_from_array(arr)
        }
        None => Pubkey::default(),
    }
}

// --------------------------------------------------------------------------
// Context: everything needed to build instructions for one mint
// --------------------------------------------------------------------------

/// All the accounts a pump buy/sell needs for one (mint, user) pair.
#[derive(Debug, Clone)]
pub struct PumpContext {
    pub mint: Pubkey,
    pub user: Pubkey,
    /// SPL Token or Token-2022, whichever owns `mint`.
    pub token_program: Pubkey,
    pub global: Pubkey,
    pub fee_recipient: Pubkey,
    pub bonding_curve: Pubkey,
    pub bonding_curve_v2: Pubkey,
    pub associated_bonding_curve: Pubkey,
    pub associated_user: Pubkey,
    pub creator: Pubkey,
    pub creator_vault: Pubkey,
    pub event_authority: Pubkey,
    pub global_volume_accumulator: Pubkey,
    pub user_volume_accumulator: Pubkey,
    pub fee_config: Pubkey,
    pub fee_program: Pubkey,
    /// Trailing account required by the 2026-04-28 upgrade on some program
    /// versions: one of the eight interchangeable `BREAKING_FEE_RECIPIENTS`.
    /// Picked once per context so a retry reuses the same one.
    pub trailing_fee_recipient: Pubkey,
    /// The parsed `Global` account this context was built from.
    pub global_state: GlobalState,
    /// Accounts only the v2 (multi-quote) instructions need.
    pub quote_mint: Pubkey,
    pub quote_token_program: Pubkey,
    pub associated_token_program: Pubkey,
    pub buyback_fee_recipient: Pubkey,
    pub associated_quote_fee_recipient: Pubkey,
    pub associated_quote_buyback_fee_recipient: Pubkey,
    pub associated_base_bonding_curve: Pubkey,
    pub associated_quote_bonding_curve: Pubkey,
    pub associated_base_user: Pubkey,
    pub associated_quote_user: Pubkey,
    pub associated_creator_vault: Pubkey,
    pub associated_user_volume_accumulator: Pubkey,
    pub sharing_config: Pubkey,
    /// Alias: `buy_v2` calls the base mint `base_mint` in the IDL.
    pub base_mint: Pubkey,
    pub base_token_program: Pubkey,
    pub curve: BondingCurveState,
    /// Whether the user's ATA already exists (skip the create instruction).
    pub user_ata_exists: bool,
}

impl PumpContext {
    /// Name every derivable account so a learned layout can be matched back.
    pub fn named_accounts(&self) -> HashMap<String, Pubkey> {
        let mut m = HashMap::new();
        m.insert("global".to_string(), self.global);
        m.insert("fee_recipient".to_string(), self.fee_recipient);
        m.insert("mint".to_string(), self.mint);
        m.insert("bonding_curve".to_string(), self.bonding_curve);
        m.insert("bonding_curve_v2".to_string(), self.bonding_curve_v2);
        m.insert(
            "associated_bonding_curve".to_string(),
            self.associated_bonding_curve,
        );
        m.insert("associated_user".to_string(), self.associated_user);
        m.insert("user".to_string(), self.user);
        m.insert("system_program".to_string(), system_program::id());
        m.insert("token_program".to_string(), self.token_program);
        m.insert("creator".to_string(), self.creator);
        m.insert("creator_vault".to_string(), self.creator_vault);
        m.insert("event_authority".to_string(), self.event_authority);
        m.insert("program".to_string(), *PUMP_PROGRAM_ID);
        m.insert(
            "global_volume_accumulator".to_string(),
            self.global_volume_accumulator,
        );
        m.insert(
            "user_volume_accumulator".to_string(),
            self.user_volume_accumulator,
        );
        m.insert("fee_config".to_string(), self.fee_config);
        m.insert("fee_program".to_string(), self.fee_program);
        m.insert(
            "trailing_fee_recipient".to_string(),
            self.trailing_fee_recipient,
        );
        // v2 (multi-quote) names.
        m.insert("base_mint".to_string(), self.base_mint);
        m.insert("quote_mint".to_string(), self.quote_mint);
        m.insert("base_token_program".to_string(), self.base_token_program);
        m.insert("quote_token_program".to_string(), self.quote_token_program);
        m.insert(
            "associated_token_program".to_string(),
            self.associated_token_program,
        );
        m.insert(
            "buyback_fee_recipient".to_string(),
            self.buyback_fee_recipient,
        );
        m.insert(
            "associated_quote_fee_recipient".to_string(),
            self.associated_quote_fee_recipient,
        );
        m.insert(
            "associated_quote_buyback_fee_recipient".to_string(),
            self.associated_quote_buyback_fee_recipient,
        );
        m.insert(
            "associated_base_bonding_curve".to_string(),
            self.associated_base_bonding_curve,
        );
        m.insert(
            "associated_quote_bonding_curve".to_string(),
            self.associated_quote_bonding_curve,
        );
        m.insert(
            "associated_base_user".to_string(),
            self.associated_base_user,
        );
        m.insert(
            "associated_quote_user".to_string(),
            self.associated_quote_user,
        );
        m.insert(
            "associated_creator_vault".to_string(),
            self.associated_creator_vault,
        );
        m.insert(
            "associated_user_volume_accumulator".to_string(),
            self.associated_user_volume_accumulator,
        );
        m.insert("sharing_config".to_string(), self.sharing_config);
        m.insert("account".to_string(), self.bonding_curve);
        m
    }

    /// Inverse of [`named_accounts`], used when learning a layout.
    pub fn reverse_names(&self) -> HashMap<Pubkey, String> {
        self.named_accounts()
            .into_iter()
            .map(|(k, v)| (v, k))
            .collect()
    }

    /// Fetch the bonding curve + global config and derive every account.
    ///
    /// `fee_recipient_override` lets the operator pin the fee recipient if the
    /// Global account layout has drifted and the heuristic is not trusted.
    pub async fn load(
        rpc: &Rpc,
        mint: &Pubkey,
        user: &Pubkey,
        fee_recipient_override: Option<Pubkey>,
    ) -> BotResult<Self> {
        let bonding_curve = bonding_curve_pda(mint);
        let global = *PUMP_GLOBAL;

        // The pump Global account is semi-static (protocol fees only change
        // on admin action), so it goes through the warm cache when one is
        // configured; the bonding curve is price-bearing and ALWAYS fetched
        // fresh. On a cache hit only the curve round trip remains; on a miss
        // both are batched into one `getMultipleAccounts` like before.
        let accounts = match rpc
            .account_cache()
            .get(&global, rpc.account_cache_ttl())
            .await
        {
            Some(cached_global) => {
                let curve = rpc.get_account(&bonding_curve).await?;
                vec![Some(cached_global), curve]
            }
            None => {
                let fetched = rpc.get_multiple_accounts(&[global, bonding_curve]).await?;
                if let Some(g) = fetched.first().and_then(|a| a.as_ref()) {
                    rpc.account_cache().insert(global, g.clone()).await;
                }
                fetched
            }
        };

        let global_data = accounts
            .first()
            .and_then(|a| a.as_ref())
            .map(|a| a.data.clone())
            .ok_or_else(|| BotError::solana("global account not found"))?;
        let curve_data = accounts
            .get(1)
            .and_then(|a| a.as_ref())
            .ok_or_else(|| {
                BotError::solana(format!(
                    "bonding curve {bonding_curve} not found — token may have graduated to PumpSwap"
                ))
            })?
            .data
            .clone();

        let global_state = GlobalState::parse(&global_data)?;
        if global_state.heuristic {
            tracing::warn!(
                "pump Global account did not match the canonical layout; fee_recipient was \
                 recovered heuristically as {}. Verify it, or set an explicit override.",
                global_state.fee_recipient
            );
        }
        let curve = BondingCurveState::parse(&curve_data)?;

        // Token-2022 mints use a different token program; ask the chain rather
        // than guessing, because the ATA address depends on it.
        let token_program = rpc.token_program_of(mint).await.unwrap_or(*TOKEN_PROGRAM);

        let user_ata = associated_user(mint, user, &token_program);
        // Cached: an ATA flips false→true at most once (our own buy creates
        // it), and only positive answers are ever cached.
        let user_ata_exists = rpc.account_exists_cached(&user_ata).await.unwrap_or(false);

        // Quote side. A v1 bonding curve has no quote_mint field and is always
        // SOL-quoted; a v2 curve can be quoted in a whitelisted stablecoin.
        let quote_mint = curve.quote_mint_or_wsol();
        let quote_token_program = if quote_mint == *WSOL_MINT {
            *TOKEN_PROGRAM
        } else {
            rpc.token_program_of(&quote_mint)
                .await
                .unwrap_or(*TOKEN_PROGRAM)
        };

        let fee_recipient = fee_recipient_override.unwrap_or(global_state.fee_recipient);
        // The buyback recipient defaults to the protocol fee recipient when
        // Global does not carry a buyback list yet.
        let buyback_fee_recipient = global_state
            .fee_recipients
            .first()
            .copied()
            .unwrap_or(fee_recipient);
        let creator_vault = creator_vault_pda(&curve.creator);
        let user_volume_accumulator = user_volume_accumulator_pda(user);
        let ata = |owner: &Pubkey, program: &Pubkey, m: &Pubkey| {
            spl_associated_token_account::get_associated_token_address_with_program_id(
                owner, m, program,
            )
        };

        Ok(PumpContext {
            base_mint: *mint,
            base_token_program: token_program,
            quote_mint,
            quote_token_program,
            associated_token_program: *ASSOCIATED_TOKEN_PROGRAM,
            buyback_fee_recipient,
            associated_quote_fee_recipient: ata(&fee_recipient, &quote_token_program, &quote_mint),
            associated_quote_buyback_fee_recipient: ata(
                &buyback_fee_recipient,
                &quote_token_program,
                &quote_mint,
            ),
            associated_base_bonding_curve: ata(&bonding_curve, &token_program, mint),
            associated_quote_bonding_curve: ata(&bonding_curve, &quote_token_program, &quote_mint),
            associated_base_user: ata(user, &token_program, mint),
            associated_quote_user: ata(user, &quote_token_program, &quote_mint),
            associated_creator_vault: ata(&creator_vault, &quote_token_program, &quote_mint),
            associated_user_volume_accumulator: ata(
                &user_volume_accumulator,
                &quote_token_program,
                &quote_mint,
            ),
            sharing_config: sharing_config_pda(mint),
            global_state: global_state.clone(),
            mint: *mint,
            user: *user,
            token_program,
            global,
            fee_recipient,
            bonding_curve,
            bonding_curve_v2: bonding_curve_v2_pda(mint),
            associated_bonding_curve: associated_bonding_curve(mint, &token_program),
            associated_user: user_ata,
            creator: curve.creator,
            creator_vault,
            event_authority: event_authority_pda(),
            global_volume_accumulator: global_volume_accumulator_pda(),
            user_volume_accumulator,
            fee_config: fee_config_pda(),
            fee_program: *PUMP_FEES_PROGRAM_ID,
            trailing_fee_recipient: pick_breaking_fee_recipient(),
            curve,
            user_ata_exists,
        })
    }
}

// --------------------------------------------------------------------------
// Default (code-derived) layouts
// --------------------------------------------------------------------------

/// The `buy` / `buy_exact_sol_in` layout, straight from the official IDL
/// (16 accounts).
///
/// IMPORTANT: third-party write-ups disagree with the official IDL about the
/// trailing accounts. The IDL shipped by pump-fun lists exactly these 16, with
/// no `bonding-curve-v2` and no extra fee recipient; a widely-circulated blog
/// post documents 17 with `bonding-curve-v2` appended, and the 2026-04-28
/// upgrade note documents 18 with a trailing mutable fee recipient. All three
/// have been live at some point.
///
/// Rather than guessing, [`default_buy_layout`] returns the IDL form and
/// [`buy_layout_variants`] enumerates every candidate in the order they should
/// be tried. `PumpLayoutDoctor` simulates each one and persists the winner in
/// the [`LayoutStore`], so the bot converges on whatever the program wants
/// today without a code change.
pub fn default_buy_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*PUMP_PROGRAM_ID, "buy", Vec::new());
    l.push_named("global", false, false);
    l.push_named("fee_recipient", true, false);
    l.push_named("mint", false, false);
    l.push_named("bonding_curve", true, false);
    l.push_named("associated_bonding_curve", true, false);
    l.push_named("associated_user", true, false);
    l.push_named("user", true, true);
    l.push_named("system_program", false, false);
    l.push_named("token_program", false, false);
    l.push_named("creator_vault", true, false);
    l.push_named("event_authority", false, false);
    l.push_named("program", false, false);
    l.push_named("global_volume_accumulator", false, false);
    l.push_named("user_volume_accumulator", true, false);
    l.push_named("fee_config", false, false);
    l.push_named("fee_program", false, false);
    l
}

/// The `sell` layout from the official IDL (14 accounts).
///
/// Note the IDL ordering: `system_program` at 7, then `creator_vault` at 8 and
/// `token_program` at 9 — the reverse of the buy layout, which is an easy
/// mistake to make by symmetry.
pub fn default_sell_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*PUMP_PROGRAM_ID, "sell", Vec::new());
    l.push_named("global", false, false);
    l.push_named("fee_recipient", true, false);
    l.push_named("mint", false, false);
    l.push_named("bonding_curve", true, false);
    l.push_named("associated_bonding_curve", true, false);
    l.push_named("associated_user", true, false);
    l.push_named("user", true, true);
    l.push_named("system_program", false, false);
    l.push_named("creator_vault", true, false);
    l.push_named("token_program", false, false);
    l.push_named("event_authority", false, false);
    l.push_named("program", false, false);
    l.push_named("fee_config", false, false);
    l.push_named("fee_program", false, false);
    l
}

/// The `buy_v2` layout: 27 accounts, multi-quote aware, from the official IDL.
pub fn default_buy_v2_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*PUMP_PROGRAM_ID, "buy_v2", Vec::new());
    l.push_named("global", false, false);
    l.push_named("base_mint", false, false);
    l.push_named("quote_mint", false, false);
    l.push_named("base_token_program", false, false);
    l.push_named("quote_token_program", false, false);
    l.push_named("associated_token_program", false, false);
    l.push_named("fee_recipient", true, false);
    l.push_named("associated_quote_fee_recipient", true, false);
    l.push_named("buyback_fee_recipient", true, false);
    l.push_named("associated_quote_buyback_fee_recipient", true, false);
    l.push_named("bonding_curve", true, false);
    l.push_named("associated_base_bonding_curve", true, false);
    l.push_named("associated_quote_bonding_curve", true, false);
    l.push_named("user", true, true);
    l.push_named("associated_base_user", true, false);
    l.push_named("associated_quote_user", true, false);
    l.push_named("creator_vault", true, false);
    l.push_named("associated_creator_vault", true, false);
    l.push_named("sharing_config", false, false);
    l.push_named("global_volume_accumulator", false, false);
    l.push_named("user_volume_accumulator", true, false);
    l.push_named("associated_user_volume_accumulator", true, false);
    l.push_named("fee_config", false, false);
    l.push_named("fee_program", false, false);
    l.push_named("system_program", false, false);
    l.push_named("event_authority", false, false);
    l.push_named("program", false, false);
    l
}

/// The `sell_v2` layout: 26 accounts (no `global_volume_accumulator`).
pub fn default_sell_v2_layout() -> AccountLayout {
    let mut l = default_buy_v2_layout();
    l.instruction = "sell_v2".to_string();
    // sell_v2 drops `global_volume_accumulator` (index 19 in buy_v2).
    let pos = l
        .slots
        .iter()
        .position(|s| matches!(s, Slot::Named { name, .. } if name == "global_volume_accumulator"));
    if let Some(i) = pos {
        l.slots.remove(i);
    }
    l
}

/// Every candidate trailing-account shape for `buy`, in the order they should
/// be probed. The IDL form comes first because it is authoritative today.
pub fn buy_layout_variants() -> Vec<AccountLayout> {
    let base = default_buy_layout();

    // Variant B: + bonding-curve-v2 (the cashback upgrade shape, 17 accounts).
    let mut b = base.clone();
    b.push_named("bonding_curve_v2", false, false);

    // Variant C: + bonding-curve-v2 + mutable trailing fee recipient (the
    // 2026-04-28 shape, 18 accounts).
    let mut c = b.clone();
    c.push_named("trailing_fee_recipient", true, false);

    // Variant D: + mutable trailing fee recipient only (17 accounts).
    let mut d = base.clone();
    d.push_named("trailing_fee_recipient", true, false);

    vec![base, b, c, d]
}

/// Every candidate trailing-account shape for `sell`.
pub fn sell_layout_variants() -> Vec<AccountLayout> {
    let base = default_sell_layout();

    let mut b = base.clone();
    b.push_named("bonding_curve_v2", false, false);

    let mut c = b.clone();
    c.push_named("trailing_fee_recipient", true, false);

    let mut d = base.clone();
    d.push_named("trailing_fee_recipient", true, false);

    vec![base, b, c, d]
}

/// `extend_account` — 5 accounts on both pump programs.
pub fn extend_account_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*PUMP_PROGRAM_ID, "extend_account", Vec::new());
    l.push_named("account", true, false);
    l.push_named("user", true, true);
    l.push_named("system_program", false, false);
    l.push_named("event_authority", false, false);
    l.push_named("program", false, false);
    l
}

/// Resolve the effective layout: a learned/trusted template wins, otherwise the
/// code-derived default plus any operator-supplied extra accounts.
fn effective_layout(
    store: &LayoutStore,
    default: AccountLayout,
    opts: &BuildOptions,
) -> AccountLayout {
    if let Some(learned) = store.get(&PUMP_PROGRAM_ID, &default.instruction) {
        if learned.trusted {
            debug!(
                instruction = %learned.instruction,
                accounts = learned.len(),
                "using learned account layout"
            );
            return learned.clone();
        }
    }

    let mut layout = default;
    if !opts.append_bonding_curve_v2 {
        layout
            .slots
            .retain(|s| !matches!(s, Slot::Named { name, .. } if name == "bonding_curve_v2"));
    }
    if !opts.append_trailing_fee_recipient {
        layout
            .slots
            .retain(|s| !matches!(s, Slot::Named { name, .. } if name == "trailing_fee_recipient"));
    }
    if opts.variant_extra_accounts {
        // Probe order is handled by PumpLayoutDoctor; nothing to do here.
    }
    for extra in &opts.extra_accounts {
        match Pubkey::try_from(extra.trim()) {
            Ok(pk) => layout.push_fixed(pk, false, false),
            Err(e) => {
                tracing::warn!(extra, error = %e, "ignoring unparsable pump_extra_accounts entry")
            }
        }
    }
    layout
}

/// Options that change how the instruction is built.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Extra literal accounts appended after the derived ones.
    pub extra_accounts: Vec<String>,
    /// Set false to strip a `bonding-curve-v2` slot from the chosen layout.
    pub append_bonding_curve_v2: bool,
    /// Set false to strip a `trailing_fee_recipient` slot from the layout.
    pub append_trailing_fee_recipient: bool,
    /// Reserved: kept so `effective_layout` has one place to extend.
    pub variant_extra_accounts: bool,
    /// Use `buy_exact_sol_in` (pins SOL, floors tokens) instead of `buy`
    /// (pins tokens, ceilings SOL). Mainnet traffic mostly uses the former.
    pub exact_sol_in: bool,
    /// Set the `track_volume` OptionBool argument on `buy`.
    pub track_volume: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            extra_accounts: Vec::new(),
            append_bonding_curve_v2: true,
            append_trailing_fee_recipient: true,
            variant_extra_accounts: false,
            exact_sol_in: false,
            track_volume: false,
        }
    }
}

/// Build a bonding-curve **buy** instruction.
///
/// `amount` is tokens out and `max_sol_cost` is the lamport ceiling; for
/// `exact_sol_in` they are `sol_in` and `min_tokens_out` respectively.
pub fn build_buy_ix(
    ctx: &PumpContext,
    store: &LayoutStore,
    opts: &BuildOptions,
    amount: u64,
    max_sol_cost: u64,
) -> BotResult<Instruction> {
    let discriminator = if opts.exact_sol_in {
        PUMP_DISC_BUY_EXACT_SOL_IN
    } else {
        PUMP_DISC_BUY
    };

    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&discriminator);
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&max_sol_cost.to_le_bytes());
    if !opts.exact_sol_in {
        // `track_volume: OptionBool` — a single byte.
        data.push(if opts.track_volume { 1 } else { 0 });
    }

    let default = default_buy_layout();
    let layout = effective_layout(store, default, opts);
    let accounts = layout.build(&ctx.named_accounts())?;

    Ok(Instruction {
        program_id: *PUMP_PROGRAM_ID,
        accounts,
        data,
    })
}

/// Build a bonding-curve **sell** instruction.
pub fn build_sell_ix(
    ctx: &PumpContext,
    store: &LayoutStore,
    opts: &BuildOptions,
    amount: u64,
    min_sol_output: u64,
) -> BotResult<Instruction> {
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMP_DISC_SELL);
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&min_sol_output.to_le_bytes());

    let default = default_sell_layout();
    let layout = effective_layout(store, default, opts);
    let accounts = layout.build(&ctx.named_accounts())?;

    debug!(
        mint = %ctx.mint,
        cashback = ?ctx.curve.cashback_enabled,
        accounts = accounts.len(),
        "built pump sell instruction"
    );

    Ok(Instruction {
        program_id: *PUMP_PROGRAM_ID,
        accounts,
        data,
    })
}

/// `extend_account` — required once for bonding curves created before the
/// cashback upgrade (account shorter than 83 bytes). Five accounts per the IDL:
/// the target account, the payer, the system program, the event authority and
/// the program itself.
pub fn build_extend_account_ix(ctx: &PumpContext) -> Instruction {
    Instruction {
        program_id: *PUMP_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(ctx.bonding_curve, false),
            AccountMeta::new(ctx.user, true),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(ctx.event_authority, false),
            AccountMeta::new_readonly(*PUMP_PROGRAM_ID, false),
        ],
        data: PUMP_DISC_EXTEND_ACCOUNT.to_vec(),
    }
}

// --------------------------------------------------------------------------
// High-level helpers used by Module 1
// --------------------------------------------------------------------------

/// Convert a SOL budget into `(tokens_out, max_sol_cost)` for a `buy`.
///
/// The bonding curve has a 1% protocol fee, which is added on top of the
/// curve cost before slippage is applied — matching how the reference clients
/// compute `max_sol_cost`.
pub fn plan_buy(
    curve: &BondingCurveState,
    sol_in: u64,
    slippage_pct: f64,
    fee_bps: u64,
) -> BotResult<(u64, u64)> {
    if sol_in == 0 {
        return Err(BotError::invalid("sol_in must be > 0"));
    }
    if curve.complete {
        return Err(BotError::solana(
            "bonding curve is complete — token has graduated, use PumpSwap/Raydium",
        ));
    }
    // Deduct the protocol fee from the SOL budget to get the curve amount.
    let fee = sol_in
        .checked_mul(fee_bps)
        .ok_or_else(|| BotError::other("fee overflow"))?
        / maths::BPS_DENOM;
    let sol_for_curve = sol_in.saturating_sub(fee);
    if sol_for_curve == 0 {
        return Err(BotError::invalid(
            "sol_in is smaller than the protocol fee on its own",
        ));
    }

    let tokens_out = curve
        .tokens_for_sol(sol_for_curve)
        .ok_or_else(|| BotError::solana("bonding curve overflow computing tokens out"))?;
    if tokens_out == 0 {
        return Err(BotError::solana(
            "computed tokens_out = 0 — buy size too small for this curve",
        ));
    }
    if tokens_out > curve.real_token_reserves {
        return Err(BotError::solana(format!(
            "not enough tokens left on the curve: {} available, {tokens_out} requested",
            curve.real_token_reserves
        )));
    }

    // Re-derive the cost from the token amount and add the fee + slippage.
    let curve_cost = curve
        .buy_cost(tokens_out)
        .ok_or_else(|| BotError::solana("bonding curve overflow computing buy cost"))?;
    let with_fee = curve_cost.saturating_add(curve_cost * fee_bps / maths::BPS_DENOM);
    let max_sol_cost = maths::apply_pct_u64(with_fee, slippage_pct);

    Ok((tokens_out, max_sol_cost))
}

/// Convert a token amount into `(amount, min_sol_output)` for a `sell`.
pub fn plan_sell(
    curve: &BondingCurveState,
    tokens_in: u64,
    slippage_pct: f64,
) -> BotResult<(u64, u64)> {
    if tokens_in == 0 {
        return Err(BotError::invalid("tokens_in must be > 0"));
    }
    let expected_sol = curve
        .sol_for_tokens(tokens_in)
        .ok_or_else(|| BotError::solana("bonding curve overflow computing sell output"))?;
    let min_sol_output = maths::minus_pct_u64(expected_sol, slippage_pct);
    Ok((tokens_in, min_sol_output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve_data(cashback: bool, len: usize) -> Vec<u8> {
        let mut d = vec![0u8; len.max(BONDING_CURVE_MIN_LEN)];
        d[..8].copy_from_slice(&PUMP_ACC_DISC_BONDING_CURVE);
        d[BC_OFF_VIRTUAL_TOKEN_RESERVES..BC_OFF_VIRTUAL_TOKEN_RESERVES + 8]
            .copy_from_slice(&maths::PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES.to_le_bytes());
        d[BC_OFF_VIRTUAL_SOL_RESERVES..BC_OFF_VIRTUAL_SOL_RESERVES + 8]
            .copy_from_slice(&maths::PUMP_INITIAL_VIRTUAL_SOL_RESERVES.to_le_bytes());
        d[BC_OFF_REAL_TOKEN_RESERVES..BC_OFF_REAL_TOKEN_RESERVES + 8]
            .copy_from_slice(&maths::PUMP_INITIAL_REAL_TOKEN_RESERVES.to_le_bytes());
        d[BC_OFF_TOKEN_TOTAL_SUPPLY..BC_OFF_TOKEN_TOTAL_SUPPLY + 8]
            .copy_from_slice(&maths::PUMP_TOKEN_TOTAL_SUPPLY.to_le_bytes());
        d[BC_OFF_CREATOR..BC_OFF_CREATOR + 32].copy_from_slice(&[7u8; 32]);
        if len > BC_OFF_CASHBACK_ENABLED {
            d[BC_OFF_CASHBACK_ENABLED] = if cashback { 1 } else { 0 };
        }
        d
    }

    #[test]
    fn parses_bonding_curve_fields() {
        let c = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_FULL_LEN)).unwrap();
        assert_eq!(
            c.virtual_sol_reserves,
            maths::PUMP_INITIAL_VIRTUAL_SOL_RESERVES
        );
        assert_eq!(
            c.virtual_token_reserves,
            maths::PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES
        );
        assert_eq!(c.cashback_enabled, Some(false));
        assert!(!c.complete);
        assert!(!c.needs_extend());
        assert!(c.spot_price_sol() > 0.0);
        assert!(c.market_cap_sol() > 0.0);
    }

    #[test]
    fn cashback_flag_is_read_from_byte_82() {
        let on = BondingCurveState::parse(&curve_data(true, BONDING_CURVE_FULL_LEN)).unwrap();
        let off = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_FULL_LEN)).unwrap();
        assert_eq!(on.cashback_enabled, Some(true));
        assert_eq!(off.cashback_enabled, Some(false));
        // The pre-upgrade 81-byte account has no flag at all.
        let old = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_MIN_LEN)).unwrap();
        assert_eq!(old.cashback_enabled, None);
        assert!(old.needs_extend());
    }

    #[test]
    fn wrong_discriminator_is_rejected() {
        let mut d = curve_data(false, BONDING_CURVE_FULL_LEN);
        d[0] ^= 0xff;
        assert!(BondingCurveState::parse(&d).is_err());
    }

    #[test]
    fn short_account_is_rejected() {
        assert!(BondingCurveState::parse(&[0u8; 40]).is_err());
    }

    #[test]
    fn global_parses_canonical_layout() {
        // Build a canonical Global account: disc + initialized + authority + fee_recipient + reserves.
        let mut d = vec![0u8; 160];
        d[..8].copy_from_slice(&PUMP_ACC_DISC_GLOBAL);
        d[GLOBAL_OFF_INITIALIZED] = 1;
        d[GLOBAL_OFF_AUTHORITY..GLOBAL_OFF_AUTHORITY + 32].copy_from_slice(&[1u8; 32]);
        d[GLOBAL_OFF_FEE_RECIPIENT..GLOBAL_OFF_FEE_RECIPIENT + 32].copy_from_slice(&[2u8; 32]);
        let o = GLOBAL_OFF_INITIAL_VIRTUAL_TOKEN_RESERVES;
        d[o..o + 8].copy_from_slice(&maths::PUMP_INITIAL_VIRTUAL_TOKEN_RESERVES.to_le_bytes());
        d[o + 8..o + 16].copy_from_slice(&(30u64 * 1_000_000_000).to_le_bytes());
        d[o + 16..o + 24].copy_from_slice(&maths::PUMP_INITIAL_REAL_TOKEN_RESERVES.to_le_bytes());
        d[o + 24..o + 32].copy_from_slice(&maths::PUMP_TOKEN_TOTAL_SUPPLY.to_le_bytes());
        d[o + 32..o + 40].copy_from_slice(&100u64.to_le_bytes());

        let g = GlobalState::parse(&d).unwrap();
        assert!(!g.heuristic);
        assert_eq!(g.fee_recipient, Pubkey::new_from_array([2u8; 32]));
        assert_eq!(g.fee_basis_points, 100);
        assert_eq!(g.initial_virtual_sol_reserves, 30 * 1_000_000_000);
    }

    #[test]
    fn buy_layout_has_sixteen_accounts_in_order() {
        // The official IDL shape: 16 accounts, ending at fee_program. The
        // cashback `bonding_curve_v2` account is a *variant* (see
        // buy_layout_variants), not part of the IDL default.
        let l = default_buy_layout();
        assert_eq!(l.len(), 16, "buy must have 16 accounts");
        let names: Vec<&str> = l.slots.iter().map(|s| s.name()).collect();
        assert_eq!(names[0], "global");
        assert_eq!(names[1], "fee_recipient");
        assert_eq!(names[6], "user");
        assert_eq!(names[12], "global_volume_accumulator");
        assert_eq!(names[13], "user_volume_accumulator");
        assert_eq!(names[14], "fee_config");
        assert_eq!(names[15], "fee_program");
        assert!(l.slots.iter().all(|s| s.name() != "bonding_curve_v2"));
        assert!(l.slots[1].writable() && !l.slots[0].writable());
        assert!(l.slots[6].signer());
    }

    #[test]
    fn sell_layout_matches_the_idl_account_count() {
        let l = default_sell_layout();
        assert_eq!(l.len(), 14);
        // IDL ordering: system_program@7, creator_vault@8, token_program@9.
        assert_eq!(l.slots[7].name(), "system_program");
        assert_eq!(l.slots[8].name(), "creator_vault");
        assert_eq!(l.slots[9].name(), "token_program");
        assert_eq!(l.slots.last().unwrap().name(), "fee_program");
    }

    #[test]
    fn sell_variants_add_the_cashback_and_fee_shapes() {
        let v = sell_layout_variants();
        let lens: Vec<usize> = v.iter().map(|l| l.len()).collect();
        // base(14), +bonding_curve_v2(15), +v2+trailing(16), +trailing(15)
        assert_eq!(lens, vec![14, 15, 16, 15]);
        // Only the variants that append it carry `bonding_curve_v2`.
        assert!(v[0].slots.iter().all(|s| s.name() != "bonding_curve_v2"));
        assert!(v[1].slots.iter().any(|s| s.name() == "bonding_curve_v2"));
        assert!(v[2]
            .slots
            .iter()
            .any(|s| s.name() == "trailing_fee_recipient"));
    }

    #[test]
    fn bonding_curve_v2_can_be_disabled() {
        // Variant 1 is base + bonding_curve_v2 (17 accounts for buy).
        let with_v2 = buy_layout_variants().remove(1);
        assert!(with_v2.slots.iter().any(|s| s.name() == "bonding_curve_v2"));
        let opts = BuildOptions {
            append_bonding_curve_v2: false,
            ..Default::default()
        };
        let l = effective_layout(&LayoutStore::default(), with_v2, &opts);
        assert_eq!(l.len(), 16);
        assert!(l.slots.iter().all(|s| s.name() != "bonding_curve_v2"));
    }

    #[test]
    fn extra_accounts_are_appended_last() {
        let extra = Pubkey::new_unique();
        let opts = BuildOptions {
            extra_accounts: vec![extra.to_string(), "not-a-pubkey!!".to_string()],
            ..Default::default()
        };
        let l = effective_layout(&LayoutStore::default(), default_buy_layout(), &opts);
        // default_buy_layout = 16, +1 valid extra (the invalid one is skipped).
        assert_eq!(l.len(), 17);
        assert!(matches!(&l.slots[16], Slot::Fixed { pubkey, .. } if pubkey == &extra.to_string()));
    }

    #[test]
    fn a_trusted_learned_layout_wins_over_the_default() {
        let mut store = LayoutStore::default();
        let mut learned = AccountLayout::new(*PUMP_PROGRAM_ID, "buy", Vec::new());
        learned.push_named("mint", false, false);
        learned.trusted = true;
        store.insert(learned);

        let l = effective_layout(&store, default_buy_layout(), &BuildOptions::default());
        assert_eq!(l.len(), 1, "the learned template must replace the default");
        assert!(l.trusted);
    }

    #[test]
    fn plan_buy_adds_fee_then_slippage() {
        let c = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_FULL_LEN)).unwrap();
        let sol_in = 100_000_000; // 0.1 SOL
        let (tokens, max_cost) = plan_buy(&c, sol_in, 10.0, 100).unwrap();
        assert!(tokens > 0);
        // max_sol_cost must exceed the raw input because of fee + slippage.
        assert!(
            max_cost > sol_in,
            "max_cost {max_cost} should exceed sol_in {sol_in}"
        );
        // ... but not by more than fee+slippage+rounding.
        assert!(
            (max_cost as f64) < (sol_in as f64) * 1.12,
            "max_cost {max_cost} is too far above sol_in {sol_in}"
        );
        // Buying more tokens must cost more.
        let (_, max_cost_2x) = plan_buy(&c, sol_in * 2, 10.0, 100).unwrap();
        assert!(max_cost_2x > max_cost);
    }

    #[test]
    fn plan_buy_rejects_zero_and_complete_curves() {
        let mut c = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_FULL_LEN)).unwrap();
        assert!(plan_buy(&c, 0, 10.0, 100).is_err());
        c.complete = true;
        assert!(plan_buy(&c, 100_000_000, 10.0, 100).is_err());
    }

    #[test]
    fn plan_buy_rejects_more_than_the_curve_holds() {
        let mut c = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_FULL_LEN)).unwrap();
        // Drain the curve so any meaningful buy exceeds the remaining reserve.
        c.real_token_reserves = 1;
        assert!(plan_buy(&c, 1_000_000_000, 10.0, 100).is_err());
    }

    #[test]
    fn plan_sell_applies_slippage_downwards() {
        let c = BondingCurveState::parse(&curve_data(false, BONDING_CURVE_FULL_LEN)).unwrap();
        let tokens = 1_000_000_000_000;
        let (amount, min_out) = plan_sell(&c, tokens, 10.0).unwrap();
        assert_eq!(amount, tokens);
        let expected = c.sol_for_tokens(tokens).unwrap();
        assert!(
            min_out < expected,
            "min_out must sit below the expected output"
        );
        assert!((min_out as f64 - expected as f64 * 0.9).abs() < 2.0);
        assert!(plan_sell(&c, 0, 10.0).is_err());
    }
}
