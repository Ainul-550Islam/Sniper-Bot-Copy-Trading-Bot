//! Per-trade platform fees (GAP-MAP v2 P1).
//!
//! Until this module existed the product billed ONLY subscriptions: there was
//! no per-trade revenue model. This module adds the missing half:
//!
//! 1. A per-plan fee policy ([`PlatformFeePolicy`]): basis points of the
//!    trade notional, with an optional cap and a minimum charge. All values
//!    are integers — basis points and micro-units of the quote currency —
//!    so fee math never touches floating point until the final ledger
//!    projection.
//! 2. Exact fee computation ([`compute_platform_fee`]) with checked
//!    arithmetic: overflow is impossible (it saturates at the documented
//!    bound instead of wrapping), and rounding is HALF-UP on the micro unit.
//! 3. An accounting hook ([`PlatformFeeCharge::entry`]) that expands one
//!    charge into a balanced [`crate::accounting::posting::Entry`] using the
//!    same [`crate::accounting::posting::Posting`] vocabulary as the rest of
//!    the ledger: the tenant book recognises the fee (Debit `Fees`) against
//!    its cash (Credit `Cash`), and a platform-side revenue line is carried
//!    in [`PlatformFeeCharge::platform_revenue_posting`] for the operator's
//!    own book (`platform_fee_ledger`, migration 0049).
//!
//! Nothing here sends money anywhere; execution venues already charge their
//! own venue fees. This is the operator's service fee on top, quoted and
//! journaled separately so a tenant's statement can always show venue fee
//! and platform fee as distinct lines.

use serde::{Deserialize, Serialize};

use crate::accounting::posting::{Account, Entry, EntrySide, Posting};
use crate::billing::plan::PlanCode;

/// Basis-point denominator.
pub const BPS_DENOMINATOR: i64 = 10_000;

/// Hard cap on any configured fee rate (5%). A higher rate would be
/// confiscatory at HFT volumes; configuration above this is rejected.
pub const MAX_PLATFORM_FEE_BPS: u16 = 500;

/// The default per-plan fee schedule (bps of notional). Kept in code so a
/// deployment without a fee table still has a deterministic, reviewable
/// default; the durable override lives in `platform_fee_ledger`'s
/// configuration row (migration 0048).
pub const DEFAULT_FEE_BPS: [(PlanCode, u16); 4] = [
    (PlanCode::Starter, 30),    // 0.30%
    (PlanCode::Pro, 20),        // 0.20%
    (PlanCode::Business, 10),   // 0.10%
    (PlanCode::Enterprise, 5),  // 0.05% (usually renegotiated)
];

/// Errors from fee policy handling. Low-cardinality on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformFeeError {
    /// `fee_bps` exceeds [`MAX_PLATFORM_FEE_BPS`].
    RateTooHigh,
    /// A minimum larger than the cap can never be satisfied.
    MinimumExceedsCap,
    /// The notional is negative — a fee is computed on traded value only.
    NegativeNotional,
}

impl std::fmt::Display for PlatformFeeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformFeeError::RateTooHigh => write!(
                f,
                "platform fee rate exceeds the {MAX_PLATFORM_FEE_BPS} bps cap"
            ),
            PlatformFeeError::MinimumExceedsCap => {
                write!(f, "platform fee minimum exceeds the fee cap")
            }
            PlatformFeeError::NegativeNotional => {
                write!(f, "platform fee notional must be >= 0")
            }
        }
    }
}

impl std::error::Error for PlatformFeeError {}

/// Per-plan trade-fee policy.
///
/// All monetary fields are MICRO-UNITS of the quote currency (1 unit =
/// 1_000_000 micros; e.g. for a USDC-quoted venue, 1 USDC = 1e6 micros),
/// so the whole fee pipeline stays in checked integer arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformFeePolicy {
    /// Basis points of the trade notional (100 = 1%).
    pub fee_bps: u16,
    /// Absolute cap per trade, in micro-units. `None` = uncapped.
    pub cap_micros: Option<i64>,
    /// Minimum charge per trade, in micro-units. `0` = no minimum.
    /// A zero notional always charges zero regardless of the minimum:
    /// minimums apply to trades, not to no-ops.
    pub minimum_micros: i64,
}

