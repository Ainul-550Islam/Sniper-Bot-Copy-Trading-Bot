//! Bundler / coordinated-buyer detection (GAP-MAP v2, P2).
//!
//! A launch sniped by a bundle of wallets funded from ONE source is the
//! classic supply-capture play: the bundler holds a large hidden stake from
//! slot one and dumps it on real buyers. The gate `max_bundler_ratio`
//! (fraction of first-slot buyers that look coordinated) already exists from
//! the P1 work; this module supplies the measurement.
//!
//! Model (deliberately conservative, all pure integer math):
//! * Each first-slot buyer is attributed to the wallet that FUNDED it
//!   (the fee payer / SOL source). The buyer's own address is not trusted —
//!   fresh throwaway wallets are the whole point of bundling.
//! * A funder that backs `>= 2` distinct buyers is a coordination cluster.
//! * `bundler_ratio` = (buyers inside such clusters) / (all first-slot
//!   buyers), in basis points.
//!
//! What this does NOT claim: it cannot see funder-of-funder chains, exchange
//! hot wallets, or MEV searchers that happen to share a fee payer. It is a
//! first-order clustering signal, and the threshold that turns it into a
//! reject is the operator's (`max_bundler_ratio`), documented as such.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// Basis-point denominator.
pub const BPS: u64 = 10_000;

/// One first-slot buy, attributed to its funding wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstSlotBuy {
    /// The buying token account / wallet (identity of the buy).
    pub buyer: Pubkey,
    /// The wallet that paid for it (the coordination key).
    pub funder: Pubkey,
}

/// Result of clustering a launch's first-slot buys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundlerReport {
    /// Total first-slot buyers observed.
    pub total_buyers: u32,
    /// Buyers whose funder also funded at least one other buyer.
    pub clustered_buyers: u32,
    /// Distinct funders that backed >= 2 buyers.
    pub clusters: u32,
    /// clustered_buyers / total_buyers in bps (0 when there are no buyers).
    pub bundler_ratio_bps: u64,
}

impl BundlerReport {
    /// True when no coordination was detected (or there were no buyers).
    pub fn clean(&self) -> bool {
        self.clustered_buyers == 0
    }
}

/// Cluster the first-slot buys and compute the bundler ratio.
///
/// Deterministic: depends only on the (buyer, funder) set, not on order.
/// A buyer that appears twice is counted once (same buy, repeated event).
pub fn analyze(buys: &[FirstSlotBuy]) -> BundlerReport {
    // De-duplicate by buyer: the same buy observed through two feeds must
    // not inflate either the numerator or the denominator.
    let mut seen_buyers: HashMap<Pubkey, Pubkey> = HashMap::new();
    for buy in buys {
        seen_buyers.entry(buy.buyer).or_insert(buy.funder);
    }

    // Funder -> number of DISTINCT buyers it funded.
    let mut funded_by: HashMap<Pubkey, u32> = HashMap::new();
    for funder in seen_buyers.values() {
        *funded_by.entry(*funder).or_insert(0) += 1;
    }

    let total_buyers = seen_buyers.len() as u32;
    if total_buyers == 0 {
        return BundlerReport {
            total_buyers: 0,
            clustered_buyers: 0,
            clusters: 0,
            bundler_ratio_bps: 0,
        };
    }

    let mut clustered_buyers = 0u32;
    let mut clusters = 0u32;
    for funder in seen_buyers.values() {
        let count = funded_by.get(funder).copied().unwrap_or(0);
        if count >= 2 {
            clustered_buyers += 1;
        }
    }
    for count in funded_by.values() {
        if *count >= 2 {
            clusters += 1;
        }
    }

    let bundler_ratio_bps = ((clustered_buyers as u64) * BPS) / (total_buyers as u64);

    BundlerReport {
        total_buyers,
        clustered_buyers,
        clusters,
        bundler_ratio_bps,
    }
}

