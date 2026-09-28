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

use crate::event::LaunchProtocol;
use crate::pipeline::RejectReason;

/// Base-token decimals above this are treated as corrupt mint data.
pub const MAX_SANE_DECIMALS: u8 = 12;

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
    /// The creator's opening buy in SOL, when the source reports it.
    pub creator_initial_buy_sol: Option<f64>,
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
pub const GATE_MIN_LIQUIDITY: &str = "min_liquidity";
pub const GATE_PRICE_SANE: &str = "price_sane";
pub const GATE_DECIMALS_SANE: &str = "decimals_sane";
pub const GATE_CREATOR_CONCENTRATION: &str = "creator_concentration";
pub const GATE_POOL_SUPPLY_FRACTION: &str = "pool_supply_fraction";
pub const GATE_SNAPSHOT_FRESHNESS: &str = "snapshot_freshness";

/// All gates, in evaluation order.
pub const ALL_GATES: &[&str] = &[
    GATE_POOL_STATE,
    GATE_POOL_OPEN_TIME,
    GATE_MINT_AUTHORITY,
    GATE_FREEZE_AUTHORITY,
    GATE_MIN_LIQUIDITY,
    GATE_PRICE_SANE,
    GATE_DECIMALS_SANE,
    GATE_CREATOR_CONCENTRATION,
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
        GATE_MIN_LIQUIDITY | GATE_PRICE_SANE => RejectReason::InsufficientLiquidity,
        GATE_CREATOR_CONCENTRATION | GATE_POOL_SUPPLY_FRACTION => RejectReason::ConcentrationLimit,
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
            creator_initial_buy_sol: Some(0.5),
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
}
