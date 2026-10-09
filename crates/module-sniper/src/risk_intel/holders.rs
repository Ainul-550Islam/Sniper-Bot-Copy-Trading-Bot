//! Holder-concentration intelligence (GAP-MAP v2, P2).
//!
//! Bridges `solana_kit::holders` (fetch + pure bps math) into the sniper's
//! gate vocabulary. The entry gate `max_top_holder_pct` (percent) is already
//! in `crate::gates` from the P1 work; this file supplies the measurement
//! and the percent/bps conversion, keeping every arithmetic rule in ONE
//! place (`solana_kit::holders`).
//!
//! Exclusions are critical: bonding-curve vaults, AMM pools/vaults and burn
//! addresses are INFRASTRUCTURE, not holders. Measuring them as whales would
//! reject every healthy launch. Callers pass the exclusion set they derive
//! from the launch context; this module never guesses an exclusion.

use std::collections::HashSet;

use solana_sdk::pubkey::Pubkey;

use bot_core::error::BotResult;
use solana_kit::holders::{self as kit, ConcentrationLimits, ConcentrationReport, ConcentrationVerdict};
use solana_kit::rpc::Rpc;

/// Basis-point denominator (mirrors the kit's).
pub const BPS: u64 = 10_000;

/// Convert a percent threshold (gate config) to basis points.
/// Non-finite or negative input maps to 0 (gate off), >100% maps to the
/// 100% ceiling — a gate threshold can never mean "more than everything".
pub fn bps_from_pct(pct: f64) -> u64 {
    if !pct.is_finite() || pct <= 0.0 {
        return 0;
    }
    let scaled = pct * 100.0; // 1% -> 100 bps
    if scaled >= BPS as f64 {
        return BPS;
    }
    scaled.round() as u64
}

/// Convert basis points back to a percent (for human-readable reasons).
pub fn pct_from_bps(bps: u64) -> f64 {
    (bps as f64) / 100.0
}

/// Measure holder concentration for `mint`, excluding infrastructure
/// accounts. Returns the kit's report (bps of float) — the gate layer turns
/// it into pass/reject against `max_top_holder_pct`.
pub async fn measure_concentration(
    rpc: &Rpc,
    mint: &Pubkey,
    excluded: &HashSet<Pubkey>,
) -> BotResult<ConcentrationReport> {
    let holders = kit::fetch_largest_holders(rpc, mint).await?;
    Ok(kit::concentration(&holders, excluded))
}

/// Evaluate a measured report against the operator's top-holder percentage
/// threshold. `max_top_holder_pct <= 0` disables the check (returns Pass).
///
/// Note the gate only looks at the SINGLE largest holder — that is what
/// `max_top_holder_pct` means. Top-5/top-10 clustering belongs to the
/// richer [`kit::evaluate_concentration`] limits, which operators can wire
/// through `ConcentrationLimits` when they want it.
pub fn gate_single_holder(report: &ConcentrationReport, max_top_holder_pct: f64) -> ConcentrationVerdict {
    let limit_bps = bps_from_pct(max_top_holder_pct);
    if limit_bps == 0 {
        return ConcentrationVerdict::Pass;
    }
    let limits = ConcentrationLimits {
        max_top1_bps: limit_bps,
        // Leave the cluster limits at "always pass" — this entry gate is
        // about the single largest holder only.
        max_top5_bps: BPS,
        max_top10_bps: BPS,
    };
    kit::evaluate_concentration(report, &limits)
}

/// Render a rejection reason from a verdict + report (deterministic text for
/// the gate summary and telemetry).
pub fn reason(verdict: ConcentrationVerdict, report: &ConcentrationReport) -> String {
    match verdict {
        ConcentrationVerdict::Pass => format!(
            "holder concentration ok (top1 {:.2}% of float)",
            pct_from_bps(report.top1_bps)
        ),
        ConcentrationVerdict::SingleWhale => format!(
            "single holder controls {:.2}% of float",
            pct_from_bps(report.top1_bps)
        ),
        ConcentrationVerdict::TopFiveCluster => format!(
            "top-5 holders control {:.2}% of float",
            pct_from_bps(report.top5_bps)
        ),
        ConcentrationVerdict::TopTenCluster => format!(
            "top-10 holders control {:.2}% of float",
            pct_from_bps(report.top10_bps)
        ),
        ConcentrationVerdict::EmptyFloat => {
            "no tradable float outside infrastructure accounts".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_kit::holders::HolderSnapshot;

    fn holder(seed: u8, amount: u64) -> HolderSnapshot {
        HolderSnapshot {
            address: Pubkey::new_from_array([seed; 32]),
            amount,
        }
    }

    #[test]
    fn pct_bps_round_trip() {
        assert_eq!(bps_from_pct(1.0), 100);
        assert_eq!(bps_from_pct(20.0), 2_000);
        assert_eq!(bps_from_pct(0.0), 0, "zero disables");
        assert_eq!(bps_from_pct(-5.0), 0, "negative disables");
        assert_eq!(bps_from_pct(f64::NAN), 0, "non-finite disables");
        assert_eq!(bps_from_pct(150.0), BPS, "capped at 100%");
        assert!((pct_from_bps(2_500) - 25.0).abs() < 1e-9);
    }

    #[test]
    fn disabled_threshold_always_passes() {
        let report = kit::concentration(&[holder(1, 9_000), holder(2, 1_000)], &HashSet::new());
        assert_eq!(
            gate_single_holder(&report, 0.0),
            ConcentrationVerdict::Pass
        );
    }

    #[test]
    fn single_holder_gate_ignores_cluster_limits() {
        // Five wallets at 15% each: top1 is fine at a 20% single-holder
        // limit even though top5 = 75% would trip a cluster rule. The entry
        // gate measures the SINGLE largest holder only.
        let holders: Vec<HolderSnapshot> = (1..=6).map(|i| holder(i, 150)).collect();
        let report = kit::concentration(&holders, &HashSet::new());
        assert_eq!(
            gate_single_holder(&report, 20.0),
            ConcentrationVerdict::Pass
        );
    }

    #[test]
    fn whale_is_rejected_with_a_readable_reason() {
        let report = kit::concentration(&[holder(1, 5_000), holder(2, 5_000)], &HashSet::new());
        let verdict = gate_single_holder(&report, 20.0);
        assert_eq!(verdict, ConcentrationVerdict::SingleWhale);
        assert!(reason(verdict, &report).contains("50.00%"));
    }
}
