//! Holder-concentration helpers (GAP-MAP v2, P2) — the data layer behind the
//! module-sniper `risk_intel` holder gate.
//!
//! Two layers, deliberately separated:
//! * **Fetch** — [`fetch_largest_holders`] calls `getTokenLargestAccounts`
//!   through the kit's [`Rpc`] (so it inherits provider failover, timeouts
//!   and retry classification). RPC-dependent, gated in tests.
//! * **Analysis** — [`concentration`] and [`evaluate_concentration`] are pure
//!   integer functions over a [`HolderSnapshot`] list; they hold every rule
//!   the gate cares about and are fully unit-tested offline.
//!
//! Concentration is measured in BASIS POINTS of the circulating supply
//! (supply − excluded accounts, e.g. the bonding-curve/AMM pool and known
//! burn addresses). A launch where one wallet holds 40% of the float is a
//! rug vector regardless of what the price chart says — that is the whole
//! point of this module.

use std::collections::HashSet;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

use crate::rpc::Rpc;

/// Basis-point denominator.
pub const BPS: u64 = 10_000;

/// One holder position parsed from the RPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderSnapshot {
    /// Token account address (NOT the owner wallet).
    pub address: Pubkey,
    /// Raw token units held (already decimal-scaled by the mint).
    pub amount: u64,
}

/// Fetch the largest token accounts of `mint`.
///
/// Returns the RPC's truncated view (top ~20 accounts). Empty result means
/// the mint has no holders at all — callers decide whether that is a reject
/// (it usually is for a snipe: nothing to buy against).
pub async fn fetch_largest_holders(rpc: &Rpc, mint: &Pubkey) -> BotResult<Vec<HolderSnapshot>> {
    let accounts = rpc.get_token_largest_accounts(mint).await?;
    let mut out = Vec::with_capacity(accounts.len());
    for entry in accounts {
        let address = Pubkey::from_str(&entry.address)
            .map_err(|e| BotError::encoding(format!("holders: bad address {}: {e}", entry.address)))?;
        let raw = entry.amount.amount;
        let amount: u64 = raw
            .parse()
            .map_err(|e| BotError::encoding(format!("holders: bad amount '{raw}' : {e}")))?;
        out.push(HolderSnapshot { address, amount });
    }
    Ok(out)
}

/// Gate thresholds, in basis points of the circulating (non-excluded) supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcentrationLimits {
    /// Reject when the single largest holder exceeds this share.
    pub max_top1_bps: u64,
    /// Reject when the top-5 combined share exceeds this.
    pub max_top5_bps: u64,
    /// Reject when the top-10 combined share exceeds this.
    pub max_top10_bps: u64,
}

impl Default for ConcentrationLimits {
    /// Conservative defaults: no wallet > 20%, top-5 < 50%, top-10 < 66%.
    fn default() -> Self {
        ConcentrationLimits {
            max_top1_bps: 2_000,
            max_top5_bps: 5_000,
            max_top10_bps: 6_600,
        }
    }
}

/// Measured concentration, in bps of circulating supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcentrationReport {
    /// Supply minus excluded accounts (the tradable float).
    pub float_amount: u64,
    /// Number of accounts removed before measuring (pool/burn/LP).
    pub excluded_count: u32,
    /// Largest single holder share, bps of float (0 when float == 0).
    pub top1_bps: u64,
    /// Top-5 combined share, bps.
    pub top5_bps: u64,
    /// Top-10 combined share, bps.
    pub top10_bps: u64,
    /// Accounts considered (post-exclusion).
    pub holders_counted: u32,
}

/// Share of `float` held by `amount`, in bps, saturating and overflow-safe.
/// A zero float reports 0 (nothing measurable — the verdict layer decides
/// whether "no float" is a reject; the math never panics or divides by 0).
fn share_bps(amount: u64, float: u64) -> u64 {
    if float == 0 {
        return 0;
    }
    // amount ≤ total supply ≤ u64::MAX; widening to u128 makes the multiply
    // safe for every input, and the result is ≤ 10_000 by construction.
    ((amount as u128) * (BPS as u128) / (float as u128)).min(BPS as u128) as u64
}

/// Measure concentration over `holders`, excluding `excluded` accounts
/// (pool vaults, burn addresses, the curve itself).
///
/// Deterministic: holders are re-sorted by amount DESC, ties broken by
/// address, so the same input set always yields the same report regardless
/// of the RPC's ordering.
pub fn concentration(holders: &[HolderSnapshot], excluded: &HashSet<Pubkey>) -> ConcentrationReport {
    let mut considered: Vec<&HolderSnapshot> = holders
        .iter()
        .filter(|h| !excluded.contains(&h.address))
        .collect();
    considered.sort_by(|a, b| b.amount.cmp(&a.amount).then(a.address.cmp(&b.address)));

    let float_amount: u64 = considered
        .iter()
        .fold(0u64, |acc, h| acc.saturating_add(h.amount));

    let top = |n: usize| -> u64 {
        let sum: u64 = considered
            .iter()
            .take(n)
            .fold(0u64, |acc, h| acc.saturating_add(h.amount));
        share_bps(sum, float_amount)
    };

    ConcentrationReport {
        float_amount,
        excluded_count: (holders.len() - considered.len()) as u32,
        top1_bps: top(1),
        top5_bps: top(5),
        top10_bps: top(10),
        holders_counted: considered.len() as u32,
    }
}

