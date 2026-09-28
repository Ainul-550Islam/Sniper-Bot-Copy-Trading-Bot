//! Mirror sizing (TASK 3 §06).
//!
//! Turns a leader's SOL size into the SOL we *request* from the risk engine.
//! Pure arithmetic, fully deterministic, and defensive about the inputs the
//! feeds hand over (NaN, infinities, zero, negative). The order is:
//!
//! 1. **base** — the wallet rule: `fixed_sol` when set (> 0), else
//!    `leader_sol × fraction_of_their_size`;
//! 2. **rule cap** — `max_sol` (> 0);
//! 3. **global caps** — `[copy].max_sol_per_trade` (> 0) and
//!    `[copy].max_balance_fraction × available` (> 0, balance known);
//! 4. **floors** — must be finite and positive, and at least
//!    `[copy].min_mirror_sol` when that is set.
//!
//! The result is a *request*: `RiskEngine::check_entry` still applies the
//! generic and copy-specific caps and may reduce it (`AllowReduced`) or
//! refuse it. Sizing never reads state and never sees the balance except as
//! the number the caller passes in.

use bot_core::config::{CopyConfig, CopyWallet};
use serde::{Deserialize, Serialize};

/// Which rule produced the base size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizingMode {
    /// `fixed_sol` on the wallet rule.
    Fixed,
    /// `fraction_of_their_size × leader size`.
    Proportional,
}

impl SizingMode {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SizingMode::Fixed => "fixed",
            SizingMode::Proportional => "proportional",
        }
    }
}

/// A cap that reduced the size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeClamp {
    /// Wallet rule `max_sol`.
    RuleMaxSol,
    /// `[copy].max_sol_per_trade`.
    GlobalMaxPerTrade,
    /// `[copy].max_balance_fraction × available`.
    BalanceFraction,
}

impl SizeClamp {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SizeClamp::RuleMaxSol => "rule_max_sol",
            SizeClamp::GlobalMaxPerTrade => "global_max_per_trade",
            SizeClamp::BalanceFraction => "balance_fraction",
        }
    }
}

/// Why no size could be produced.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizingError {
    /// An input or intermediate was NaN / infinite.
    NonFinite,
    /// The size is zero or negative.
    NonPositive,
    /// The size is under `[copy].min_mirror_sol`.
    Dust {
        /// Computed size.
        requested: f64,
        /// Configured floor.
        floor: f64,
    },
}

impl std::fmt::Display for SizingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SizingError::NonFinite => f.write_str("mirror size is not finite"),
            SizingError::NonPositive => f.write_str("mirror size is zero or negative"),
            SizingError::Dust { requested, floor } => write!(
                f,
                "mirror size {requested:.6} SOL is under copy.min_mirror_sol {floor:.6}"
            ),
        }
    }
}

impl std::error::Error for SizingError {}

/// A successful sizing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SizeDecision {
    /// Rule that produced the base.
    pub mode: SizingMode,
    /// Leader's SOL size (input).
    pub leader_sol: f64,
    /// Base before any cap.
    pub base_sol: f64,
    /// Final requested size.
    pub requested_sol: f64,
    /// Caps that actually reduced the size, in order.
    pub clamps: Vec<SizeClamp>,
}

impl SizeDecision {
    /// `requested / leader` (`0` when the leader size is unknown).
    pub fn ratio(&self) -> f64 {
        if self.leader_sol > 0.0 {
            self.requested_sol / self.leader_sol
        } else {
            0.0
        }
    }
}

/// Base size from the wallet rule alone — identical to the pre-TASK-3
/// `mirror::size_for` arithmetic (fixed wins, else fraction, then `max_sol`).
pub fn rule_size(rule: &CopyWallet, leader_sol: f64) -> (SizingMode, f64, bool) {
    let (mode, base) = match rule.fixed_sol {
        Some(f) if f > 0.0 => (SizingMode::Fixed, f),
        _ => (
            SizingMode::Proportional,
            leader_sol * rule.fraction_of_their_size.max(0.0),
        ),
    };
    if rule.max_sol > 0.0 && base > rule.max_sol {
        (mode, rule.max_sol, true)
    } else {
        (mode, base, false)
    }
}

