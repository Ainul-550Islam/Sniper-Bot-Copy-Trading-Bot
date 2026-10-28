//! The Raydium LaunchLab (`launchpad`) venue adapter.
//!
//! This type is the **public face** of the LaunchLab venue: it owns
//! detection, quoting, instruction building and pool lookup. The curve maths
//! live in [`crate::venues::raydium_launchlab::curve`], the account decoding
//! in [`crate::venues::raydium_launchlab::accounts`], the PDAs in
//! [`crate::venues::raydium_launchlab::pda`], the instruction builders in
//! [`crate::venues::raydium_launchlab::instructions`] and the migration
//! hand-off in [`crate::venues::raydium_launchlab::migration`] — this file
//! only orchestrates them.
//!
//! # Layout verification status
//!
//! [`VenueAdapter::layout_verified`] returns **`true`**: the layouts come from
//! the official vendored IDL
//! (`crates/solana-kit/idl/raydium_launchpad.json`, upstream
//! `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`)
//! and every discriminator is asserted against both the IDL and the Anchor
//! sighash convention.
//!
//! What is **not** claimed: a mainnet-validated round trip. The fixtures in
//! `crates/solana-kit/fixtures/raydium/` are synthetic. See
//! `docs/gap-updates/batch-03-rows.md`.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::consts::*;
use crate::venues::raydium_launchlab::accounts::{GlobalConfig, PoolState};
use crate::venues::raydium_launchlab::curve::{
    self, ConstantProductCurve, CurveState, CurveType, FixedPriceCurve, LinearPriceCurve,
};
use crate::venues::raydium_launchlab::instructions::{self, TradeAccounts, TradeArgs, TradeKind};
use crate::venues::raydium_launchlab::migration::{self, MigrationDecision, MigrationTarget};
use crate::venues::raydium_launchlab::pda;
use crate::venues::{VenueAdapter, VenueError, VenueSwapRequest};

/// A LaunchLab pool plus the config it follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchLabPool {
    /// The pool account address.
    pub pool: Pubkey,
    /// The decoded pool state.
    pub state: PoolState,
    /// The decoded `GlobalConfig`.
    pub config: GlobalConfig,
}

impl LaunchLabPool {
    /// The curve state the curve maths reads.
    pub fn curve_state(&self) -> CurveState {
        CurveState {
            virtual_base: self.state.virtual_base,
            virtual_quote: self.state.virtual_quote,
            real_base: self.state.real_base,
            real_quote: self.state.real_quote,
            total_base_sell: self.state.total_base_sell,
            total_quote_fund_raising: self.state.total_quote_fund_raising,
        }
    }

    /// The curve type of the pool, taken from the global config.
    pub fn curve_type(&self) -> BotResult<CurveType> {
        self.config.curve()
    }

    /// The protocol trade fee rate, in hundredths of a bip.
    pub fn trade_fee_rate(&self) -> u64 {
        self.config.trade_fee_rate
    }

    /// The platform fee rate (from the platform config; zero when the caller
    /// does not have one).
    pub fn platform_fee_rate(
        &self,
        platform_config: Option<&crate::venues::raydium_launchlab::accounts::PlatformConfig>,
    ) -> u64 {
        platform_config.map(|p| p.fee_rate).unwrap_or(0)
    }

    /// The creator fee rate (from the platform config; zero when the caller
    /// does not have one).
    pub fn creator_fee_rate(
        &self,
        platform_config: Option<&crate::venues::raydium_launchlab::accounts::PlatformConfig>,
    ) -> u64 {
        platform_config.map(|p| p.creator_fee_rate).unwrap_or(0)
    }
}

/// The Raydium LaunchLab venue adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct RaydiumLaunchLab {
    enabled: bool,
}

impl RaydiumLaunchLab {
    /// An enabled adapter.
    pub fn new() -> Self {
        RaydiumLaunchLab { enabled: true }
    }

    /// A config-driven constructor.
    pub fn with_enabled(enabled: bool) -> Self {
        RaydiumLaunchLab { enabled }
    }

    /// `true` when `account_owner` is the LaunchLab program.
    pub fn is_pool_owner(&self, account_owner: &Pubkey) -> bool {
        account_owner == &*RAYDIUM_LAUNCHLAB
    }

