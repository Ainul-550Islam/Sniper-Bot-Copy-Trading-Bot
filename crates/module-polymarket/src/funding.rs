//! Live-money separation — collateral reads and pre-broadcast funding checks.
//!
//! * PAPER / SIMULATE size against the seeded demo balance (or a real read
//!   when one is possible, else [`PAPER_USDC_BALANCE`]); nothing leaves the
//!   process.
//! * LIVE sizes against a verified on-chain ERC-20 read
//!   ([`crate::collateral`], freshness-bounded by
//!   [`COLLATERAL_CACHE_TTL_SECS`]) and, before any order is broadcast,
//!   [`PolyBot::ensure_live_funding`] requires the funder's balance AND — for
//!   EOA signing — the settling exchange's allowance to cover the
//!   risk-approved notional plus the collateral already committed by resting
//!   orders. Every failure is a typed rejection
//!   (`BalanceUnavailable` / `InsufficientFunding`); there is no fallback.
//!
//! [`resolve_sizing_balance`] is the single pure place where a demo figure
//! may be chosen, so the invariant is unit-tested without state or network.

use chrono::Utc;

use bot_core::config::PolymarketConfig;
use bot_core::models::ExecutionMode;

use crate::collateral;
use crate::error::{PolyError, PolyResult};
use crate::strategy::OrderDecision;
use crate::PolyBot;

/// Demo USDC balance used for PAPER/SIMULATE sizing only. LIVE mode never
/// uses this figure: a live entry whose real collateral balance cannot be
/// verified is rejected with [`PolyError::BalanceUnavailable`].
const PAPER_USDC_BALANCE: f64 = 1_000.0;
/// Freshness bound for reusing one verified collateral snapshot across the
/// decisions of a single scan. Every live sizing/funding decision therefore
/// runs against an on-chain read at most this old.
const COLLATERAL_CACHE_TTL_SECS: i64 = 15;

/// One verified on-chain collateral read (never a cache seed, never paper).
#[derive(Clone, Debug)]
pub struct CollateralSnapshot {
    /// Balance in raw token units (as returned by `balanceOf`).
    pub raw: u128,
    /// On-chain `decimals()` of the collateral token, validated 1..=18.
    pub decimals: u8,
    /// `raw` converted to whole-token (USD) units via `decimals`.
    pub usd: f64,
    /// When the read happened — the freshness bound.
    pub ts: chrono::DateTime<Utc>,
}

impl PolyBot {
    /// Collateral available for sizing, with STRICT paper/live separation.
    ///
    /// * LIVE: a verified on-chain read is REQUIRED. The cached dashboard
    ///   balance is ignored (it may be a paper seed left over from a
    ///   paper→live mode switch) and the paper figure is never returned. Any
    ///   failure yields [`PolyError::BalanceUnavailable`], which rejects the
    ///   entry upstream — there is no fallback path.
    /// * PAPER / SIMULATE (demo paths that never broadcast): the seeded demo
    ///   balance first, a real read when one is possible, else the paper
    ///   figure, so demo mode keeps working with no funds and no RPC.
    pub(crate) async fn available_collateral(&self, mode: ExecutionMode) -> PolyResult<f64> {
        let cached = self.state.balances().await.usdc_polygon;
        if mode != ExecutionMode::Live && cached > 0.0 {
            return Ok(cached);
        }
        let live = if self.collateral.is_some() {
            Some(self.read_collateral().await.map(|s| s.usd))
        } else {
            None
        };
        resolve_sizing_balance(mode, cached, live)
    }

