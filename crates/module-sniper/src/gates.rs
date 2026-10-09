//! Configurable, testable, observable safety gates (TASK 2 §G).
//!
//! The gates are pure functions over a [`MarketSnapshot`] — a protocol-neutral
//! summary of what the chain said about the token and its pool at one instant
//! — and the live [`SniperConfig`]. Loading the snapshot (RPC reads, protocol
//! decoding) happens in `market.rs`; deciding happens here, so every rule is
//! exercised by unit tests and by the replay system without a network.
//!
//! Each gate yields one of three outcomes:
//!
//! * `Pass` — the datum was available and within limits;
//! * `Fail(detail)` — the datum was available and outside limits;
//! * `Skip(detail)` — the protocol/feed does not expose the datum. Skips are
//!   counted (`sniper_gate_results_total{gate,outcome="skip"}`) and become
//!   failures when `sniper.strict_gates = true`.
//!
//! The report keeps every result, not just the first failure, so an operator
//! can see *all* the reasons a launch was refused.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use bot_core::config::SniperConfig;
use bot_core::maths;

use solana_kit::token_safety::SellProbeOutcome;

use crate::event::LaunchProtocol;
use crate::pipeline::RejectReason;

/// Base-token decimals above this are treated as corrupt mint data.
pub const MAX_SANE_DECIMALS: u8 = 12;

/// One entry of the top-holder table used by the holder-concentration
/// gate (GAP-MAP P1). Populated by the holder-list ingestor (data-plane
/// work); the gate logic itself is pure over this struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenHolder {
    /// Wallet or account address holding the tokens.
    pub address: String,
    /// Share of the total supply, in percent (0..=100).
    pub pct_of_supply: f64,
    /// `true` for infrastructure accounts that must NOT count toward
    /// concentration: the bonding curve itself, the graduated pool, AMM
    /// vaults, burn addresses, the pump.fun AMM program's holdings.
    #[serde(default)]
    pub is_infrastructure: bool,
}

/// First-slot bundler signal (GAP-MAP P1): computed by the launch-event
/// analyser from the creation transaction's inner buys. Pure data — the
/// gate decides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundlingSignal {
    /// How many distinct wallets bought in the token's first slot.
    pub first_slot_buyers: u64,
    /// Of those, how many are attributed to one coordinated bundler
    /// (same funding source / same Jito tip pattern / same slot cluster).
    pub bundled_wallets: u64,
}

/// Protocol-neutral view of the market at one instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub protocol: LaunchProtocol,
    /// Bonding curve or AMM pool the numbers were read from.
    pub pool: Option<String>,
    /// SOL that can actually leave the venue on a sell, in lamports: the
    /// curve's REAL SOL reserves or the AMM's quote vault balance.
    pub quote_reserve_lamports: u64,
    /// Reserve the price model runs against, in lamports: the curve's
    /// VIRTUAL SOL reserves (x·y=k is defined on the virtual pair) or, for
    /// an AMM, the same vault balance as above.
    pub pricing_quote_reserve_lamports: u64,
    /// Base tokens on the venue side, raw units.
    pub base_reserve_raw: u64,
    pub base_decimals: u8,
    /// Total supply of the base token in raw units, when known.
    pub total_supply_raw: Option<u64>,
    /// Venue fee taken on a buy, in basis points.
    pub fee_bps: u64,
    /// `true` when the venue accepts a buy right now (curve not complete,
    /// AMM status swappable, buys not disabled).
    pub tradable: bool,
    /// Why `tradable` is false (empty when it is true).
    pub tradable_detail: String,
    /// Raydium `pool_open_time` (unix seconds); `None` for the others.
    pub pool_open_time: Option<u64>,
    /// Mint-authority state when the mint account was read.
    pub mint_authority_revoked: Option<bool>,
    /// Freeze-authority state when the mint account was read.
    pub freeze_authority_revoked: Option<bool>,
    /// Sell-path simulation result (GAP-MAP P1): did the venue's own sell
    /// instruction survive `simulateTransaction`? `None` = the probe was
    /// not run (disabled or unsupported venue). `serde(default)` keeps
    /// pre-P1 replay fixtures loading.
    #[serde(default)]
    pub sell_probe: Option<SellProbeOutcome>,
    /// The creator's opening buy in SOL, when the source reports it.
    pub creator_initial_buy_sol: Option<f64>,
    /// Top-holder table (GAP-MAP P1): the largest token holders with their
    /// share of supply. `None` = the data plane has not ingested holders
    /// for this venue yet (ingestion ships with the P2 data-plane work).
    /// Infrastructure accounts (bonding curve, pool, AMM vaults, burn) are
    /// flagged by the ingestor and excluded from the concentration check.
    /// `serde(default)` keeps pre-P1 replay fixtures loading.
    #[serde(default)]
    pub top_holders: Option<Vec<TokenHolder>>,
    /// Bundler signal (GAP-MAP P1): how many of the first-slot buyers look
    /// bundler-coordinated, when the launch feed reports it. `None` = not
    /// available for this source (same serde rationale as `top_holders`).
    #[serde(default)]
    pub bundling: Option<BundlingSignal>,
    /// Spot price in SOL per whole token.
    pub spot_price_sol: f64,
    pub fetched_at: DateTime<Utc>,
}

