//! Market data loaders: one chain read per protocol, producing the
//! protocol-neutral [`MarketSnapshot`] the gates and the slippage engine
//! consume, plus the venue context the builders need.
//!
//! This is the only I/O between "event validated" and "risk approved". Every
//! read is fresh (never the warm account cache) because the numbers drive a
//! money decision made seconds after a launch, when they move fastest.

use chrono::{DateTime, Utc};
use solana_sdk::pubkey::Pubkey;
use tracing::debug;

use bot_core::config::SniperConfig;
use bot_core::error::BotError;
use bot_core::maths;

use solana_kit::consts::WSOL_MINT;
use solana_kit::jupiter::{Jupiter, JupiterQuote, QuoteRequest};
use solana_kit::pump::PumpContext;
use solana_kit::pumpswap::PumpSwapContext;
use solana_kit::raydium::{PoolSide, RaydiumPool};
use solana_kit::rpc::Rpc;
use solana_kit::tokens::MintInfo;

use crate::event::{LaunchEvent, LaunchProtocol};
use crate::gates::MarketSnapshot;
use crate::pipeline::{select_route, EntryRoute, RejectReason};

/// Venue context the route builder needs, alongside the snapshot.
#[derive(Debug, Clone)]
pub enum VenueData {
    PumpCurve(Box<PumpContext>),
    PumpSwap(Box<PumpSwapContext>),
    Raydium(Box<RaydiumPool>),
    /// Jupiter route: the quote for the sized trade IS the market data.
    Jupiter(Box<JupiterQuote>),
}

/// What [`load_market`] hands back.
#[derive(Debug, Clone)]
pub struct MarketData {
    pub route: EntryRoute,
    pub snapshot: MarketSnapshot,
    pub venue: VenueData,
    /// Mint account state (authorities, supply, decimals) when it was read.
    pub mint: Option<MintInfo>,
}

/// Why the market could not be loaded, already classified for the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketError {
    pub reason: RejectReason,
    pub detail: String,
}

impl MarketError {
    fn unavailable(detail: impl Into<String>) -> Self {
        MarketError {
            reason: RejectReason::ExecutionUnavailable,
            detail: detail.into(),
        }
    }
    fn not_ready(detail: impl Into<String>) -> Self {
        MarketError {
            reason: RejectReason::PoolNotReady,
            detail: detail.into(),
        }
    }
    fn route(detail: impl Into<String>) -> Self {
        MarketError {
            reason: RejectReason::InvalidRoute,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for MarketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason, self.detail)
    }
}

/// Read the mint account. A missing/unparseable mint is reported as `None`
/// so the gates can decide (skip vs strict) rather than the loader.
pub async fn read_mint(rpc: &Rpc, mint: &Pubkey) -> Option<MintInfo> {
    match rpc.get_account(mint).await {
        Ok(Some(acc)) => match MintInfo::parse(&acc.data) {
            Ok(info) => Some(info),
            Err(e) => {
                debug!(%mint, error = %e, "mint account did not parse");
                None
            }
        },
        Ok(None) => {
            debug!(%mint, "mint account not found");
            None
        }
        Err(e) => {
            debug!(%mint, error = %e, "mint account read failed");
            None
        }
    }
}

