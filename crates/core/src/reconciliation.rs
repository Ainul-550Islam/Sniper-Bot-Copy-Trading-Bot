//! Reconciliation engine (Prompt 2 / §C,§J,§K): deterministic comparison of
//! EXPECTED LOCAL STATE against OBSERVED EXTERNAL STATE.
//!
//! # State boundaries (authoritative model)
//!
//! 1. **Local intent** — an OMS `Order` (`oms.rs`): what we *wanted* to do.
//!    Never evidence of money movement.
//! 2. **Execution attempt** — `ExecutionRecord` + `transactions` rows: what we
//!    *sent*, keyed by signature/external id.
//! 3. **External observation** — what the venue/chain *actually* shows
//!    (transaction status, token balances, CLOB order state). The only source
//!    of truth for money movement.
//! 4. **Reconciled state** — positions/orders after this engine's verdict has
//!    been applied.
//! 5. **Derived state** — PnL/risk exposure, always *recomputed* from
//!    reconciled fills ([`reconstruct_pnl`]), never carried over blindly.
//!
//! The engine is deliberately **pure**: comparison functions take snapshots
//! and return typed [`ReconOutcome`]s with no I/O and no side effects, so the
//! whole decision matrix is unit-testable and the venue adapters
//! (`server/src/recon.rs`) stay thin. Corrections applied by adapters must be
//! auditable (risk event + audit trail) and must never fabricate proceeds —
//! when authoritative fill data cannot explain an observation the outcome is
//! [`ReconOutcome::RecoveryRequired`] and operators get paged.
//!
//! # "No balance" vs "could not read balance"
//!
//! [`ExternalState::Observed`] with a zero quantity means the external world
//! *answered* and there is nothing there. [`ExternalState::Unavailable`] means
//! we could not read it — reconciliation MUST NOT conclude success from an
//! unavailable source; adapters translate it to a retry, never to a
//! correction.

use serde::{Deserialize, Serialize};

/// Result of an external-state read attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum ExternalState<T> {
    /// The external system answered definitively.
    Observed(T),
    /// The external system could not be read (RPC down, timeout, malformed
    /// response). This is NOT the same as observing zero/absent.
    Unavailable { reason: String },
}

impl<T> ExternalState<T> {
    pub fn is_available(&self) -> bool {
        matches!(self, ExternalState::Observed(_))
    }
}

/// Aggregated external token-balance observation for one owner/mint pair
/// (Solana adapter: the sum over ALL of the owner's token accounts for the
/// mint — ATAs plus any auxiliary accounts — each validated for owner/mint
/// identity so unrelated accounts can never be counted).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BalanceObservation {
    /// Total raw (integer) units across every validated account.
    pub raw_total: u64,
    /// Total in human units (raw / 10^decimals).
    pub ui_total: f64,
    /// Mint decimals reported by the chain.
    pub decimals: u8,
    /// How many token accounts contributed (0 = no account exists at all).
    pub accounts: usize,
}

/// Local position facts relevant to reconciliation (a projection of
/// `models::Position` — adapters build this; the engine never touches the
/// full model).
#[derive(Debug, Clone)]
pub struct PositionSnapshot {
    pub id: String,
    /// Open quantity in human units.
    pub qty: f64,
    /// Local book says the position is open.
    pub open: bool,
    /// True when at least one money-moving execution for this position is
    /// still unresolved (pending confirmation / Unknown order). While true,
    /// divergences are EXPECTED and must not trigger corrections.
    pub has_unresolved_execution: bool,
    /// Persisted, confirmed fill data for this position (authoritative
    /// execution history used for PnL reconstruction).
    pub fills: Vec<FillRecord>,
}

/// One confirmed fill (buy or sell) belonging to a position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillRecord {
    /// "buy" or "sell" (lowercase).
    pub side: String,
    /// Filled quantity, human units, > 0.
    pub qty: f64,
    /// Total quote proceeds/cost for this fill INCLUDING fees paid, i.e. the
    /// actual cash that moved (buy: quote spent; sell: quote received net of
    /// venue fee). Network/priority fees are accounted separately by the
    /// caller if desired.
    pub quote: f64,
}

