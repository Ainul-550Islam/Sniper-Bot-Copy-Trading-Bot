//! Raydium AMM v4 (OpenBook-backed constant product) pool parsing and swaps.
//!
//! Offsets and the instruction account list are taken from the program source
//! (`raydium-io/raydium-amm`, `program/src/state.rs` + `instruction.rs`), not
//! from third-party re-implementations — the two disagree in places.
//!
//! ## Which swap instruction to use
//!
//! * `SwapBaseInV2` (tag 16) — 8 accounts, no orderbook. This is what the
//!   Raydium UI sends and what you want: fewer accounts means a smaller
//!   transaction and fewer compute units.
//! * `SwapBaseIn` (tag 9) — 17 accounts, includes the OpenBook market accounts.
//!   Still required by older pools whose orderbook has not been retired.
//!
//! [`RaydiumPool::swap_base_in`] picks V2 when `prefer_v2` is set and falls
//! back to the full form otherwise. Because a wrong choice fails with
//! `IncorrectAccount` rather than something descriptive, [`RaydiumPool::doctor`]
//! exists to validate a pool against the chain before it is traded.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::maths;

use crate::consts::*;
use crate::layout::{AccountLayout, LayoutStore};
use crate::rpc::Rpc;

/// The parsed `AmmInfo` pool-state account (784 bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmmInfo {
    pub status: u64,
    pub nonce: u64,
    pub coin_decimals: u64,
    pub pc_decimals: u64,
    pub state: u64,
    pub coin_lot_size: u64,
    pub pc_lot_size: u64,
    /// Trade fee as a fraction: `trade_fee_numerator / trade_fee_denominator`.
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub pool_open_time: u64,
    pub coin_vault: Pubkey,
    pub pc_vault: Pubkey,
    pub coin_mint: Pubkey,
    pub pc_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub open_orders: Pubkey,
    pub market: Pubkey,
    pub market_program: Pubkey,
    pub target_orders: Pubkey,
    pub amm_owner: Pubkey,
    pub lp_amount: u64,
    /// Total bytes the account was parsed from; used by the doctor.
    pub data_len: usize,
}

impl AmmInfo {
    pub fn parse(data: &[u8]) -> BotResult<Self> {
        if data.len() < AMM_LEN {
            return Err(BotError::encoding(format!(
                "amm account is {} bytes, expected {AMM_LEN}",
                data.len()
            )));
        }
        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(b)
        };
        let u128_at = |off: usize| -> u128 {
            let mut b = [0u8; 16];
            b.copy_from_slice(&data[off..off + 16]);
            u128::from_le_bytes(b)
        };
        let pk_at = |off: usize| -> Pubkey {
            let mut b = [0u8; 32];
            b.copy_from_slice(&data[off..off + 32]);
            Pubkey::new_from_array(b)
        };
        let _ = u128_at; // swap accumulators are deprecated upstream; not read

        Ok(AmmInfo {
            status: u64_at(AMM_OFF_STATUS),
            nonce: u64_at(AMM_OFF_NONCE),
            coin_decimals: u64_at(AMM_OFF_COIN_DECIMALS),
            pc_decimals: u64_at(AMM_OFF_PC_DECIMALS),
            state: u64_at(AMM_OFF_STATE),
            coin_lot_size: u64_at(AMM_OFF_COIN_LOT_SIZE),
            pc_lot_size: u64_at(AMM_OFF_PC_LOT_SIZE),
            trade_fee_numerator: u64_at(AMM_OFF_TRADE_FEE_NUMERATOR),
            trade_fee_denominator: u64_at(AMM_OFF_TRADE_FEE_DENOMINATOR),
            pool_open_time: u64_at(AMM_OFF_POOL_OPEN_TIME),
            coin_vault: pk_at(AMM_OFF_COIN_VAULT),
            pc_vault: pk_at(AMM_OFF_PC_VAULT),
            coin_mint: pk_at(AMM_OFF_COIN_MINT),
            pc_mint: pk_at(AMM_OFF_PC_MINT),
            lp_mint: pk_at(AMM_OFF_LP_MINT),
            open_orders: pk_at(AMM_OFF_OPEN_ORDERS),
            market: pk_at(AMM_OFF_MARKET),
            market_program: pk_at(AMM_OFF_MARKET_PROGRAM),
            target_orders: pk_at(AMM_OFF_TARGET_ORDERS),
            amm_owner: pk_at(AMM_OFF_AMM_OWNER),
            lp_amount: u64_at(AMM_OFF_LP_AMOUNT),
            data_len: data.len(),
        })
    }

    pub fn is_swappable(&self) -> bool {
        amm_status_allows_swap(self.status)
    }

    /// `true` when the quote side is wrapped SOL.
    pub fn is_sol_quote(&self) -> bool {
        self.pc_mint == *WSOL_MINT
    }

    /// Which side of the pool `mint` is on.
    pub fn side_of(&self, mint: &Pubkey) -> Option<PoolSide> {
        if *mint == self.coin_mint {
            Some(PoolSide::Coin)
        } else if *mint == self.pc_mint {
            Some(PoolSide::Pc)
        } else {
            None
        }
    }

    pub fn trade_fee_bps(&self) -> u64 {
        if self.trade_fee_denominator == 0 {
            return 0;
        }
        self.trade_fee_numerator.saturating_mul(maths::BPS_DENOM) / self.trade_fee_denominator
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolSide {
    /// The base token side.
    Coin,
    /// The quote side (WSOL for SOL pairs).
    Pc,
}

/// The OpenBook/Serum `MarketState` v3 account backing a pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketStateV3 {
    pub own_address: Pubkey,
    pub vault_signer_nonce: u64,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub request_queue: Pubkey,
    pub event_queue: Pubkey,
    pub bids: Pubkey,
    pub asks: Pubkey,
    pub base_deposits_total: u64,
    pub quote_deposits_total: u64,
}

impl MarketStateV3 {
    pub fn parse(data: &[u8]) -> BotResult<Self> {
        if data.len() < MARKET_V3_LEN {
            return Err(BotError::encoding(format!(
                "market account is {} bytes, expected {MARKET_V3_LEN}",
                data.len()
            )));
        }
        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(b)
        };
        let pk_at = |off: usize| -> Pubkey {
            let mut b = [0u8; 32];
            b.copy_from_slice(&data[off..off + 32]);
            Pubkey::new_from_array(b)
        };
        Ok(MarketStateV3 {
            own_address: pk_at(MARKET_OFF_OWN_ADDRESS),
            vault_signer_nonce: u64_at(MARKET_OFF_VAULT_SIGNER_NONCE),
            base_mint: pk_at(MARKET_OFF_BASE_MINT),
            quote_mint: pk_at(MARKET_OFF_QUOTE_MINT),
            base_vault: pk_at(MARKET_OFF_BASE_VAULT),
            quote_vault: pk_at(MARKET_OFF_QUOTE_VAULT),
            request_queue: pk_at(MARKET_OFF_REQUEST_QUEUE),
            event_queue: pk_at(MARKET_OFF_EVENT_QUEUE),
            bids: pk_at(MARKET_OFF_BIDS),
            asks: pk_at(MARKET_OFF_ASKS),
            base_deposits_total: u64_at(MARKET_OFF_BASE_DEPOSITS_TOTAL),
            quote_deposits_total: u64_at(MARKET_OFF_QUOTE_DEPOSITS_TOTAL),
        })
    }

    /// The market's vault signer PDA, needed as a readonly account on the
    /// 17-account swap form.
    pub fn vault_signer(&self, market_program: &Pubkey) -> BotResult<Pubkey> {
        let nonce = self.vault_signer_nonce;
        let seeds: &[&[u8]] = &[self.own_address.as_ref(), &nonce.to_le_bytes()];
        Pubkey::create_program_address(seeds, market_program)
            .map_err(|e| BotError::solana(format!("market vault signer: {e}")))
    }
}