/// Load the venue for `event`, choose the route and summarise the market.
///
/// `trade_lamports` is the SOL the strategy intends to spend; the Jupiter
/// route quotes exactly that amount so its price impact is the real one.
pub async fn load_market(
    rpc: &Rpc,
    user: &Pubkey,
    event: &LaunchEvent,
    cfg: &SniperConfig,
    trade_lamports: u64,
    now: DateTime<Utc>,
) -> Result<MarketData, MarketError> {
    let mint = event
        .mint_pubkey()
        .ok_or_else(|| MarketError::unavailable(format!("mint {} is not a pubkey", event.mint)))?;
    let creator_buy = (event.launch.initial_buy_sol > 0.0).then_some(event.launch.initial_buy_sol);
    let mint_info = read_mint(rpc, &mint).await;

    match event.protocol {
        LaunchProtocol::PumpFun => {
            let ctx = match PumpContext::load(rpc, &mint, user, None).await {
                Ok(ctx) => ctx,
                Err(BotError::NotFound(m)) => return Err(MarketError::not_ready(m)),
                Err(e) if e.to_string().contains("not found") => {
                    return Err(MarketError::not_ready(e.to_string()))
                }
                Err(e) => return Err(MarketError::unavailable(format!("pump curve: {e}"))),
            };
            let route = select_route(event.protocol, cfg, ctx.curve.complete)
                .map_err(MarketError::route)?;
            match route {
                EntryRoute::PumpCurve => {
                    let curve = &ctx.curve;
                    let snapshot = MarketSnapshot {
                        protocol: event.protocol,
                        pool: Some(ctx.bonding_curve.to_string()),
                        quote_reserve_lamports: curve.real_sol_reserves,
                        pricing_quote_reserve_lamports: curve.virtual_sol_reserves,
                        base_reserve_raw: curve.real_token_reserves,
                        base_decimals: 6,
                        total_supply_raw: Some(curve.token_total_supply)
                            .filter(|s| *s > 0)
                            .or(mint_info.map(|m| m.supply)),
                        fee_bps: ctx.global_state.fee_basis_points
                            + curve
                                .creator_fee_bps
                                .unwrap_or(ctx.global_state.creator_fee_basis_points),
                        tradable: !curve.complete,
                        tradable_detail: if curve.complete {
                            "bonding curve is complete (token graduated)".into()
                        } else {
                            String::new()
                        },
                        pool_open_time: None,
                        mint_authority_revoked: mint_info.map(|m| m.mint_authority_revoked()),
                        freeze_authority_revoked: mint_info.map(|m| m.freeze_authority_revoked()),
                        creator_initial_buy_sol: creator_buy,
                        spot_price_sol: maths::pump_spot_price_sol(
                            curve.virtual_sol_reserves,
                            curve.virtual_token_reserves,
                        ),
                        fetched_at: now,
                    };
                    Ok(MarketData {
                        route,
                        snapshot,
                        venue: VenueData::PumpCurve(Box::new(ctx)),
                        mint: mint_info,
                    })
                }
                EntryRoute::PumpSwapDirect => {
                    load_pumpswap(rpc, user, event, &mint, None, mint_info, creator_buy, now).await
                }
                EntryRoute::Jupiter => {
                    load_jupiter(event, &mint, trade_lamports, mint_info, creator_buy, now).await
                }
                EntryRoute::RaydiumV4Direct => Err(MarketError::route(
                    "pump.fun launches never route to Raydium directly",
                )),
            }
        }
        LaunchProtocol::PumpSwap => {
            let route = select_route(event.protocol, cfg, true).map_err(MarketError::route)?;
            match route {
                EntryRoute::PumpSwapDirect => {
                    load_pumpswap(
                        rpc,
                        user,
                        event,
                        &mint,
                        event.pool_pubkey(),
                        mint_info,
                        creator_buy,
                        now,
                    )
                    .await
                }
                EntryRoute::Jupiter => {
                    load_jupiter(event, &mint, trade_lamports, mint_info, creator_buy, now).await
                }
                other => Err(MarketError::route(format!(
                    "pump_swap launches cannot execute on {other}"
                ))),
            }
        }
        LaunchProtocol::RaydiumAmmV4 => {
            let route = select_route(event.protocol, cfg, true).map_err(MarketError::route)?;
            match route {
                EntryRoute::RaydiumV4Direct => {
                    let amm_id = event.pool_pubkey().ok_or_else(|| {
                        MarketError::unavailable("raydium event carries no pool address")
                    })?;
                    load_raydium(rpc, &amm_id, &mint, event, mint_info, creator_buy, now).await
                }
                EntryRoute::Jupiter => {
                    load_jupiter(event, &mint, trade_lamports, mint_info, creator_buy, now).await
                }
                other => Err(MarketError::route(format!(
                    "raydium_amm_v4 launches cannot execute on {other}"
                ))),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn load_pumpswap(
    rpc: &Rpc,
    user: &Pubkey,
    event: &LaunchEvent,
    mint: &Pubkey,
    pool: Option<Pubkey>,
    mint_info: Option<MintInfo>,
    creator_buy: Option<f64>,
    now: DateTime<Utc>,
) -> Result<MarketData, MarketError> {
    let ctx = match PumpSwapContext::load(rpc, mint, user, pool).await {
        Ok(ctx) => ctx,
        Err(BotError::NotFound(m)) => return Err(MarketError::not_ready(m)),
        Err(e) if e.to_string().contains("not found") => {
            return Err(MarketError::not_ready(e.to_string()))
        }
        Err(e) => return Err(MarketError::unavailable(format!("pumpswap pool: {e}"))),
    };
    if ctx.quote_mint != *WSOL_MINT {
        return Err(MarketError::route(format!(
            "pumpswap pool {} is quoted in {}, not SOL",
            ctx.pool, ctx.quote_mint
        )));
    }
    let buys_disabled = ctx.config.buys_disabled();
    let snapshot = MarketSnapshot {
        protocol: event.protocol,
        pool: Some(ctx.pool.to_string()),
        quote_reserve_lamports: ctx.quote_reserve,
        pricing_quote_reserve_lamports: ctx.quote_reserve,
        base_reserve_raw: ctx.base_reserve,
        base_decimals: ctx.base_decimals,
        total_supply_raw: mint_info.map(|m| m.supply).filter(|s| *s > 0),
        fee_bps: ctx.config.total_fee_bps(),
        tradable: !buys_disabled,
        tradable_detail: if buys_disabled {
            "pumpswap global config has buys disabled".into()
        } else {
            String::new()
        },
        pool_open_time: None,
        mint_authority_revoked: mint_info.map(|m| m.mint_authority_revoked()),
        freeze_authority_revoked: mint_info.map(|m| m.freeze_authority_revoked()),
        creator_initial_buy_sol: creator_buy,
        spot_price_sol: ctx.price(),
        fetched_at: now,
    };
    Ok(MarketData {
        route: EntryRoute::PumpSwapDirect,
        snapshot,
        venue: VenueData::PumpSwap(Box::new(ctx)),
        mint: mint_info,
    })
}

async fn load_raydium(
    rpc: &Rpc,
    amm_id: &Pubkey,
    mint: &Pubkey,
    event: &LaunchEvent,
    mint_info: Option<MintInfo>,
    creator_buy: Option<f64>,
    now: DateTime<Utc>,
) -> Result<MarketData, MarketError> {
    let pool = match RaydiumPool::load(rpc, amm_id, true).await {
        Ok(p) => p,
        Err(BotError::NotFound(m)) => return Err(MarketError::not_ready(m)),
        Err(e) => return Err(MarketError::unavailable(format!("raydium pool: {e}"))),
    };
    let amm = &pool.amm;
    let (quote_reserve, base_reserve, base_decimals, spot) = match amm.side_of(mint) {
        Some(PoolSide::Coin) if amm.pc_mint == *WSOL_MINT => (
            pool.pc_vault_balance,
            pool.coin_vault_balance,
            u8::try_from(amm.coin_decimals).unwrap_or(u8::MAX),
            pool.price_pc_per_coin(),
        ),
        Some(PoolSide::Pc) if amm.coin_mint == *WSOL_MINT => {
            let p = pool.price_pc_per_coin();
            (
                pool.coin_vault_balance,
                pool.pc_vault_balance,
                u8::try_from(amm.pc_decimals).unwrap_or(u8::MAX),
                if p > 0.0 { 1.0 / p } else { 0.0 },
            )
        }
        Some(_) => {
            return Err(MarketError::route(format!(
                "raydium pool {amm_id} is not a SOL pair ({} / {})",
                amm.coin_mint, amm.pc_mint
            )))
        }
        None => {
            return Err(MarketError::route(format!(
                "raydium pool {amm_id} does not contain mint {mint}"
            )))
        }
    };
    let swappable = amm.is_swappable();
    let snapshot = MarketSnapshot {
        protocol: event.protocol,
        pool: Some(amm_id.to_string()),
        quote_reserve_lamports: quote_reserve,
        pricing_quote_reserve_lamports: quote_reserve,
        base_reserve_raw: base_reserve,
        base_decimals,
        total_supply_raw: mint_info.map(|m| m.supply).filter(|s| *s > 0),
        fee_bps: amm.trade_fee_bps(),
        tradable: swappable,
        tradable_detail: if swappable {
            String::new()
        } else {
            format!("raydium amm status {} does not allow swaps", amm.status)
        },
        pool_open_time: Some(amm.pool_open_time),
        mint_authority_revoked: mint_info.map(|m| m.mint_authority_revoked()),
        freeze_authority_revoked: mint_info.map(|m| m.freeze_authority_revoked()),
        creator_initial_buy_sol: creator_buy,
        spot_price_sol: spot,
        fetched_at: now,
    };
    Ok(MarketData {
        route: EntryRoute::RaydiumV4Direct,
        snapshot,
        venue: VenueData::Raydium(Box::new(pool)),
        mint: mint_info,
    })
}

async fn load_jupiter(
    event: &LaunchEvent,
    mint: &Pubkey,
    trade_lamports: u64,
    mint_info: Option<MintInfo>,
    creator_buy: Option<f64>,
    now: DateTime<Utc>,
) -> Result<MarketData, MarketError> {
    if trade_lamports == 0 {
        return Err(MarketError::unavailable(
            "trade size rounds to zero lamports",
        ));
    }
    let quote = Jupiter::new()
        .quote(&QuoteRequest::new(*WSOL_MINT, *mint, trade_lamports))
        .await
        .map_err(|e| MarketError::unavailable(format!("jupiter quote: {e}")))?;
    let out = quote
        .out_amount_u64()
        .map_err(|e| MarketError::unavailable(format!("jupiter quote out amount: {e}")))?;
    let decimals = mint_info
        .map(|m| m.decimals)
        .or(event.base_decimals)
        .unwrap_or(6);
    // Jupiter reports the impact of THIS trade (in percent); invert the
    // constant-product relation `impact = t / (R + t)` to a modelled quote
    // reserve so the slippage engine and gates see the same shape as a
    // direct pool.
    let impact = (quote.price_impact_pct() / 100.0).clamp(0.0, 1.0);
    let modelled_reserve = if impact <= 0.0 {
        u64::MAX / 4
    } else {
        let r = trade_lamports as f64 * (1.0 - impact) / impact;
        if r.is_finite() && r >= 0.0 {
            r.min((u64::MAX / 4) as f64) as u64
        } else {
            0
        }
    };
    let exec_price = if out > 0 {
        maths::lamports_to_sol(trade_lamports) / maths::from_raw_amount(out, decimals)
    } else {
        0.0
    };
    let spot = if impact < 1.0 {
        exec_price * (1.0 - impact)
    } else {
        0.0
    };
    let snapshot = MarketSnapshot {
        protocol: event.protocol,
        pool: None,
        quote_reserve_lamports: modelled_reserve,
        pricing_quote_reserve_lamports: modelled_reserve,
        base_reserve_raw: out,
        base_decimals: decimals,
        total_supply_raw: mint_info.map(|m| m.supply).filter(|s| *s > 0),
        fee_bps: quote
            .platform_fee
            .as_ref()
            .and_then(|f| f.fee_bps)
            .unwrap_or(0),
        tradable: out > 0,
        tradable_detail: if out > 0 {
            String::new()
        } else {
            "jupiter quoted zero output".into()
        },
        pool_open_time: None,
        mint_authority_revoked: mint_info.map(|m| m.mint_authority_revoked()),
        freeze_authority_revoked: mint_info.map(|m| m.freeze_authority_revoked()),
        creator_initial_buy_sol: creator_buy,
        spot_price_sol: spot,
        fetched_at: now,
    };
    Ok(MarketData {
        route: EntryRoute::Jupiter,
        snapshot,
        venue: VenueData::Jupiter(Box::new(quote)),
        mint: mint_info,
    })
}