/// Tolerance policy for quantity comparisons.
#[derive(Debug, Clone, Copy)]
pub struct QuantityTolerance {
    /// Relative tolerance (e.g. 0.01 = 1%) — covers fee dust and rounding.
    pub relative: f64,
    /// Absolute dust floor in human units: differences at or below this are
    /// always InSync (rent/dust artifacts on tiny positions).
    pub dust: f64,
}

impl Default for QuantityTolerance {
    fn default() -> Self {
        QuantityTolerance {
            relative: 0.01,
            dust: 1e-9,
        }
    }
}

/// Typed reconciliation verdicts (§C). Names follow the existing
/// `ReconVerdict`/`OrderStatus` conventions; every adapter maps these onto
/// queue mechanics (resolve / retry / park) and metrics.
#[derive(Debug, Clone, PartialEq)]
pub enum ReconOutcome {
    /// Local and external state agree (within tolerance).
    InSync,
    /// External shows MORE than local (e.g. a fill we never recorded).
    ExternalAhead { local: f64, external: f64 },
    /// Local shows MORE than external (e.g. tokens left without a recorded
    /// exit, or an exit confirmed externally but not locally applied).
    LocalAhead { local: f64, external: f64 },
    /// Quantities differ beyond tolerance (direction carried in the values).
    QuantityMismatch { local: f64, external: f64 },
    /// Native/quote balance differs from what local state expects.
    BalanceMismatch { expected: f64, observed: f64 },
    /// Local book has an open position; externally the holding is zero.
    MissingPosition,
    /// External holding exists with no local position.
    UnexpectedPosition { external: f64 },
    /// The execution attempt's external outcome is still undetermined.
    UnknownExecution,
    /// No external trace of a transaction we believe we sent (past the
    /// confirmation horizon).
    MissingTransaction,
    /// The same intent produced (or would produce) two executions.
    DuplicateExecution,
    /// Local state is older than the external observation and must be
    /// refreshed before any decision (e.g. mark/qty predate a confirmed fill).
    StaleLocalState,
    /// External state could not be read — NEVER a conclusion, always a retry.
    ExternalStateUnavailable { reason: String },
    /// Divergence that automated correction must not touch (authoritative
    /// data cannot explain the observation): park + page an operator.
    RecoveryRequired { reason: String },
}

impl ReconOutcome {
    /// Stable low-cardinality metric/log label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReconOutcome::InSync => "in_sync",
            ReconOutcome::ExternalAhead { .. } => "external_ahead",
            ReconOutcome::LocalAhead { .. } => "local_ahead",
            ReconOutcome::QuantityMismatch { .. } => "quantity_mismatch",
            ReconOutcome::BalanceMismatch { .. } => "balance_mismatch",
            ReconOutcome::MissingPosition => "missing_position",
            ReconOutcome::UnexpectedPosition { .. } => "unexpected_position",
            ReconOutcome::UnknownExecution => "unknown_execution",
            ReconOutcome::MissingTransaction => "missing_transaction",
            ReconOutcome::DuplicateExecution => "duplicate_execution",
            ReconOutcome::StaleLocalState => "stale_local_state",
            ReconOutcome::ExternalStateUnavailable { .. } => "external_unavailable",
            ReconOutcome::RecoveryRequired { .. } => "recovery_required",
        }
    }

    /// True when local and external demonstrably diverge (correction, flag or
    /// operator action required — as opposed to "fine", "unknown" or "can't
    /// tell yet").
    pub fn is_divergent(&self) -> bool {
        matches!(
            self,
            ReconOutcome::ExternalAhead { .. }
                | ReconOutcome::LocalAhead { .. }
                | ReconOutcome::QuantityMismatch { .. }
                | ReconOutcome::BalanceMismatch { .. }
                | ReconOutcome::MissingPosition
                | ReconOutcome::UnexpectedPosition { .. }
                | ReconOutcome::DuplicateExecution
                | ReconOutcome::RecoveryRequired { .. }
        )
    }

    /// True when the verdict must translate into "retry later" rather than
    /// any conclusion.
    pub fn is_inconclusive(&self) -> bool {
        matches!(
            self,
            ReconOutcome::UnknownExecution
                | ReconOutcome::ExternalStateUnavailable { .. }
                | ReconOutcome::StaleLocalState
        )
    }
}

