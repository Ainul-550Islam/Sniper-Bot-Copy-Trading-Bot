//! Market data loaders: one chain read per protocol, producing the
//! protocol-neutral [`MarketSnapshot`] the gates and the slippage engine
//! consume, plus the venue context the builders need.
//!
//! This is the only I/O between "event validated" and "risk approved". Every
//! read is fresh (never the warm account cache) because the numbers drive a
//! money decision made seconds after a launch, when they move fastest.

use chrono::{DateTime, Utc};
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use std::str::FromStr;

use tracing::debug;

use bot_core::config::SniperConfig;
use bot_core::error::BotError;
use bot_core::maths;

use solana_kit::consts::WSOL_MINT;
use solana_kit::holders::{fetch_largest_holders, HolderSnapshot};
use solana_kit::jupiter::{Jupiter, JupiterQuote, QuoteRequest};
use solana_kit::layout::LayoutStore;
use solana_kit::pump::{self, BuildOptions, PumpContext};
use solana_kit::pumpswap::{self, PumpSwapContext};
use solana_kit::raydium::{PoolSide, RaydiumPool};
use solana_kit::rpc::Rpc;
use solana_kit::token_safety::{
    classify_sell_probe, SellProbeOutcome, TokenSafetyAuditor, TokenSafetyReport,
};
use solana_kit::tokens::MintInfo;

use crate::event::{LaunchEvent, LaunchProtocol};
use crate::gates::{MarketSnapshot, TokenHolder};
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
    /// Authority-only safety audit (GAP-MAP P1): the TokenSafetyAuditor
    /// wired to what the mint account proves. `None` when the mint could
    /// not be read. Holder/LP/bundler signals are NOT invented here.
    pub safety: Option<TokenSafetyReport>,
}

/// Authority-only token-safety audit for whatever the mint read proved.
fn safety_report(mint: &Pubkey, mint_info: Option<MintInfo>) -> Option<TokenSafetyReport> {
    let info = mint_info?;
    Some(TokenSafetyAuditor::audit_authorities(
        mint,
        info.mint_authority.as_ref(),
        info.freeze_authority.as_ref(),
    ))
}

/// Simulate one probe instruction and classify the outcome.
///
/// The probe is signed with a DEFAULT signature against a ZERO blockhash:
/// `Rpc::simulate` sets `sig_verify: false` and
/// `replace_recent_blockhash: true`, so the node fills in a fresh hash and
/// never checks the signature — the probe needs no wallet and can never
/// land on chain (it is only simulated).
async fn simulate_probe(rpc: &Rpc, ix: Instruction, payer: &Pubkey) -> SellProbeOutcome {
    let message = Message::new_with_blockhash(std::slice::from_ref(&ix), Some(payer), &Hash::default());
    let tx = VersionedTransaction {
        signatures: vec![Signature::default()],
        message: VersionedMessage::Legacy(message),
    };
    match rpc.simulate(&tx).await {
        Ok(resp) => {
            let logs = resp.value.logs.clone().unwrap_or_default();
            match resp.value.err.as_ref() {
                None => SellProbeOutcome::Sellable,
                Some(err) => match serde_json::to_value(err) {
                    Ok(value) => classify_sell_probe(Some(&value), &logs),
                    // Not serialisable: report the debug shape verbatim and
                    // stay inconclusive rather than guess.
                    Err(_) => SellProbeOutcome::Unknown(format!(
                        "sell simulation failed: {err:?}"
                    )),
                },
            }
        }
        Err(e) => SellProbeOutcome::Unknown(format!("simulateTransaction transport error: {e}")),
    }
}