    /// Verified on-chain collateral snapshot for the funder wallet, reused
    /// only within [`COLLATERAL_CACHE_TTL_SECS`]. Every failure mode is
    /// surfaced as [`PolyError::BalanceUnavailable`] — a caller in live mode
    /// MUST reject rather than substitute any other figure. Successful reads
    /// mirror the REAL balance into shared state (dashboard/telemetry).
    async fn read_collateral(&self) -> PolyResult<CollateralSnapshot> {
        let client = self.collateral.as_ref().ok_or_else(|| {
            PolyError::balance_unavailable(
                "collateral reader not configured ([polymarket].ctf_rpc_url / collateral_address)",
            )
        })?;
        if let Some(snap) = self.collateral_cache.read().await.clone() {
            if Utc::now().signed_duration_since(snap.ts).num_seconds() < COLLATERAL_CACHE_TTL_SECS {
                return Ok(snap);
            }
        }
        let cfg = self.state.config_snapshot().await;
        let owner = cfg
            .polymarket
            .funder_address
            .clone()
            .or_else(|| self.address.clone())
            .ok_or_else(|| {
                PolyError::balance_unavailable("no funder/EOA address for collateral read")
            })?;
        let raw = client.balance_of(&owner).await.map_err(|e| {
            PolyError::balance_unavailable(format!("collateral balanceOf failed: {e}"))
        })?;
        let decimals = client.decimals().await.map_err(|e| {
            PolyError::balance_unavailable(format!("collateral decimals read failed: {e}"))
        })?;
        // Identity/plausibility: the read targeted exactly the configured
        // `collateral_address` (validated 0x+40 hex at construction), and a
        // Polymarket collateral token is a stablecoin with sane decimals —
        // anything outside 1..=18 means the config points at the wrong
        // contract and MUST NOT be used to scale a balance.
        if !(1..=18).contains(&decimals) {
            return Err(PolyError::balance_unavailable(format!(
                "collateral decimals {decimals} implausible for a stablecoin (expected 1..=18)"
            )));
        }
        let usd = collateral::raw_to_usd(raw, decimals);
        let snap = CollateralSnapshot {
            raw,
            decimals,
            usd,
            ts: Utc::now(),
        };
        *self.collateral_cache.write().await = Some(snap.clone());
        self.state.set_balances(None, Some(usd)).await;
        Ok(snap)
    }

    /// Pre-broadcast funding verification for LIVE orders:
    ///
    /// * the funder's on-chain collateral balance (fresh read, same TTL
    ///   bound as sizing) must cover the risk-approved notional PLUS the
    ///   collateral already committed by our resting buy orders;
    /// * for EOA signing (`signature_type == 0`) the exchange contract that
    ///   will settle this order (`neg_risk` selects between the two) must
    ///   also hold an ERC-20 allowance of at least that amount — without it
    ///   the CLOB cannot pull the funds and the order would fail on-chain or
    ///   rest unfundable. Proxy-wallet flows (`signature_type` 1/2/3) move
    ///   funds through the funder proxy itself, so only the balance check
    ///   applies there.
    ///
    /// Read failures reject with [`PolyError::BalanceUnavailable`];
    /// insufficient funds/allowance reject with
    /// [`PolyError::InsufficientFunding`]. No fallback exists.
    pub(crate) async fn ensure_live_funding(
        &self,
        decision: &OrderDecision,
        approved_stake: f64,
        reserved_usd: f64,
        poly: &PolymarketConfig,
    ) -> PolyResult<()> {
        let client = self.collateral.as_ref().ok_or_else(|| {
            PolyError::balance_unavailable(
                "collateral reader not configured ([polymarket].ctf_rpc_url / collateral_address)",
            )
        })?;
        let owner = poly
            .funder_address
            .clone()
            .or_else(|| self.address.clone())
            .ok_or_else(|| {
                PolyError::balance_unavailable("no funder/EOA address for funding check")
            })?;

        let balance = self.read_collateral().await?;
        let required = collateral::usd_to_raw(approved_stake, balance.decimals)?;
        let reserved = collateral::usd_to_raw(reserved_usd.max(0.0), balance.decimals)?;
        let allowance = if poly.signature_type == 0 {
            let spender = if decision.neg_risk {
                &poly.neg_risk_exchange_address
            } else {
                &poly.exchange_address
            };
            Some(client.allowance(&owner, spender).await.map_err(|e| {
                PolyError::balance_unavailable(format!("collateral allowance read failed: {e}"))
            })?)
        } else {
            None
        };
        collateral::verify_funding(balance.raw, allowance, reserved, required)
    }
}