/// Local view of one execution attempt at classification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecutionState {
    /// We have a signature but never observed confirmation (crash/timeout).
    SubmittedUnconfirmed,
    /// We recorded it as confirmed/filled.
    RecordedConfirmed,
    /// We recorded it as failed.
    RecordedFailed,
    /// We have no record at all (e.g. DB write lost) but hold a signature.
    NoLocalRecord,
}

/// External view of one transaction/order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalExecutionState {
    /// Landed successfully (Solana: confirmed/finalized without error;
    /// CLOB: matched/filled).
    Succeeded,
    /// Landed but failed (Solana: transaction error; CLOB: cancelled/expired
    /// maps here only when the venue says it can no longer fill).
    FailedOnExternal,
    /// Known to the venue but still pending (mempool/resting on book).
    Pending,
    /// The venue has no trace of it.
    NotFound,
}

/// Classify one execution attempt: local record × external truth → outcome.
///
/// Deterministic decision table (§E/§F case matrix):
///
/// | local \ external        | Succeeded        | Failed         | Pending        | NotFound           |
/// |-------------------------|------------------|----------------|----------------|--------------------|
/// | SubmittedUnconfirmed    | InSync→confirm   | InSync→fail    | UnknownExec    | MissingTransaction |
/// | RecordedConfirmed       | InSync           | RecoveryReq.   | StaleLocal     | RecoveryReq.       |
/// | RecordedFailed          | RecoveryReq.     | InSync         | RecoveryReq.   | InSync             |
/// | NoLocalRecord           | RecoveryRequired | UnknownExec    | RecoveryReq.   | InSync(nothing)    |
///
/// "divergence" entries are concrete outcomes below. Adapters apply the
/// resulting corrections (mark the order Filled/Failed, replay the missed
/// confirmation, or park for operators).
pub fn classify_execution(
    local: LocalExecutionState,
    external: ExternalState<ExternalExecutionState>,
) -> ReconOutcome {
    let observed = match external {
        ExternalState::Observed(o) => o,
        ExternalState::Unavailable { reason } => {
            return ReconOutcome::ExternalStateUnavailable { reason }
        }
    };
    use ExternalExecutionState as E;
    use LocalExecutionState as L;
    match (local, observed) {
        // We weren't sure; the chain answers definitively → the ambiguity is
        // RESOLVED (the adapter transitions the order to Filled/Failed and
        // replays any missed effects). Both answers are "in sync" outcomes:
        // external truth fully explains the attempt.
        (L::SubmittedUnconfirmed, E::Succeeded) => ReconOutcome::InSync,
        (L::SubmittedUnconfirmed, E::FailedOnExternal) => ReconOutcome::InSync,
        (L::SubmittedUnconfirmed, E::Pending) => ReconOutcome::UnknownExecution,
        (L::SubmittedUnconfirmed, E::NotFound) => ReconOutcome::MissingTransaction,

        (L::RecordedConfirmed, E::Succeeded) => ReconOutcome::InSync,
        // DB says success but the chain says failure (case 10 in §V): fill
        // effects (trades, PnL, position) were already applied locally, so
        // correction is not mechanical — park for operators with an alert.
        (L::RecordedConfirmed, E::FailedOnExternal) => ReconOutcome::RecoveryRequired {
            reason: "recorded confirmed but failed on chain".into(),
        },
        // We finalized locally but the venue still shows pending — our record
        // ran ahead of the observation source (weak commitment / lagging
        // fallback): refresh, don't conclude.
        (L::RecordedConfirmed, E::Pending) => ReconOutcome::StaleLocalState,
        (L::RecordedConfirmed, E::NotFound) => ReconOutcome::RecoveryRequired {
            reason: "recorded confirmed but externally unknown".into(),
        },

        (L::RecordedFailed, E::Succeeded) => ReconOutcome::RecoveryRequired {
            reason: "recorded failed but confirmed externally — money moved".into(),
        },
        (L::RecordedFailed, E::FailedOnExternal) => ReconOutcome::InSync,
        (L::RecordedFailed, E::Pending) => ReconOutcome::RecoveryRequired {
            reason: "recorded failed but still pending externally".into(),
        },
        (L::RecordedFailed, E::NotFound) => ReconOutcome::InSync,

        // Signature known but every local record was lost (crash between
        // broadcast and persistence, case 4/6 in §F).
        (L::NoLocalRecord, E::Succeeded) => ReconOutcome::RecoveryRequired {
            reason: "externally confirmed execution with no local record".into(),
        },
        (L::NoLocalRecord, E::FailedOnExternal) => ReconOutcome::UnknownExecution,
        (L::NoLocalRecord, E::Pending) => ReconOutcome::RecoveryRequired {
            reason: "pending external execution with no local record".into(),
        },
        (L::NoLocalRecord, E::NotFound) => ReconOutcome::InSync,
    }
}