/// Everything needed to build a swap against one pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaydiumPool {
    pub amm_id: Pubkey,
    pub amm: AmmInfo,
    pub market: Option<MarketStateV3>,
    /// `create_program_address(&[b"amm authority", &[nonce]])`.
    pub amm_authority: Pubkey,
    pub market_vault_signer: Option<Pubkey>,
    /// Base/quote reserves as reported by the vault token accounts.
    pub coin_vault_balance: u64,
    pub pc_vault_balance: u64,
}

impl RaydiumPool {
    /// Load a pool by its AMM id, fetching the pool state, the OpenBook market
    /// and both vault balances in as few round trips as possible.
    ///
    /// Set `with_market` false when you only intend to use `SwapBaseInV2`.
    pub async fn load(rpc: &Rpc, amm_id: &Pubkey, with_market: bool) -> BotResult<Self> {
        let amm_account = rpc
            .get_account(amm_id)
            .await?
            .ok_or_else(|| BotError::NotFound(format!("raydium pool {amm_id} does not exist")))?;
        if amm_account.owner != *RAYDIUM_AMM_V4 {
            return Err(BotError::solana(format!(
                "{amm_id} is owned by {}, not the Raydium AMM v4 program",
                amm_account.owner
            )));
        }
        let amm = AmmInfo::parse(&amm_account.data)?;

        let amm_authority = amm_authority_pda(amm.nonce)?;

        let mut market = None;
        let mut market_vault_signer = None;
        let mut coin_vault_balance = 0;
        let mut pc_vault_balance = 0;

        if with_market {
            let accounts = rpc
                .get_multiple_accounts(&[amm.market, amm.coin_vault, amm.pc_vault])
                .await?;
            if let Some(market_account) = accounts.first().and_then(|a| a.as_ref()) {
                let parsed = MarketStateV3::parse(&market_account.data)?;
                market_vault_signer =
                    Some(parsed.vault_signer(&amm.market_program).map_err(|e| {
                        BotError::solana(format!(
                            "pool {amm_id} market vault signer: {e} — the market_program field \
                             at offset {AMM_OFF_MARKET_PROGRAM} may be misaligned for this pool"
                        ))
                    })?);
                market = Some(parsed);
            }
            coin_vault_balance = accounts
                .get(1)
                .and_then(|a| a.as_ref())
                .and_then(|a| token_account_amount(&a.data))
                .unwrap_or(0);
            pc_vault_balance = accounts
                .get(2)
                .and_then(|a| a.as_ref())
                .and_then(|a| token_account_amount(&a.data))
                .unwrap_or(0);
        }

        Ok(RaydiumPool {
            amm_id: *amm_id,
            amm,
            market,
            amm_authority,
            market_vault_signer,
            coin_vault_balance,
            pc_vault_balance,
        })
    }

    /// Reserves oriented as (input, output) for a trade that spends `mint`.
    ///
    /// The vault balances are the tradable reserves; the OpenBook orderbook
    /// holds the rest, which is why a pool can quote better than its vaults
    /// alone suggest. We use the vaults because they are always available and
    /// because a sniper needs a conservative estimate, not an optimistic one.
    pub fn reserves_for(&self, spend_mint: &Pubkey) -> BotResult<(u64, u64)> {
        match self.amm.side_of(spend_mint) {
            Some(PoolSide::Coin) => Ok((self.coin_vault_balance, self.pc_vault_balance)),
            Some(PoolSide::Pc) => Ok((self.pc_vault_balance, self.coin_vault_balance)),
            None => Err(BotError::invalid(format!(
                "{spend_mint} is neither the coin mint ({}) nor the pc mint ({}) of pool {}",
                self.amm.coin_mint, self.amm.pc_mint, self.amm_id
            ))),
        }
    }

    /// Expected raw output for `amount_in` raw units of `spend_mint`, using the
    /// constant-product formula net of the pool's trade fee.
    pub fn quote(&self, spend_mint: &Pubkey, amount_in: u64) -> BotResult<u64> {
        let (reserve_in, reserve_out) = self.reserves_for(spend_mint)?;
        if reserve_in == 0 || reserve_out == 0 {
            return Err(BotError::solana(format!(
                "pool {} has empty reserves (in={reserve_in}, out={reserve_out}) — \
                 load it with with_market = true or it has no liquidity",
                self.amm_id
            )));
        }
        // `constant_product_out` applies the fee itself, so pass the pool's
        // own numerator/denominator rather than pre-discounting the input.
        let out = maths::constant_product_out(
            amount_in,
            reserve_in,
            reserve_out,
            self.amm.trade_fee_numerator,
            self.amm.trade_fee_denominator,
        );
        if out == 0 {
            return Err(BotError::solana(format!(
                "pool {} quotes 0 out for {amount_in} in — the trade is too small for \
                 this pool's lot size, or the reserves are exhausted",
                self.amm_id
            )));
        }
        Ok(out)
    }

    /// Price of the coin side expressed in the pc side, in human units.
    pub fn price_pc_per_coin(&self) -> f64 {
        if self.coin_vault_balance == 0 {
            return 0.0;
        }
        let coin = maths::from_raw_amount(
            self.coin_vault_balance,
            u8::try_from(self.amm.coin_decimals).unwrap_or(6),
        );
        let pc = maths::from_raw_amount(
            self.pc_vault_balance,
            u8::try_from(self.amm.pc_decimals).unwrap_or(9),
        );
        if coin == 0.0 {
            return 0.0;
        }
        pc / coin
    }

    /// Named accounts, so a learned layout can be applied to this pool.
    pub fn named_accounts(
        &self,
        user: &Pubkey,
        source: &Pubkey,
        destination: &Pubkey,
    ) -> HashMap<String, Pubkey> {
        let mut m = HashMap::new();
        m.insert("token_program".to_string(), *TOKEN_PROGRAM);
        m.insert("amm_id".to_string(), self.amm_id);
        m.insert("amm_authority".to_string(), self.amm_authority);
        m.insert("amm_open_orders".to_string(), self.amm.open_orders);
        m.insert("amm_target_orders".to_string(), self.amm.target_orders);
        m.insert("amm_coin_vault".to_string(), self.amm.coin_vault);
        m.insert("amm_pc_vault".to_string(), self.amm.pc_vault);
        if let Some(market) = &self.market {
            m.insert("market_program".to_string(), self.amm.market_program);
            m.insert("market".to_string(), self.amm.market);
            m.insert("market_bids".to_string(), market.bids);
            m.insert("market_asks".to_string(), market.asks);
            m.insert("market_event_queue".to_string(), market.event_queue);
            m.insert("market_coin_vault".to_string(), market.base_vault);
            m.insert("market_pc_vault".to_string(), market.quote_vault);
        }
        if let Some(signer) = self.market_vault_signer {
            m.insert("market_vault_signer".to_string(), signer);
        }
        m.insert("user_source".to_string(), *source);
        m.insert("user_destination".to_string(), *destination);
        m.insert("user_owner".to_string(), *user);
        m
    }