/// Honeypot probe for a live pump.fun bonding curve (GAP-MAP P1): build the
/// curve's own SELL instruction for one token and let the chain say whether
/// the sell path works. The probe holds no tokens, so a healthy curve
/// answers with the SPL "insufficient funds" error — proof the instruction,
/// program id and account wiring are correct (see
/// [`classify_sell_probe`]). The real position will hold the tokens.
pub async fn probe_curve_sell(rpc: &Rpc, ctx: &PumpContext, user: &Pubkey) -> SellProbeOutcome {
    let store = LayoutStore::default(); // variants fall back to the default layout
    let ix = match pump::build_sell_ix(ctx, &store, &BuildOptions::default(), 1, 1) {
        Ok(ix) => ix,
        Err(e) => {
            return SellProbeOutcome::Unknown(format!("could not build probe sell: {e}"));
        }
    };
    simulate_probe(rpc, ix, user).await
}

/// Honeypot probe for a PumpSwap pool: same idea through the AMM's own
/// sell instruction. A pool with sells disabled refuses at build time.
pub async fn probe_pumpswap_sell(
    rpc: &Rpc,
    ctx: &PumpSwapContext,
    user: &Pubkey,
) -> SellProbeOutcome {
    let store = LayoutStore::default();
    let ix = match pumpswap::build_sell_ix(ctx, &store, 1, 1) {
        Ok(ix) => ix,
        Err(e) => {
            return SellProbeOutcome::Unknown(format!("could not build probe sell: {e}"));
        }
    };
    simulate_probe(rpc, ix, user).await
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

/// The incinerator. Tokens sent here are gone, so they never count as a
/// concentrated holder.
const BURN_ADDRESS: &str = "1nc1nerator11111111111111111111111111111111";

/// Convert a holder table into gate holders. `supply_raw` is the mint's total
/// supply; `infrastructure` lists the venue's own accounts (bonding curve,
/// pool vaults) that hold the float and must not count toward concentration.
///
/// Returns `None` when the supply is unknown (zero). A share cannot be
/// computed without it, so the holder gate must see "no data" and refuse
/// when enabled, rather than a table of zero percentages that would pass.
pub(crate) fn to_gate_holders(
    holders: &[HolderSnapshot],
    supply_raw: u64,
    infrastructure: &[Pubkey],
) -> Option<Vec<TokenHolder>> {
    if supply_raw == 0 {
        return None;
    }
    let burn = Pubkey::from_str(BURN_ADDRESS).ok();
    let supply = supply_raw as f64;
    Some(
        holders
            .iter()
            .map(|h| TokenHolder {
                address: h.address.to_string(),
                pct_of_supply: (h.amount as f64 / supply) * 100.0,
                is_infrastructure: infrastructure.contains(&h.address) || Some(h.address) == burn,
            })
            .collect(),
    )
}

/// Read the largest token accounts of `mint` and convert them for the holder
/// gate.
///
/// * Gate disabled (`max_top_holder_pct <= 0`): no RPC call, `None`.
/// * RPC read fails or supply is unknown: `None`. An enabled gate then fails
///   closed (see `gates.rs`, holder concentration).
///
/// The read is token-account level, the granularity `getTokenLargestAccounts`
/// reports. Owner-level aggregation is not performed here.
async fn load_top_holders(
    rpc: &Rpc,
    mint: &Pubkey,
    supply_raw: Option<u64>,
    cfg: &SniperConfig,
    infrastructure: &[Pubkey],
) -> Option<Vec<TokenHolder>> {
    if cfg.max_top_holder_pct <= 0.0 {
        return None;
    }
    let supply = supply_raw.filter(|s| *s > 0)?;
    match fetch_largest_holders(rpc, mint).await {
        Ok(holders) => to_gate_holders(&holders, supply, infrastructure),
        Err(error) => {
            debug!(%mint, %error, "holder table read failed; holder gate fails closed if enabled");
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
    platform_fee_cfg: &bot_core::config::PlatformFeeConfig,
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
                    // Honeypot probe (GAP-MAP P1): before committing, let
                    // the curve itself prove its sell path works.
                    let sell_probe = if cfg.simulate_sell {
                        Some(probe_curve_sell(rpc, &ctx, user).await)
                    } else {
                        None
                    };
                    let total_supply_raw = Some(curve.token_total_supply)
                        .filter(|s| *s > 0)
                        .or(mint_info.map(|m| m.supply));
                    let top_holders = load_top_holders(
                        rpc,
                        &mint,
                        total_supply_raw,
                        cfg,
                        &[ctx.bonding_curve, ctx.associated_bonding_curve],
                    )
                    .await;
                    let snapshot = MarketSnapshot {
                        protocol: event.protocol,
                        pool: Some(ctx.bonding_curve.to_string()),
                        quote_reserve_lamports: curve.real_sol_reserves,
                        pricing_quote_reserve_lamports: curve.virtual_sol_reserves,
                        base_reserve_raw: curve.real_token_reserves,
                        base_decimals: 6,
                        total_supply_raw,
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
                        sell_probe,
                        creator_initial_buy_sol: creator_buy,
                        top_holders,
                        // Bundler signal is not wired yet (first-slot data).
                        bundling: None,
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
                        safety: safety_report(&mint, mint_info),
                        mint: mint_info,
                    })
                }
                EntryRoute::PumpSwapDirect => {
                    load_pumpswap(rpc, user, event, &mint, None, cfg, mint_info, creator_buy, now)
                        .await
                }
                EntryRoute::Jupiter => {
                    load_jupiter(event, &mint, trade_lamports, cfg, &platform_fee_cfg, mint_info, creator_buy, now).await
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
                        cfg,
                        mint_info,
                        creator_buy,
                        now,
                    )
                    .await
                }
                EntryRoute::Jupiter => {
                    load_jupiter(event, &mint, trade_lamports, cfg, &platform_fee_cfg, mint_info, creator_buy, now).await
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
                    load_raydium(rpc, &amm_id, &mint, event, cfg, mint_info, creator_buy, now).await
                }
                EntryRoute::Jupiter => {
                    load_jupiter(event, &mint, trade_lamports, cfg, &platform_fee_cfg, mint_info, creator_buy, now).await
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
    cfg: &SniperConfig,
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
    // Honeypot probe through the AMM's own sell instruction (GAP-MAP P1).
    let sell_probe = if cfg.simulate_sell {
        Some(probe_pumpswap_sell(rpc, &ctx, user).await)
    } else {
        None
    };
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
        sell_probe,
        creator_initial_buy_sol: creator_buy,
        top_holders: load_top_holders(
            rpc,
            mint,
            mint_info.map(|m| m.supply),
            cfg,
            &[ctx.pool, ctx.pool_base_token_account],
        )
        .await,
        // Bundler signal is not wired yet (first-slot data).
        bundling: None,
        spot_price_sol: ctx.price(),
        fetched_at: now,
    };
    Ok(MarketData {
        route: EntryRoute::PumpSwapDirect,
        snapshot,
        venue: VenueData::PumpSwap(Box::new(ctx)),
        safety: safety_report(mint, mint_info),
        mint: mint_info,
    })
}