/// Compare one local position against the aggregated external balance (§J).
///
/// Rules, in order:
/// 1. An unreadable external source is never a conclusion.
/// 2. While an execution touching this position is unresolved, divergences
///    are expected → inconclusive (retry after the execution resolves).
/// 3. No local open position + external balance > dust → UnexpectedPosition.
/// 4. Local open + external zero:
///    * confirmed exit fills explain the disappearance → MissingPosition
///      (the adapter closes the position and RECOMPUTES realized PnL from
///      `fills` — never an arbitrary overwrite);
///    * no exit fills → RecoveryRequired (tokens left without authoritative
///      execution data — operator territory).
/// 5. Within tolerance (relative or dust) → InSync.
/// 6. External > local → ExternalAhead; external < local → LocalAhead; the
///    adapter decides correction vs flag (flag by default; corrections only
///    from authoritative fill data).
pub fn compare_position(
    local: &PositionSnapshot,
    external: &ExternalState<BalanceObservation>,
    tolerance: QuantityTolerance,
) -> ReconOutcome {
    let observed = match external {
        ExternalState::Observed(o) => o,
        ExternalState::Unavailable { reason } => {
            return ReconOutcome::ExternalStateUnavailable {
                reason: reason.clone(),
            }
        }
    };

    if !local.open {
        // Closed locally: any non-dust external holding is unexpected (a sell
        // that failed to land, or an external deposit).
        return if observed.ui_total > tolerance.dust {
            ReconOutcome::UnexpectedPosition {
                external: observed.ui_total,
            }
        } else {
            ReconOutcome::InSync
        };
    }

    if local.has_unresolved_execution {
        // Money may be in flight in EITHER direction; every divergence here
        // is premature to judge.
        return ReconOutcome::UnknownExecution;
    }

    let diff = observed.ui_total - local.qty;
    let within = diff.abs() <= tolerance.dust.max(local.qty.abs() * tolerance.relative);
    if within {
        return ReconOutcome::InSync;
    }

    if observed.ui_total <= tolerance.dust {
        // External says zero while the book says open.
        let has_exit_fill = local
            .fills
            .iter()
            .any(|f| f.side.eq_ignore_ascii_case("sell") && f.qty > 0.0);
        return if has_exit_fill {
            ReconOutcome::MissingPosition
        } else {
            ReconOutcome::RecoveryRequired {
                reason: "position open locally, zero on-chain, no recorded exit fill".into(),
            }
        };
    }

    if diff > 0.0 {
        ReconOutcome::ExternalAhead {
            local: local.qty,
            external: observed.ui_total,
        }
    } else {
        ReconOutcome::LocalAhead {
            local: local.qty,
            external: observed.ui_total,
        }
    }
}