impl PlatformFeePolicy {
    /// Build a policy, validating the rate and the cap/minimum relation.
    pub fn new(
        fee_bps: u16,
        cap_micros: Option<i64>,
        minimum_micros: i64,
    ) -> Result<Self, PlatformFeeError> {
        if fee_bps > MAX_PLATFORM_FEE_BPS {
            return Err(PlatformFeeError::RateTooHigh);
        }
        if minimum_micros < 0 {
            return Err(PlatformFeeError::NegativeNotional);
        }
        if let Some(cap) = cap_micros {
            if cap < 0 {
                return Err(PlatformFeeError::NegativeNotional);
            }
            if minimum_micros > cap {
                return Err(PlatformFeeError::MinimumExceedsCap);
            }
        }
        Ok(PlatformFeePolicy {
            fee_bps,
            cap_micros,
            minimum_micros,
        })
    }

    /// The compiled-in default for a plan.
    pub fn default_for(plan: PlanCode) -> PlatformFeePolicy {
        let bps = DEFAULT_FEE_BPS
            .iter()
            .find(|(p, _)| *p == plan)
            .map(|(_, bps)| *bps)
            .unwrap_or(0);
        PlatformFeePolicy {
            fee_bps: bps,
            cap_micros: None,
            minimum_micros: 0,
        }
    }
}

/// Compute the platform fee for one trade, in micro-units of the quote
/// currency. Returns `0` for a zero notional or a zero-rate policy.
///
/// Rounding: HALF-UP on the micro unit. Overflow is impossible: every
/// intermediate uses checked math and saturates to the nearest bound that
/// still respects cap and minimum rather than wrapping.
pub fn compute_platform_fee(notional_micros: i64, policy: &PlatformFeePolicy) -> i64 {
    if notional_micros <= 0 || policy.fee_bps == 0 {
        return 0;
    }
    // notional * bps, half-up division by 10_000. Checked throughout; on
    // overflow the fee would exceed any sane cap, so saturate to i64::MAX
    // and let the cap (when present) bring it back. Without a cap the
    // saturated value is still an honest upper bound, never a wrap.
    let raw = match notional_micros
        .checked_mul(policy.fee_bps as i64)
        .and_then(|v| v.checked_add(BPS_DENOMINATOR / 2))
        .and_then(|v| v.checked_div(BPS_DENOMINATOR))
    {
        Some(value) => value,
        None => i64::MAX,
    };
    let mut fee = raw;
    if let Some(cap) = policy.cap_micros {
        fee = fee.min(cap);
    }
    // Minimums apply to real trades only (zero notional returned above).
    if fee < policy.minimum_micros {
        fee = policy.minimum_micros;
    }
    fee
}

/// Convert micro-units to ledger units (the `f64` the posting layer uses).
/// Exact for values up to 2^53 micros — vastly above any single-trade fee;
/// beyond that the value is clamped, never wrapped.
pub fn micros_to_units(micros: i64) -> f64 {
    const MAX_EXACT: i64 = 1i64 << 53;
    let clamped = micros.clamp(0, MAX_EXACT);
    clamped as f64 / 1_000_000.0
}

/// One assessed platform fee, ready for journaling and posting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformFeeCharge {
    /// Unique charge id (idempotency: the same id never posts twice).
    pub charge_id: String,
    /// Tenant organization the charge belongs to.
    pub organization_id: String,
    /// Venue the underlying trade executed on.
    pub venue: String,
    /// Quote asset the fee is denominated in (e.g. "USDC", "SOL").
    pub asset: String,
    /// Trade notional in micro-units.
    pub notional_micros: i64,
    /// Assessed fee in micro-units (from [`compute_platform_fee`]).
    pub fee_micros: i64,
    /// The policy that produced the fee (auditability).
    pub policy: PlatformFeePolicy,
    /// The tenant-side wallet/book the fee is charged against.
    pub tenant_wallet: String,
    /// The operator wallet that recognises the revenue.
    pub platform_wallet: String,
}

impl PlatformFeeCharge {
    /// Assess a charge for one trade under a policy.
    pub fn assess(
        charge_id: impl Into<String>,
        organization_id: impl Into<String>,
        venue: impl Into<String>,
        asset: impl Into<String>,
        notional_micros: i64,
        policy: PlatformFeePolicy,
        tenant_wallet: impl Into<String>,
        platform_wallet: impl Into<String>,
    ) -> Result<PlatformFeeCharge, PlatformFeeError> {
        if notional_micros < 0 {
            return Err(PlatformFeeError::NegativeNotional);
        }
        let fee_micros = compute_platform_fee(notional_micros, &policy);
        Ok(PlatformFeeCharge {
            charge_id: charge_id.into(),
            organization_id: organization_id.into(),
            venue: venue.into(),
            asset: asset.into(),
            notional_micros,
            fee_micros,
            policy,
            tenant_wallet: tenant_wallet.into(),
            platform_wallet: platform_wallet.into(),
        })
    }