async fn load_raydium(
    rpc: &Rpc,
    amm_id: &Pubkey,
    mint: &Pubkey,
    event: &LaunchEvent,
    cfg: &SniperConfig,
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
    // Raydium AMM v4 has no simulated sell probe yet: the AMM-status gate
    // (`tradable`) covers the venue side, and the sell-simulation gate
    // reports this honestly as inconclusive instead of faking a pass.
    let sell_probe = Some(SellProbeOutcome::Unknown(
        "sell simulation is not implemented for raydium_amm_v4".into(),
    ));
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
        sell_probe,
        creator_initial_buy_sol: creator_buy,
        top_holders: load_top_holders(
            rpc,
            mint,
            mint_info.map(|m| m.supply),
            cfg,
            &[amm.coin_vault, amm.pc_vault],
        )
        .await,
        // Bundler signal is not wired yet (first-slot data).
        bundling: None,
        spot_price_sol: spot,
        fetched_at: now,
    };
    Ok(MarketData {
        route: EntryRoute::RaydiumV4Direct,
        snapshot,
        venue: VenueData::Raydium(Box::new(pool)),
        safety: safety_report(mint, mint_info),
        mint: mint_info,
    })
}

async fn load_jupiter(
    event: &LaunchEvent,
    mint: &Pubkey,
    trade_lamports: u64,
    cfg: &SniperConfig,
    platform_fee: &bot_core::config::PlatformFeeConfig,
    mint_info: Option<MintInfo>,
    creator_buy: Option<f64>,
    now: DateTime<Utc>,
) -> Result<MarketData, MarketError> {
    if trade_lamports == 0 {
        return Err(MarketError::unavailable(
            "trade size rounds to zero lamports",
        ));
    }
    // Atomic platform fee, aggregator path (GAP-MAP P1): Jupiter deducts
    // the fee inside its own program when the quote carries it.
    let quote_request = solana_kit::fee_transfer::apply_jupiter_fee(
        QuoteRequest::new(*WSOL_MINT, *mint, trade_lamports),
        platform_fee,
    );
    let _ = cfg; // reserved for future venue-specific quote policy
    let quote = Jupiter::new()
        .quote(&quote_request)
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
        // The aggregator route is venue-neutral: there is no single venue
        // instruction to probe, so the gate sees an honest Unknown.
        sell_probe: Some(SellProbeOutcome::Unknown(
            "sell simulation is not implemented for jupiter routes".into(),
        )),
        creator_initial_buy_sol: creator_buy,
        // The aggregator route does not expose the venue's vault accounts, so
        // holder concentration cannot exclude the pool. It stays None: an enabled
        // holder gate fails closed on this route.
        top_holders: None,
        bundling: None,
        spot_price_sol: spot,
        fetched_at: now,
    };
    Ok(MarketData {
        route: EntryRoute::Jupiter,
        snapshot,
        venue: VenueData::Jupiter(Box::new(quote)),
        safety: safety_report(mint, mint_info),
        mint: mint_info,
    })
}

