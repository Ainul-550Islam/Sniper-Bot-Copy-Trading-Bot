//! Global risk vocabulary (TASK 5 §1): the request, the closed reject-reason
//! set and the decision record. Every reason is deterministic — the same
//! request against the same ledger state and configuration yields the same
//! verdict and the same reason.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{BotModule, ExecutionMode, Venue};

/// Why the global layer refused an entry. Closed set (bounded metric label,
/// stable journal / audit text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalRejectReason {
    /// The process-wide kill switch is engaged.
    GlobalKillSwitch,
    /// The venue is killed (`[global_risk].killed_venues` or an operator).
    VenueKillSwitch,
    /// The strategy is killed (`[global_risk].killed_strategies` or an operator).
    StrategyKillSwitch,
    /// A reference-denominated limit is on but the request's quote asset has
    /// no reference rate — the limit cannot be evaluated, so the entry is
    /// refused (fail closed).
    ReferenceRateMissing,
    /// `max_open_positions` reached across every module.
    MaxOpenPositions,
    /// The order's notional exceeds `max_order_notional_ref`.
    OrderNotional,
    /// Portfolio exposure would exceed `max_portfolio_exposure_ref`.
    PortfolioExposure,
    /// Wallet exposure would exceed `max_wallet_exposure_ref`.
    WalletExposure,
    /// Venue exposure would exceed `max_venue_exposure_ref`.
    VenueExposure,
    /// Strategy exposure would exceed `max_strategy_exposure_ref`.
    StrategyExposure,
    /// Asset exposure would exceed `max_asset_exposure_ref`.
    AssetExposure,
    /// Today's net realized loss reached `max_daily_loss_ref`.
    DailyLoss,
    /// Drawdown from the realized peak reached `max_drawdown_ref` /
    /// `max_drawdown_pct`.
    Drawdown,
    /// The request itself is malformed (non-finite / non-positive size).
    InvalidRequest,
}

impl GlobalRejectReason {
    /// Every reason, stable order.
    pub const ALL: [GlobalRejectReason; 14] = [
        GlobalRejectReason::GlobalKillSwitch,
        GlobalRejectReason::VenueKillSwitch,
        GlobalRejectReason::StrategyKillSwitch,
        GlobalRejectReason::ReferenceRateMissing,
        GlobalRejectReason::MaxOpenPositions,
        GlobalRejectReason::OrderNotional,
        GlobalRejectReason::PortfolioExposure,
        GlobalRejectReason::WalletExposure,
        GlobalRejectReason::VenueExposure,
        GlobalRejectReason::StrategyExposure,
        GlobalRejectReason::AssetExposure,
        GlobalRejectReason::DailyLoss,
        GlobalRejectReason::Drawdown,
        GlobalRejectReason::InvalidRequest,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            GlobalRejectReason::GlobalKillSwitch => "global_kill_switch",
            GlobalRejectReason::VenueKillSwitch => "venue_kill_switch",
            GlobalRejectReason::StrategyKillSwitch => "strategy_kill_switch",
            GlobalRejectReason::ReferenceRateMissing => "reference_rate_missing",
            GlobalRejectReason::MaxOpenPositions => "max_open_positions",
            GlobalRejectReason::OrderNotional => "order_notional",
            GlobalRejectReason::PortfolioExposure => "portfolio_exposure",
            GlobalRejectReason::WalletExposure => "wallet_exposure",
            GlobalRejectReason::VenueExposure => "venue_exposure",
            GlobalRejectReason::StrategyExposure => "strategy_exposure",
            GlobalRejectReason::AssetExposure => "asset_exposure",
            GlobalRejectReason::DailyLoss => "daily_loss",
            GlobalRejectReason::Drawdown => "drawdown",
            GlobalRejectReason::InvalidRequest => "invalid_request",
        }
    }

    /// Inverse of [`GlobalRejectReason::as_str`].
    pub fn parse(s: &str) -> Option<GlobalRejectReason> {
        GlobalRejectReason::ALL
            .iter()
            .copied()
            .find(|r| r.as_str() == s)
    }

    /// True for capacity / exposure refusals (as opposed to switches or a
    /// malformed request).
    pub fn is_exposure_limit(&self) -> bool {
        matches!(
            self,
            GlobalRejectReason::MaxOpenPositions
                | GlobalRejectReason::OrderNotional
                | GlobalRejectReason::PortfolioExposure
                | GlobalRejectReason::WalletExposure
                | GlobalRejectReason::VenueExposure
                | GlobalRejectReason::StrategyExposure
                | GlobalRejectReason::AssetExposure
        )
    }

    /// True for the three kill switches.
    pub fn is_kill_switch(&self) -> bool {
        matches!(
            self,
            GlobalRejectReason::GlobalKillSwitch
                | GlobalRejectReason::VenueKillSwitch
                | GlobalRejectReason::StrategyKillSwitch
        )
    }
}

impl std::fmt::Display for GlobalRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Accept or reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalVerdict {
    /// Proceed to the module's own checks.
    Accept,
    /// Refuse.
    Reject,
}