/// Apply the operator's `max_bundler_ratio` (a fraction in [0, 1], e.g.
/// `0.5`). `<= 0` disables the gate. Returns true when the launch should be
/// REJECTED.
pub fn exceeds(report: &BundlerReport, max_ratio: f64) -> bool {
    if !max_ratio.is_finite() || max_ratio <= 0.0 {
        return false;
    }
    let limit_bps = (max_ratio.clamp(0.0, 1.0) * BPS as f64).round() as u64;
    report.bundler_ratio_bps > limit_bps
}

/// Readable reason for telemetry / gate summaries.
pub fn reason(report: &BundlerReport, rejected: bool) -> String {
    if report.total_buyers == 0 {
        return "no first-slot buys observed".to_string();
    }
    let pct = (report.bundler_ratio_bps as f64) / 100.0;
    if rejected {
        format!(
            "{}/{} first-slot buyers in {} same-funder clusters ({pct:.1}%)",
            report.clustered_buyers, report.total_buyers, report.clusters
        )
    } else {
        format!(
            "bundler clustering ok ({pct:.1}% of {} first-slot buyers)",
            report.total_buyers
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buy(buyer_seed: u8, funder_seed: u8) -> FirstSlotBuy {
        FirstSlotBuy {
            buyer: Pubkey::new_from_array([buyer_seed; 32]),
            funder: Pubkey::new_from_array([funder_seed; 32]),
        }
    }

    #[test]
    fn no_buyers_is_a_clean_empty_report() {
        let r = analyze(&[]);
        assert_eq!(r.total_buyers, 0);
        assert!(r.clean());
        assert_eq!(r.bundler_ratio_bps, 0);
        assert!(!exceeds(&r, 0.5));
    }

    #[test]
    fn independent_funders_are_clean() {
        let r = analyze(&[buy(1, 11), buy(2, 12), buy(3, 13), buy(4, 14)]);
        assert_eq!(r.total_buyers, 4);
        assert!(r.clean());
        assert_eq!(r.bundler_ratio_bps, 0);
    }

    #[test]
    fn one_funder_backing_everyone_is_fully_clustered() {
        let r = analyze(&[buy(1, 99), buy(2, 99), buy(3, 99), buy(4, 99)]);
        assert_eq!(r.clustered_buyers, 4);
        assert_eq!(r.clusters, 1);
        assert_eq!(r.bundler_ratio_bps, BPS);
        assert!(exceeds(&r, 0.5));
    }

    #[test]
    fn ratio_counts_only_clustered_buyers() {
        // 2 of 4 buyers share a funder; the other 2 are independent.
        let r = analyze(&[buy(1, 50), buy(2, 50), buy(3, 60), buy(4, 70)]);
        assert_eq!(r.total_buyers, 4);
        assert_eq!(r.clustered_buyers, 2);
        assert_eq!(r.bundler_ratio_bps, 5_000);
        assert!(exceeds(&r, 0.4), "50% > 40% limit");
        assert!(!exceeds(&r, 0.5), "50% is not above a 50% limit");
    }

    #[test]
    fn duplicate_buy_events_are_not_double_counted() {
        let r = analyze(&[buy(1, 50), buy(1, 50), buy(2, 50)]);
        assert_eq!(r.total_buyers, 2, "buyer 1 counted once");
        assert_eq!(r.clustered_buyers, 2);
        assert_eq!(r.bundler_ratio_bps, BPS);
    }

    #[test]
    fn disabled_threshold_never_rejects() {
        let r = analyze(&[buy(1, 99), buy(2, 99)]);
        assert!(!exceeds(&r, 0.0));
        assert!(!exceeds(&r, -1.0));
        assert!(!exceeds(&r, f64::NAN));
    }

    #[test]
    fn order_does_not_change_the_report() {
        let a = analyze(&[buy(1, 50), buy(2, 50), buy(3, 60)]);
        let b = analyze(&[buy(3, 60), buy(2, 50), buy(1, 50)]);
        assert_eq!(a, b);
    }
}