    /// Detect whether a raw account is a LaunchLab pool and decode it.
    ///
    /// The check is the Anchor discriminator plus the exact length, so a
    /// different LaunchLab account is *not* mistaken for a pool.
    pub fn detect_pool(data: &[u8]) -> Option<PoolState> {
        if data.len() != RAYDIUM_LAUNCHLAB_POOL_STATE_LEN {
            return None;
        }
        PoolState::decode(data).ok()
    }

    /// Decode a raw LaunchLab pool account, failing loudly on a mismatch.
    pub fn decode_pool(data: &[u8]) -> BotResult<PoolState> {
        PoolState::decode(data)
    }

    /// Decode a raw LaunchLab `GlobalConfig` account, failing loudly on a
    /// mismatch.
    pub fn decode_global_config(data: &[u8]) -> BotResult<GlobalConfig> {
        GlobalConfig::decode(data)
    }

    /// Quote a buy: how much base a given quote input buys.
    ///
    /// The fee is taken off the quote input *before* the curve is applied
    /// (the SDK's `buyExactIn`), and the fee is **ceiled**.
    pub fn quote_buy(
        &self,
        pool: &LaunchLabPool,
        amount_in: u64,
        platform_fee_rate: u64,
        creator_fee_rate: u64,
        share_fee_rate: u64,
    ) -> BotResult<u64> {
        let state = pool.curve_state();
        let fee_rate = curve::total_fee_rate(
            pool.trade_fee_rate(),
            platform_fee_rate,
            share_fee_rate,
            creator_fee_rate,
        )?;
        let total_fee = curve::calculate_fee(u128::from(amount_in), fee_rate)?;
        let amount_less_fee = u128::from(amount_in)
            .checked_sub(total_fee)
            .ok_or_else(|| BotError::solana("LaunchLab fee exceeds the input amount"))?;
        let curve_type = pool.curve_type()?;
        let out = match curve_type {
            CurveType::ConstantProduct => {
                ConstantProductCurve::buy_exact_in(&state, amount_less_fee)?
            }
            CurveType::FixedPrice => FixedPriceCurve::buy_exact_in(&state, amount_less_fee)?,
            CurveType::LinearPrice => LinearPriceCurve::buy_exact_in(&state, amount_less_fee)?,
        };
        u64::try_from(out)
            .map_err(|_| BotError::solana("LaunchLab quote output exceeds the u64 range"))
    }

    /// Quote a buy for an exact base output: how much quote it costs.
    pub fn quote_buy_exact_out(
        &self,
        pool: &LaunchLabPool,
        amount_out: u64,
        platform_fee_rate: u64,
        creator_fee_rate: u64,
        share_fee_rate: u64,
    ) -> BotResult<u64> {
        let state = pool.curve_state();
        let curve_type = pool.curve_type()?;
        let amount_less_fee = match curve_type {
            CurveType::ConstantProduct => {
                ConstantProductCurve::buy_exact_out(&state, u128::from(amount_out))?
            }
            CurveType::FixedPrice => {
                FixedPriceCurve::buy_exact_out(&state, u128::from(amount_out))?
            }
            CurveType::LinearPrice => {
                LinearPriceCurve::buy_exact_out(&state, u128::from(amount_out))?
            }
        };
        let fee_rate = curve::total_fee_rate(
            pool.trade_fee_rate(),
            platform_fee_rate,
            share_fee_rate,
            creator_fee_rate,
        )?;
        let gross = curve::calculate_pre_fee(amount_less_fee, fee_rate)?;
        u64::try_from(gross)
            .map_err(|_| BotError::solana("LaunchLab quote input exceeds the u64 range"))
    }