/// Everything the global engine evaluates for one candidate entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalRiskRequest {
    /// Requesting module.
    pub module: BotModule,
    /// Venue the order would go to.
    pub venue: Venue,
    /// Our wallet / account.
    pub wallet: String,
    /// Strategy label.
    pub strategy: String,
    /// Base asset (mint / token id).
    pub asset: String,
    /// Quote asset the size is denominated in.
    pub quote_asset: String,
    /// Requested size in quote units.
    pub requested_quote: f64,
    /// Execution mode.
    pub mode: ExecutionMode,
}

/// Exposure figures the decision was taken against (in reference units;
/// `0` where the slice was empty). Journaled with every decision so the
/// verdict is reproducible after the fact.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecisionSnapshot {
    /// Requested size converted to reference units (`0` when no rate).
    pub requested_ref: f64,
    /// Reference rate used (`0` when none).
    pub rate: f64,
    /// Portfolio exposure before the order.
    pub portfolio_ref: f64,
    /// Wallet exposure before the order.
    pub wallet_ref: f64,
    /// Venue exposure before the order.
    pub venue_ref: f64,
    /// Strategy exposure before the order.
    pub strategy_ref: f64,
    /// Asset exposure before the order.
    pub asset_ref: f64,
    /// Open aggregated positions.
    pub open_positions: usize,
    /// Net realized today.
    pub realized_today_ref: f64,
    /// Drawdown from the realized peak (incl. unrealized).
    pub drawdown_ref: f64,
    /// Capital base (`0` unknown).
    pub capital_base_ref: f64,
}

/// The final global decision for one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalRiskDecision {
    /// Deterministic id (`grd_` + digest of request identity, time and
    /// sequence).
    pub decision_id: String,
    /// When it was taken.
    pub ts: DateTime<Utc>,
    /// The request.
    pub request: GlobalRiskRequest,
    /// Accept / reject.
    pub verdict: GlobalVerdict,
    /// Reason on reject.
    pub reason: Option<GlobalRejectReason>,
    /// Human detail (limit and figures).
    pub detail: String,
    /// Figures the decision was taken against.
    pub snapshot: DecisionSnapshot,
    /// Replica that decided.
    pub replica_id: String,
}

impl GlobalRiskDecision {
    /// True on accept.
    pub fn accepted(&self) -> bool {
        self.verdict == GlobalVerdict::Accept
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "decision={} verdict={:?} reason={} module={} venue={} wallet={} strategy={} asset={} quote={} requested={:.8} requested_ref={:.4} portfolio_ref={:.4} wallet_ref={:.4} venue_ref={:.4} strategy_ref={:.4} asset_ref={:.4} open={} realized_today_ref={:.4} drawdown_ref={:.4} detail={}",
            self.decision_id,
            self.verdict,
            self.reason.map(|r| r.as_str()).unwrap_or("-"),
            self.request.module,
            self.request.venue.as_str(),
            self.request.wallet,
            self.request.strategy,
            self.request.asset,
            self.request.quote_asset,
            self.request.requested_quote,
            self.snapshot.requested_ref,
            self.snapshot.portfolio_ref,
            self.snapshot.wallet_ref,
            self.snapshot.venue_ref,
            self.snapshot.strategy_ref,
            self.snapshot.asset_ref,
            self.snapshot.open_positions,
            self.snapshot.realized_today_ref,
            self.snapshot.drawdown_ref,
            self.detail
        )
    }
}

/// Deterministic decision id.
pub(crate) fn decision_id(req: &GlobalRiskRequest, ts: DateTime<Utc>, seq: u64) -> String {
    let mut h = Sha256::new();
    h.update(b"global-risk-decision-v1|");
    h.update(req.module.as_str().as_bytes());
    h.update(b"|");
    h.update(req.venue.as_str().as_bytes());
    h.update(b"|");
    h.update(req.wallet.as_bytes());
    h.update(b"|");
    h.update(req.strategy.as_bytes());
    h.update(b"|");
    h.update(req.asset.as_bytes());
    h.update(b"|");
    h.update(ts.timestamp_millis().to_string().as_bytes());
    h.update(b"|");
    h.update(seq.to_string().as_bytes());
    format!("grd_{}", &hex::encode(h.finalize())[..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_labels_round_trip_and_classify() {
        for r in GlobalRejectReason::ALL {
            assert_eq!(GlobalRejectReason::parse(r.as_str()), Some(r));
        }
        assert!(GlobalRejectReason::PortfolioExposure.is_exposure_limit());
        assert!(!GlobalRejectReason::DailyLoss.is_exposure_limit());
        assert!(GlobalRejectReason::VenueKillSwitch.is_kill_switch());
        assert_eq!(GlobalRejectReason::ALL.len(), 14);
    }

    #[test]
    fn decision_ids_are_deterministic() {
        let req = GlobalRiskRequest {
            module: BotModule::Sniper,
            venue: Venue::PumpFun,
            wallet: "w".into(),
            strategy: "sniper".into(),
            asset: "MINT".into(),
            quote_asset: "SOL".into(),
            requested_quote: 1.0,
            mode: ExecutionMode::Paper,
        };
        let ts = Utc::now();
        assert_eq!(decision_id(&req, ts, 1), decision_id(&req, ts, 1));
        assert_ne!(decision_id(&req, ts, 1), decision_id(&req, ts, 2));
    }
}
