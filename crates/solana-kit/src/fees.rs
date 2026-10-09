//! Priority-fee infrastructure: a configurable policy (bounds, emergency
//! limit, per-retry escalation), adaptive selection from
//! `getRecentPrioritizationFees`, and metrics for every decision.
//!
//! The policy is deliberately separate from [`crate::execute::ExecPolicy`]
//! so existing callers (and their struct literals) are untouched: the
//! executor carries a `FeePolicy` next to its `ExecPolicy`, defaulting to
//! the config defaults, and modules pass `FeePolicy::from_config`.
//!
//! Decision order for one attempt (see [`FeePolicy::decide`]):
//! 1. start from the request's fee (`TxRequest::priority_fee_micro_lamports`);
//! 2. in `adaptive` mode raise it to the configured percentile of recent
//!    fees when the oracle has a sample (never lower it — the request's fee
//!    is the caller's floor);
//! 3. escalate by `escalation_pct` per retry attempt;
//! 4. clamp to `[min, max]`;
//! 5. refuse outright when the *requested* fee (before clamping) exceeds
//!    the emergency limit — a runaway fee is a bug, not a market condition.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use bot_core::config::ExecutionConfig;
use bot_core::obs::metrics;

use crate::rpc::Rpc;

/// Priority-fee policy in micro-lamports per compute unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeePolicy {
    /// `true` = `fee_mode = "adaptive"`: consult the oracle.
    pub adaptive: bool,
    /// Lower bound applied to every decision.
    pub min_micro_lamports: u64,
    /// Upper bound: adaptive quotes and escalation are clamped here.
    pub max_micro_lamports: u64,
    /// Requests above this are refused, not clamped.
    pub emergency_max_micro_lamports: u64,
    /// Percentile (1–100) of recent fees paid in adaptive mode.
    pub percentile: u8,
    /// Escalation per retry attempt, percent of the previous fee.
    pub escalation_pct: u64,
    /// Validity of one oracle sample.
    pub oracle_ttl: Duration,
    /// Maximum age of a cached blockhash the executor will sign with.
    pub max_blockhash_age: Duration,
}

impl Default for FeePolicy {
    fn default() -> Self {
        FeePolicy::from_config(&ExecutionConfig::default())
    }
}

impl FeePolicy {
    pub fn from_config(cfg: &ExecutionConfig) -> Self {
        let min = cfg.fee_min_micro_lamports;
        let max = cfg.fee_max_micro_lamports.max(min);
        FeePolicy {
            adaptive: cfg.fee_adaptive(),
            min_micro_lamports: min,
            max_micro_lamports: max,
            emergency_max_micro_lamports: cfg.fee_emergency_max_micro_lamports.max(max),
            percentile: cfg.fee_percentile.clamp(1, 100),
            escalation_pct: cfg.fee_escalation_pct,
            oracle_ttl: Duration::from_millis(cfg.fee_oracle_ttl_ms),
            max_blockhash_age: Duration::from_millis(cfg.max_blockhash_age_ms.max(1)),
        }
    }

    /// Clamp a fee into `[min, max]`.
    pub fn clamp(&self, fee: u64) -> u64 {
        fee.clamp(self.min_micro_lamports, self.max_micro_lamports)
    }

    /// Fee for retry `attempt` (1-based; attempt 1 = no escalation),
    /// compounded and capped at `max`.
    pub fn escalate(&self, base: u64, attempt: u32) -> u64 {
        let mut fee = base;
        for _ in 1..attempt.max(1) {
            let bump = fee.saturating_mul(self.escalation_pct) / 100;
            fee = fee.saturating_add(bump.max(if self.escalation_pct > 0 { 1 } else { 0 }));
            if fee >= self.max_micro_lamports {
                return self.max_micro_lamports;
            }
        }
        fee.min(self.max_micro_lamports)
    }