impl MarketSnapshot {
    /// Age of the snapshot at `now`, in milliseconds.
    pub fn age_ms(&self, now: DateTime<Utc>) -> u64 {
        now.signed_duration_since(self.fetched_at)
            .num_milliseconds()
            .max(0) as u64
    }

    /// Fraction of the total supply sitting on the venue side, if known.
    pub fn pool_supply_fraction(&self) -> Option<f64> {
        let supply = self.total_supply_raw?;
        if supply == 0 {
            return None;
        }
        Some(self.base_reserve_raw as f64 / supply as f64)
    }

    /// Quote reserve in SOL (human units).
    pub fn quote_reserve_sol(&self) -> f64 {
        maths::lamports_to_sol(self.quote_reserve_lamports)
    }
}

/// Outcome of one gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "detail")]
pub enum GateOutcome {
    Pass,
    Fail(String),
    Skip(String),
}

impl GateOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateOutcome::Pass => "pass",
            GateOutcome::Fail(_) => "fail",
            GateOutcome::Skip(_) => "skip",
        }
    }
}

/// Stable gate identifiers (metric label values and audit text).
pub const GATE_POOL_STATE: &str = "pool_state";
pub const GATE_POOL_OPEN_TIME: &str = "pool_open_time";
pub const GATE_MINT_AUTHORITY: &str = "mint_authority";
pub const GATE_FREEZE_AUTHORITY: &str = "freeze_authority";
pub const GATE_SELL_SIMULATION: &str = "sell_simulation";
pub const GATE_MIN_LIQUIDITY: &str = "min_liquidity";
pub const GATE_PRICE_SANE: &str = "price_sane";
pub const GATE_DECIMALS_SANE: &str = "decimals_sane";
pub const GATE_CREATOR_CONCENTRATION: &str = "creator_concentration";
pub const GATE_HOLDER_CONCENTRATION: &str = "holder_concentration";
pub const GATE_BUNDLER_DETECTION: &str = "bundler_detection";
pub const GATE_POOL_SUPPLY_FRACTION: &str = "pool_supply_fraction";
pub const GATE_SNAPSHOT_FRESHNESS: &str = "snapshot_freshness";

/// All gates, in evaluation order.
pub const ALL_GATES: &[&str] = &[
    GATE_POOL_STATE,
    GATE_POOL_OPEN_TIME,
    GATE_MINT_AUTHORITY,
    GATE_FREEZE_AUTHORITY,
    GATE_SELL_SIMULATION,
    GATE_MIN_LIQUIDITY,
    GATE_PRICE_SANE,
    GATE_DECIMALS_SANE,
    GATE_CREATOR_CONCENTRATION,
    GATE_HOLDER_CONCENTRATION,
    GATE_BUNDLER_DETECTION,
    GATE_POOL_SUPPLY_FRACTION,
    GATE_SNAPSHOT_FRESHNESS,
];

/// The machine-readable reason a failing gate produces.
pub fn reason_for_gate(gate: &str) -> RejectReason {
    match gate {
        GATE_POOL_STATE | GATE_POOL_OPEN_TIME => RejectReason::PoolNotReady,
        GATE_MINT_AUTHORITY | GATE_FREEZE_AUTHORITY | GATE_DECIMALS_SANE => {
            RejectReason::TokenStateInvalid
        }
        GATE_SELL_SIMULATION => RejectReason::HoneypotRisk,
        GATE_MIN_LIQUIDITY | GATE_PRICE_SANE => RejectReason::InsufficientLiquidity,
        GATE_CREATOR_CONCENTRATION
        | GATE_HOLDER_CONCENTRATION
        | GATE_BUNDLER_DETECTION
        | GATE_POOL_SUPPLY_FRACTION => RejectReason::ConcentrationLimit,
        GATE_SNAPSHOT_FRESHNESS => RejectReason::StaleEvent,
        _ => RejectReason::InvalidState,
    }
}

/// Every gate's result for one snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReport {
    pub results: Vec<(String, GateOutcome)>,
}

impl GateReport {
    fn push(&mut self, gate: &str, outcome: GateOutcome) {
        self.results.push((gate.to_string(), outcome));
    }