    /// Build `SwapBaseInV2` — 8 accounts, no orderbook.
    pub fn swap_base_in_v2_ix(
        &self,
        user: &Pubkey,
        source: &Pubkey,
        destination: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Instruction {
        let mut data = Vec::with_capacity(17);
        data.push(RAYDIUM_IX_SWAP_BASE_IN_V2);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());

        Instruction {
            program_id: *RAYDIUM_AMM_V4,
            accounts: vec![
                AccountMeta::new_readonly(*TOKEN_PROGRAM, false),
                AccountMeta::new(self.amm_id, false),
                AccountMeta::new_readonly(self.amm_authority, false),
                AccountMeta::new(self.amm.coin_vault, false),
                AccountMeta::new(self.amm.pc_vault, false),
                AccountMeta::new(*source, false),
                AccountMeta::new(*destination, false),
                AccountMeta::new_readonly(*user, true),
            ],
            data,
        }
    }

    /// Build the full 17-account `SwapBaseIn`.
    ///
    /// Requires the market to have been loaded (`with_market = true`).
    pub fn swap_base_in_ix(
        &self,
        user: &Pubkey,
        source: &Pubkey,
        destination: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> BotResult<Instruction> {
        let market = self.market.as_ref().ok_or_else(|| {
            BotError::invalid(
                "swap_base_in needs the OpenBook market; load the pool with with_market = true",
            )
        })?;
        let vault_signer = self.market_vault_signer.ok_or_else(|| {
            BotError::invalid("swap_base_in needs the market vault signer, which failed to derive")
        })?;

        let mut data = Vec::with_capacity(17);
        data.push(RAYDIUM_IX_SWAP_BASE_IN);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());

        Ok(Instruction {
            program_id: *RAYDIUM_AMM_V4,
            accounts: vec![
                AccountMeta::new_readonly(*TOKEN_PROGRAM, false),
                AccountMeta::new(self.amm_id, false),
                AccountMeta::new_readonly(self.amm_authority, false),
                AccountMeta::new(self.amm.open_orders, false),
                AccountMeta::new(self.amm.coin_vault, false),
                AccountMeta::new(self.amm.pc_vault, false),
                AccountMeta::new_readonly(self.amm.market_program, false),
                AccountMeta::new(self.amm.market, false),
                AccountMeta::new(market.bids, false),
                AccountMeta::new(market.asks, false),
                AccountMeta::new(market.event_queue, false),
                AccountMeta::new(market.base_vault, false),
                AccountMeta::new(market.quote_vault, false),
                AccountMeta::new_readonly(vault_signer, false),
                AccountMeta::new(*source, false),
                AccountMeta::new(*destination, false),
                AccountMeta::new_readonly(*user, true),
            ],
            data,
        })
    }

    /// Pick the right swap form.
    pub fn swap_base_in(
        &self,
        user: &Pubkey,
        source: &Pubkey,
        destination: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
        prefer_v2: bool,
    ) -> BotResult<Instruction> {
        if prefer_v2 {
            return Ok(self.swap_base_in_v2_ix(
                user,
                source,
                destination,
                amount_in,
                minimum_amount_out,
            ));
        }
        self.swap_base_in_ix(user, source, destination, amount_in, minimum_amount_out)
    }

    /// Build using a learned layout from the [`LayoutStore`], falling back to
    /// the hardcoded form when nothing has been learned.
    // Flat arg list mirrors `swap_base_in` (plus store + prefer_v2) — the
    // convention for every instruction builder in this crate.
    #[allow(clippy::too_many_arguments)]
    pub fn swap_base_in_learned(
        &self,
        store: &LayoutStore,
        user: &Pubkey,
        source: &Pubkey,
        destination: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
        prefer_v2: bool,
    ) -> BotResult<Instruction> {
        let instruction = if prefer_v2 { SWAP_V2_IX } else { SWAP_FULL_IX };
        let Some(layout) = store.get(&RAYDIUM_AMM_V4, instruction) else {
            return self.swap_base_in(
                user,
                source,
                destination,
                amount_in,
                minimum_amount_out,
                prefer_v2,
            );
        };
        let names = self.named_accounts(user, source, destination);
        let accounts = layout.build(&names)?;

        let tag = if prefer_v2 {
            RAYDIUM_IX_SWAP_BASE_IN_V2
        } else {
            RAYDIUM_IX_SWAP_BASE_IN
        };
        let mut data = Vec::with_capacity(17);
        data.push(tag);
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());

        debug!(
            instruction,
            accounts = accounts.len(),
            "using learned raydium layout"
        );
        Ok(Instruction {
            program_id: *RAYDIUM_AMM_V4,
            accounts,
            data,
        })
    }

    /// Cross-check this pool against the chain and report anything that looks
    /// wrong. Run this once at startup for every configured pool: a silently
    /// misparsed pool produces transactions that fail with an opaque error.
    pub async fn doctor(&self, rpc: &Rpc) -> BotResult<Vec<String>> {
        let mut problems = Vec::new();

        if self.amm.data_len != AMM_LEN {
            problems.push(format!(
                "pool account is {} bytes, expected {AMM_LEN} — the AmmInfo layout has changed",
                self.amm.data_len
            ));
        }
        if !self.amm.is_swappable() {
            problems.push(format!(
                "pool status {} does not permit swaps (need 1, 6 or 7)",
                self.amm.status
            ));
        }
        if self.amm_authority != amm_authority_pda(self.amm.nonce)? {
            problems.push("derived amm_authority does not match the stored nonce".into());
        }
        if self.amm.market_program != *OPENBOOK_MARKET_PROGRAM
            && self.amm.market_program != *RAYDIUM_AMM_V4
        {
            // Not fatal — pools can point at the old Serum v3 program — but
            // worth surfacing because the vault-signer derivation depends on it.
            problems.push(format!(
                "market_program {} is not the OpenBook program {} (may be legacy Serum)",
                self.amm.market_program, *OPENBOOK_MARKET_PROGRAM
            ));
        }
        if self.amm.coin_mint == self.amm.pc_mint {
            problems.push(format!(
                "coin_mint and pc_mint are both {} — the mint offsets are misaligned",
                self.amm.coin_mint
            ));
        }

        // Verify the vaults really are token accounts for the mints we parsed.
        let vaults = rpc
            .get_multiple_accounts(&[self.amm.coin_vault, self.amm.pc_vault])
            .await?;
        for (label, account, expected_mint) in [
            (
                "coin_vault",
                vaults.first().and_then(|a| a.as_ref()),
                self.amm.coin_mint,
            ),
            (
                "pc_vault",
                vaults.get(1).and_then(|a| a.as_ref()),
                self.amm.pc_mint,
            ),
        ] {
            match account {
                None => problems.push(format!("{label} {} does not exist", expected_mint)),
                Some(a) => {
                    if a.data.len() >= 32 {
                        let mut mint_bytes = [0u8; 32];
                        mint_bytes.copy_from_slice(&a.data[..32]);
                        let mint = Pubkey::new_from_array(mint_bytes);
                        if mint != expected_mint {
                            problems.push(format!(
                                "{label} holds mint {mint} but the pool says {expected_mint}"
                            ));
                        }
                    } else {
                        problems.push(format!("{label} is not a token account"));
                    }
                }
            }
        }

        if let Some(market) = &self.market {
            if market.own_address != self.amm.market {
                problems.push(format!(
                    "market own_address {} != amm.market {}",
                    market.own_address, self.amm.market
                ));
            }
            if market.base_mint != self.amm.coin_mint {
                problems.push(format!(
                    "market base_mint {} != pool coin_mint {}",
                    market.base_mint, self.amm.coin_mint
                ));
            }
            if market.quote_mint != self.amm.pc_mint {
                problems.push(format!(
                    "market quote_mint {} != pool pc_mint {}",
                    market.quote_mint, self.amm.pc_mint
                ));
            }
        }

        Ok(problems)
    }
}