    /// Take the configured percentile of an ascending fee sample.
    pub fn percentile_of(&self, sorted: &[u64]) -> Option<u64> {
        if sorted.is_empty() {
            return None;
        }
        let rank = (self.percentile as usize * sorted.len()).div_ceil(100);
        Some(sorted[rank.clamp(1, sorted.len()) - 1])
    }

    /// Would [`FeePolicy::decide`] refuse a request for `requested`
    /// µlamports/CU? Pure and side-effect free (no metric is published), so
    /// a caller can ask *before* committing to an attempt — the sniper's
    /// pre-submission fee check reports a certain veto as `FEE_LIMIT`
    /// instead of recording a failed submission. `decide` stays the
    /// authority at submission time and uses this same predicate.
    pub fn would_refuse(&self, requested: u64) -> bool {
        requested > self.emergency_max_micro_lamports
            || self.clamp(requested) > self.emergency_max_micro_lamports
    }

    /// The highest fee (µlamports/CU) `decide` can settle on for `requested`
    /// over `attempts` attempts of one execution: in adaptive mode the
    /// policy's `max` (an oracle quote may raise the fee up to it), otherwise
    /// the clamped request escalated for the last attempt (escalation is
    /// capped at `max` as well). Pure; used to bound an entry's fee budget.
    pub fn max_payable(&self, requested: u64, attempts: u32) -> u64 {
        let base = self.clamp(requested);
        if self.adaptive {
            base.max(self.max_micro_lamports)
        } else {
            self.escalate(base, attempts.max(1)).max(base)
        }
    }

    /// Decide the fee for one attempt. `oracle` is the adaptive quote (if
    /// any); `attempt` is 1-based.
    pub fn decide(&self, requested: u64, oracle: Option<u64>, attempt: u32) -> FeeDecision {
        let mut fee = requested;
        let mut source = FeeSource::Requested;
        if self.adaptive {
            if let Some(q) = oracle {
                if q > fee {
                    fee = q;
                    source = FeeSource::Adaptive;
                }
            }
        }
        if attempt > 1 && self.escalation_pct > 0 {
            let escalated = self.escalate(fee, attempt);
            if escalated != fee {
                fee = escalated;
                source = FeeSource::Escalated;
            }
        }
        // Oracle/escalation output is always capped at `max`, so the
        // emergency line guards two things: a caller that explicitly asks
        // for more than we are ever willing to pay, and a misconfiguration
        // where `max` itself sits above the emergency limit.
        let clamped_fee = self.clamp(fee);
        let clamped = clamped_fee != fee;
        let refused =
            self.would_refuse(requested) || clamped_fee > self.emergency_max_micro_lamports;
        let decision = FeeDecision {
            requested,
            fee: clamped_fee,
            source,
            clamped,
            refused,
            attempt,
        };
        decision.publish();
        decision
    }
}

/// Where a fee decision came from (metric label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeSource {
    Requested,
    Adaptive,
    Escalated,
}

impl FeeSource {
    pub fn label(&self) -> &'static str {
        match self {
            FeeSource::Requested => "requested",
            FeeSource::Adaptive => "adaptive",
            FeeSource::Escalated => "escalated",
        }
    }
}

/// Outcome of [`FeePolicy::decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeDecision {
    /// What the request asked for.
    pub requested: u64,
    /// What will be paid (after clamping).
    pub fee: u64,
    pub source: FeeSource,
    /// The bound moved the fee.
    pub clamped: bool,
    /// The emergency limit was crossed: do NOT broadcast.
    pub refused: bool,
    pub attempt: u32,
}

impl FeeDecision {
    /// Human explanation for the veto error / audit detail.
    pub fn refusal_reason(&self, policy: &FeePolicy) -> String {
        format!(
            "priority fee {} µlamports/CU exceeds emergency limit {} (requested {}, source {})",
            self.fee.max(self.requested),
            policy.emergency_max_micro_lamports,
            self.requested,
            self.source.label()
        )
    }