/// Why a holder-structure check rejected a launch. Low-cardinality on
/// purpose: these labels end up in gate telemetry and the replay fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcentrationVerdict {
    /// Holder structure is inside every limit.
    Pass,
    /// A single wallet holds more than `max_top1_bps`.
    SingleWhale,
    /// The top-5 wallets hold more than `max_top5_bps`.
    TopFiveCluster,
    /// The top-10 wallets hold more than `max_top10_bps`.
    TopTenCluster,
    /// No tradable float remained after exclusions — nothing to buy.
    EmptyFloat,
}

/// Evaluate a report against limits.
///
/// Order matters for the label: an empty float is reported first (it is a
/// data-condition, not a behaviour), then the most specific violation.
pub fn evaluate_concentration(report: &ConcentrationReport, limits: &ConcentrationLimits) -> ConcentrationVerdict {
    if report.float_amount == 0 {
        return ConcentrationVerdict::EmptyFloat;
    }
    if report.top1_bps > limits.max_top1_bps {
        return ConcentrationVerdict::SingleWhale;
    }
    if report.top5_bps > limits.max_top5_bps {
        return ConcentrationVerdict::TopFiveCluster;
    }
    if report.top10_bps > limits.max_top10_bps {
        return ConcentrationVerdict::TopTenCluster;
    }
    ConcentrationVerdict::Pass
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(seed: u8, amount: u64) -> HolderSnapshot {
        HolderSnapshot {
            address: Pubkey::new_from_array([seed; 32]),
            amount,
        }
    }

    #[test]
    fn share_bps_is_exact_and_safe() {
        assert_eq!(share_bps(1_000, 10_000), 1_000); // 10%
        assert_eq!(share_bps(10_000, 10_000), BPS); // 100%
        assert_eq!(share_bps(0, 10_000), 0);
        assert_eq!(share_bps(5, 0), 0, "zero float measures 0, never panics");
        // u64 extremes: widening math must not wrap.
        assert!(share_bps(u64::MAX, u64::MAX) <= BPS);
    }

    #[test]
    fn concentration_excludes_listed_accounts_and_sorts() {
        let holders = vec![
            holder(1, 500),
            holder(2, 300),
            holder(3, 200),
            holder(9, 5_000), // the pool vault — excluded
        ];
        let excluded: HashSet<Pubkey> = [Pubkey::new_from_array([9; 32])].into_iter().collect();
        let report = concentration(&holders, &excluded);
        assert_eq!(report.excluded_count, 1);
        assert_eq!(report.float_amount, 1_000);
        assert_eq!(report.holders_counted, 3);
        assert_eq!(report.top1_bps, share_bps(500, 1_000)); // 50%
        assert_eq!(report.top5_bps, BPS, "all float is top-5 here");
    }

    #[test]
    fn concentration_is_independent_of_input_order() {
        let a = vec![holder(1, 100), holder(2, 900)];
        let b = vec![holder(2, 900), holder(1, 100)];
        assert_eq!(concentration(&a, &HashSet::new()), concentration(&b, &HashSet::new()));
    }

    #[test]
    fn verdicts_fire_in_specificity_order() {
        let limits = ConcentrationLimits {
            max_top1_bps: 2_000,
            max_top5_bps: 5_000,
            max_top10_bps: 6_600,
        };
        // One whale at 50%.
        let whale = concentration(&[holder(1, 500), holder(2, 500)], &HashSet::new());
        assert_eq!(evaluate_concentration(&whale, &limits), ConcentrationVerdict::SingleWhale);
        // Five mid-size wallets at 15% each = 75% top5.
        let cluster: Vec<HolderSnapshot> = (1..=6).map(|i| holder(i, 150)).collect();
        let clustered = concentration(&cluster, &HashSet::new());
        assert_eq!(evaluate_concentration(&clustered, &limits), ConcentrationVerdict::TopFiveCluster);
        // Healthy spread.
        let healthy: Vec<HolderSnapshot> = (1..=20).map(|i| holder(i, 100)).collect();
        let spread = concentration(&healthy, &HashSet::new());
        assert_eq!(evaluate_concentration(&spread, &limits), ConcentrationVerdict::Pass);
    }

    #[test]
    fn empty_float_is_its_own_verdict() {
        let report = concentration(&[], &HashSet::new());
        assert_eq!(report.float_amount, 0);
        let limits = ConcentrationLimits::default();
        assert_eq!(evaluate_concentration(&report, &limits), ConcentrationVerdict::EmptyFloat);
    }

    #[test]
    fn excluded_everything_is_empty_float_not_zero_divide() {
        let holders = vec![holder(9, 1_000)];
        let excluded: HashSet<Pubkey> = [Pubkey::new_from_array([9; 32])].into_iter().collect();
        let report = concentration(&holders, &excluded);
        assert_eq!(report.float_amount, 0);
        assert_eq!(report.top1_bps, 0);
        assert_eq!(
            evaluate_concentration(&report, &ConcentrationLimits::default()),
            ConcentrationVerdict::EmptyFloat
        );
    }

    #[test]
    fn boundary_at_exact_limit_passes() {
        let limits = ConcentrationLimits {
            max_top1_bps: 5_000,
            max_top5_bps: BPS,
            max_top10_bps: BPS,
        };
        let exactly_half = concentration(&[holder(1, 500), holder(2, 500)], &HashSet::new());
        assert_eq!(exactly_half.top1_bps, 5_000);
        // The gate rejects only ABOVE the limit: == passes.
        assert_eq!(evaluate_concentration(&exactly_half, &limits), ConcentrationVerdict::Pass);
    }
}