/// `Pubkey::create_program_address(&[b"amm authority", &[nonce]], program_id)`.
///
/// The program stores `nonce` as a `u64` in `AmmInfo` but only the first byte
/// is a real ed25519 bump, so truncate before deriving.
pub fn amm_authority_pda(nonce: u64) -> BotResult<Pubkey> {
    let bump = [u8::try_from(nonce & 0xff).unwrap_or(0)];
    Pubkey::create_program_address(&[RAYDIUM_AMM_AUTHORITY_SEED, &bump], &RAYDIUM_AMM_V4)
        .map_err(|e| BotError::solana(format!("derive amm authority (nonce {nonce}): {e}")))
}

/// The authority for pools created by the current program, derived without
/// reading `nonce` first. Must equal [`amm_authority_pda`] for a healthy pool.
pub fn amm_authority_canonical() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RAYDIUM_AMM_AUTHORITY_SEED], &RAYDIUM_AMM_V4)
}

/// Read the `amount` field of an SPL token account (offset 64).
pub fn token_account_amount(data: &[u8]) -> Option<u64> {
    if data.len() < 72 {
        return None;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&data[64..72]);
    Some(u64::from_le_bytes(b))
}

/// Read the `mint` field of an SPL token account (offset 0).
pub fn token_account_mint(data: &[u8]) -> Option<Pubkey> {
    if data.len() < 32 {
        return None;
    }
    let mut b = [0u8; 32];
    b.copy_from_slice(&data[..32]);
    Some(Pubkey::new_from_array(b))
}

// --------------------------------------------------------------------------
// Pool initialisation (launch detection)
// --------------------------------------------------------------------------

/// `initialize2` account positions (`raydium-io/raydium-amm` →
/// `program/src/instruction.rs`, `AmmInstruction::Initialize2`).
const INIT2_ACC_AMM: usize = 4;
const INIT2_ACC_LP_MINT: usize = 7;
const INIT2_ACC_COIN_MINT: usize = 8;
const INIT2_ACC_PC_MINT: usize = 9;
const INIT2_ACC_COIN_VAULT: usize = 10;
const INIT2_ACC_PC_VAULT: usize = 11;
const INIT2_ACC_MARKET: usize = 16;
const INIT2_ACC_USER_WALLET: usize = 17;
/// `initialize2` needs at least the accounts up to the user wallet.
const INIT2_MIN_ACCOUNTS: usize = INIT2_ACC_USER_WALLET + 1;
/// `tag(1) nonce(1) open_time(8) init_pc_amount(8) init_coin_amount(8)`.
const INIT2_DATA_LEN: usize = 26;

/// A Raydium AMM v4 pool creation, decoded from the `initialize2`
/// instruction of the creating transaction. This is the launch signal for the
/// Raydium protocol: the AMM is *not* an Anchor program and emits no
/// `Program data:` event, so the mints and the initial deposit have to be read
/// from the instruction itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolInitEvent {
    pub amm_id: Pubkey,
    pub lp_mint: Pubkey,
    pub coin_mint: Pubkey,
    pub pc_mint: Pubkey,
    pub coin_vault: Pubkey,
    pub pc_vault: Pubkey,
    pub market: Pubkey,
    /// The wallet that created the pool (fee payer of the deposit).
    pub creator: Pubkey,
    pub nonce: u8,
    /// Unix time from which swaps are accepted; `0` = immediately.
    pub open_time: u64,
    pub init_pc_amount: u64,
    pub init_coin_amount: u64,
}

impl PoolInitEvent {
    /// `true` when the quote (pc) side is wrapped SOL — the only pair shape
    /// the sniper trades.
    pub fn is_sol_quote(&self) -> bool {
        self.pc_mint == *WSOL_MINT
    }

    /// The non-SOL side of a SOL pair (`None` for non-SOL pairs and for the
    /// degenerate SOL/SOL case).
    pub fn base_mint(&self) -> Option<Pubkey> {
        if self.pc_mint == *WSOL_MINT && self.coin_mint != *WSOL_MINT {
            Some(self.coin_mint)
        } else if self.coin_mint == *WSOL_MINT && self.pc_mint != *WSOL_MINT {
            Some(self.pc_mint)
        } else {
            None
        }
    }

    /// Initial SOL-side deposit in lamports (`None` for non-SOL pairs).
    pub fn initial_sol_lamports(&self) -> Option<u64> {
        if self.pc_mint == *WSOL_MINT {
            Some(self.init_pc_amount)
        } else if self.coin_mint == *WSOL_MINT {
            Some(self.init_coin_amount)
        } else {
            None
        }
    }

    /// Initial base-side deposit in raw token units (`None` for non-SOL pairs).
    pub fn initial_base_raw(&self) -> Option<u64> {
        if self.pc_mint == *WSOL_MINT {
            Some(self.init_coin_amount)
        } else if self.coin_mint == *WSOL_MINT {
            Some(self.init_pc_amount)
        } else {
            None
        }
    }

    /// `true` when the pool accepts swaps at `now_unix` (open time reached).
    pub fn is_open_at(&self, now_unix: i64) -> bool {
        self.open_time == 0 || i64::try_from(self.open_time).is_ok_and(|t| t <= now_unix)
    }

    /// Decode from one resolved instruction. Returns `None` for anything that
    /// is not a well-formed `initialize2` on the AMM v4 program.
    pub fn from_instruction(program_id: &Pubkey, accounts: &[Pubkey], data: &[u8]) -> Option<Self> {
        if *program_id != *RAYDIUM_AMM_V4 {
            return None;
        }
        if data.len() < INIT2_DATA_LEN || data[0] != RAYDIUM_IX_INITIALIZE2 {
            return None;
        }
        if accounts.len() < INIT2_MIN_ACCOUNTS {
            return None;
        }
        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[off..off + 8]);
            u64::from_le_bytes(b)
        };
        Some(PoolInitEvent {
            amm_id: accounts[INIT2_ACC_AMM],
            lp_mint: accounts[INIT2_ACC_LP_MINT],
            coin_mint: accounts[INIT2_ACC_COIN_MINT],
            pc_mint: accounts[INIT2_ACC_PC_MINT],
            coin_vault: accounts[INIT2_ACC_COIN_VAULT],
            pc_vault: accounts[INIT2_ACC_PC_VAULT],
            market: accounts[INIT2_ACC_MARKET],
            creator: accounts[INIT2_ACC_USER_WALLET],
            nonce: data[1],
            open_time: u64_at(2),
            init_pc_amount: u64_at(10),
            init_coin_amount: u64_at(18),
        })
    }

    /// Find the pool initialisation among a transaction's resolved
    /// instructions (see [`crate::decode::decode_instructions`]).
    pub fn find(instructions: &[crate::decode::DecodedInstruction]) -> Option<Self> {
        instructions
            .iter()
            .find_map(|ix| Self::from_instruction(&ix.program_id, &ix.accounts, &ix.data))
    }
}

/// The parameters the AMM prints when a pool is initialised, parsed from the
/// `Program log: initialize2: InitializeInstruction2 { nonce: 254, open_time:
/// 0, init_pc_amount: …, init_coin_amount: … }` line. A `logsSubscribe` feed
/// only sees this line (no account keys), so it is used as the cheap trigger
/// to fetch the full transaction — never as the launch record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitializeLogParams {
    pub nonce: u8,
    pub open_time: u64,
    pub init_pc_amount: u64,
    pub init_coin_amount: u64,
}

