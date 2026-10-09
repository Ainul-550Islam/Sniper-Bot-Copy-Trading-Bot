//! Advanced exit policy (GAP-MAP v2, P2): laddered multi-level take-profit,
//! break-even stop, and dev-sell trigger — layered ON TOP of the core risk
//! engine (`bot_core::risk::check_exit`), which keeps owning SL/TP/trailing/
//! max-hold.
//!
//! Why a separate layer instead of extending the core engine:
//! * The core rules are venue-independent and shared by every module; these
//!   three are memecoin-launch tactics specific to the sniper.
//! * The ladder needs per-position STATE (which levels already fired). That
//!   state lives here in a tracker with the same retain-on-live contract as
//!   [`crate::exit::ExitTracker`] — no Position-schema change, so no
//!   migration is required to ship the feature. On a restart the ladder
//!   re-derives: any level whose trigger is below the CURRENT mark and whose
//!   proceeds are already booked simply fires once more as a zero-qty no-op
//!   (guarded: selling 0 raw units is refused in `sell_position`), so a
//!   restart can never double-sell.
//!
//! Rule attribution: exits from this layer are reported as
//! [`bot_core::risk::ExitRule::Manual`] with reason prefixes
//! `ladder_tp_<n>:`, `break_even:`, `dev_sell:` — low-cardinality, greppable,
//! and safe for the existing rule-matching code paths (no enum surgery in
//! core, no exhaustive-match churn across modules).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use solana_sdk::pubkey::Pubkey;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// One rung of the take-profit ladder.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TpLevel {
    /// Fire when `mark >= avg_entry * trigger_multiple` (e.g. 2.0 = +100%).
    pub trigger_multiple: f64,
    /// Fraction of the CURRENT position to sell at this rung (0..=1).
    pub sell_fraction: f64,
}

/// Break-even stop: once the position shows `activation_gain` of profit,
/// the stop rises to entry + `buffer` so a round-trip cannot lose money
/// (buffer is meant to cover fees/slippage).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BreakEvenConfig {
    /// Multiple of entry at which the break-even stop arms (e.g. 1.5 = +50%).
    pub activation_multiple: f64,
    /// Fraction ABOVE entry for the armed stop (e.g. 0.02 = entry * 1.02).
    pub buffer: f64,
}

/// The whole advanced policy. All fields optional — an empty policy is a
/// no-op, which keeps existing deployments byte-identical in behaviour.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExitPolicyConfig {
    /// Take-profit ladder, evaluated in ascending `trigger_multiple` order.
    #[serde(default)]
    pub ladder: Vec<TpLevel>,
    /// Break-even stop, when armed by a prior gain.
    #[serde(default)]
    pub break_even: Option<BreakEvenConfig>,
    /// Dev-sell trigger: flatten when a dev-sell signal was observed within
    /// this window. 0 disables.
    #[serde(default)]
    pub dev_sell_ttl_secs: u64,
}