/// Size a mirror. `available_sol` is the spendable balance when known.
pub fn size_mirror(
    rule: &CopyWallet,
    cfg: &CopyConfig,
    leader_sol: f64,
    available_sol: Option<f64>,
) -> Result<SizeDecision, SizingError> {
    if !leader_sol.is_finite() {
        return Err(SizingError::NonFinite);
    }
    let (mode, mut size, capped) = rule_size(rule, leader_sol);
    if !size.is_finite() {
        return Err(SizingError::NonFinite);
    }
    let base_sol = match rule.fixed_sol {
        Some(f) if f > 0.0 => f,
        _ => leader_sol * rule.fraction_of_their_size.max(0.0),
    };
    let mut clamps = Vec::new();
    if capped {
        clamps.push(SizeClamp::RuleMaxSol);
    }
    if cfg.max_sol_per_trade > 0.0 && size > cfg.max_sol_per_trade {
        size = cfg.max_sol_per_trade;
        clamps.push(SizeClamp::GlobalMaxPerTrade);
    }
    if cfg.max_balance_fraction > 0.0 {
        if let Some(avail) = available_sol {
            if avail.is_finite() && avail >= 0.0 {
                let cap = avail * cfg.max_balance_fraction.min(1.0);
                if size > cap {
                    size = cap;
                    clamps.push(SizeClamp::BalanceFraction);
                }
            }
        }
    }
    if !size.is_finite() {
        return Err(SizingError::NonFinite);
    }
    if size <= 0.0 {
        return Err(SizingError::NonPositive);
    }
    if cfg.min_mirror_sol > 0.0 && size < cfg.min_mirror_sol {
        return Err(SizingError::Dust {
            requested: size,
            floor: cfg.min_mirror_sol,
        });
    }
    Ok(SizeDecision {
        mode,
        leader_sol,
        base_sol,
        requested_sol: size,
        clamps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> CopyWallet {
        CopyWallet {
            address: "whale".into(),
            fixed_sol: None,
            fraction_of_their_size: 0.1,
            max_sol: 0.3,
            ..CopyWallet::default()
        }
    }

    #[test]
    fn matches_legacy_size_for_arithmetic() {
        let cfg = CopyConfig::default();
        let mut fixed = rule();
        fixed.fixed_sol = Some(0.25);
        fixed.max_sol = 10.0;
        let d = size_mirror(&fixed, &cfg, 5.0, None).unwrap();
        assert_eq!(d.mode, SizingMode::Fixed);
        assert!((d.requested_sol - 0.25).abs() < 1e-9);
        assert!(d.clamps.is_empty());

        let d = size_mirror(&rule(), &cfg, 5.0, None).unwrap();
        assert_eq!(d.mode, SizingMode::Proportional);
        assert!((d.base_sol - 0.5).abs() < 1e-9);
        assert!((d.requested_sol - 0.3).abs() < 1e-9);
        assert_eq!(d.clamps, vec![SizeClamp::RuleMaxSol]);

        let mut uncapped = rule();
        uncapped.max_sol = 0.0;
        uncapped.fraction_of_their_size = 0.05;
        let d = size_mirror(&uncapped, &cfg, 2.0, None).unwrap();
        assert!((d.requested_sol - 0.1).abs() < 1e-9);
        assert!((d.ratio() - 0.05).abs() < 1e-9);

        // fixed_sol = 0 is "unset", as before.
        let mut zero_fixed = rule();
        zero_fixed.fixed_sol = Some(0.0);
        assert_eq!(
            size_mirror(&zero_fixed, &cfg, 1.0, None).unwrap().mode,
            SizingMode::Proportional
        );
    }

    #[test]
    fn global_caps_apply_after_the_rule() {
        let mut cfg = CopyConfig {
            max_sol_per_trade: 0.2,
            ..CopyConfig::default()
        };
        let d = size_mirror(&rule(), &cfg, 5.0, None).unwrap();
        assert!((d.requested_sol - 0.2).abs() < 1e-9);
        assert_eq!(
            d.clamps,
            vec![SizeClamp::RuleMaxSol, SizeClamp::GlobalMaxPerTrade]
        );

        cfg.max_sol_per_trade = 0.0;
        cfg.max_balance_fraction = 0.1;
        let d = size_mirror(&rule(), &cfg, 5.0, Some(1.0)).unwrap();
        assert!((d.requested_sol - 0.1).abs() < 1e-9);
        assert_eq!(
            d.clamps,
            vec![SizeClamp::RuleMaxSol, SizeClamp::BalanceFraction]
        );
        // Unknown balance → the fraction cap cannot apply.
        let d = size_mirror(&rule(), &cfg, 5.0, None).unwrap();
        assert!((d.requested_sol - 0.3).abs() < 1e-9);
        // NaN balance is ignored, not propagated.
        let d = size_mirror(&rule(), &cfg, 5.0, Some(f64::NAN)).unwrap();
        assert!((d.requested_sol - 0.3).abs() < 1e-9);
        // Fraction above 1 is clamped to the whole balance.
        cfg.max_balance_fraction = 1.0;
        let d = size_mirror(&rule(), &cfg, 5.0, Some(0.05)).unwrap();
        assert!((d.requested_sol - 0.05).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_inputs_and_dust() {
        let mut cfg = CopyConfig {
            min_mirror_sol: 0.0,
            ..CopyConfig::default()
        };
        assert_eq!(
            size_mirror(&rule(), &cfg, f64::NAN, None),
            Err(SizingError::NonFinite)
        );
        assert_eq!(
            size_mirror(&rule(), &cfg, f64::INFINITY, None),
            Err(SizingError::NonFinite)
        );
        assert_eq!(
            size_mirror(&rule(), &cfg, 0.0, None),
            Err(SizingError::NonPositive)
        );
        assert_eq!(
            size_mirror(&rule(), &cfg, -1.0, None),
            Err(SizingError::NonPositive)
        );
        let mut neg_fraction = rule();
        neg_fraction.fraction_of_their_size = -0.5;
        assert_eq!(
            size_mirror(&neg_fraction, &cfg, 1.0, None),
            Err(SizingError::NonPositive)
        );
        let mut nan_fixed = rule();
        nan_fixed.fixed_sol = Some(f64::NAN);
        // NaN > 0 is false → treated as unset → proportional.
        assert_eq!(
            size_mirror(&nan_fixed, &cfg, 1.0, None).unwrap().mode,
            SizingMode::Proportional
        );
        let mut inf_fixed = rule();
        inf_fixed.fixed_sol = Some(f64::INFINITY);
        inf_fixed.max_sol = 0.0;
        assert_eq!(
            size_mirror(&inf_fixed, &cfg, 1.0, None),
            Err(SizingError::NonFinite)
        );
        cfg.min_mirror_sol = 0.05;
        let mut small = rule();
        small.fraction_of_their_size = 0.01;
        match size_mirror(&small, &cfg, 1.0, None) {
            Err(SizingError::Dust { requested, floor }) => {
                assert!((requested - 0.01).abs() < 1e-9);
                assert_eq!(floor, 0.05);
            }
            other => panic!("expected dust, got {other:?}"),
        }
        assert!(size_mirror(&rule(), &cfg, 1.0, None).is_ok());
        assert_eq!(SizingMode::Fixed.as_str(), "fixed");
        assert_eq!(SizeClamp::BalanceFraction.as_str(), "balance_fraction");
        assert!(SizingError::NonFinite.to_string().contains("not finite"));
    }
}