    fn publish(&self) {
        let reg = metrics::global();
        reg.counter(
            "bot_priority_fee_decisions_total",
            "Priority-fee decisions by source.",
            &[("source", self.source.label())],
        )
        .inc();
        reg.gauge(
            "bot_priority_fee_last_micro_lamports",
            "Priority fee chosen by the most recent decision (µlamports/CU).",
            &[("source", self.source.label())],
        )
        .set(self.fee.min(i64::MAX as u64) as i64);
        if self.clamped {
            reg.counter(
                "bot_priority_fee_clamped_total",
                "Fee decisions moved by the min/max bounds.",
                &[],
            )
            .inc();
        }
        if self.refused {
            reg.counter(
                "bot_priority_fee_refused_total",
                "Fee decisions refused by the emergency limit.",
                &[],
            )
            .inc();
        }
    }
}

/// Cached `getRecentPrioritizationFees` sampler.
pub struct FeeOracle {
    rpc: Rpc,
    policy: FeePolicy,
    sample: Mutex<Option<(Instant, Vec<u64>)>>,
}

impl FeeOracle {
    pub fn new(rpc: Rpc, policy: FeePolicy) -> Arc<Self> {
        Arc::new(FeeOracle {
            rpc,
            policy,
            sample: Mutex::new(None),
        })
    }

    pub fn policy(&self) -> &FeePolicy {
        &self.policy
    }