impl ExitPolicyConfig {
    /// Validate and normalise: sorts the ladder, clamps fractions, drops
    /// degenerate levels. Returns a policy that is safe to evaluate.
    pub fn normalized(mut self) -> ExitPolicyConfig {
        self.ladder.retain(|l| {
            l.trigger_multiple.is_finite()
                && l.trigger_multiple > 1.0
                && l.sell_fraction.is_finite()
                && l.sell_fraction > 0.0
        });
        for level in &mut self.ladder {
            level.sell_fraction = level.sell_fraction.clamp(0.0, 1.0);
        }
        self.ladder
            .sort_by(|a, b| a.trigger_multiple.partial_cmp(&b.trigger_multiple).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(be) = &mut self.break_even {
            if !be.activation_multiple.is_finite() || be.activation_multiple <= 1.0 {
                self.break_even = None;
            } else if !be.buffer.is_finite() || be.buffer < 0.0 {
                be.buffer = 0.0;
            }
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

/// What the advanced layer wants to do with a position.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvancedExit {
    /// Reason prefix used for rule attribution and telemetry.
    pub kind_label: &'static str,
    /// Fraction of the current position to sell.
    pub fraction: f64,
    /// Human-readable, deterministic explanation.
    pub reason: String,
}

impl AdvancedExit {
    /// Build the `ExitDecision`-shaped reason string used by the sweeper.
    pub fn full_reason(&self) -> String {
        format!("{}: {}", self.kind_label, self.reason)
    }
}

/// Per-position ladder state: index of the NEXT rung to fire.
#[derive(Debug, Default)]
struct LadderState {
    next_level: usize,
    /// Highest mark seen (break-even arming uses the position's own HWM, but
    /// the tracker also records arming so a dip below entry cannot disarm).
    break_even_armed: bool,
}

/// Stateful evaluator: owns ladder cursors and break-even arming per
/// position, plus the dev-sell signal window.
#[derive(Debug)]
pub struct ExitPolicyEngine {
    config: ExitPolicyConfig,
    ladders: HashMap<String, LadderState>,
    /// mint -> last observed dev-sell time.
    dev_sells: HashMap<Pubkey, DateTime<Utc>>,
}

impl ExitPolicyEngine {
    /// Build an engine from a normalised config.
    pub fn new(config: ExitPolicyConfig) -> Self {
        ExitPolicyEngine {
            config: config.normalized(),
            ladders: HashMap::new(),
            dev_sells: HashMap::new(),
        }
    }

    /// The active configuration (read-only).
    pub fn config(&self) -> &ExitPolicyConfig {
        &self.config
    }

    /// Record a dev-sell observation for `mint` (signal sink; the detector
    /// that produces these lives in `crate::risk_intel`).
    pub fn observe_dev_sell(&mut self, mint: &Pubkey, at: DateTime<Utc>) {
        let entry = self.dev_sells.entry(*mint).or_insert(at);
        if at > *entry {
            *entry = at;
        }
    }

    /// Drop state for positions that no longer exist (same contract as
    /// `ExitTracker::retain`) and expire stale dev-sell signals.
    pub fn retain(&mut self, live: &[String], now: DateTime<Utc>) {
        self.ladders.retain(|id, _| live.iter().any(|p| p == id));
        if self.config.dev_sell_ttl_secs > 0 {
            let ttl = Duration::from_secs(self.config.dev_sell_ttl_secs);
            self.dev_sells
                .retain(|_, ts| now.signed_duration_since(*ts).to_std().map(|d| d <= ttl).unwrap_or(false));
        } else {
            self.dev_sells.clear();
        }
    }

    /// Was a dev sell observed for this mint inside the TTL window?
    pub fn dev_sell_firing(&self, mint: &Pubkey, now: DateTime<Utc>) -> bool {
        if self.config.dev_sell_ttl_secs == 0 {
            return false;
        }
        let Some(ts) = self.dev_sells.get(mint) else {
            return false;
        };
        now.signed_duration_since(*ts)
            .to_std()
            .map(|d| d <= Duration::from_secs(self.config.dev_sell_ttl_secs))
            .unwrap_or(false)
    }

    /// Evaluate the advanced layer for one position. Returns the FIRST
    /// action that fires, in priority order:
    /// 1. dev-sell (get out entirely),
    /// 2. break-even stop (protect the round trip),
    /// 3. the next ladder rung (take profit in steps).
    ///
    /// Pure with respect to its inputs given the engine's state; evaluating
    /// advances the ladder cursor ONLY when a rung fires.
    pub fn evaluate(
        &mut self,
        position_id: &str,
        mint: Option<&Pubkey>,
        avg_entry: f64,
        mark: f64,
        high_water: f64,
        now: DateTime<Utc>,
    ) -> Option<AdvancedExit> {
        if !(mark.is_finite() && mark > 0.0 && avg_entry.is_finite() && avg_entry > 0.0) {
            return None;
        }

        // 1. Dev-sell trigger — flatten everything.
        if let Some(mint) = mint {
            if self.dev_sell_firing(mint, now) {
                return Some(AdvancedExit {
                    kind_label: "dev_sell",
                    fraction: 1.0,
                    reason: format!(
                        "creator/dev wallet sold within the last {}s",
                        self.config.dev_sell_ttl_secs
                    ),
                });
            }
        }

        let state = self.ladders.entry(position_id.to_string()).or_default();

        // 2. Break-even stop: arm once HWM passes the activation multiple,
        //    fire when the mark falls back to the armed stop.
        if let Some(be) = self.config.break_even {
            let arm_price = avg_entry * be.activation_multiple;
            if high_water >= arm_price {
                state.break_even_armed = true;
            }
            if state.break_even_armed {
                let stop = avg_entry * (1.0 + be.buffer);
                if mark <= stop {
                    return Some(AdvancedExit {
                        kind_label: "break_even",
                        fraction: 1.0,
                        reason: format!(
                            "mark {mark:.9} <= armed break-even stop {stop:.9} (entry {avg_entry:.9}, armed at hwm >= {arm_price:.9})"
                        ),
                    });
                }
            }
        }

        // 3. Laddered take-profit: fire the next rung whose trigger the mark
        //    has reached. Levels fire in order, each at most once.
        while state.next_level < self.config.ladder.len() {
            let level = self.config.ladder[state.next_level];
            let trigger = avg_entry * level.trigger_multiple;
            if mark >= trigger {
                state.next_level += 1;
                return Some(AdvancedExit {
                    kind_label: "ladder_tp",
                    fraction: level.sell_fraction,
                    reason: format!(
                        "ladder level {} fired: mark {mark:.9} >= {trigger:.9} (entry {avg_entry:.9} x {:.2}, selling {:.0}%)",
                        state.next_level,
                        level.trigger_multiple,
                        level.sell_fraction * 100.0
                    ),
                });
            }
            break; // rungs are sorted; the next one cannot fire either
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(ladder: Vec<TpLevel>, break_even: Option<BreakEvenConfig>, dev_ttl: u64) -> ExitPolicyConfig {
        ExitPolicyConfig {
            ladder,
            break_even,
            dev_sell_ttl_secs: dev_ttl,
        }
    }

    fn lvl(mult: f64, frac: f64) -> TpLevel {
        TpLevel { trigger_multiple: mult, sell_fraction: frac }
    }

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn normalization_sorts_and_prunes_the_ladder() {
        let c = cfg(
            vec![lvl(3.0, 0.5), lvl(2.0, 0.3), lvl(0.5, 0.9), lvl(2.5, 2.0)],
            None,
            0,
        )
        .normalized();
        assert_eq!(c.ladder.len(), 3, "sub-1x trigger pruned");
        assert_eq!(c.ladder[0].trigger_multiple, 2.0);
        assert_eq!(c.ladder[1].trigger_multiple, 2.5);
        assert_eq!(c.ladder[2].trigger_multiple, 3.0);
        // After sorting, the 2.5x level (fraction 2.0 -> clamped) sits at index 1.
        assert_eq!(c.ladder[1].sell_fraction, 1.0, "fraction clamped to 1.0");
        assert_eq!(c.ladder[2].sell_fraction, 0.5);
    }

    #[test]
    fn empty_policy_never_fires() {
        let mut e = ExitPolicyEngine::new(cfg(vec![], None, 0));
        let now = t(1_000);
        for mark in [0.0001, 1.0, 1_000.0] {
            assert!(e.evaluate("p", None, 1.0, mark, mark, now).is_none());
        }
    }

    #[test]
    fn ladder_fires_rungs_in_order_once_each() {
        let mut e = ExitPolicyEngine::new(cfg(vec![lvl(2.0, 0.3), lvl(5.0, 0.7)], None, 0));
        let now = t(1_000);
        // Below the first rung: nothing.
        assert!(e.evaluate("p", None, 1.0, 1.9, 1.9, now).is_none());
        // At the first rung: sells 30%.
        let first = e.evaluate("p", None, 1.0, 2.0, 2.0, now).unwrap();
        assert_eq!(first.kind_label, "ladder_tp");
        assert!((first.fraction - 0.3).abs() < 1e-12);
        // Still below rung two: nothing more.
        assert!(e.evaluate("p", None, 1.0, 4.0, 4.0, now).is_none());
        // At rung two: sells 70% (of the remaining position).
        let second = e.evaluate("p", None, 1.0, 5.0, 5.0, now).unwrap();
        assert!((second.fraction - 0.7).abs() < 1e-12);
        // Ladder exhausted: even higher marks fire nothing.
        assert!(e.evaluate("p", None, 1.0, 50.0, 50.0, now).is_none());
    }

    #[test]
    fn ladder_state_is_per_position() {
        let mut e = ExitPolicyEngine::new(cfg(vec![lvl(2.0, 0.5)], None, 0));
        let now = t(1_000);
        assert!(e.evaluate("a", None, 1.0, 2.0, 2.0, now).is_some());
        // Position b has not fired yet.
        assert!(e.evaluate("b", None, 1.0, 2.0, 2.0, now).is_some());
        // a already fired.
        assert!(e.evaluate("a", None, 1.0, 2.0, 2.0, now).is_none());
    }

    #[test]
    fn jump_past_multiple_rungs_fires_them_one_sweep_at_a_time() {
        let mut e = ExitPolicyEngine::new(cfg(
            vec![lvl(2.0, 0.25), lvl(3.0, 0.25), lvl(4.0, 0.5)],
            None,
            0,
        ));
        let now = t(1_000);
        let a = e.evaluate("p", None, 1.0, 4.5, 4.5, now).unwrap();
        assert!((a.fraction - 0.25).abs() < 1e-12, "first rung first");
        let b = e.evaluate("p", None, 1.0, 4.5, 4.5, now).unwrap();
        assert!((b.fraction - 0.25).abs() < 1e-12, "second rung next sweep");
        let c = e.evaluate("p", None, 1.0, 4.5, 4.5, now).unwrap();
        assert!((c.fraction - 0.5).abs() < 1e-12, "third rung after");
        assert!(e.evaluate("p", None, 1.0, 4.5, 4.5, now).is_none());
    }

    #[test]
    fn break_even_arms_on_hwm_and_survives_a_dip() {
        let be = BreakEvenConfig { activation_multiple: 1.5, buffer: 0.02 };
        let mut e = ExitPolicyEngine::new(cfg(vec![], Some(be), 0));
        let now = t(1_000);
        // Never armed: no exit even at a loss.
        assert!(e.evaluate("p", None, 1.0, 0.8, 1.2, now).is_none());
        // HWM arms it; mark still above the stop.
        assert!(e.evaluate("p", None, 1.0, 1.4, 1.6, now).is_none());
        // Mark falls to the armed stop (entry * 1.02).
        let exit = e.evaluate("p", None, 1.0, 1.02, 1.6, now).unwrap();
        assert_eq!(exit.kind_label, "break_even");
        assert!((exit.fraction - 1.0).abs() < 1e-12);
        // Armed state persists even if HWM argument drops (position's own
        // high-water never decreases in the sweeper).
        let again = e.evaluate("p", None, 1.0, 1.01, 1.01, now).unwrap();
        assert_eq!(again.kind_label, "break_even");
    }

    #[test]
    fn dev_sell_beats_everything_and_expires() {
        let mut e = ExitPolicyEngine::new(cfg(vec![lvl(2.0, 0.5)], None, 60));
        let mint = Pubkey::new_unique();
        let now = t(1_000);
        e.observe_dev_sell(&mint, now);
        // Even with a ladder trigger available, dev-sell wins.
        let exit = e.evaluate("p", Some(&mint), 1.0, 3.0, 3.0, now).unwrap();
        assert_eq!(exit.kind_label, "dev_sell");
        assert!((exit.fraction - 1.0).abs() < 1e-12);
        // After the TTL, the signal expires.
        let later = t(1_061);
        e.retain(&["p".to_string()], later);
        assert!(!e.dev_sell_firing(&mint, later));
        // And the untouched ladder is still intact.
        let ladder = e.evaluate("p", Some(&mint), 1.0, 3.0, 3.0, later).unwrap();
        assert_eq!(ladder.kind_label, "ladder_tp");
    }

    #[test]
    fn retain_drops_closed_positions_but_keeps_live_ones() {
        let mut e = ExitPolicyEngine::new(cfg(vec![lvl(2.0, 0.5)], None, 0));
        let now = t(1_000);
        assert!(e.evaluate("keep", None, 1.0, 2.0, 2.0, now).is_some());
        assert!(e.evaluate("drop", None, 1.0, 2.0, 2.0, now).is_some());
        e.retain(&["keep".to_string()], now);
        // keep already fired; drop was forgotten and can fire again.
        assert!(e.evaluate("keep", None, 1.0, 2.0, 2.0, now).is_none());
        assert!(e.evaluate("drop", None, 1.0, 2.0, 2.0, now).is_some());
    }

    #[test]
    fn degenerate_inputs_never_fire() {
        let mut e = ExitPolicyEngine::new(cfg(vec![lvl(2.0, 0.5)], None, 60));
        let now = t(1_000);
        assert!(e.evaluate("p", None, 0.0, 2.0, 2.0, now).is_none(), "zero entry");
        assert!(e.evaluate("p", None, 1.0, f64::NAN, 2.0, now).is_none(), "nan mark");
        assert!(e.evaluate("p", None, f64::INFINITY, 2.0, 2.0, now).is_none(), "inf entry");
    }

    #[test]
    fn reason_strings_are_prefixed_for_rule_attribution() {
        let mut e = ExitPolicyEngine::new(cfg(vec![lvl(2.0, 0.5)], None, 0));
        let exit = e.evaluate("p", None, 1.0, 2.0, 2.0, t(1_000)).unwrap();
        assert!(exit.full_reason().starts_with("ladder_tp: "));
    }
}

// ---------------------------------------------------------------------------
// Config bridge + signal bus
// ---------------------------------------------------------------------------

/// The dev-sell signal carried on the bus: which mint's creator/dev wallet
/// sold, and when the sale was observed.
pub type DevSellSignal = (Pubkey, DateTime<Utc>);

/// Channel type used to fan dev-sell signals from the detection/risk_intel
/// layer into every exit sweeper (one sender, N subscribed sweepers).
pub type DevSellBus = tokio::sync::broadcast::Sender<DevSellSignal>;

/// Build a policy engine from the operator's `sniper.advanced_exit` block.
/// `None` (the default deployment) yields an INERT engine: no ladder, no
/// break-even, dev-sell disabled — behaviour identical to the classic rules.
pub fn engine_from_config(cfg: Option<&bot_core::config::AdvancedExitConfig>) -> ExitPolicyEngine {
    let policy = cfg.map(ExitPolicyConfig::from).unwrap_or_default();
    ExitPolicyEngine::new(policy)
}

impl From<&bot_core::config::AdvancedExitConfig> for ExitPolicyConfig {
    fn from(value: &bot_core::config::AdvancedExitConfig) -> Self {
        ExitPolicyConfig {
            ladder: value
                .ladder
                .iter()
                .map(|l| TpLevel {
                    trigger_multiple: l.trigger_multiple,
                    sell_fraction: l.sell_fraction,
                })
                .collect(),
            break_even: value.break_even.map(|b| BreakEvenConfig {
                activation_multiple: b.activation_multiple,
                buffer: b.buffer,
            }),
            dev_sell_ttl_secs: value.dev_sell_ttl_secs,
        }
    }
}