    /// The first failing gate (skips promoted to failures under
    /// `strict_gates`), with its detail.
    pub fn first_failure(&self, strict: bool) -> Option<(&str, String)> {
        self.results.iter().find_map(|(g, o)| match o {
            GateOutcome::Fail(d) => Some((g.as_str(), d.clone())),
            GateOutcome::Skip(d) if strict => Some((g.as_str(), format!("strict_gates: {d}"))),
            _ => None,
        })
    }

    pub fn passed(&self, strict: bool) -> bool {
        self.first_failure(strict).is_none()
    }

    pub fn skipped(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter(|(_, o)| matches!(o, GateOutcome::Skip(_)))
            .map(|(g, _)| g.as_str())
            .collect()
    }

    /// One-line summary for audit events (`gate=outcome,…`).
    pub fn summary(&self) -> String {
        self.results
            .iter()
            .map(|(g, o)| format!("{g}={}", o.as_str()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Count every result in `sniper_gate_results_total{gate,outcome}`.
    pub fn record_metrics(&self) {
        let reg = bot_core::obs::metrics::global();
        for (gate, outcome) in &self.results {
            reg.counter(
                "sniper_gate_results_total",
                "Safety-gate evaluations by gate and outcome (pass/fail/skip).",
                &[("gate", gate.as_str()), ("outcome", outcome.as_str())],
            )
            .inc();
        }
    }
}

/// Evaluate every gate against `snapshot` at `now`. Pure.
pub fn evaluate(snapshot: &MarketSnapshot, cfg: &SniperConfig, now: DateTime<Utc>) -> GateReport {
    let mut report = GateReport::default();

    // 1. Venue accepts buys.
    report.push(
        GATE_POOL_STATE,
        if snapshot.tradable {
            GateOutcome::Pass
        } else {
            GateOutcome::Fail(if snapshot.tradable_detail.is_empty() {
                "venue is not accepting buys".to_string()
            } else {
                snapshot.tradable_detail.clone()
            })
        },
    );

    // 2. Raydium open time (only that protocol has one).
    report.push(
        GATE_POOL_OPEN_TIME,
        match snapshot.pool_open_time {
            Some(t) if i64::try_from(t).map_or(true, |t| t > now.timestamp()) => {
                GateOutcome::Fail(format!("pool opens at unix {t}, now {}", now.timestamp()))
            }
            Some(_) => GateOutcome::Pass,
            None if snapshot.protocol == LaunchProtocol::RaydiumAmmV4 => {
                GateOutcome::Skip("pool open time not read".into())
            }
            None => GateOutcome::Pass,
        },
    );

    // 3./4. Authorities.
    report.push(
        GATE_MINT_AUTHORITY,
        authority_gate(
            cfg.require_mint_authority_revoked,
            snapshot.mint_authority_revoked,
            "mint authority",
        ),
    );
    report.push(
        GATE_FREEZE_AUTHORITY,
        authority_gate(
            cfg.require_freeze_authority_revoked,
            snapshot.freeze_authority_revoked,
            "freeze authority",
        ),
    );

    // 4b. Sell simulation — the real honeypot check (GAP-MAP P1). A
    // disabled gate passes silently (same convention as the other
    // threshold gates); an ENABLED gate with no probe result is a skip,
    // which strict_gates promotes to a refusal.
    report.push(
        GATE_SELL_SIMULATION,
        if !cfg.simulate_sell {
            GateOutcome::Pass
        } else {
            match &snapshot.sell_probe {
                Some(SellProbeOutcome::Sellable) => GateOutcome::Pass,
                Some(SellProbeOutcome::NotSellable(detail)) => GateOutcome::Fail(format!(
                    "sell simulation says this token cannot be sold: {detail}"
                )),
                Some(SellProbeOutcome::Unknown(detail)) => {
                    GateOutcome::Skip(format!("sell simulation inconclusive: {detail}"))
                }
                None => GateOutcome::Skip("sell simulation enabled but no probe was run".into()),
            }
        },
    );

    // 5. Liquidity threshold (and: something to price against at all).
    let min_lamports = maths::sol_to_lamports(cfg.min_liquidity_sol.max(0.0));
    report.push(
        GATE_MIN_LIQUIDITY,
        if snapshot.pricing_quote_reserve_lamports == 0 {
            GateOutcome::Fail("pricing reserve is zero — nothing to trade against".into())
        } else if min_lamports > 0 && snapshot.quote_reserve_lamports < min_lamports {
            GateOutcome::Fail(format!(
                "quote liquidity {:.4} SOL below minimum {:.4} SOL",
                snapshot.quote_reserve_sol(),
                cfg.min_liquidity_sol
            ))
        } else {
            GateOutcome::Pass
        },
    );

    // 6. A price we can reason about.
    report.push(
        GATE_PRICE_SANE,
        if snapshot.spot_price_sol.is_finite() && snapshot.spot_price_sol > 0.0 {
            GateOutcome::Pass
        } else {
            GateOutcome::Fail(format!(
                "spot price {} is not a positive finite number",
                snapshot.spot_price_sol
            ))
        },
    );

    // 7. Decimals within the range real tokens use.
    report.push(
        GATE_DECIMALS_SANE,
        if snapshot.base_decimals <= MAX_SANE_DECIMALS {
            GateOutcome::Pass
        } else {
            GateOutcome::Fail(format!(
                "base decimals {} exceed {MAX_SANE_DECIMALS}",
                snapshot.base_decimals
            ))
        },
    );

    // 8. Creator concentration (opening buy).
    report.push(
        GATE_CREATOR_CONCENTRATION,
        if cfg.max_creator_initial_buy_sol <= 0.0 {
            GateOutcome::Pass
        } else {
            match snapshot.creator_initial_buy_sol {
                Some(buy) if buy > cfg.max_creator_initial_buy_sol => GateOutcome::Fail(format!(
                    "creator opening buy {buy:.4} SOL exceeds {:.4} SOL",
                    cfg.max_creator_initial_buy_sol
                )),
                Some(_) => GateOutcome::Pass,
                None => GateOutcome::Skip("creator opening buy not reported by this source".into()),
            }
        },
    );

    // 8b. Holder concentration (GAP-MAP P1). Disabled (`<= 0`) passes
    // silently, matching every other threshold gate. ENABLED but no data is a
    // FAILURE, regardless of strict_gates: an operator who turned the gate on
    // required the signal, and a missing required signal must never pass
    // silently (fail-closed). Previously this was a Skip that only strict mode
    // refused.
    report.push(
        GATE_HOLDER_CONCENTRATION,
        if cfg.max_top_holder_pct <= 0.0 {
            GateOutcome::Pass
        } else {
            match &snapshot.top_holders {
                Some(holders) => {
                    let worst = holders
                        .iter()
                        .filter(|h| !h.is_infrastructure)
                        .max_by(|a, b| {
                            a.pct_of_supply
                                .partial_cmp(&b.pct_of_supply)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                    match worst {
                        Some(h) if h.pct_of_supply > cfg.max_top_holder_pct => GateOutcome::Fail(
                            format!(
                                "holder {} controls {:.2}% of supply (max {:.2}%)",
                                h.address, h.pct_of_supply, cfg.max_top_holder_pct
                            ),
                        ),
                        Some(_) => GateOutcome::Pass,
                        // Table present but empty or all-infrastructure:
                        // nothing concentrates the supply.
                        None => GateOutcome::Pass,
                    }
                }
                None => GateOutcome::Fail(
                    "holder gate enabled but no holder table was ingested for this venue".into(),
                ),
            }
        },
    );

    // 8c. Bundler detection (GAP-MAP P1). The ratio is only meaningful
    // with at least one first-slot buyer; a zero-buyer signal is Skip
    // (the analyser misfired), never Pass.
    report.push(
        GATE_BUNDLER_DETECTION,
        if cfg.max_bundler_ratio <= 0.0 {
            GateOutcome::Pass
        } else {
            match &snapshot.bundling {
                Some(sig) if sig.first_slot_buyers > 0 => {
                    let ratio = sig.bundled_wallets.min(sig.first_slot_buyers) as f64
                        / sig.first_slot_buyers as f64;
                    if ratio >= cfg.max_bundler_ratio {
                        GateOutcome::Fail(format!(
                            "{}/{} first-slot buyers look bundler-coordinated (ratio {:.2} >= {:.2})",
                            sig.bundled_wallets.min(sig.first_slot_buyers),
                            sig.first_slot_buyers,
                            ratio,
                            cfg.max_bundler_ratio
                        ))
                    } else {
                        GateOutcome::Pass
                    }
                }
                Some(_) => GateOutcome::Fail(
                    "bundler gate enabled but the signal carries zero first-slot buyers".into(),
                ),
                None => GateOutcome::Fail(
                    "bundler gate enabled but the launch source reported no bundler signal".into(),
                ),
            }
        },
    );

    // 9. Pool share of supply.
    report.push(
        GATE_POOL_SUPPLY_FRACTION,
        if cfg.min_pool_supply_fraction <= 0.0 {
            GateOutcome::Pass
        } else {
            match snapshot.pool_supply_fraction() {
                Some(f) if f < cfg.min_pool_supply_fraction => GateOutcome::Fail(format!(
                    "only {:.1}% of supply is on the venue (minimum {:.1}%)",
                    f * 100.0,
                    cfg.min_pool_supply_fraction * 100.0
                )),
                Some(_) => GateOutcome::Pass,
                None => GateOutcome::Skip("total supply unknown".into()),
            }
        },
    );

    // 10. The snapshot itself is fresh enough to act on.
    let age = snapshot.age_ms(now);
    report.push(
        GATE_SNAPSHOT_FRESHNESS,
        if age <= cfg.max_snapshot_age_ms {
            GateOutcome::Pass
        } else {
            GateOutcome::Fail(format!(
                "market snapshot is {age} ms old (max {} ms)",
                cfg.max_snapshot_age_ms
            ))
        },
    );

    report
}

fn authority_gate(required: bool, revoked: Option<bool>, what: &str) -> GateOutcome {
    if !required {
        return GateOutcome::Pass;
    }
    match revoked {
        Some(true) => GateOutcome::Pass,
        Some(false) => GateOutcome::Fail(format!("{what} is still set")),
        None => GateOutcome::Skip(format!("{what} state unknown (mint not read)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey::Pubkey;

    const SOL: u64 = 1_000_000_000;

    fn snapshot() -> MarketSnapshot {
        MarketSnapshot {
            protocol: LaunchProtocol::PumpFun,
            pool: None,
            quote_reserve_lamports: 2 * SOL,
            pricing_quote_reserve_lamports: 32 * SOL,
            base_reserve_raw: 793_100_000_000_000,
            base_decimals: 6,
            total_supply_raw: Some(1_000_000_000_000_000),
            fee_bps: 100,
            tradable: true,
            tradable_detail: String::new(),
            pool_open_time: None,
            mint_authority_revoked: Some(true),
            freeze_authority_revoked: Some(true),
            // Default config has simulate_sell on, so the healthy snapshot
            // carries a probe that passed.
            sell_probe: Some(SellProbeOutcome::Sellable),
            creator_initial_buy_sol: Some(0.5),
            // Both gates default OFF (0.0), so the healthy snapshot needs
            // no holder/bundler data to pass — including under strict.
            top_holders: None,
            bundling: None,
            spot_price_sol: 0.00000003,
            fetched_at: Utc::now(),
        }
    }

    fn cfg() -> SniperConfig {
        SniperConfig::default()
    }

    #[test]
    fn default_config_passes_a_healthy_pump_curve() {
        let r = evaluate(&snapshot(), &cfg(), Utc::now());
        assert!(r.passed(false), "{}", r.summary());
        assert!(
            r.passed(true),
            "no skips on a full snapshot: {}",
            r.summary()
        );
        assert_eq!(r.results.len(), ALL_GATES.len());
        for (i, (g, _)) in r.results.iter().enumerate() {
            assert_eq!(g, ALL_GATES[i], "evaluation order is stable");
        }
    }

    #[test]
    fn pool_state_and_open_time_gate_the_venue() {
        let now = Utc::now();
        let mut s = snapshot();
        s.tradable = false;
        s.tradable_detail = "curve complete".into();
        let r = evaluate(&s, &cfg(), now);
        let (g, d) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_POOL_STATE);
        assert!(d.contains("curve complete"));
        assert_eq!(reason_for_gate(g), RejectReason::PoolNotReady);

        let mut s = snapshot();
        s.protocol = LaunchProtocol::RaydiumAmmV4;
        s.pool_open_time = Some((now.timestamp() + 600) as u64);
        let r = evaluate(&s, &cfg(), now);
        assert_eq!(r.first_failure(false).unwrap().0, GATE_POOL_OPEN_TIME);
        s.pool_open_time = Some((now.timestamp() - 1) as u64);
        assert!(evaluate(&s, &cfg(), now).passed(false));
        s.pool_open_time = Some(0);
        assert!(evaluate(&s, &cfg(), now).passed(false));
        // Unknown open time on Raydium is a skip → strict fails it.
        s.pool_open_time = None;
        let r = evaluate(&s, &cfg(), now);
        assert!(r.passed(false));
        assert!(!r.passed(true));
        assert_eq!(r.skipped(), vec![GATE_POOL_OPEN_TIME]);
        // Absurd open time (beyond i64) is treated as "not open".
        s.pool_open_time = Some(u64::MAX);
        assert!(!evaluate(&s, &cfg(), now).passed(false));
    }

    #[test]
    fn authority_gates_follow_config_and_strictness() {
        let now = Utc::now();
        let mut c = cfg();
        let mut s = snapshot();
        // Default: freeze required, mint not.
        s.mint_authority_revoked = Some(false);
        assert!(evaluate(&s, &c, now).passed(false));
        s.freeze_authority_revoked = Some(false);
        let r = evaluate(&s, &c, now);
        assert_eq!(r.first_failure(false).unwrap().0, GATE_FREEZE_AUTHORITY);
        assert_eq!(
            reason_for_gate(GATE_FREEZE_AUTHORITY),
            RejectReason::TokenStateInvalid
        );
        // Turn the mint requirement on.
        c.require_mint_authority_revoked = true;
        s.freeze_authority_revoked = Some(true);
        let r = evaluate(&s, &c, now);
        assert_eq!(r.first_failure(false).unwrap().0, GATE_MINT_AUTHORITY);
        // Unknown state: skip unless strict.
        s.mint_authority_revoked = None;
        s.freeze_authority_revoked = None;
        let r = evaluate(&s, &c, now);
        assert!(r.passed(false));
        assert_eq!(
            r.skipped(),
            vec![GATE_MINT_AUTHORITY, GATE_FREEZE_AUTHORITY]
        );
        let (g, d) = r.first_failure(true).unwrap();
        assert_eq!(g, GATE_MINT_AUTHORITY);
        assert!(d.starts_with("strict_gates:"));
        // Both requirements off: nothing is even skipped.
        c.require_mint_authority_revoked = false;
        c.require_freeze_authority_revoked = false;
        assert!(evaluate(&s, &c, now).skipped().is_empty());
    }

    #[test]
    fn sell_simulation_gate_is_the_honeypot_check() {
        let now = Utc::now();
        // Healthy probe passes.
        let s = snapshot();
        assert!(evaluate(&s, &cfg(), now).passed(true));
        assert_eq!(reason_for_gate(GATE_SELL_SIMULATION), RejectReason::HoneypotRisk);

        // A venue that refuses the sell is a honeypot: hard failure with
        // the HoneypotRisk reason.
        let mut s = snapshot();
        s.sell_probe = Some(SellProbeOutcome::NotSellable(
            "sell simulation reported 'frozen'".into(),
        ));
        let r = evaluate(&s, &cfg(), now);
        let (g, d) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_SELL_SIMULATION);
        assert!(d.contains("cannot be sold"), "{d}");
        assert!(d.contains("frozen"), "{d}");

        // An inconclusive probe skips — and strict promotes the skip.
        let mut s = snapshot();
        s.sell_probe = Some(SellProbeOutcome::Unknown("rpc timeout".into()));
        let r = evaluate(&s, &cfg(), now);
        assert!(r.passed(false));
        assert_eq!(r.skipped(), vec![GATE_SELL_SIMULATION]);
        let (g, d) = r.first_failure(true).unwrap();
        assert_eq!(g, GATE_SELL_SIMULATION);
        assert!(d.starts_with("strict_gates:"), "{d}");

        // Enabled gate with NO probe result is also a skip (strict fails).
        let mut s = snapshot();
        s.sell_probe = None;
        assert_eq!(evaluate(&s, &cfg(), now).skipped(), vec![GATE_SELL_SIMULATION]);

        // Gate switched off: silent pass, never a skip.
        let mut c = cfg();
        c.simulate_sell = false;
        let mut s = snapshot();
        s.sell_probe = None;
        let r = evaluate(&s, &c, now);
        assert!(r.passed(true), "disabled gate must not even skip: {}", r.summary());
        assert!(r.skipped().is_empty());
    }

    #[test]
    fn liquidity_gate_uses_real_reserves_and_refuses_empty_pricing() {
        let now = Utc::now();
        let mut c = cfg();
        c.min_liquidity_sol = 5.0;
        let s = snapshot(); // 2 SOL real
        let r = evaluate(&s, &c, now);
        let (g, d) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_MIN_LIQUIDITY);
        assert!(d.contains("2.0000 SOL below minimum 5.0000"));
        assert_eq!(reason_for_gate(g), RejectReason::InsufficientLiquidity);
        c.min_liquidity_sol = 2.0;
        assert!(evaluate(&s, &c, now).passed(false), "boundary is inclusive");
        // Zero pricing reserve always fails, even with the threshold off.
        let mut s = snapshot();
        s.pricing_quote_reserve_lamports = 0;
        assert_eq!(
            evaluate(&s, &cfg(), now).first_failure(false).unwrap().0,
            GATE_MIN_LIQUIDITY
        );
        // Zero REAL reserve with the threshold off passes (fresh curve).
        let mut s = snapshot();
        s.quote_reserve_lamports = 0;
        assert!(evaluate(&s, &cfg(), now).passed(false));
    }

    #[test]
    fn price_and_decimals_sanity_gates() {
        let now = Utc::now();
        let mut s = snapshot();
        s.spot_price_sol = 0.0;
        assert_eq!(
            evaluate(&s, &cfg(), now).first_failure(false).unwrap().0,
            GATE_PRICE_SANE
        );
        s.spot_price_sol = f64::NAN;
        assert_eq!(
            evaluate(&s, &cfg(), now).first_failure(false).unwrap().0,
            GATE_PRICE_SANE
        );
        let mut s = snapshot();
        s.base_decimals = 13;
        let r = evaluate(&s, &cfg(), now);
        assert_eq!(r.first_failure(false).unwrap().0, GATE_DECIMALS_SANE);
        assert_eq!(
            reason_for_gate(GATE_DECIMALS_SANE),
            RejectReason::TokenStateInvalid
        );
    }

    #[test]
    fn concentration_gates_use_creator_buy_and_pool_share() {
        let now = Utc::now();
        let mut c = cfg();
        c.max_creator_initial_buy_sol = 0.4;
        let s = snapshot(); // creator bought 0.5
        let r = evaluate(&s, &c, now);
        assert_eq!(
            r.first_failure(false).unwrap().0,
            GATE_CREATOR_CONCENTRATION
        );
        assert_eq!(
            reason_for_gate(GATE_CREATOR_CONCENTRATION),
            RejectReason::ConcentrationLimit
        );
        c.max_creator_initial_buy_sol = 0.5;
        assert!(evaluate(&s, &c, now).passed(false), "boundary inclusive");
        // Unknown creator buy → skip.
        let mut s = snapshot();
        s.creator_initial_buy_sol = None;
        let r = evaluate(&s, &c, now);
        assert_eq!(r.skipped(), vec![GATE_CREATOR_CONCENTRATION]);

        // Pool share: 79.31% pooled; require 90% → fail, 50% → pass.
        let mut c = cfg();
        c.min_pool_supply_fraction = 0.9;
        let s = snapshot();
        assert_eq!(
            evaluate(&s, &c, now).first_failure(false).unwrap().0,
            GATE_POOL_SUPPLY_FRACTION
        );
        c.min_pool_supply_fraction = 0.5;
        assert!(evaluate(&s, &c, now).passed(false));
        let mut s = snapshot();
        s.total_supply_raw = None;
        assert_eq!(
            evaluate(&s, &c, now).skipped(),
            vec![GATE_POOL_SUPPLY_FRACTION]
        );
        s.total_supply_raw = Some(0);
        assert_eq!(
            s.pool_supply_fraction(),
            None,
            "zero supply is unknown, not infinite"
        );
    }

    #[test]
    fn snapshot_freshness_is_evaluated_against_now() {
        let now = Utc::now();
        let mut s = snapshot();
        s.fetched_at = now - chrono::Duration::milliseconds(2_500);
        let r = evaluate(&s, &cfg(), now);
        let (g, d) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_SNAPSHOT_FRESHNESS);
        assert!(d.contains("2500 ms old"));
        assert_eq!(reason_for_gate(g), RejectReason::StaleEvent);
        assert!(s.age_ms(now) >= 2_500);
        // Clock skew (fetched "in the future") is not stale.
        s.fetched_at = now + chrono::Duration::seconds(1);
        assert_eq!(s.age_ms(now), 0);
    }

    #[test]
    fn report_summary_and_serde() {
        let r = evaluate(&snapshot(), &cfg(), Utc::now());
        let text = r.summary();
        assert!(text.starts_with("pool_state=pass,"));
        let json = serde_json::to_string(&r).unwrap();
        let back: GateReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        r.record_metrics();
        assert_eq!(reason_for_gate("unknown"), RejectReason::InvalidState);
    }

    // ------------------------------------------------------------------
    // Holder-concentration gate (GAP-MAP P1)
    // ------------------------------------------------------------------

    fn holder(pct: f64, infrastructure: bool) -> TokenHolder {
        TokenHolder {
            address: Pubkey::new_unique().to_string(),
            pct_of_supply: pct,
            is_infrastructure: infrastructure,
        }
    }

    #[test]
    fn holder_gate_disabled_passes_without_data() {
        let r = evaluate(&snapshot(), &cfg(), Utc::now());
        let outcome = r
            .results
            .iter()
            .find(|(g, _)| g == GATE_HOLDER_CONCENTRATION)
            .map(|(_, o)| o.clone())
            .unwrap();
        assert_eq!(outcome, GateOutcome::Pass, "off by default, no data needed");
    }

    #[test]
    fn holder_gate_enabled_but_no_data_refuses_in_both_modes() {
        let mut c = cfg();
        c.max_top_holder_pct = 20.0;
        let r = evaluate(&snapshot(), &c, Utc::now());
        let (g, detail) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_HOLDER_CONCENTRATION);
        assert!(detail.contains("no holder table"), "{detail}");
        // Enabled-but-missing is a refusal in BOTH modes (fail-closed).
        assert!(!r.passed(false));
        assert!(!r.passed(true));
        assert_eq!(reason_for_gate(g), RejectReason::ConcentrationLimit);
    }

    #[test]
    fn holder_gate_fails_on_whale_and_ignores_infrastructure() {
        let mut c = cfg();
        c.max_top_holder_pct = 20.0;
        let mut s = snapshot();
        s.top_holders = Some(vec![
            holder(55.0, true),  // the bonding curve itself: must not count
            holder(25.0, false), // a real whale: must fail the gate
            holder(5.0, false),
        ]);
        let r = evaluate(&s, &c, Utc::now());
        let (g, detail) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_HOLDER_CONCENTRATION);
        assert!(detail.contains("25.00%"), "{detail}");
        assert!(detail.contains("max 20.00%"), "{detail}");
    }

    #[test]
    fn holder_gate_passes_when_only_infrastructure_is_large() {
        let mut c = cfg();
        c.max_top_holder_pct = 20.0;
        let mut s = snapshot();
        s.top_holders = Some(vec![
            holder(79.0, true), // curve holds most of the supply: normal
            holder(12.0, false),
            holder(4.0, false),
        ]);
        let r = evaluate(&s, &c, Utc::now());
        assert!(r.passed(true), "{}", r.summary());
    }

    #[test]
    fn holder_gate_passes_on_empty_or_all_infrastructure_tables() {
        let mut c = cfg();
        c.max_top_holder_pct = 20.0;
        let mut s = snapshot();
        s.top_holders = Some(vec![]);
        assert!(evaluate(&s, &c, Utc::now()).passed(true));
        s.top_holders = Some(vec![holder(60.0, true)]);
        assert!(evaluate(&s, &c, Utc::now()).passed(true));
    }

    // ------------------------------------------------------------------
    // Bundler-detection gate (GAP-MAP P1)
    // ------------------------------------------------------------------

    #[test]
    fn bundler_gate_disabled_passes_without_data() {
        let r = evaluate(&snapshot(), &cfg(), Utc::now());
        let outcome = r
            .results
            .iter()
            .find(|(g, _)| g == GATE_BUNDLER_DETECTION)
            .map(|(_, o)| o.clone())
            .unwrap();
        assert_eq!(outcome, GateOutcome::Pass);
    }

    #[test]
    fn bundler_gate_enabled_but_no_signal_refuses_in_both_modes() {
        let mut c = cfg();
        c.max_bundler_ratio = 0.5;
        let r = evaluate(&snapshot(), &c, Utc::now());
        assert!(!r.passed(false));
        assert!(!r.passed(true));
        let (g, _) = r.first_failure(true).unwrap();
        assert_eq!(g, GATE_BUNDLER_DETECTION);
    }

    #[test]
    fn bundler_gate_fails_at_or_above_the_ratio() {
        let mut c = cfg();
        c.max_bundler_ratio = 0.5;
        let mut s = snapshot();
        s.bundling = Some(BundlingSignal {
            first_slot_buyers: 10,
            bundled_wallets: 5, // exactly 0.5 >= 0.5: refuse
        });
        let r = evaluate(&s, &c, Utc::now());
        let (g, detail) = r.first_failure(false).unwrap();
        assert_eq!(g, GATE_BUNDLER_DETECTION);
        assert!(detail.contains("5/10"), "{detail}");
        assert_eq!(reason_for_gate(g), RejectReason::ConcentrationLimit);

        s.bundling = Some(BundlingSignal {
            first_slot_buyers: 10,
            bundled_wallets: 4, // 0.4 < 0.5: fine
        });
        assert!(evaluate(&s, &c, Utc::now()).passed(true));
    }

    #[test]
    fn bundler_gate_never_counts_more_than_all_buyers() {
        let mut c = cfg();
        c.max_bundler_ratio = 0.5;
        let mut s = snapshot();
        // A buggy analyser must not be able to report 150% bundling: the
        // ratio clamps to the buyer count.
        s.bundling = Some(BundlingSignal {
            first_slot_buyers: 10,
            bundled_wallets: 15,
        });
        let r = evaluate(&s, &c, Utc::now());
        let (_, detail) = r.first_failure(false).unwrap();
        assert!(detail.contains("10/10"), "{detail}");
    }

    #[test]
    fn bundler_gate_zero_buyers_refuses_not_passes() {
        let mut c = cfg();
        c.max_bundler_ratio = 0.5;
        let mut s = snapshot();
        s.bundling = Some(BundlingSignal {
            first_slot_buyers: 0,
            bundled_wallets: 0,
        });
        let r = evaluate(&s, &c, Utc::now());
        assert!(!r.passed(false), "an unusable enabled signal must refuse");
        assert!(!r.skipped().contains(&GATE_BUNDLER_DETECTION));
    }
}