    /// Percentile quote (µlamports/CU) from a fresh-enough sample, or `None`
    /// when the oracle is unavailable — the caller then keeps the requested
    /// fee. `accounts` narrows the sample to the writable accounts of the
    /// transaction (the fee market is per account).
    pub async fn quote(&self, accounts: &[Pubkey]) -> Option<u64> {
        let mut guard = self.sample.lock().await;
        let stale = match &*guard {
            Some((at, _)) => at.elapsed() > self.policy.oracle_ttl,
            None => true,
        };
        if stale {
            // Cap the account list: the RPC rejects more than 128.
            let keys: Vec<Pubkey> = accounts.iter().take(128).copied().collect();
            match self.rpc.recent_prioritization_fees(&keys).await {
                Ok(sorted) => {
                    metrics::global()
                        .counter(
                            "bot_priority_fee_oracle_samples_total",
                            "getRecentPrioritizationFees samples by outcome.",
                            &[("outcome", "ok")],
                        )
                        .inc();
                    *guard = Some((Instant::now(), sorted));
                }
                Err(e) => {
                    metrics::global()
                        .counter(
                            "bot_priority_fee_oracle_samples_total",
                            "getRecentPrioritizationFees samples by outcome.",
                            &[("outcome", "error")],
                        )
                        .inc();
                    warn!(error = %e, "priority fee oracle unavailable, using requested fee");
                    // Keep a stale sample rather than nothing.
                    if guard.is_none() {
                        return None;
                    }
                }
            }
        }
        let quote = guard
            .as_ref()
            .and_then(|(_, sorted)| self.policy.percentile_of(sorted));
        if let Some(q) = quote {
            debug!(
                quote = q,
                percentile = self.policy.percentile,
                "adaptive priority fee quote"
            );
            metrics::global()
                .gauge(
                    "bot_priority_fee_oracle_quote_micro_lamports",
                    "Latest adaptive fee quote at the configured percentile.",
                    &[],
                )
                .set(q.min(i64::MAX as u64) as i64);
        }
        quote
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> FeePolicy {
        FeePolicy {
            adaptive: false,
            min_micro_lamports: 1_000,
            max_micro_lamports: 1_000_000,
            emergency_max_micro_lamports: 5_000_000,
            percentile: 75,
            escalation_pct: 50,
            oracle_ttl: Duration::from_secs(2),
            max_blockhash_age: Duration::from_secs(20),
        }
    }

    #[test]
    fn defaults_come_from_the_execution_config() {
        let p = FeePolicy::default();
        let cfg = ExecutionConfig::default();
        assert_eq!(p.adaptive, cfg.fee_adaptive());
        assert_eq!(p.max_micro_lamports, cfg.fee_max_micro_lamports);
        assert_eq!(
            p.emergency_max_micro_lamports,
            cfg.fee_emergency_max_micro_lamports
        );
        assert_eq!(p.percentile, cfg.fee_percentile);
        assert_eq!(
            p.max_blockhash_age,
            Duration::from_millis(cfg.max_blockhash_age_ms)
        );
        // Incoherent config is normalised, never panics.
        let bad = ExecutionConfig {
            fee_min_micro_lamports: 10,
            fee_max_micro_lamports: 5,
            fee_emergency_max_micro_lamports: 1,
            fee_percentile: 0,
            ..Default::default()
        };
        let p = FeePolicy::from_config(&bad);
        assert_eq!(p.max_micro_lamports, 10);
        assert_eq!(p.emergency_max_micro_lamports, 10);
        assert_eq!(p.percentile, 1);
    }

    #[test]
    fn fixed_mode_keeps_the_requested_fee_inside_bounds() {
        let p = policy();
        let d = p.decide(250_000, Some(900_000), 1);
        assert_eq!(d.fee, 250_000, "fixed mode ignores the oracle");
        assert_eq!(d.source, FeeSource::Requested);
        assert!(!d.clamped && !d.refused);

        let low = p.decide(10, None, 1);
        assert_eq!(low.fee, 1_000, "raised to the minimum");
        assert!(low.clamped);

        let high = p.decide(2_000_000, None, 1);
        assert_eq!(high.fee, 1_000_000, "clamped to the maximum");
        assert!(high.clamped && !high.refused);
    }

    #[test]
    fn emergency_limit_refuses_instead_of_clamping() {
        let p = policy();
        let d = p.decide(5_000_001, None, 1);
        assert!(d.refused, "above the emergency limit");
        assert_eq!(
            d.fee, 1_000_000,
            "still reports the clamped value for the audit row"
        );
        assert!(d.refusal_reason(&p).contains("emergency limit"));
        // Escalation that would cross the emergency line is refused too.
        let tight = FeePolicy {
            max_micro_lamports: 10_000_000,
            emergency_max_micro_lamports: 10_000_000,
            ..p
        };
        let ok = tight.decide(8_000_000, None, 1);
        assert!(!ok.refused);
        let esc = tight.decide(8_000_000, None, 2);
        assert!(
            !esc.refused,
            "escalation is capped at max, which is <= emergency"
        );
        assert_eq!(esc.fee, 10_000_000);
    }

    #[test]
    fn would_refuse_agrees_with_decide() {
        let p = policy();
        for requested in [
            0u64,
            1_000,
            250_000,
            1_000_000,
            5_000_000,
            5_000_001,
            u64::MAX,
        ] {
            assert_eq!(
                p.would_refuse(requested),
                p.decide(requested, None, 1).refused,
                "requested {requested}"
            );
            // An oracle quote or a retry never turns an accepted request
            // into a refusal (both are capped at max <= emergency).
            for attempt in 1..=3 {
                assert_eq!(
                    p.would_refuse(requested),
                    p.decide(requested, Some(50_000_000), attempt).refused,
                    "requested {requested} attempt {attempt}"
                );
            }
        }
        assert!(!p.would_refuse(5_000_000), "the limit itself is allowed");
        assert!(p.would_refuse(5_000_001));
        // A hand-built policy whose max sits above the emergency line: the
        // clamped request crosses it even though the request is below max.
        let odd = FeePolicy {
            max_micro_lamports: 10_000_000,
            emergency_max_micro_lamports: 6_000_000,
            min_micro_lamports: 7_000_000,
            ..p
        };
        assert!(odd.would_refuse(100));
        assert!(odd.decide(100, None, 1).refused);
    }

    #[test]
    fn max_payable_bounds_every_attempt() {
        let p = policy();
        assert_eq!(
            p.max_payable(250_000, 1),
            250_000,
            "one attempt: as requested"
        );
        assert_eq!(p.max_payable(250_000, 2), 375_000, "second attempt: +50%");
        assert_eq!(
            p.max_payable(250_000, 0),
            250_000,
            "0 attempts behaves as 1"
        );
        assert_eq!(
            p.max_payable(900_000, 5),
            1_000_000,
            "escalation caps at max"
        );
        assert_eq!(p.max_payable(10, 1), 1_000, "raised to min first");
        let flat = FeePolicy {
            escalation_pct: 0,
            ..p
        };
        assert_eq!(flat.max_payable(250_000, 5), 250_000);
        let adaptive = FeePolicy {
            adaptive: true,
            ..p
        };
        assert_eq!(
            adaptive.max_payable(250_000, 1),
            1_000_000,
            "adaptive: the oracle may raise the fee to max"
        );
        // Whatever decide returns for any attempt/oracle is never above it.
        for attempt in 1..=5 {
            for oracle in [None, Some(10), Some(600_000), Some(50_000_000)] {
                assert!(p.decide(250_000, oracle, attempt).fee <= p.max_payable(250_000, 5));
                assert!(
                    adaptive.decide(250_000, oracle, attempt).fee
                        <= adaptive.max_payable(250_000, 1)
                );
            }
        }
    }

    #[test]
    fn escalation_compounds_per_attempt_and_caps() {
        let p = policy();
        assert_eq!(p.escalate(100_000, 1), 100_000);
        assert_eq!(p.escalate(100_000, 2), 150_000);
        assert_eq!(p.escalate(100_000, 3), 225_000);
        assert_eq!(p.escalate(900_000, 3), 1_000_000, "capped at max");
        let d = p.decide(100_000, None, 2);
        assert_eq!(d.fee, 150_000);
        assert_eq!(d.source, FeeSource::Escalated);
        let none = FeePolicy {
            escalation_pct: 0,
            ..p
        };
        assert_eq!(none.decide(100_000, None, 4).fee, 100_000);
        assert_eq!(none.decide(100_000, None, 4).source, FeeSource::Requested);
    }

    #[test]
    fn adaptive_mode_only_raises_and_respects_the_cap() {
        let p = FeePolicy {
            adaptive: true,
            ..policy()
        };
        let raised = p.decide(100_000, Some(400_000), 1);
        assert_eq!(raised.fee, 400_000);
        assert_eq!(raised.source, FeeSource::Adaptive);
        let kept = p.decide(500_000, Some(400_000), 1);
        assert_eq!(kept.fee, 500_000, "the request is a floor");
        assert_eq!(kept.source, FeeSource::Requested);
        let capped = p.decide(100_000, Some(50_000_000), 1);
        assert_eq!(capped.fee, 1_000_000);
        assert!(capped.clamped);
        assert!(
            !capped.refused,
            "an insane oracle is clamped, the request was sane"
        );
        let missing = p.decide(100_000, None, 1);
        assert_eq!(missing.fee, 100_000, "no sample → requested fee");
    }

    #[test]
    fn percentile_selection_is_nearest_rank() {
        let p = policy();
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(p.percentile_of(&sorted), Some(75));
        assert_eq!(
            FeePolicy {
                percentile: 100,
                ..p
            }
            .percentile_of(&sorted),
            Some(100)
        );
        assert_eq!(
            FeePolicy { percentile: 1, ..p }.percentile_of(&sorted),
            Some(1)
        );
        assert_eq!(
            FeePolicy {
                percentile: 50,
                ..p
            }
            .percentile_of(&[10, 20, 30]),
            Some(20)
        );
        assert_eq!(p.percentile_of(&[]), None);
    }
}