    /// Quote a sell: how much quote a given base input returns.
    ///
    /// The fee is taken off the quote *output* (the SDK's `sellExactIn`).
    pub fn quote_sell(
        &self,
        pool: &LaunchLabPool,
        amount_in: u64,
        platform_fee_rate: u64,
        creator_fee_rate: u64,
        share_fee_rate: u64,
    ) -> BotResult<u64> {
        let state = pool.curve_state();
        let curve_type = pool.curve_type()?;
        let gross = match curve_type {
            CurveType::ConstantProduct => {
                ConstantProductCurve::sell_exact_in(&state, u128::from(amount_in))?
            }
            CurveType::FixedPrice => FixedPriceCurve::sell_exact_in(&state, u128::from(amount_in))?,
            CurveType::LinearPrice => {
                LinearPriceCurve::sell_exact_in(&state, u128::from(amount_in))?
            }
        };
        let fee_rate = curve::total_fee_rate(
            pool.trade_fee_rate(),
            platform_fee_rate,
            share_fee_rate,
            creator_fee_rate,
        )?;
        let total_fee = curve::calculate_fee(gross, fee_rate)?;
        let net = gross
            .checked_sub(total_fee)
            .ok_or_else(|| BotError::solana("LaunchLab fee exceeds the output amount"))?;
        u64::try_from(net)
            .map_err(|_| BotError::solana("LaunchLab quote output exceeds the u64 range"))
    }

    /// Quote a sell for an exact quote output: how much base must be sold.
    pub fn quote_sell_exact_out(
        &self,
        pool: &LaunchLabPool,
        amount_out: u64,
        platform_fee_rate: u64,
        creator_fee_rate: u64,
        share_fee_rate: u64,
    ) -> BotResult<u64> {
        let state = pool.curve_state();
        let curve_type = pool.curve_type()?;
        let fee_rate = curve::total_fee_rate(
            pool.trade_fee_rate(),
            platform_fee_rate,
            share_fee_rate,
            creator_fee_rate,
        )?;
        // The gross-up: the curve must move `gross` out of the quote reserve.
        let gross = curve::calculate_pre_fee(u128::from(amount_out), fee_rate)?;
        let needed = match curve_type {
            CurveType::ConstantProduct => ConstantProductCurve::sell_exact_out(&state, gross)?,
            CurveType::FixedPrice => FixedPriceCurve::sell_exact_out(&state, gross)?,
            CurveType::LinearPrice => LinearPriceCurve::sell_exact_out(&state, gross)?,
        };
        u64::try_from(needed)
            .map_err(|_| BotError::solana("LaunchLab quote input exceeds the u64 range"))
    }

    /// Where the pool's tokens trade now.
    pub fn route(&self, pool: &LaunchLabPool) -> MigrationDecision {
        migration::decide_route(&pool.state)
    }

    /// The adapter to use for the pool's tokens right now.
    pub fn migration_target(&self, pool: &LaunchLabPool) -> MigrationTarget {
        migration::decide_route(&pool.state).target
    }

    /// Whether the pool's curve is still open for trading.
    pub fn is_tradable(&self, pool: &LaunchLabPool) -> bool {
        pool.state.is_curve_open()
    }

    /// Build the buy instruction.
    #[allow(clippy::too_many_arguments)]
    pub fn build_buy_ix(
        &self,
        pool: &LaunchLabPool,
        user: &Pubkey,
        user_base_token: &Pubkey,
        user_quote_token: &Pubkey,
        amount_in: u64,
        min_out: u64,
        share_fee_rate: u64,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        self.build_trade_ix(
            pool,
            user,
            user_base_token,
            user_quote_token,
            amount_in,
            min_out,
            share_fee_rate,
            TradeKind::BuyExactIn,
        )
    }

    /// Build the sell instruction.
    #[allow(clippy::too_many_arguments)]
    pub fn build_sell_ix(
        &self,
        pool: &LaunchLabPool,
        user: &Pubkey,
        user_base_token: &Pubkey,
        user_quote_token: &Pubkey,
        amount_in: u64,
        min_out: u64,
        share_fee_rate: u64,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        self.build_trade_ix(
            pool,
            user,
            user_base_token,
            user_quote_token,
            amount_in,
            min_out,
            share_fee_rate,
            TradeKind::SellExactIn,
        )
    }