#[cfg(test)]
mod holder_wiring_tests {
    use super::*;

    fn key(seed: u8) -> Pubkey {
        Pubkey::new_from_array([seed; 32])
    }

    #[test]
    fn percentages_are_share_of_total_supply() {
        let holders = vec![
            HolderSnapshot { address: key(1), amount: 300 },
            HolderSnapshot { address: key(2), amount: 50 },
        ];
        let out = to_gate_holders(&holders, 1_000, &[]).expect("supply is known");
        assert_eq!(out.len(), 2);
        assert!((out[0].pct_of_supply - 30.0).abs() < 1e-9);
        assert!((out[1].pct_of_supply - 5.0).abs() < 1e-9);
        assert!(!out[0].is_infrastructure);
    }

    #[test]
    fn venue_accounts_and_burn_are_marked_infrastructure() {
        let burn = Pubkey::from_str(BURN_ADDRESS).expect("burn constant parses");
        let holders = vec![
            HolderSnapshot { address: key(1), amount: 900 },
            HolderSnapshot { address: burn, amount: 80 },
            HolderSnapshot { address: key(2), amount: 10 },
        ];
        let out = to_gate_holders(&holders, 1_000, &[key(1)]).expect("supply is known");
        assert!(out[0].is_infrastructure, "venue vault is infrastructure");
        assert!(out[1].is_infrastructure, "burn address is infrastructure");
        assert!(!out[2].is_infrastructure, "a real holder is not infrastructure");
    }

    #[test]
    fn unknown_supply_yields_no_table_so_the_gate_fails_closed() {
        let holders = vec![HolderSnapshot { address: key(1), amount: 1 }];
        assert!(to_gate_holders(&holders, 0, &[]).is_none());
    }

    #[test]
    fn empty_holder_table_is_an_honest_empty_table() {
        let out = to_gate_holders(&[], 1_000, &[]).expect("supply is known");
        assert!(out.is_empty());
    }
}