    /// The balanced TENANT-side entry: Debit `Fees` (the tenant recognises
    /// the fee expense), Credit `Cash` (the fee leaves the tenant wallet).
    /// Zero-fee charges produce an EMPTY entry — they are journaled for the
    /// audit trail but must not create postings.
    pub fn entry(&self) -> Entry {
        let amount = micros_to_units(self.fee_micros);
        if amount <= 0.0 {
            return Entry {
                postings: Vec::new(),
            };
        }
        Entry {
            postings: vec![
                Posting {
                    event_id: self.charge_id.clone(),
                    seq: 0,
                    account: Account::Fees,
                    wallet: self.tenant_wallet.clone(),
                    asset: self.asset.clone(),
                    side: EntrySide::Debit,
                    amount,
                    quantity: 0.0,
                    base_asset: None,
                },
                Posting {
                    event_id: self.charge_id.clone(),
                    seq: 1,
                    account: Account::Cash,
                    wallet: self.tenant_wallet.clone(),
                    asset: self.asset.clone(),
                    side: EntrySide::Credit,
                    amount,
                    quantity: 0.0,
                    base_asset: None,
                },
            ],
        }
    }

    /// The PLATFORM-side revenue posting for the operator's own book
    /// (journaled to `platform_fee_ledger`): the fee arrives as platform
    /// cash. Kept separate from [`Self::entry`] because the tenant book and
    /// the operator book are different ledgers; neither may contain the
    /// other's lines.
    pub fn platform_revenue_posting(&self) -> Option<Posting> {
        let amount = micros_to_units(self.fee_micros);
        if amount <= 0.0 {
            return None;
        }
        Some(Posting {
            event_id: self.charge_id.clone(),
            seq: 0,
            account: Account::Cash,
            wallet: self.platform_wallet.clone(),
            asset: self.asset.clone(),
            side: EntrySide::Debit,
            amount,
            quantity: 0.0,
            base_asset: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(bps: u16, cap: Option<i64>, minimum: i64) -> PlatformFeePolicy {
        PlatformFeePolicy::new(bps, cap, minimum).expect("test policy must build")
    }

    #[test]
    fn basis_points_math_is_exact_and_half_up() {
        let p = policy(25, None, 0); // 0.25%
        // 1_000_000_000 micros (1,000 units) -> 2_500_000 micros (2.5 units)
        assert_eq!(compute_platform_fee(1_000_000_000, &p), 2_500_000);
        // Half-up rounding: 3 micros at 25bps = 0.0075 -> rounds to 0.
        assert_eq!(compute_platform_fee(3, &p), 0);
        // 20 micros at 500bps = exactly 1 micro.
        let p5 = policy(500, None, 0);
        assert_eq!(compute_platform_fee(20, &p5), 1);
        // 2 micros at 500bps = 0.1 micro -> rounds half-up to 0.
        assert_eq!(compute_platform_fee(2, &p5), 0);
        // 1 micro at 500bps = 0.05 -> rounds to 0.
        assert_eq!(compute_platform_fee(1, &p5), 0);
        // 3 micros at 500bps = 0.15 -> rounds to 0; 10 micros -> 0.5 -> 1 (half-up).
        assert_eq!(compute_platform_fee(3, &p5), 0);
        assert_eq!(compute_platform_fee(10, &p5), 1);
    }

    #[test]
    fn cap_and_minimum_apply_in_order() {
        // Cap applies to the computed fee, THEN the minimum lifts the result.
        let p = policy(100, Some(500), 0); // 1% capped at 500 micros
        assert_eq!(compute_platform_fee(10_000, &p), 100); // 1% = 100
        assert_eq!(compute_platform_fee(1_000_000, &p), 500); // 1% = 10_000 -> cap 500
        let m = policy(100, Some(5_000), 250); // min 250, cap 5000
        assert_eq!(compute_platform_fee(1_000, &m), 250); // 1% = 10 < min -> 250
        assert_eq!(compute_platform_fee(100_000, &m), 1_000); // 1% = 1000
        assert_eq!(compute_platform_fee(1_000_000, &m), 5_000); // 1% = 10_000 -> cap
    }

    #[test]
    fn zero_notional_and_zero_rate_charge_nothing_even_with_minimum() {
        let p = policy(100, None, 250);
        assert_eq!(compute_platform_fee(0, &p), 0, "no trade -> no fee");
        assert_eq!(compute_platform_fee(-5, &p), 0, "never charge on negative");
        let z = policy(0, None, 250);
        assert_eq!(compute_platform_fee(1_000_000, &z), 0, "zero rate -> no fee");
    }

    #[test]
    fn overflow_saturates_instead_of_wrapping() {
        let p = policy(MAX_PLATFORM_FEE_BPS, None, 0);
        // i64::MAX notional * 500 bps overflows checked_mul -> saturates.
        let fee = compute_platform_fee(i64::MAX, &p);
        assert!(fee > 0, "saturated fee is still a positive upper bound");
        assert!(fee <= i64::MAX);
        // With a cap, the saturated value collapses back to the cap.
        let capped = policy(MAX_PLATFORM_FEE_BPS, Some(1_000), 0);
        assert_eq!(compute_platform_fee(i64::MAX, &capped), 1_000);
    }

    #[test]
    fn policy_validation_rejects_bad_configs() {
        assert_eq!(
            PlatformFeePolicy::new(MAX_PLATFORM_FEE_BPS + 1, None, 0),
            Err(PlatformFeeError::RateTooHigh)
        );
        assert_eq!(
            PlatformFeePolicy::new(10, Some(100), 101),
            Err(PlatformFeeError::MinimumExceedsCap)
        );
        assert_eq!(
            PlatformFeePolicy::new(10, None, -1),
            Err(PlatformFeeError::NegativeNotional)
        );
        assert!(PlatformFeePolicy::new(MAX_PLATFORM_FEE_BPS, Some(0), 0).is_ok());
    }

    #[test]
    fn default_policies_cover_every_plan_and_are_sane() {
        for plan in [
            PlanCode::Starter,
            PlanCode::Pro,
            PlanCode::Business,
            PlanCode::Enterprise,
        ] {
            let p = PlatformFeePolicy::default_for(plan);
            assert!(p.fee_bps <= MAX_PLATFORM_FEE_BPS);
            assert_eq!(p.minimum_micros, 0);
            assert!(p.cap_micros.is_none());
        }
        // Higher tiers never pay a higher default rate than lower tiers.
        assert!(
            PlatformFeePolicy::default_for(PlanCode::Starter).fee_bps
                >= PlatformFeePolicy::default_for(PlanCode::Enterprise).fee_bps
        );
    }

    #[test]
    fn charge_entry_is_balanced_and_zero_fee_posts_nothing() {
        let charge = PlatformFeeCharge::assess(
            "fee-1",
            "org-1",
            "pumpswap",
            "SOL",
            1_000_000_000, // 1,000 SOL notional in micros
            policy(20, None, 0),
            "tenant:wallet-a",
            "platform:fees",
        )
        .expect("assess");
        assert_eq!(charge.fee_micros, 2_000_000); // 0.2%
        let entry = charge.entry();
        assert!(entry.is_balanced());
        assert_eq!(entry.postings.len(), 2);
        assert_eq!(entry.postings[0].account, Account::Fees);
        assert_eq!(entry.postings[0].side, EntrySide::Debit);
        assert_eq!(entry.postings[1].account, Account::Cash);
        assert_eq!(entry.postings[1].side, EntrySide::Credit);
        assert!((entry.postings[0].amount - 2.0).abs() < 1e-9);
        // Platform revenue line is separate and present.
        let revenue = charge.platform_revenue_posting().expect("revenue line");
        assert_eq!(revenue.wallet, "platform:fees");
        assert_eq!(revenue.side, EntrySide::Debit);

        // Zero notional: journaled, but NO postings on either book.
        let zero = PlatformFeeCharge::assess(
            "fee-0",
            "org-1",
            "pumpswap",
            "SOL",
            0,
            policy(20, None, 100),
            "tenant:wallet-a",
            "platform:fees",
        )
        .expect("assess zero");
        assert_eq!(zero.fee_micros, 0);
        assert!(zero.entry().postings.is_empty());
        assert!(zero.platform_revenue_posting().is_none());
    }

    #[test]
    fn micros_to_units_is_exact_below_the_fp_boundary() {
        assert!((micros_to_units(2_500_000) - 2.5).abs() < 1e-12);
        assert_eq!(micros_to_units(0), 0.0);
        assert_eq!(micros_to_units(-5), 0.0, "never negative");
        // Beyond 2^53 micros the value clamps instead of losing integrity.
        let huge = micros_to_units(i64::MAX);
        assert!(huge.is_finite() && huge > 0.0);
    }
}