    /// Build one of the four trade instructions.
    #[allow(clippy::too_many_arguments)]
    pub fn build_trade_ix(
        &self,
        pool: &LaunchLabPool,
        user: &Pubkey,
        user_base_token: &Pubkey,
        user_quote_token: &Pubkey,
        amount: u64,
        limit: u64,
        share_fee_rate: u64,
        kind: TradeKind,
    ) -> BotResult<solana_sdk::instruction::Instruction> {
        if !pool.state.is_curve_open() {
            return Err(BotError::solana(format!(
                "LaunchLab pool {} is no longer open for trading (status {:?})",
                pool.pool, pool.state.status
            )));
        }
        let authority = self.pool_authority(&pool.state)?;
        let accounts = TradeAccounts {
            payer: *user,
            authority,
            global_config: pool.state.global_config,
            platform_config: pool.state.platform_config,
            pool_state: pool.pool,
            user_base_token: *user_base_token,
            user_quote_token: *user_quote_token,
            base_vault: pool.state.base_vault,
            quote_vault: pool.state.quote_vault,
            base_token_mint: pool.state.base_mint,
            quote_token_mint: pool.state.quote_mint,
            base_token_program: pool.state.base_token_program(),
            quote_token_program: pool.state.quote_token_program(),
            platform_claim_fee_vault: pda::platform_vault(
                &pool.state.platform_config,
                &pool.state.quote_mint,
            ),
            creator_claim_fee_vault: pda::creator_vault(
                &pool.state.creator,
                &pool.state.quote_mint,
            ),
            share_fee_receiver: None,
        };
        instructions::build_trade_ix(
            kind,
            &accounts,
            TradeArgs {
                amount,
                limit,
                share_fee_rate,
            },
        )
    }

    /// The pool's authority PDA, derived from the pool state's stored bump.
    pub fn pool_authority(&self, state: &PoolState) -> BotResult<Pubkey> {
        Pubkey::create_program_address(
            &[RAYDIUM_LAUNCHLAB_SEED_AUTH, &[state.auth_bump]],
            &RAYDIUM_LAUNCHLAB,
        )
        .map_err(|e| BotError::solana(format!("derive LaunchLab pool authority: {e}")))
    }

    /// The canonical LaunchLab authority, which must match
    /// [`pool_authority`](Self::pool_authority) for a healthy pool.
    pub fn canonical_authority(&self) -> Pubkey {
        pda::auth()
    }

    /// The PDA of the pool for a base/quote mint pair.
    pub fn pool_pda(base_mint: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
        pda::pool(base_mint, quote_mint)
    }
}