/// Parse one log line; `None` when it is not an `initialize2` log.
pub fn parse_initialize2_log(line: &str) -> Option<InitializeLogParams> {
    let body = line.trim().strip_prefix("Program log: ")?;
    let body = body.strip_prefix("initialize2: InitializeInstruction2")?;
    let field = |name: &str| -> Option<u64> {
        let start = body.find(name)? + name.len();
        let rest = body[start..].trim_start_matches([':', ' ']);
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok()
    };
    Some(InitializeLogParams {
        nonce: u8::try_from(field("nonce")?).ok()?,
        open_time: field("open_time")?,
        init_pc_amount: field("init_pc_amount")?,
        init_coin_amount: field("init_coin_amount")?,
    })
}

/// Scan a transaction's log lines for the AMM v4 pool-initialisation log.
pub fn find_initialize2_log(logs: &[String]) -> Option<InitializeLogParams> {
    logs.iter().find_map(|l| parse_initialize2_log(l))
}

/// Discover Raydium v4 pools whose quote side is WSOL and whose base side is
/// `base_mint`. Uses two `memcmp` filters so the RPC does the work.
pub async fn find_pools_for_mint(rpc: &Rpc, base_mint: &Pubkey) -> BotResult<Vec<Pubkey>> {
    use solana_client::rpc_config::RpcProgramAccountsConfig;
    use solana_client::rpc_filter::{Memcmp, MemcmpEncodedBytes, RpcFilterType};

    let config = RpcProgramAccountsConfig {
        filters: Some(vec![
            RpcFilterType::DataSize(AMM_LEN as u64),
            RpcFilterType::Memcmp(Memcmp::new(
                AMM_OFF_COIN_MINT,
                MemcmpEncodedBytes::Bytes(base_mint.to_bytes().to_vec()),
            )),
            RpcFilterType::Memcmp(Memcmp::new(
                AMM_OFF_PC_MINT,
                MemcmpEncodedBytes::Bytes(WSOL_MINT.to_bytes().to_vec()),
            )),
        ]),
        account_config: solana_client::rpc_config::RpcAccountInfoConfig {
            encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
            data_slice: Some(solana_account_decoder::UiDataSliceConfig {
                offset: 0,
                length: AMM_LEN,
            }),
            commitment: Some(rpc.commitment()),
            min_context_slot: None,
        },
        with_context: Some(false),
        sort_results: None,
    };

    let accounts = rpc
        .raw()
        .get_program_accounts_with_config(&RAYDIUM_AMM_V4, config)
        .await
        .map_err(|e| BotError::rpc(format!("getProgramAccounts for raydium pools: {e}")))?;

    let pools: Vec<Pubkey> = accounts.into_iter().map(|(pk, _)| pk).collect();
    debug!(%base_mint, count = pools.len(), "discovered raydium pools");
    Ok(pools)
}

/// Turn a confirmed transaction's account list into a layout template.
///
/// `names` maps the pubkeys we can derive ourselves back to their slot names so
/// the template keeps working for other pools; anything else is pinned as a
/// literal. Returns `None` for an empty list.
pub fn layout_from_accounts(
    accounts: &[AccountMeta],
    names: &HashMap<Pubkey, String>,
    prefer_v2: bool,
) -> Option<AccountLayout> {
    if accounts.is_empty() {
        return None;
    }
    let instruction = if prefer_v2 { SWAP_V2_IX } else { SWAP_FULL_IX };
    let mut layout = AccountLayout::new(*RAYDIUM_AMM_V4, instruction, Vec::new());
    layout.learn(accounts, names, None);
    Some(layout)
}

/// Instruction names used as [`LayoutStore`] keys.
pub const SWAP_V2_IX: &str = "swap_base_in_v2";
pub const SWAP_FULL_IX: &str = "swap_base_in";

/// Inverse-name helper for [`learn_layout_from_accounts`].
pub fn reverse_named(
    pool: &RaydiumPool,
    user: &Pubkey,
    source: &Pubkey,
    destination: &Pubkey,
) -> HashMap<Pubkey, String> {
    pool.named_accounts(user, source, destination)
        .into_iter()
        .map(|(k, v)| (v, k))
        .collect()
}

/// Summary used by the dashboard and Telegram alerts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSummary {
    pub amm_id: String,
    pub base_mint: String,
    pub quote_mint: String,
    pub status: u64,
    pub swappable: bool,
    pub price_quote_per_base: f64,
    pub base_reserve: f64,
    pub quote_reserve: f64,
    pub trade_fee_bps: u64,
    pub has_market: bool,
}

impl From<&RaydiumPool> for PoolSummary {
    fn from(p: &RaydiumPool) -> Self {
        PoolSummary {
            amm_id: p.amm_id.to_string(),
            base_mint: p.amm.coin_mint.to_string(),
            quote_mint: p.amm.pc_mint.to_string(),
            status: p.amm.status,
            swappable: p.amm.is_swappable(),
            price_quote_per_base: p.price_pc_per_coin(),
            base_reserve: maths::from_raw_amount(
                p.coin_vault_balance,
                u8::try_from(p.amm.coin_decimals).unwrap_or(6),
            ),
            quote_reserve: maths::from_raw_amount(
                p.pc_vault_balance,
                u8::try_from(p.amm.pc_decimals).unwrap_or(9),
            ),
            trade_fee_bps: p.amm.trade_fee_bps(),
            has_market: p.market.is_some(),
        }
    }
}

/// Warn loudly if a parsed pool looks structurally wrong. Cheap insurance
/// against silently trading against garbage offsets.
pub fn sanity_warn(pool: &RaydiumPool) {
    if pool.amm.coin_mint == Pubkey::default() || pool.amm.pc_mint == Pubkey::default() {
        warn!(
            amm_id = %pool.amm_id,
            "parsed pool has a zero mint — the AmmInfo layout is wrong for this account"
        );
    }
    if pool.amm.status == AMM_STATUS_UNINITIALIZED {
        warn!(amm_id = %pool.amm_id, "pool is uninitialized");
    }
}

/// The 8-account V2 layout as an [`AccountLayout`], matching the flags in
/// `swap_base_in_v2` upstream: token program and authority readonly, everything
/// else writable, user signs.
pub fn default_v2_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*RAYDIUM_AMM_V4, SWAP_V2_IX, Vec::new());
    l.push_named("token_program", false, false);
    l.push_named("amm_id", true, false);
    l.push_named("amm_authority", false, false);
    l.push_named("amm_coin_vault", true, false);
    l.push_named("amm_pc_vault", true, false);
    l.push_named("user_source", true, false);
    l.push_named("user_destination", true, false);
    l.push_named("user_owner", false, true);
    l
}