/// Pure paper/live separation rule for the sizing balance — the single place
/// where a demo figure may be chosen, so the invariant is unit-testable
/// without state or network:
///
/// * LIVE: only a verified live read counts. The cached state balance is
///   IGNORED (it may be a paper seed left over from a mode switch), a failed
///   or missing read is `BalanceUnavailable`, and an implausible read
///   (negative / non-finite) is rejected too.
/// * PAPER / SIMULATE: cached demo balance first, then a live read when one
///   was possible, else [`PAPER_USDC_BALANCE`].
pub(crate) fn resolve_sizing_balance(
    mode: ExecutionMode,
    cached_state: f64,
    live: Option<PolyResult<f64>>,
) -> PolyResult<f64> {
    if mode == ExecutionMode::Live {
        let v = match live {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                return Err(match e {
                    PolyError::BalanceUnavailable(_) | PolyError::InsufficientFunding(_) => e,
                    other => PolyError::balance_unavailable(other.to_string()),
                });
            }
            None => {
                return Err(PolyError::balance_unavailable(
                    "live mode requires a verified on-chain collateral read; reader not configured",
                ));
            }
        };
        if !v.is_finite() || v < 0.0 {
            return Err(PolyError::balance_unavailable(format!(
                "live collateral read implausible: {v}"
            )));
        }
        return Ok(v);
    }
    if cached_state > 0.0 {
        return Ok(cached_state);
    }
    if let Some(Ok(v)) = live {
        if v.is_finite() && v >= 0.0 {
            return Ok(v);
        }
    }
    Ok(PAPER_USDC_BALANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // resolve_sizing_balance: the paper/live separation invariant.
    // ------------------------------------------------------------------

    #[test]
    fn live_mode_uses_only_the_verified_read() {
        // A real read of 42.5 wins regardless of any cached value.
        let got = resolve_sizing_balance(ExecutionMode::Live, 999.0, Some(Ok(42.5))).unwrap();
        assert!((got - 42.5).abs() < f64::EPSILON);
        // Zero is a legitimate verified balance (risk gate will reject the
        // order) — it is NOT replaced by the paper figure.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Live, 1000.0, Some(Ok(0.0))).unwrap(),
            0.0
        );
    }

    #[test]
    fn live_mode_ignores_a_poisoned_cache_seed() {
        // Regression for the paper→live mode-switch defect: a 1000 USDC demo
        // seed left in shared state must never size a live order when the
        // real read says the wallet holds 3 USDC.
        let got = resolve_sizing_balance(ExecutionMode::Live, 1000.0, Some(Ok(3.0))).unwrap();
        assert!((got - 3.0).abs() < f64::EPSILON);
        // And with no live read at all, the seed must NOT leak through.
        let err = resolve_sizing_balance(ExecutionMode::Live, 1000.0, None)
            .expect_err("live without a reader must reject");
        assert!(matches!(err, PolyError::BalanceUnavailable(_)));
    }

    #[test]
    fn live_mode_rejects_failed_and_missing_reads() {
        // Read failed -> typed error, never the paper figure.
        let err = resolve_sizing_balance(
            ExecutionMode::Live,
            0.0,
            Some(Err(PolyError::http("rpc down"))),
        )
        .expect_err("failed read must reject");
        assert!(matches!(err, PolyError::BalanceUnavailable(_)));
        assert!(err.to_string().contains("rpc down"));
        // Already-typed balance errors pass through unchanged.
        let err = resolve_sizing_balance(
            ExecutionMode::Live,
            0.0,
            Some(Err(PolyError::balance_unavailable("decimals implausible"))),
        )
        .expect_err("must reject");
        assert!(err.to_string().contains("decimals implausible"));
        // No reader configured -> reject.
        assert!(matches!(
            resolve_sizing_balance(ExecutionMode::Live, 0.0, None),
            Err(PolyError::BalanceUnavailable(_))
        ));
    }

    #[test]
    fn live_mode_rejects_implausible_reads() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                resolve_sizing_balance(ExecutionMode::Live, 0.0, Some(Ok(bad))),
                Err(PolyError::BalanceUnavailable(_))
            ));
        }
    }

    #[test]
    fn paper_and_simulate_prefer_cache_then_live_then_paper_figure() {
        // Seeded demo balance wins (paper mode works offline with no funds).
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 1000.0, None).unwrap(),
            1000.0
        );
        // No seed + successful read -> real value.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, Some(Ok(7.25))).unwrap(),
            7.25
        );
        // No seed + failed/absent read -> paper figure (demo keeps working).
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, Some(Err(PolyError::http("x"))))
                .unwrap(),
            PAPER_USDC_BALANCE
        );
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, None).unwrap(),
            PAPER_USDC_BALANCE
        );
        // Simulate is a demo path (never broadcasts) and behaves like paper.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Simulate, 55.0, None).unwrap(),
            55.0
        );
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Simulate, 0.0, None).unwrap(),
            PAPER_USDC_BALANCE
        );
        // An implausible live value in paper mode falls through to the demo
        // figure rather than propagating NaN into sizing.
        assert_eq!(
            resolve_sizing_balance(ExecutionMode::Paper, 0.0, Some(Ok(f64::NAN))).unwrap(),
            PAPER_USDC_BALANCE
        );
    }
}