impl VenueAdapter for RaydiumLaunchLab {
    fn id(&self) -> &'static str {
        "raydium_launchlab"
    }

    fn label(&self) -> &'static str {
        "Raydium LaunchLab"
    }

    fn program_id(&self) -> &Pubkey {
        &RAYDIUM_LAUNCHLAB
    }

    fn layout_verified(&self) -> bool {
        // The LaunchLab layouts come from the official vendored IDL
        // (`idl/raydium_launchpad.json`). Every discriminator and every
        // account length is asserted in the solana-kit test suite.
        true
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    /// Build a swap through the neutral venue request.
    ///
    /// The neutral request does not carry the pool state, so this override
    /// exists so the generic trait path fails with a *precise* error rather
    /// than the generic "unverified" one.
    fn build_swap(&self, _request: &VenueSwapRequest) -> BotResult<Vec<u8>> {
        Err(VenueError::NoPool.into_bot_error(self.id()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::raydium_launchlab::accounts::{
        encode_synthetic_global_config, SyntheticPoolState,
    };
    use crate::venues::raydium_launchlab::migration::LaunchLabRoute;

    fn sample_pool(curve_type: u8) -> LaunchLabPool {
        let s = SyntheticPoolState {
            auth_bump: pda::auth_pda().1,
            ..Default::default()
        };
        let state = PoolState::decode(&s.encode()).unwrap();
        let config = GlobalConfig::decode(&encode_synthetic_global_config(
            curve_type,
            &state.quote_mint,
        ))
        .unwrap();
        LaunchLabPool {
            pool: Pubkey::new_unique(),
            state,
            config,
        }
    }

    #[test]
    fn pool_owner_check_matches_the_pinned_program() {
        let venue = RaydiumLaunchLab::new();
        assert!(venue.is_pool_owner(&RAYDIUM_LAUNCHLAB));
        assert!(!venue.is_pool_owner(&Pubkey::new_unique()));
    }

    #[test]
    fn disabled_adapter_reports_disabled() {
        assert!(!RaydiumLaunchLab::with_enabled(false).enabled());
        assert!(RaydiumLaunchLab::with_enabled(true).enabled());
    }

    #[test]
    fn detect_pool_accepts_a_pool_and_rejects_anything_else() {
        let s = SyntheticPoolState::default();
        let bytes = s.encode();
        assert!(RaydiumLaunchLab::detect_pool(&bytes).is_some());
        assert!(RaydiumLaunchLab::detect_pool(&bytes[..100]).is_none());
        let cfg = encode_synthetic_global_config(0, &Pubkey::new_unique());
        assert!(RaydiumLaunchLab::detect_pool(&cfg).is_none());
        assert!(RaydiumLaunchLab::detect_pool(&[0u8; 530]).is_none());
    }

    #[test]
    fn the_quote_is_grossed_up_by_the_fee() {
        let pool = sample_pool(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        let venue = RaydiumLaunchLab::new();
        // The config's trade fee rate is 2_500 (0.25%).
        let with_fee = venue.quote_buy(&pool, 1_000_000, 0, 0, 0).unwrap();
        let mut no_fee_pool = pool.clone();
        no_fee_pool.config.trade_fee_rate = 0;
        let without_fee = venue.quote_buy(&no_fee_pool, 1_000_000, 0, 0, 0).unwrap();
        assert!(with_fee < without_fee, "the fee must reduce the output");
        // The fee is 2_500 on 1_000_000, so the curve sees 997_500.
        let expected = ConstantProductCurve::buy_exact_in(&pool.curve_state(), 997_500).unwrap();
        assert_eq!(with_fee, expected as u64);
    }

    #[test]
    fn the_sell_quote_is_net_of_the_fee() {
        let pool = sample_pool(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        let venue = RaydiumLaunchLab::new();
        let gross = ConstantProductCurve::sell_exact_in(&pool.curve_state(), 1_000_000).unwrap();
        let fee = curve::calculate_fee(gross, pool.trade_fee_rate()).unwrap();
        let net = venue.quote_sell(&pool, 1_000_000, 0, 0, 0).unwrap();
        assert_eq!(net, (gross - fee) as u64);
    }

    #[test]
    fn every_curve_type_quotes() {
        // The three curves have very different reserve conventions, so each
        // gets a state that its own formula can work with.
        //
        // * constant product: reserves `virtual_quote + real_quote` and
        //   `virtual_base - real_base`.
        // * fixed price: `virtual_quote` is the init input and
        //   `virtual_base` the init output.
        // * linear price: `virtual_base` is `a` (the `Q64` slope), the quote
        //   accumulates in `real_quote` and the base in `real_base`.
        let cases: [(u8, CurveState); 3] = [
            (
                RAYDIUM_LAUNCHLAB_CURVE_CONSTANT,
                CurveState {
                    virtual_base: 1_000_000_000,
                    virtual_quote: 200_000_000,
                    real_base: 0,
                    real_quote: 0,
                    total_base_sell: 1_000_000_000,
                    total_quote_fund_raising: 200_000_000,
                },
            ),
            (
                RAYDIUM_LAUNCHLAB_CURVE_FIXED,
                CurveState {
                    virtual_base: 1_000_000_000,
                    virtual_quote: 200_000_000,
                    real_base: 0,
                    real_quote: 0,
                    total_base_sell: 1_000_000_000,
                    total_quote_fund_raising: 200_000_000,
                },
            ),
            (
                RAYDIUM_LAUNCHLAB_CURVE_LINEAR,
                CurveState {
                    // The SDK's `getInitParam` computes
                    // `a = 2 * Q64 * fundRaising / totalSell^2`; this is a
                    // scaled-down but structurally identical slope.
                    virtual_base: 1_000_000_000_000_000,
                    virtual_quote: 0,
                    // `real_base` is the curve's resting point:
                    // `floor(sqrt(2 * real_quote * Q64 / a))`, so a buy
                    // never underflows and a sell has base to give.
                    real_base: 6_074_000,
                    real_quote: 1_000_000_000,
                    total_base_sell: 1_000_000_000_000,
                    total_quote_fund_raising: 1_000_000_000,
                },
            ),
        ];
        for (curve_type, curve_state) in cases {
            let mut pool = sample_pool(curve_type);
            pool.state.virtual_base = curve_state.virtual_base;
            pool.state.virtual_quote = curve_state.virtual_quote;
            pool.state.real_base = curve_state.real_base;
            pool.state.real_quote = curve_state.real_quote;
            pool.state.total_base_sell = curve_state.total_base_sell;
            pool.state.total_quote_fund_raising = curve_state.total_quote_fund_raising;
            let venue = RaydiumLaunchLab::new();
            let buy = venue.quote_buy(&pool, 1_000_000, 0, 0, 0).unwrap();
            assert!(buy > 0, "curve {curve_type} must quote a buy");
            let sell = venue.quote_sell(&pool, 1_000_000, 0, 0, 0).unwrap();
            assert!(sell > 0, "curve {curve_type} must quote a sell");
        }
    }

    #[test]
    fn the_route_follows_the_pool_status() {
        let venue = RaydiumLaunchLab::new();
        let s = SyntheticPoolState {
            status: RAYDIUM_LAUNCHLAB_STATUS_FUND,
            ..Default::default()
        };
        let state = PoolState::decode(&s.encode()).unwrap();
        let config =
            GlobalConfig::decode(&encode_synthetic_global_config(0, &state.quote_mint)).unwrap();
        let pool = LaunchLabPool {
            pool: Pubkey::new_unique(),
            state,
            config,
        };
        assert_eq!(venue.route(&pool).route, LaunchLabRoute::Curve);
        assert!(venue.is_tradable(&pool));

        let s = SyntheticPoolState {
            status: RAYDIUM_LAUNCHLAB_STATUS_TRADE,
            migrate_type: RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP,
            ..Default::default()
        };
        let state = PoolState::decode(&s.encode()).unwrap();
        let config =
            GlobalConfig::decode(&encode_synthetic_global_config(0, &state.quote_mint)).unwrap();
        let pool = LaunchLabPool {
            pool: Pubkey::new_unique(),
            state,
            config,
        };
        assert_eq!(venue.route(&pool).route, LaunchLabRoute::Cpmm);
        assert_eq!(venue.migration_target(&pool), MigrationTarget::RaydiumCpmm);
        assert!(!venue.is_tradable(&pool));
    }

    #[test]
    fn building_a_trade_on_a_closed_pool_is_refused() {
        let s = SyntheticPoolState {
            status: RAYDIUM_LAUNCHLAB_STATUS_TRADE,
            ..Default::default()
        };
        let state = PoolState::decode(&s.encode()).unwrap();
        let config =
            GlobalConfig::decode(&encode_synthetic_global_config(0, &state.quote_mint)).unwrap();
        let pool = LaunchLabPool {
            pool: Pubkey::new_unique(),
            state,
            config,
        };
        let venue = RaydiumLaunchLab::new();
        let err = venue
            .build_buy_ix(
                &pool,
                &Pubkey::new_unique(),
                &Pubkey::new_unique(),
                &Pubkey::new_unique(),
                1_000,
                900,
                0,
            )
            .unwrap_err();
        assert!(err.to_string().contains("no longer open"), "{err}");
    }

    #[test]
    fn the_built_instruction_uses_the_pool_accounts() {
        let pool = sample_pool(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        let venue = RaydiumLaunchLab::new();
        let user = Pubkey::new_unique();
        let user_base = Pubkey::new_unique();
        let user_quote = Pubkey::new_unique();
        let ix = venue
            .build_buy_ix(&pool, &user, &user_base, &user_quote, 1_000, 900, 0)
            .unwrap();
        assert_eq!(ix.program_id, *RAYDIUM_LAUNCHLAB);
        assert_eq!(ix.accounts[0].pubkey, user);
        assert!(ix.accounts[0].is_signer);
        assert_eq!(ix.accounts[4].pubkey, pool.pool);
        assert_eq!(ix.accounts[5].pubkey, user_base);
        assert_eq!(ix.accounts[6].pubkey, user_quote);
        assert_eq!(&ix.data[..8], &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN);
    }

    #[test]
    fn the_neutral_trait_path_reports_no_pool() {
        let venue = RaydiumLaunchLab::new();
        let req = VenueSwapRequest {
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            buy: true,
            amount_in: 1,
            min_out: 1,
        };
        let err = venue.build_swap(&req).unwrap_err();
        assert!(err.to_string().contains("no pool"), "{err}");
    }
}