/// Reconstructed PnL state (§K) — recomputed from authoritative fills only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PnlReconstruction {
    /// Remaining open quantity after netting all fills.
    pub open_qty: f64,
    /// Volume-weighted average entry cost of the REMAINING position
    /// (quote per unit, fees included in the cashflows).
    pub avg_entry: f64,
    /// Total quote spent on buys (including fees).
    pub buy_cost: f64,
    /// Total quote received on sells (net of venue fees).
    pub sell_proceeds: f64,
    /// Realized PnL = proceeds of sold quantity valued at the running average
    /// entry, i.e. `sell_proceeds - avg_cost_of_sold_units`.
    pub realized: f64,
    /// Cost basis still tied up in the open quantity (`open_qty * avg_entry`).
    pub open_cost_basis: f64,
}

/// Recompute position economics from the ordered fill list (average-cost
/// method — the same convention the live position bookkeeping uses).
///
/// Deterministic and total: malformed entries (non-positive qty, unknown
/// side) are skipped, sells beyond the open quantity are clamped (over-sell
/// cannot invent negative cost basis), and no division by zero is possible.
/// Unconfirmed transactions must never appear in `fills` — the caller feeds
/// only confirmed/persisted execution data, so an unconfirmed send can never
/// leak into realized PnL.
pub fn reconstruct_pnl(fills: &[FillRecord]) -> PnlReconstruction {
    let mut open_qty = 0.0f64;
    let mut open_cost = 0.0f64; // quote tied up in open_qty
    let mut buy_cost = 0.0f64;
    let mut sell_proceeds = 0.0f64;
    let mut realized = 0.0f64;

    for f in fills {
        if !(f.qty.is_finite() && f.qty > 0.0 && f.quote.is_finite()) {
            continue;
        }
        if f.side.eq_ignore_ascii_case("buy") {
            open_qty += f.qty;
            open_cost += f.quote;
            buy_cost += f.quote;
        } else if f.side.eq_ignore_ascii_case("sell") {
            let qty = f.qty.min(open_qty); // clamp over-sell
            if qty <= 0.0 {
                continue;
            }
            let avg = if open_qty > 0.0 {
                open_cost / open_qty
            } else {
                0.0
            };
            let cost_of_sold = avg * qty;
            // Proceeds attributable to the clamped quantity.
            let proceeds = f.quote * (qty / f.qty);
            realized += proceeds - cost_of_sold;
            sell_proceeds += proceeds;
            open_cost -= cost_of_sold;
            open_qty -= qty;
        }
        // Unknown sides are skipped (defensive; adapters validate).
    }

    let avg_entry = if open_qty > 0.0 {
        open_cost / open_qty
    } else {
        0.0
    };
    PnlReconstruction {
        open_qty,
        avg_entry,
        buy_cost,
        sell_proceeds,
        realized,
        open_cost_basis: open_cost.max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ui: f64) -> ExternalState<BalanceObservation> {
        ExternalState::Observed(BalanceObservation {
            raw_total: (ui * 1e9) as u64,
            ui_total: ui,
            decimals: 9,
            accounts: if ui > 0.0 { 1 } else { 0 },
        })
    }

    fn snap(qty: f64, open: bool, unresolved: bool, fills: Vec<FillRecord>) -> PositionSnapshot {
        PositionSnapshot {
            id: "pos1".into(),
            qty,
            open,
            has_unresolved_execution: unresolved,
            fills,
        }
    }

    fn buy(qty: f64, quote: f64) -> FillRecord {
        FillRecord {
            side: "buy".into(),
            qty,
            quote,
        }
    }
    fn sell(qty: f64, quote: f64) -> FillRecord {
        FillRecord {
            side: "sell".into(),
            qty,
            quote,
        }
    }

    // ---- V1: local == external -------------------------------------------
    #[test]
    fn in_sync_when_balances_match() {
        let local = snap(100.0, true, false, vec![]);
        assert_eq!(
            compare_position(&local, &obs(100.0), QuantityTolerance::default()),
            ReconOutcome::InSync
        );
        // Within 1% relative tolerance → still InSync (fee dust).
        assert_eq!(
            compare_position(&local, &obs(99.5), QuantityTolerance::default()),
            ReconOutcome::InSync
        );
    }

    // ---- V2/V3: external lower / higher ----------------------------------
    #[test]
    fn external_lower_than_local_is_local_ahead() {
        let local = snap(100.0, true, false, vec![]);
        assert_eq!(
            compare_position(&local, &obs(50.0), QuantityTolerance::default()),
            ReconOutcome::LocalAhead {
                local: 100.0,
                external: 50.0
            }
        );
    }

    #[test]
    fn external_higher_than_local_is_external_ahead() {
        let local = snap(100.0, true, false, vec![]);
        assert_eq!(
            compare_position(&local, &obs(150.0), QuantityTolerance::default()),
            ReconOutcome::ExternalAhead {
                local: 100.0,
                external: 150.0
            }
        );
    }

    // ---- V4: local position missing externally ----------------------------
    #[test]
    fn missing_position_requires_exit_fill_evidence() {
        // With a recorded exit fill: adapter may close + recompute PnL.
        let local = snap(
            100.0,
            true,
            false,
            vec![buy(100.0, 10.0), sell(100.0, 12.0)],
        );
        assert_eq!(
            compare_position(&local, &obs(0.0), QuantityTolerance::default()),
            ReconOutcome::MissingPosition
        );
        // Without one: tokens vanished with no authoritative execution data —
        // never auto-correct.
        let local = snap(100.0, true, false, vec![buy(100.0, 10.0)]);
        assert!(matches!(
            compare_position(&local, &obs(0.0), QuantityTolerance::default()),
            ReconOutcome::RecoveryRequired { .. }
        ));
    }

    // ---- V5: unexpected external position ---------------------------------
    #[test]
    fn unexpected_position_when_closed_locally_but_tokens_exist() {
        let local = snap(0.0, false, false, vec![]);
        assert_eq!(
            compare_position(&local, &obs(25.0), QuantityTolerance::default()),
            ReconOutcome::UnexpectedPosition { external: 25.0 }
        );
        // Dust on a closed position is fine.
        assert_eq!(
            compare_position(&local, &obs(0.0), QuantityTolerance::default()),
            ReconOutcome::InSync
        );
    }

    // ---- V6: transaction pending → inconclusive ---------------------------
    #[test]
    fn unresolved_execution_blocks_verdicts() {
        let local = snap(100.0, true, true, vec![]);
        assert_eq!(
            compare_position(&local, &obs(0.0), QuantityTolerance::default()),
            ReconOutcome::UnknownExecution
        );
        assert!(ReconOutcome::UnknownExecution.is_inconclusive());
        assert!(!ReconOutcome::UnknownExecution.is_divergent());
    }

    // ---- V7/V8/V9/V10: execution classification matrix --------------------
    #[test]
    fn submitted_unconfirmed_resolves_from_external_truth() {
        use ExternalExecutionState as E;
        // V7: confirmed after restart.
        assert_eq!(
            classify_execution(
                LocalExecutionState::SubmittedUnconfirmed,
                ExternalState::Observed(E::Succeeded)
            ),
            ReconOutcome::InSync
        );
        // V8: failed after restart — the ambiguity resolved to "failed".
        assert_eq!(
            classify_execution(
                LocalExecutionState::SubmittedUnconfirmed,
                ExternalState::Observed(E::FailedOnExternal)
            ),
            ReconOutcome::InSync
        );
        // Still pending → unknown, retry later (never "failed").
        assert_eq!(
            classify_execution(
                LocalExecutionState::SubmittedUnconfirmed,
                ExternalState::Observed(E::Pending)
            ),
            ReconOutcome::UnknownExecution
        );
        // V9-adjacent: signature known, never seen on chain.
        assert_eq!(
            classify_execution(
                LocalExecutionState::SubmittedUnconfirmed,
                ExternalState::Observed(E::NotFound)
            ),
            ReconOutcome::MissingTransaction
        );
    }

    #[test]
    fn db_success_chain_failure_is_a_divergence() {
        // V10: DB says success but chain says failure.
        let out = classify_execution(
            LocalExecutionState::RecordedConfirmed,
            ExternalState::Observed(ExternalExecutionState::FailedOnExternal),
        );
        assert!(out.is_divergent(), "{out:?}");
        // And the inverse is operator territory: money moved, book says no.
        let out = classify_execution(
            LocalExecutionState::RecordedFailed,
            ExternalState::Observed(ExternalExecutionState::Succeeded),
        );
        assert!(matches!(out, ReconOutcome::RecoveryRequired { .. }));
    }

    #[test]
    fn no_local_record_with_external_success_requires_recovery() {
        // Case 4/6 in §F: crashed before persistence, chain executed.
        let out = classify_execution(
            LocalExecutionState::NoLocalRecord,
            ExternalState::Observed(ExternalExecutionState::Succeeded),
        );
        assert!(matches!(out, ReconOutcome::RecoveryRequired { .. }));
    }

    // ---- V13/V14/V15: source unavailable is never a conclusion -------------
    #[test]
    fn unavailable_external_state_is_inconclusive() {
        let local = snap(100.0, true, false, vec![]);
        let out = compare_position(
            &local,
            &ExternalState::Unavailable {
                reason: "rpc timeout".into(),
            },
            QuantityTolerance::default(),
        );
        assert_eq!(
            out,
            ReconOutcome::ExternalStateUnavailable {
                reason: "rpc timeout".into()
            }
        );
        assert!(out.is_inconclusive());
        assert!(!out.is_divergent());

        let out = classify_execution(
            LocalExecutionState::RecordedConfirmed,
            ExternalState::Unavailable {
                reason: "down".into(),
            },
        );
        assert!(out.is_inconclusive());
    }

    // ---- V19-V23: fill lifecycle + PnL reconstruction ----------------------
    #[test]
    fn pnl_reconstruction_complete_fill_and_exit() {
        // V20 + V22: bought 100 @ total cost 10, sold 100 for 12 → +2.
        let r = reconstruct_pnl(&[buy(100.0, 10.0), sell(100.0, 12.0)]);
        assert_eq!(r.open_qty, 0.0);
        assert!((r.realized - 2.0).abs() < 1e-12);
        assert_eq!(r.buy_cost, 10.0);
        assert_eq!(r.sell_proceeds, 12.0);
        assert_eq!(r.open_cost_basis, 0.0);
    }

    #[test]
    fn pnl_reconstruction_partial_fill_and_partial_sell() {
        // V19: two buys (partial fills) 60 @ 6 then 40 @ 5 → avg 11/100=0.11.
        // V21: partial sell 50 @ 8 → realized 8 - 50*0.11 = 2.5.
        let r = reconstruct_pnl(&[buy(60.0, 6.0), buy(40.0, 5.0), sell(50.0, 8.0)]);
        assert!((r.open_qty - 50.0).abs() < 1e-12);
        assert!((r.avg_entry - 0.11).abs() < 1e-12);
        assert!((r.realized - 2.5).abs() < 1e-12);
        assert!((r.open_cost_basis - 5.5).abs() < 1e-12);
    }

    #[test]
    fn pnl_reconstruction_full_sell_after_partial_sells() {
        let r = reconstruct_pnl(&[buy(100.0, 20.0), sell(40.0, 10.0), sell(60.0, 9.0)]);
        assert_eq!(r.open_qty, 0.0);
        // avg entry 0.2 → realized = 19 - 20 = -1.
        assert!((r.realized + 1.0).abs() < 1e-12);
    }

    #[test]
    fn pnl_reconstruction_is_defensive() {
        // Over-sell clamps, garbage skipped, no NaN/div-zero.
        let r = reconstruct_pnl(&[
            buy(10.0, 1.0),
            sell(50.0, 5.0), // clamped to 10
            sell(1.0, 1.0),  // nothing left → skipped
            FillRecord {
                side: "hold".into(),
                qty: 5.0,
                quote: 1.0,
            }, // unknown side skipped
            FillRecord {
                side: "buy".into(),
                qty: -1.0,
                quote: 1.0,
            }, // non-positive qty skipped
            FillRecord {
                side: "buy".into(),
                qty: f64::NAN,
                quote: 1.0,
            }, // NaN skipped
        ]);
        assert_eq!(r.open_qty, 0.0);
        // Clamped over-sell: sold all 10 (bought for 1.0) for the
        // proportional proceeds 5.0 * (10/50) = 1.0 → realized exactly 0.
        assert!(r.realized.abs() < 1e-12, "realized = {}", r.realized);
        assert!((r.sell_proceeds - 1.0).abs() < 1e-12);
        assert!(r.realized.is_finite() && r.avg_entry.is_finite());
        // Empty history → all zeros.
        let r0 = reconstruct_pnl(&[]);
        assert_eq!(r0.open_qty, 0.0);
        assert_eq!(r0.realized, 0.0);
        assert_eq!(r0.avg_entry, 0.0);
    }

    // ---- V11/V12 are exercised at the OMS idempotency layer (oms.rs tests)
    // and the executor classification tests (execute.rs) — the engine side:
    #[test]
    fn duplicate_execution_outcome_is_divergent() {
        assert!(ReconOutcome::DuplicateExecution.is_divergent());
        assert!(!ReconOutcome::InSync.is_divergent());
        assert!(!ReconOutcome::InSync.is_inconclusive());
    }

    #[test]
    fn outcome_labels_are_stable_and_low_cardinality() {
        // Metric-label contract (§T): fixed strings, no ids/addresses.
        let outs = [
            ReconOutcome::InSync,
            ReconOutcome::ExternalAhead {
                local: 0.0,
                external: 0.0,
            },
            ReconOutcome::LocalAhead {
                local: 0.0,
                external: 0.0,
            },
            ReconOutcome::QuantityMismatch {
                local: 0.0,
                external: 0.0,
            },
            ReconOutcome::BalanceMismatch {
                expected: 0.0,
                observed: 0.0,
            },
            ReconOutcome::MissingPosition,
            ReconOutcome::UnexpectedPosition { external: 0.0 },
            ReconOutcome::UnknownExecution,
            ReconOutcome::MissingTransaction,
            ReconOutcome::DuplicateExecution,
            ReconOutcome::StaleLocalState,
            ReconOutcome::ExternalStateUnavailable { reason: "x".into() },
            ReconOutcome::RecoveryRequired { reason: "x".into() },
        ];
        let labels: Vec<&str> = outs.iter().map(|o| o.as_str()).collect();
        assert_eq!(labels.len(), 13);
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels must be unique");
        for l in labels {
            assert!(
                l.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "label '{l}' must be prom-safe"
            );
        }
    }

    #[test]
    fn dust_tolerance_covers_rent_artifacts() {
        let local = snap(0.000_000_000_5, true, false, vec![]);
        // Half a lamport-unit of dust on a "closed-ish" position: InSync.
        assert_eq!(
            compare_position(&local, &obs(0.0), QuantityTolerance::default()),
            ReconOutcome::InSync
        );
    }
}