/// The 17-account full layout.
pub fn default_full_layout() -> AccountLayout {
    let mut l = AccountLayout::new(*RAYDIUM_AMM_V4, SWAP_FULL_IX, Vec::new());
    l.push_named("token_program", false, false);
    l.push_named("amm_id", true, false);
    l.push_named("amm_authority", false, false);
    l.push_named("amm_open_orders", true, false);
    l.push_named("amm_coin_vault", true, false);
    l.push_named("amm_pc_vault", true, false);
    l.push_named("market_program", false, false);
    l.push_named("market", true, false);
    l.push_named("market_bids", true, false);
    l.push_named("market_asks", true, false);
    l.push_named("market_event_queue", true, false);
    l.push_named("market_coin_vault", true, false);
    l.push_named("market_pc_vault", true, false);
    l.push_named("market_vault_signer", false, false);
    l.push_named("user_source", true, false);
    l.push_named("user_destination", true, false);
    l.push_named("user_owner", false, true);
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Slot;

    /// Build a byte-accurate 784-byte AmmInfo using the verified offsets.
    fn amm_data(coin_mint: Pubkey, pc_mint: Pubkey, status: u64) -> Vec<u8> {
        let mut d = vec![0u8; AMM_LEN];
        let put_u64 =
            |d: &mut Vec<u8>, off: usize, v: u64| d[off..off + 8].copy_from_slice(&v.to_le_bytes());
        let put_pk = |d: &mut Vec<u8>, off: usize, v: Pubkey| {
            d[off..off + 32].copy_from_slice(&v.to_bytes())
        };

        put_u64(&mut d, AMM_OFF_STATUS, status);
        // The stored nonce must be a valid ed25519 bump or `create_program_address`
        // rejects it; use the real canonical bump so the fixture is derivable.
        let (_, authority_bump) = amm_authority_canonical();
        put_u64(&mut d, AMM_OFF_NONCE, authority_bump as u64);
        put_u64(&mut d, AMM_OFF_COIN_DECIMALS, 6);
        put_u64(&mut d, AMM_OFF_PC_DECIMALS, 9);
        put_u64(&mut d, AMM_OFF_COIN_LOT_SIZE, 1);
        put_u64(&mut d, AMM_OFF_PC_LOT_SIZE, 1);
        // 25 / 10000 = 25 bps
        put_u64(&mut d, AMM_OFF_TRADE_FEE_NUMERATOR, 25);
        put_u64(&mut d, AMM_OFF_TRADE_FEE_DENOMINATOR, 10_000);
        put_u64(&mut d, AMM_OFF_POOL_OPEN_TIME, 1_700_000_000);
        put_pk(&mut d, AMM_OFF_COIN_VAULT, Pubkey::new_unique());
        put_pk(&mut d, AMM_OFF_PC_VAULT, Pubkey::new_unique());
        put_pk(&mut d, AMM_OFF_COIN_MINT, coin_mint);
        put_pk(&mut d, AMM_OFF_PC_MINT, pc_mint);
        put_pk(&mut d, AMM_OFF_LP_MINT, Pubkey::new_unique());
        put_pk(&mut d, AMM_OFF_OPEN_ORDERS, Pubkey::new_unique());
        put_pk(&mut d, AMM_OFF_MARKET, Pubkey::new_unique());
        put_pk(&mut d, AMM_OFF_MARKET_PROGRAM, *OPENBOOK_MARKET_PROGRAM);
        put_pk(&mut d, AMM_OFF_TARGET_ORDERS, Pubkey::new_unique());
        put_pk(&mut d, AMM_OFF_AMM_OWNER, Pubkey::new_unique());
        put_u64(&mut d, AMM_OFF_LP_AMOUNT, 1_000_000);
        d
    }

    #[test]
    fn parses_every_field_at_its_verified_offset() {
        let coin = Pubkey::new_unique();
        let data = amm_data(coin, *WSOL_MINT, AMM_STATUS_SWAP_ONLY);
        let amm = AmmInfo::parse(&data).unwrap();

        assert_eq!(amm.status, AMM_STATUS_SWAP_ONLY);
        assert_eq!(amm.nonce, amm_authority_canonical().1 as u64);
        assert_eq!(amm.coin_decimals, 6);
        assert_eq!(amm.pc_decimals, 9);
        assert_eq!(amm.coin_mint, coin);
        assert_eq!(amm.pc_mint, *WSOL_MINT);
        assert_eq!(amm.market_program, *OPENBOOK_MARKET_PROGRAM);
        assert_eq!(amm.lp_amount, 1_000_000);
        assert_eq!(amm.pool_open_time, 1_700_000_000);
        assert!(amm.is_swappable());
        assert!(amm.is_sol_quote());
        assert_eq!(amm.side_of(&coin), Some(PoolSide::Coin));
        assert_eq!(amm.side_of(&WSOL_MINT), Some(PoolSide::Pc));
        assert_eq!(amm.side_of(&Pubkey::new_unique()), None);
        assert_eq!(amm.data_len, AMM_LEN);
    }

    #[test]
    fn trade_fee_converts_to_bps() {
        let data = amm_data(Pubkey::new_unique(), *WSOL_MINT, 6);
        let amm = AmmInfo::parse(&data).unwrap();
        assert_eq!(amm.trade_fee_bps(), 25, "25/10000 must be 25 bps");
    }

    #[test]
    fn status_gate_matches_the_program() {
        for status in 0..=9u64 {
            let mut data = amm_data(Pubkey::new_unique(), *WSOL_MINT, status);
            data[AMM_OFF_STATUS..AMM_OFF_STATUS + 8].copy_from_slice(&status.to_le_bytes());
            let amm = AmmInfo::parse(&data).unwrap();
            let expected = matches!(status, 1 | 6 | 7);
            assert_eq!(amm.is_swappable(), expected, "status {status}");
        }
    }

    #[test]
    fn rejects_short_accounts() {
        let err = AmmInfo::parse(&[0u8; 100]).unwrap_err();
        assert!(err.to_string().contains("expected 784"), "{err}");
    }

    #[test]
    fn amm_authority_derivation_is_stable() {
        let (canonical, bump) =
            Pubkey::find_program_address(&[RAYDIUM_AMM_AUTHORITY_SEED], &RAYDIUM_AMM_V4);
        let derived = amm_authority_pda(bump as u64).unwrap();
        assert_eq!(
            derived, canonical,
            "create_program_address with the found bump must reproduce the PDA"
        );
        // The well-known Raydium authority.
        assert_eq!(
            canonical.to_string(),
            "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1"
        );
    }

    #[test]
    fn v2_swap_instruction_has_the_upstream_shape() {
        let coin = Pubkey::new_unique();
        let amm = AmmInfo::parse(&amm_data(coin, *WSOL_MINT, 6)).unwrap();
        let pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm,
            market: None,
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: None,
            coin_vault_balance: 0,
            pc_vault_balance: 0,
        };
        let user = Pubkey::new_unique();
        let src = Pubkey::new_unique();
        let dst = Pubkey::new_unique();
        let ix = pool.swap_base_in_v2_ix(&user, &src, &dst, 1_000, 900);

        assert_eq!(ix.program_id, *RAYDIUM_AMM_V4);
        assert_eq!(ix.accounts.len(), 8, "SwapBaseInV2 takes 8 accounts");
        assert_eq!(ix.data[0], RAYDIUM_IX_SWAP_BASE_IN_V2);
        assert_eq!(ix.data.len(), 17, "tag + two u64");
        assert_eq!(u64::from_le_bytes(ix.data[1..9].try_into().unwrap()), 1_000);
        assert_eq!(u64::from_le_bytes(ix.data[9..17].try_into().unwrap()), 900);
        // Flags, matching the upstream builder exactly.
        assert!(!ix.accounts[0].is_writable && !ix.accounts[0].is_signer);
        assert!(ix.accounts[1].is_writable && !ix.accounts[1].is_signer);
        assert!(!ix.accounts[2].is_writable && !ix.accounts[2].is_signer);
        assert!(ix.accounts[3].is_writable);
        assert!(ix.accounts[4].is_writable);
        assert!(ix.accounts[5].is_writable);
        assert!(ix.accounts[6].is_writable);
        assert!(!ix.accounts[7].is_writable && ix.accounts[7].is_signer);
        assert_eq!(ix.accounts[7].pubkey, user);
    }

    #[test]
    fn full_swap_instruction_needs_the_market() {
        let amm = AmmInfo::parse(&amm_data(Pubkey::new_unique(), *WSOL_MINT, 6)).unwrap();
        let pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm,
            market: None,
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: None,
            coin_vault_balance: 0,
            pc_vault_balance: 0,
        };
        let err = pool
            .swap_base_in_ix(
                &Pubkey::new_unique(),
                &Pubkey::new_unique(),
                &Pubkey::new_unique(),
                1,
                1,
            )
            .unwrap_err();
        assert!(err.to_string().contains("with_market = true"), "{err}");
    }

    #[test]
    fn full_swap_instruction_matches_upstream_when_the_market_is_present() {
        let coin = Pubkey::new_unique();
        let amm = AmmInfo::parse(&amm_data(coin, *WSOL_MINT, 6)).unwrap();
        let market = MarketStateV3 {
            own_address: amm.market,
            vault_signer_nonce: 0,
            base_mint: coin,
            quote_mint: *WSOL_MINT,
            base_vault: Pubkey::new_unique(),
            quote_vault: Pubkey::new_unique(),
            request_queue: Pubkey::new_unique(),
            event_queue: Pubkey::new_unique(),
            bids: Pubkey::new_unique(),
            asks: Pubkey::new_unique(),
            base_deposits_total: 0,
            quote_deposits_total: 0,
        };
        // vault_signer with nonce 0 is derivable for any own_address.
        let signer = market.vault_signer(&amm.market_program).ok();
        let pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm: amm.clone(),
            market: Some(market),
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: signer,
            coin_vault_balance: 0,
            pc_vault_balance: 0,
        };
        if signer.is_none() {
            // A nonce of 0 does not always produce an off-curve address; the
            // assertion below is about ordering, so skip when it is unusable.
            return;
        }
        let ix = pool
            .swap_base_in_ix(
                &Pubkey::new_unique(),
                &Pubkey::new_unique(),
                &Pubkey::new_unique(),
                5,
                4,
            )
            .unwrap();
        assert_eq!(ix.accounts.len(), 17);
        assert_eq!(ix.data[0], RAYDIUM_IX_SWAP_BASE_IN);
        assert_eq!(ix.accounts[0].pubkey, *TOKEN_PROGRAM);
        assert_eq!(ix.accounts[3].pubkey, amm.open_orders);
        assert_eq!(ix.accounts[6].pubkey, amm.market_program);
        assert_eq!(ix.accounts[7].pubkey, amm.market);
        assert!(ix.accounts[16].is_signer);
    }

    #[test]
    fn quote_applies_the_pool_fee_and_constant_product() {
        let coin = Pubkey::new_unique();
        let amm = AmmInfo::parse(&amm_data(coin, *WSOL_MINT, 6)).unwrap();
        let mut pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm,
            market: None,
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: None,
            coin_vault_balance: 1_000_000_000, // 1000 tokens @ 6dp
            pc_vault_balance: 10_000_000_000,  // 10 SOL @ 9dp
        };
        // Spending WSOL buys coin.
        let out = pool.quote(&WSOL_MINT, 10_000_000).unwrap();
        assert!(out > 0);
        // A bigger input must yield a bigger but proportionally smaller output.
        let out2 = pool.quote(&WSOL_MINT, 100_000_000).unwrap();
        assert!(out2 > out);
        assert!(
            (out2 as f64 / out as f64) < 10.0,
            "price impact must reduce the marginal rate"
        );

        // An unknown mint is an error, not a zero quote.
        assert!(pool.quote(&Pubkey::new_unique(), 1).is_err());

        // Empty reserves must error rather than divide by zero.
        pool.coin_vault_balance = 0;
        assert!(pool.quote(&WSOL_MINT, 1).is_err());
    }

    #[test]
    fn price_is_quote_per_base_in_human_units() {
        let coin = Pubkey::new_unique();
        let amm = AmmInfo::parse(&amm_data(coin, *WSOL_MINT, 6)).unwrap();
        let pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm,
            market: None,
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: None,
            coin_vault_balance: 1_000_000_000, // 1000 @ 6dp
            pc_vault_balance: 10_000_000_000,  // 10 SOL @ 9dp
        };
        let price = pool.price_pc_per_coin();
        assert!(
            (price - 0.01).abs() < 1e-9,
            "10 SOL / 1000 tokens = 0.01, got {price}"
        );
    }

    #[test]
    fn market_state_vault_signer_uses_nonce_le_bytes() {
        let mut data = vec![0u8; MARKET_V3_LEN];
        let own = Pubkey::new_unique();
        data[MARKET_OFF_OWN_ADDRESS..MARKET_OFF_OWN_ADDRESS + 32].copy_from_slice(&own.to_bytes());
        data[MARKET_OFF_VAULT_SIGNER_NONCE..MARKET_OFF_VAULT_SIGNER_NONCE + 8]
            .copy_from_slice(&3u64.to_le_bytes());
        let m = MarketStateV3::parse(&data).unwrap();
        assert_eq!(m.own_address, own);
        assert_eq!(m.vault_signer_nonce, 3);
        // Must equal create_program_address([own, nonce.to_le_bytes()], program)
        // — the Serum/OpenBook v3 vault-signer convention (8-byte LE nonce).
        let expected = Pubkey::create_program_address(
            &[own.as_ref(), &3u64.to_le_bytes()],
            &OPENBOOK_MARKET_PROGRAM,
        );
        match (expected, m.vault_signer(&OPENBOOK_MARKET_PROGRAM)) {
            (Ok(e), Ok(g)) => assert_eq!(e, g),
            (Err(_), Err(_)) => {}
            _ => panic!("derivation and parser disagree"),
        }
    }

    #[test]
    fn token_account_helpers_read_the_spl_layout() {
        let mut data = vec![0u8; 165];
        let mint = Pubkey::new_unique();
        data[..32].copy_from_slice(&mint.to_bytes());
        data[64..72].copy_from_slice(&12345u64.to_le_bytes());
        assert_eq!(token_account_mint(&data), Some(mint));
        assert_eq!(token_account_amount(&data), Some(12345));
        assert_eq!(token_account_amount(&data[..10]), None);
        assert_eq!(token_account_mint(&data[..10]), None);
    }

    #[tokio::test]
    async fn learned_layout_round_trips_through_the_store() {
        let coin = Pubkey::new_unique();
        let amm = AmmInfo::parse(&amm_data(coin, *WSOL_MINT, 6)).unwrap();
        let pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm,
            market: None,
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: None,
            coin_vault_balance: 0,
            pc_vault_balance: 0,
        };
        let user = Pubkey::new_unique();
        let src = Pubkey::new_unique();
        let dst = Pubkey::new_unique();

        // Learn from the instruction we would have built anyway.
        let ix = pool.swap_base_in_v2_ix(&user, &src, &dst, 1, 1);
        let names = reverse_named(&pool, &user, &src, &dst);
        let learned = layout_from_accounts(&ix.accounts, &names, true)
            .expect("a non-empty account list must produce a layout");
        assert!(
            learned
                .slots
                .iter()
                .all(|s| matches!(s, Slot::Named { .. })),
            "every account in the V2 swap is derivable, so none may be pinned"
        );

        let dir = std::env::temp_dir().join(format!("ray-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("layouts.json");
        let mut store = LayoutStore::default();
        store.insert(learned);
        store.save(&path).await.unwrap();

        // The learned layout must reproduce the same accounts.
        let store2 = LayoutStore::load(&path).await;
        let rebuilt = pool
            .swap_base_in_learned(&store2, &user, &src, &dst, 1, 1, true)
            .unwrap();
        let a: Vec<Pubkey> = ix.accounts.iter().map(|m| m.pubkey).collect();
        let b: Vec<Pubkey> = rebuilt.accounts.iter().map(|m| m.pubkey).collect();
        assert_eq!(a, b);
        assert_eq!(rebuilt.data, ix.data);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_layouts_have_the_right_account_counts() {
        assert_eq!(default_v2_layout().len(), 8);
        assert_eq!(default_full_layout().len(), 17);
        // The V2 default must resolve to exactly what the builder emits.
        let names = HashMap::from([
            ("token_program".to_string(), *TOKEN_PROGRAM),
            ("amm_id".to_string(), Pubkey::new_unique()),
            ("amm_authority".to_string(), Pubkey::new_unique()),
            ("amm_coin_vault".to_string(), Pubkey::new_unique()),
            ("amm_pc_vault".to_string(), Pubkey::new_unique()),
            ("user_source".to_string(), Pubkey::new_unique()),
            ("user_destination".to_string(), Pubkey::new_unique()),
            ("user_owner".to_string(), Pubkey::new_unique()),
        ]);
        let built = default_v2_layout().build(&names).unwrap();
        assert_eq!(built[0].pubkey, *TOKEN_PROGRAM);
        assert!(!built[0].is_writable);
        assert!(built[1].is_writable);
        assert!(built[7].is_signer && !built[7].is_writable);
    }

    #[test]
    fn pool_summary_is_serialisable_for_the_dashboard() {
        let amm = AmmInfo::parse(&amm_data(Pubkey::new_unique(), *WSOL_MINT, 6)).unwrap();
        let pool = RaydiumPool {
            amm_id: Pubkey::new_unique(),
            amm,
            market: None,
            amm_authority: amm_authority_canonical().0,
            market_vault_signer: None,
            coin_vault_balance: 1_000_000,
            pc_vault_balance: 2_000_000_000,
        };
        let summary = PoolSummary::from(&pool);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"swappable\":true"));
        assert_eq!(summary.quote_reserve, 2.0);
    }

    /// Byte-accurate `initialize2` data: tag, nonce, open_time, pc, coin.
    fn init2_data(nonce: u8, open_time: u64, pc: u64, coin: u64) -> Vec<u8> {
        let mut d = vec![RAYDIUM_IX_INITIALIZE2, nonce];
        d.extend_from_slice(&open_time.to_le_bytes());
        d.extend_from_slice(&pc.to_le_bytes());
        d.extend_from_slice(&coin.to_le_bytes());
        d
    }

    fn init2_accounts(coin_mint: Pubkey, pc_mint: Pubkey) -> Vec<Pubkey> {
        let mut accounts: Vec<Pubkey> = (0..21).map(|_| Pubkey::new_unique()).collect();
        accounts[INIT2_ACC_COIN_MINT] = coin_mint;
        accounts[INIT2_ACC_PC_MINT] = pc_mint;
        accounts
    }

    #[test]
    fn pool_init_event_decodes_a_sol_quoted_initialize2() {
        let base = Pubkey::new_unique();
        let accounts = init2_accounts(base, *WSOL_MINT);
        let data = init2_data(254, 0, 5 * maths::LAMPORTS_PER_SOL, 1_000_000_000_000);
        let ev = PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &accounts, &data)
            .expect("initialize2 decodes");
        assert_eq!(ev.amm_id, accounts[INIT2_ACC_AMM]);
        assert_eq!(ev.lp_mint, accounts[INIT2_ACC_LP_MINT]);
        assert_eq!(ev.coin_vault, accounts[INIT2_ACC_COIN_VAULT]);
        assert_eq!(ev.pc_vault, accounts[INIT2_ACC_PC_VAULT]);
        assert_eq!(ev.market, accounts[INIT2_ACC_MARKET]);
        assert_eq!(ev.creator, accounts[INIT2_ACC_USER_WALLET]);
        assert_eq!(ev.nonce, 254);
        assert_eq!(ev.open_time, 0);
        assert!(ev.is_sol_quote());
        assert_eq!(ev.base_mint(), Some(base));
        assert_eq!(ev.initial_sol_lamports(), Some(5 * maths::LAMPORTS_PER_SOL));
        assert_eq!(ev.initial_base_raw(), Some(1_000_000_000_000));
        assert!(ev.is_open_at(1));
    }

    #[test]
    fn pool_init_event_handles_the_inverted_pair_and_open_time() {
        // SOL on the coin side: base is the pc mint and the deposits swap.
        let base = Pubkey::new_unique();
        let accounts = init2_accounts(*WSOL_MINT, base);
        let data = init2_data(250, 2_000_000_000, 777, 3 * maths::LAMPORTS_PER_SOL);
        let ev = PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &accounts, &data).unwrap();
        assert_eq!(ev.base_mint(), Some(base));
        assert_eq!(ev.initial_sol_lamports(), Some(3 * maths::LAMPORTS_PER_SOL));
        assert_eq!(ev.initial_base_raw(), Some(777));
        // Not open yet at t = 1_999_999_999, open at exactly open_time.
        assert!(!ev.is_open_at(1_999_999_999));
        assert!(ev.is_open_at(2_000_000_000));
        // Non-SOL pair: no base, no SOL deposit.
        let other = init2_accounts(Pubkey::new_unique(), Pubkey::new_unique());
        let ev = PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &other, &data).unwrap();
        assert!(!ev.is_sol_quote());
        assert_eq!(ev.base_mint(), None);
        assert_eq!(ev.initial_sol_lamports(), None);
    }

    #[test]
    fn pool_init_event_rejects_malformed_instructions() {
        let accounts = init2_accounts(Pubkey::new_unique(), *WSOL_MINT);
        let data = init2_data(254, 0, 1, 1);
        // Wrong program.
        assert!(PoolInitEvent::from_instruction(&Pubkey::new_unique(), &accounts, &data).is_none());
        // Wrong tag (a swap).
        let mut swap = data.clone();
        swap[0] = RAYDIUM_IX_SWAP_BASE_IN;
        assert!(PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &accounts, &swap).is_none());
        // Truncated data.
        assert!(PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &accounts, &data[..20]).is_none());
        // Too few accounts.
        assert!(PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &accounts[..10], &data).is_none());
        // Empty data must not panic.
        assert!(PoolInitEvent::from_instruction(&RAYDIUM_AMM_V4, &accounts, &[]).is_none());
    }

    #[test]
    fn initialize2_log_line_parses_and_others_do_not() {
        let line = "Program log: initialize2: InitializeInstruction2 { nonce: 254, open_time: 1700000000, init_pc_amount: 5000000000, init_coin_amount: 1000000000000 }";
        let p = parse_initialize2_log(line).expect("parses");
        assert_eq!(p.nonce, 254);
        assert_eq!(p.open_time, 1_700_000_000);
        assert_eq!(p.init_pc_amount, 5_000_000_000);
        assert_eq!(p.init_coin_amount, 1_000_000_000_000);
        assert!(parse_initialize2_log("Program log: Instruction: Swap").is_none());
        assert!(parse_initialize2_log("Program log: initialize2: garbage").is_none());
        assert!(parse_initialize2_log("").is_none());
        let logs = vec![
            "Program 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 invoke [1]".to_string(),
            line.to_string(),
        ];
        assert_eq!(find_initialize2_log(&logs), Some(p));
        assert!(find_initialize2_log(&logs[..1]).is_none());
    }
}
