//! Deterministic validation pipeline vocabulary (TASK 2 §C/§F/§J).
//!
//! This module defines *what* the sniper decides and *how it is named*:
//!
//! * [`SniperStage`] — the launch lifecycle
//!   `DETECTED → VALIDATED → RISK_APPROVED → EXECUTION_READY → SUBMITTED →
//!   CONFIRMED`, with `REJECTED` / `FAILED` as the terminal exits, and the
//!   transition table that makes an out-of-order step an `INVALID_STATE`
//!   rejection instead of silent corruption;
//! * [`RejectReason`] — the machine-readable reason codes every refusal
//!   carries (metrics label, audit text, replay expectation);
//! * [`Lifecycle`] + [`LatencyTimeline`] — per-event stage history and the
//!   timestamps behind the `sniper_*_latency_ms` histograms;
//! * [`EntryRoute`] + [`select_route`] — the venue an entry is executed on,
//!   derived from the protocol, the live config and the venue state;
//! * [`FeeEstimate`] + [`check_fee_budget`] — the deterministic worst-case
//!   fee of one entry transaction against the executor's fee policy and the
//!   strategy's `sniper.max_entry_fee_lamports` (check 15);
//! * the mapping from [`RiskCode`] to a reject reason, so the risk engine
//!   stays the single authority while the sniper reports uniformly.
//!
//! Everything here is pure; the async orchestration lives in `entry.rs`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use bot_core::config::{ExecutionConfig, SniperConfig};
use bot_core::maths::{LAMPORTS_PER_SIGNATURE, MAX_TRANSACTION_COMPUTE_UNITS};
use bot_core::obs::metrics::{self, LATENCY_BUCKETS_MS};
use bot_core::risk::RiskCode;

use solana_kit::fees::FeePolicy;

use crate::event::LaunchProtocol;

/// Why an event left the pipeline without an execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RejectReason {
    /// Structural defect in the event itself (bad pubkey, bad signature,
    /// inconsistent identity, impossible timestamps).
    InvalidEvent,
    /// Too old to act on: launch age, snapshot age or the entry latency
    /// budget was exceeded.
    StaleEvent,
    /// The authoritative dedup store already saw this launch.
    DuplicateEvent,
    /// Below the liquidity threshold, or nothing to price against.
    InsufficientLiquidity,
    /// The slippage engine could not find an allowed tolerance.
    SlippageLimit,
    /// Modelled price impact above `sniper.max_price_impact_bps`.
    PriceImpactLimit,
    /// The entry's worst-case transaction fee exceeds
    /// `sniper.max_entry_fee_lamports`, or the configured priority fee would
    /// be refused by the execution engine's fee policy.
    FeeLimit,
    /// Capacity/exposure exhausted (positions, pending intents, exposure
    /// envelope, daily loss).
    ExposureLimit,
    /// The risk engine refused for any other reason (screening,
    /// cooldowns, balance, size, venue checks).
    RiskRejected,
    /// The chain / execution engine cannot be used right now (RPC pool
    /// tripped, balance unreadable, market data unreadable, build failed,
    /// broadcasting disabled while live was requested).
    ExecutionUnavailable,
    /// Global kill switch or emergency halt.
    KillSwitch,
    /// The sniper module is disabled, or its emergency-disable latch is on.
    StrategyDisabled,
    /// No enabled venue can execute this protocol.
    InvalidRoute,
    /// A lifecycle step was attempted out of order.
    InvalidState,
    /// Token account state fails a gate (authorities, decimals).
    TokenStateInvalid,
    /// Venue not accepting buys (curve complete, pool disabled/not open).
    PoolNotReady,
    /// Creator or pool concentration gate failed.
    ConcentrationLimit,
    /// The symbol is gated by unresolved reconciliation claims.
    SymbolGated,
    /// Another replica owns this launch, or our lease was fenced.
    OwnershipLost,
}

impl RejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectReason::InvalidEvent => "INVALID_EVENT",
            RejectReason::StaleEvent => "STALE_EVENT",
            RejectReason::DuplicateEvent => "DUPLICATE_EVENT",
            RejectReason::InsufficientLiquidity => "INSUFFICIENT_LIQUIDITY",
            RejectReason::SlippageLimit => "SLIPPAGE_LIMIT",
            RejectReason::PriceImpactLimit => "PRICE_IMPACT_LIMIT",
            RejectReason::FeeLimit => "FEE_LIMIT",
            RejectReason::ExposureLimit => "EXPOSURE_LIMIT",
            RejectReason::RiskRejected => "RISK_REJECTED",
            RejectReason::ExecutionUnavailable => "EXECUTION_UNAVAILABLE",
            RejectReason::KillSwitch => "KILL_SWITCH",
            RejectReason::StrategyDisabled => "STRATEGY_DISABLED",
            RejectReason::InvalidRoute => "INVALID_ROUTE",
            RejectReason::InvalidState => "INVALID_STATE",
            RejectReason::TokenStateInvalid => "TOKEN_STATE_INVALID",
            RejectReason::PoolNotReady => "POOL_NOT_READY",
            RejectReason::ConcentrationLimit => "CONCENTRATION_LIMIT",
            RejectReason::SymbolGated => "SYMBOL_GATED",
            RejectReason::OwnershipLost => "OWNERSHIP_LOST",
        }
    }

    /// Every reason, for metric pre-registration and documentation tests.
    pub const ALL: &'static [RejectReason] = &[
        RejectReason::InvalidEvent,
        RejectReason::StaleEvent,
        RejectReason::DuplicateEvent,
        RejectReason::InsufficientLiquidity,
        RejectReason::SlippageLimit,
        RejectReason::PriceImpactLimit,
        RejectReason::FeeLimit,
        RejectReason::ExposureLimit,
        RejectReason::RiskRejected,
        RejectReason::ExecutionUnavailable,
        RejectReason::KillSwitch,
        RejectReason::StrategyDisabled,
        RejectReason::InvalidRoute,
        RejectReason::InvalidState,
        RejectReason::TokenStateInvalid,
        RejectReason::PoolNotReady,
        RejectReason::ConcentrationLimit,
        RejectReason::SymbolGated,
        RejectReason::OwnershipLost,
    ];

    /// Map the risk engine's code onto the pipeline vocabulary. The risk
    /// engine remains the authority; this only chooses the label.
    pub fn from_risk_code(code: Option<RiskCode>) -> RejectReason {
        match code {
            Some(RiskCode::KillSwitch) | Some(RiskCode::GlobalKillSwitch) => {
                RejectReason::KillSwitch
            }
            Some(RiskCode::ModuleDisabled) | Some(RiskCode::SniperEmergencyDisabled) => {
                RejectReason::StrategyDisabled
            }
            Some(RiskCode::SlippageCap) => RejectReason::SlippageLimit,
            Some(c) if c.is_exposure_limit() => RejectReason::ExposureLimit,
            _ => RejectReason::RiskRejected,
        }
    }
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refusal with its human detail and the stage it happened at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub reason: RejectReason,
    pub detail: String,
    pub stage: SniperStage,
}

impl Rejection {
    pub fn new(reason: RejectReason, stage: SniperStage, detail: impl Into<String>) -> Self {
        Rejection {
            reason,
            detail: detail.into(),
            stage,
        }
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}: {}", self.reason, self.stage, self.detail)
    }
}

/// Launch lifecycle stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SniperStage {
    Detected,
    Validated,
    RiskApproved,
    ExecutionReady,
    Submitted,
    Confirmed,
    Rejected,
    Failed,
}

impl SniperStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            SniperStage::Detected => "DETECTED",
            SniperStage::Validated => "VALIDATED",
            SniperStage::RiskApproved => "RISK_APPROVED",
            SniperStage::ExecutionReady => "EXECUTION_READY",
            SniperStage::Submitted => "SUBMITTED",
            SniperStage::Confirmed => "CONFIRMED",
            SniperStage::Rejected => "REJECTED",
            SniperStage::Failed => "FAILED",
        }
    }

    pub const ALL: &'static [SniperStage] = &[
        SniperStage::Detected,
        SniperStage::Validated,
        SniperStage::RiskApproved,
        SniperStage::ExecutionReady,
        SniperStage::Submitted,
        SniperStage::Confirmed,
        SniperStage::Rejected,
        SniperStage::Failed,
    ];

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SniperStage::Confirmed | SniperStage::Rejected | SniperStage::Failed
        )
    }

    /// The transition table. Forward steps are strictly linear; a
    /// rejection is legal at any pre-submission stage; a failure is legal
    /// once a transaction was built or submitted; nothing leaves a terminal
    /// stage.
    pub fn can_transition_to(&self, next: SniperStage) -> bool {
        use SniperStage::*;
        matches!(
            (self, next),
            (Detected, Validated)
                | (Validated, RiskApproved)
                | (RiskApproved, ExecutionReady)
                | (ExecutionReady, Submitted)
                | (Submitted, Confirmed)
                | (
                    Detected | Validated | RiskApproved | ExecutionReady,
                    Rejected
                )
                | (ExecutionReady | Submitted, Failed)
        )
    }
}

impl std::fmt::Display for SniperStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Timestamps behind the latency metrics. Every mark is optional because a
/// rejected event never reaches the later ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyTimeline {
    /// When the source/chain produced the launch (event's best estimate).
    pub event_at: Option<DateTime<Utc>>,
    /// When this process observed the event.
    pub observed_at: Option<DateTime<Utc>>,
    /// When the pipeline picked it up.
    pub detected_at: Option<DateTime<Utc>>,
    pub validated_at: Option<DateTime<Utc>>,
    pub risk_at: Option<DateTime<Utc>>,
    pub built_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub confirmed_at: Option<DateTime<Utc>>,
}

fn span_ms(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Option<u64> {
    Some(to?.signed_duration_since(from?).num_milliseconds().max(0) as u64)
}

impl LatencyTimeline {
    /// event → observed (feed latency; only when the source stamps events).
    pub fn detection_ms(&self) -> Option<u64> {
        span_ms(self.event_at, self.observed_at)
    }
    /// detected → validated.
    pub fn validation_ms(&self) -> Option<u64> {
        span_ms(self.detected_at, self.validated_at)
    }
    /// validated → risk approved.
    pub fn risk_ms(&self) -> Option<u64> {
        span_ms(self.validated_at, self.risk_at)
    }
    /// risk approved → execution ready (transaction built).
    pub fn build_ms(&self) -> Option<u64> {
        span_ms(self.risk_at, self.built_at)
    }
    /// execution ready → submitted (handed to the execution engine).
    pub fn submission_ms(&self) -> Option<u64> {
        span_ms(self.built_at, self.submitted_at)
    }
    /// submitted → confirmed.
    pub fn confirmation_ms(&self) -> Option<u64> {
        span_ms(self.submitted_at, self.confirmed_at)
    }
    /// observed → the last mark reached.
    pub fn total_ms(&self) -> Option<u64> {
        let last = self
            .confirmed_at
            .or(self.submitted_at)
            .or(self.built_at)
            .or(self.risk_at)
            .or(self.validated_at)
            .or(self.detected_at);
        span_ms(self.observed_at, last)
    }

    /// Observe every available span into its histogram.
    pub fn record_metrics(&self, protocol: LaunchProtocol) {
        let reg = metrics::global();
        let labels = [("protocol", protocol.as_str())];
        let spans: [(&str, Option<u64>); 7] = [
            ("sniper_detection_latency_ms", self.detection_ms()),
            ("sniper_validation_latency_ms", self.validation_ms()),
            ("sniper_risk_latency_ms", self.risk_ms()),
            ("sniper_build_latency_ms", self.build_ms()),
            ("sniper_submission_latency_ms", self.submission_ms()),
            ("sniper_confirmation_latency_ms", self.confirmation_ms()),
            ("sniper_total_latency_ms", self.total_ms()),
        ];
        for (name, v) in spans {
            if let Some(v) = v {
                reg.histogram(name, latency_help(name), &labels, LATENCY_BUCKETS_MS)
                    .observe(v);
            }
        }
    }
}

fn latency_help(name: &str) -> &'static str {
    match name {
        "sniper_detection_latency_ms" => "Launch event time to local observation (feed latency).",
        "sniper_validation_latency_ms" => "Pipeline pickup to validated (shape, dedup, screening).",
        "sniper_risk_latency_ms" => {
            "Validated to risk-approved (market read, gates, slippage, risk)."
        }
        "sniper_build_latency_ms" => "Risk-approved to transaction built.",
        "sniper_submission_latency_ms" => "Built to handed to the execution engine.",
        "sniper_confirmation_latency_ms" => "Submission to confirmed/failed outcome.",
        _ => "Local observation to the last lifecycle mark reached.",
    }
}

/// Per-event lifecycle: current stage, history and timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub event_id: String,
    pub protocol: LaunchProtocol,
    pub stage: SniperStage,
    pub history: Vec<(SniperStage, DateTime<Utc>)>,
    pub timeline: LatencyTimeline,
    pub rejection: Option<Rejection>,
}

impl Lifecycle {
    /// Start at `DETECTED` now.
    pub fn start(
        event_id: impl Into<String>,
        protocol: LaunchProtocol,
        event_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Self {
        Lifecycle {
            event_id: event_id.into(),
            protocol,
            stage: SniperStage::Detected,
            history: vec![(SniperStage::Detected, now)],
            timeline: LatencyTimeline {
                event_at: Some(event_at),
                observed_at: Some(observed_at),
                detected_at: Some(now),
                ..Default::default()
            },
            rejection: None,
        }
    }

    /// Move to `next` at `now`. An illegal transition is reported as an
    /// `INVALID_STATE` rejection and leaves the lifecycle untouched.
    pub fn advance(&mut self, next: SniperStage, now: DateTime<Utc>) -> Result<(), Rejection> {
        if !self.stage.can_transition_to(next) {
            return Err(Rejection::new(
                RejectReason::InvalidState,
                self.stage,
                format!("illegal transition {} -> {}", self.stage, next),
            ));
        }
        self.stage = next;
        self.history.push((next, now));
        match next {
            SniperStage::Validated => self.timeline.validated_at = Some(now),
            SniperStage::RiskApproved => self.timeline.risk_at = Some(now),
            SniperStage::ExecutionReady => self.timeline.built_at = Some(now),
            SniperStage::Submitted => self.timeline.submitted_at = Some(now),
            SniperStage::Confirmed => self.timeline.confirmed_at = Some(now),
            SniperStage::Detected | SniperStage::Rejected | SniperStage::Failed => {}
        }
        Ok(())
    }

    /// Terminate with a rejection (legal from any pre-submission stage).
    /// Returns the rejection back for the caller to report. When the
    /// current stage cannot legally reject (already submitted), the
    /// lifecycle is marked `FAILED` instead so nothing is left dangling.
    pub fn reject(
        &mut self,
        reason: RejectReason,
        detail: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Rejection {
        let rejection = Rejection::new(reason, self.stage, detail);
        if self.stage.can_transition_to(SniperStage::Rejected) {
            self.stage = SniperStage::Rejected;
            self.history.push((SniperStage::Rejected, now));
        } else if self.stage.can_transition_to(SniperStage::Failed) {
            self.stage = SniperStage::Failed;
            self.history.push((SniperStage::Failed, now));
        }
        self.rejection = Some(rejection.clone());
        rejection
    }

    /// Terminate as failed (post-build / post-submit).
    pub fn fail(&mut self, detail: impl Into<String>, now: DateTime<Utc>) -> Rejection {
        let rejection = Rejection::new(RejectReason::ExecutionUnavailable, self.stage, detail);
        if self.stage.can_transition_to(SniperStage::Failed) {
            self.stage = SniperStage::Failed;
            self.history.push((SniperStage::Failed, now));
        }
        self.rejection = Some(rejection.clone());
        rejection
    }

    /// Stage names in order, for audit text.
    pub fn path(&self) -> String {
        self.history
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join(">")
    }
}

/// Venue an entry (and later its exit) executes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryRoute {
    /// pump.fun bonding curve (`buy` / `sell` on the pump program).
    PumpCurve,
    /// PumpSwap AMM, direct instruction build.
    PumpSwapDirect,
    /// Raydium AMM v4, direct instruction build.
    RaydiumV4Direct,
    /// Jupiter aggregator (prebuilt swap handed to the execution engine).
    Jupiter,
}

impl EntryRoute {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryRoute::PumpCurve => "curve",
            EntryRoute::PumpSwapDirect => "pumpswap",
            EntryRoute::RaydiumV4Direct => "raydium",
            EntryRoute::Jupiter => "jupiter",
        }
    }

    /// The venue a position on this route is booked under.
    pub fn venue(&self) -> bot_core::models::Venue {
        match self {
            EntryRoute::PumpCurve => bot_core::models::Venue::PumpFun,
            EntryRoute::PumpSwapDirect => bot_core::models::Venue::PumpSwap,
            EntryRoute::RaydiumV4Direct => bot_core::models::Venue::RaydiumAmmV4,
            EntryRoute::Jupiter => bot_core::models::Venue::Jupiter,
        }
    }
}

impl std::fmt::Display for EntryRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Static route pre-check, before any chain read: can this protocol be
/// executed at all under the live config? Returns the routes that may
/// apply (the final choice needs the venue state, see [`select_route`]).
pub fn protocol_routable(protocol: LaunchProtocol, cfg: &SniperConfig) -> Result<(), String> {
    match protocol {
        LaunchProtocol::PumpFun => Ok(()),
        LaunchProtocol::PumpSwap => {
            if cfg.trade_pumpswap || cfg.use_jupiter_fallback {
                Ok(())
            } else {
                Err(
                    "pump_swap launches need sniper.trade_pumpswap or sniper.use_jupiter_fallback"
                        .into(),
                )
            }
        }
        LaunchProtocol::RaydiumAmmV4 => {
            if cfg.trade_raydium || cfg.use_jupiter_fallback {
                Ok(())
            } else {
                Err("raydium_amm_v4 launches need sniper.trade_raydium or sniper.use_jupiter_fallback".into())
            }
        }
    }
}

/// Choose the execution route from the protocol, the config and the venue
/// state observed in the market snapshot.
///
/// * pump.fun: the curve while it is live; once complete, the PumpSwap
///   pool directly when `trade_pumpswap`, else Jupiter when the fallback is
///   on, else no route;
/// * PumpSwap: direct when `trade_pumpswap`, else Jupiter fallback;
/// * Raydium: direct when `trade_raydium`, else Jupiter fallback.
pub fn select_route(
    protocol: LaunchProtocol,
    cfg: &SniperConfig,
    curve_complete: bool,
) -> Result<EntryRoute, String> {
    match protocol {
        LaunchProtocol::PumpFun if !curve_complete => Ok(EntryRoute::PumpCurve),
        LaunchProtocol::PumpFun | LaunchProtocol::PumpSwap => {
            if cfg.trade_pumpswap {
                Ok(EntryRoute::PumpSwapDirect)
            } else if cfg.use_jupiter_fallback {
                Ok(EntryRoute::Jupiter)
            } else {
                Err(format!(
                    "{protocol}: curve complete / pool launch and neither trade_pumpswap nor use_jupiter_fallback is on"
                ))
            }
        }
        LaunchProtocol::RaydiumAmmV4 => {
            if cfg.trade_raydium {
                Ok(EntryRoute::RaydiumV4Direct)
            } else if cfg.use_jupiter_fallback {
                Ok(EntryRoute::Jupiter)
            } else {
                Err("raydium_amm_v4: neither trade_raydium nor use_jupiter_fallback is on".into())
            }
        }
    }
}

/// Deterministic worst-case cost of one entry transaction, excluding the
/// swap amount itself. Every input is configuration or policy — no chain
/// read — so the live pipeline (check 15) and the replay engine compute the
/// same number for the same config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeEstimate {
    /// Priority fee assumed, µlamports per compute unit: the most the fee
    /// policy can settle on for the configured request
    /// ([`FeePolicy::max_payable`]).
    pub priority_micro_lamports: u64,
    /// Compute units priced: `execution.compute_unit_limit` for the direct
    /// routes, the transaction maximum for an aggregator-built (Jupiter)
    /// transaction whose budget the aggregator sets.
    pub compute_units: u32,
    /// Base fee: one signature (the wallet) × 5 000 lamports.
    pub base_lamports: u64,
    /// Priority fee in lamports, rounded up.
    pub priority_lamports: u64,
    /// Jito tip when `execution.use_jito` is on, else 0.
    pub tip_lamports: u64,
    /// `base + priority + tip`, saturating.
    pub total_lamports: u64,
}

/// Compute units the entry transaction on `route` is priced at.
pub fn route_compute_units(route: EntryRoute, exec: &ExecutionConfig) -> u32 {
    match route {
        EntryRoute::Jupiter => MAX_TRANSACTION_COMPUTE_UNITS,
        EntryRoute::PumpCurve | EntryRoute::PumpSwapDirect | EntryRoute::RaydiumV4Direct => {
            exec.compute_unit_limit
        }
    }
}

/// Estimate the worst case an entry on `route` can pay in fees when the
/// executor runs it with `policy` for up to `attempts` attempts.
pub fn estimate_entry_fee(
    policy: &FeePolicy,
    exec: &ExecutionConfig,
    route: EntryRoute,
    attempts: u32,
) -> FeeEstimate {
    let priority_micro_lamports = policy.max_payable(exec.priority_fee_micro_lamports, attempts);
    let compute_units = route_compute_units(route, exec);
    let priority_lamports = u64::try_from(
        (priority_micro_lamports as u128 * compute_units as u128).div_ceil(1_000_000),
    )
    .unwrap_or(u64::MAX);
    let tip_lamports = if exec.use_jito {
        exec.jito_tip_lamports
    } else {
        0
    };
    let base_lamports = LAMPORTS_PER_SIGNATURE;
    FeeEstimate {
        priority_micro_lamports,
        compute_units,
        base_lamports,
        priority_lamports,
        tip_lamports,
        total_lamports: base_lamports
            .saturating_add(priority_lamports)
            .saturating_add(tip_lamports),
    }
}

/// Pipeline check 15 — the fee budget. Two refusals, both `FEE_LIMIT`:
///
/// 1. the execution engine's fee policy would refuse the configured priority
///    fee ([`FeePolicy::would_refuse`]) — reported here, before an attempt is
///    recorded, instead of as a failed submission;
/// 2. the worst-case fee of the transaction exceeds
///    `sniper.max_entry_fee_lamports` (`0` = no budget).
///
/// Pure: live path and replay share it. The engine's own `decide` still
/// runs at submission time; this check can only be stricter, never looser.
pub fn check_fee_budget(
    policy: &FeePolicy,
    exec: &ExecutionConfig,
    route: EntryRoute,
    attempts: u32,
    max_entry_fee_lamports: u64,
) -> Result<FeeEstimate, (RejectReason, String)> {
    if policy.would_refuse(exec.priority_fee_micro_lamports) {
        return Err((
            RejectReason::FeeLimit,
            format!(
                "priority fee {} µlamports/CU would be refused by the fee policy (emergency limit {})",
                exec.priority_fee_micro_lamports, policy.emergency_max_micro_lamports
            ),
        ));
    }
    let estimate = estimate_entry_fee(policy, exec, route, attempts);
    if max_entry_fee_lamports > 0 && estimate.total_lamports > max_entry_fee_lamports {
        return Err((
            RejectReason::FeeLimit,
            format!(
                "estimated entry fee {} lamports ({} base + {} µlamports/CU × {} CU + {} tip) exceeds max_entry_fee_lamports {}",
                estimate.total_lamports,
                estimate.base_lamports,
                estimate.priority_micro_lamports,
                estimate.compute_units,
                estimate.tip_lamports,
                max_entry_fee_lamports
            ),
        ));
    }
    Ok(estimate)
}

/// Process-state inputs for [`precheck`] (everything that is not the event
/// or the config), so the same rule runs live and in replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrecheckContext {
    pub kill_switch: bool,
    pub module_enabled: bool,
    pub emergency_disable: bool,
    pub symbol_gated: bool,
}

/// Pipeline checks 1–6 (shape, route, kill switch, enabled/emergency, age,
/// symbol gate). Pure: the live path and the replay engine share it.
pub fn precheck(
    event: &crate::event::LaunchEvent,
    cfg: &SniperConfig,
    ctx: PrecheckContext,
    now: DateTime<Utc>,
) -> Result<(), (RejectReason, String)> {
    if let Err(defect) = event.validate_shape(now) {
        return Err((RejectReason::InvalidEvent, defect.to_string()));
    }
    if let Err(detail) = protocol_routable(event.protocol, cfg) {
        return Err((RejectReason::InvalidRoute, detail));
    }
    if ctx.kill_switch {
        return Err((RejectReason::KillSwitch, "kill switch engaged".into()));
    }
    if !ctx.module_enabled {
        return Err((
            RejectReason::StrategyDisabled,
            "sniper module disabled".into(),
        ));
    }
    if ctx.emergency_disable {
        return Err((
            RejectReason::StrategyDisabled,
            "risk.sniper_emergency_disable is set".into(),
        ));
    }
    if event.is_stale(now, cfg.max_launch_age_secs) {
        return Err((
            RejectReason::StaleEvent,
            format!(
                "launch is {} ms old (max {} s)",
                event.age_ms(now),
                cfg.max_launch_age_secs
            ),
        ));
    }
    if ctx.symbol_gated {
        return Err((
            RejectReason::SymbolGated,
            "symbol gated by unresolved reconciliation".into(),
        ));
    }
    Ok(())
}

/// Count a rejection in `sniper_rejections_total{reason,stage,protocol}`.
pub fn count_rejection(rejection: &Rejection, protocol: LaunchProtocol) {
    metrics::global()
        .counter(
            "sniper_rejections_total",
            "Launch events refused by the sniper pipeline, by reason and stage.",
            &[
                ("reason", rejection.reason.as_str()),
                ("stage", rejection.stage.as_str()),
                ("protocol", protocol.as_str()),
            ],
        )
        .inc();
}

/// Count a stage entry in `sniper_stage_total{stage,protocol}`.
pub fn count_stage(stage: SniperStage, protocol: LaunchProtocol) {
    metrics::global()
        .counter(
            "sniper_stage_total",
            "Launch events entering each lifecycle stage.",
            &[("stage", stage.as_str()), ("protocol", protocol.as_str())],
        )
        .inc();
}

/// Count a normalised event in `sniper_events_total{protocol,source}`.
pub fn count_event(protocol: LaunchProtocol, source: &str) {
    metrics::global()
        .counter(
            "sniper_events_total",
            "Normalised launch events received by the pipeline, by protocol and source feed.",
            &[("protocol", protocol.as_str()), ("source", source)],
        )
        .inc();
}

/// Observe a decided slippage tolerance in `sniper_slippage_bps{mode}`.
pub fn observe_slippage(bps: u64, mode: &str) {
    metrics::global()
        .histogram(
            "sniper_slippage_bps",
            "Slippage tolerance decided for sniper entries, in basis points.",
            &[("mode", mode)],
            &[50, 100, 250, 500, 1_000, 1_500, 2_000, 3_000, 5_000, 10_000],
        )
        .observe(bps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn reject_reasons_have_stable_screaming_snake_codes() {
        for r in RejectReason::ALL {
            let s = r.as_str();
            assert!(!s.is_empty());
            assert!(s.chars().all(|c| c.is_ascii_uppercase() || c == '_'), "{s}");
            // serde uses the same spelling, so fixtures and metrics agree.
            assert_eq!(serde_json::to_string(r).unwrap(), format!("\"{s}\""));
            let back: RejectReason = serde_json::from_str(&format!("\"{s}\"")).unwrap();
            assert_eq!(back, *r);
        }
        assert_eq!(RejectReason::ALL.len(), 19);
        assert_eq!(RejectReason::SlippageLimit.to_string(), "SLIPPAGE_LIMIT");
        assert_eq!(RejectReason::FeeLimit.to_string(), "FEE_LIMIT");
    }

    #[test]
    fn fee_estimate_prices_route_policy_and_tip_deterministically() {
        let exec = ExecutionConfig::default(); // 250 000 µlamports/CU, 400 000 CU, fixed, +50%/retry
        let policy = FeePolicy::from_config(&exec);
        let one = estimate_entry_fee(&policy, &exec, EntryRoute::PumpCurve, 1);
        assert_eq!(one.priority_micro_lamports, 250_000);
        assert_eq!(one.compute_units, 400_000);
        assert_eq!(one.base_lamports, LAMPORTS_PER_SIGNATURE);
        assert_eq!(one.priority_lamports, 100_000);
        assert_eq!(one.tip_lamports, 0);
        assert_eq!(one.total_lamports, 105_000);
        // A second attempt escalates by 50%.
        let two = estimate_entry_fee(&policy, &exec, EntryRoute::PumpSwapDirect, 2);
        assert_eq!(two.priority_micro_lamports, 375_000);
        assert_eq!(two.total_lamports, 155_000);
        // Jupiter builds its own budget: priced at the transaction maximum.
        let jup = estimate_entry_fee(&policy, &exec, EntryRoute::Jupiter, 1);
        assert_eq!(jup.compute_units, MAX_TRANSACTION_COMPUTE_UNITS);
        assert_eq!(jup.priority_lamports, 350_000);
        // Jito adds the tip.
        let jito = ExecutionConfig {
            use_jito: true,
            jito_tip_lamports: 1_000_000,
            ..ExecutionConfig::default()
        };
        let tipped = estimate_entry_fee(&policy, &jito, EntryRoute::RaydiumV4Direct, 1);
        assert_eq!(tipped.tip_lamports, 1_000_000);
        assert_eq!(tipped.total_lamports, 1_105_000);
        // Rounding is up, never down.
        let odd = ExecutionConfig {
            priority_fee_micro_lamports: 1,
            compute_unit_limit: 1,
            fee_min_micro_lamports: 0,
            ..ExecutionConfig::default()
        };
        let rounded = estimate_entry_fee(
            &FeePolicy::from_config(&odd),
            &odd,
            EntryRoute::PumpCurve,
            1,
        );
        assert_eq!(rounded.priority_lamports, 1);
        // Adaptive mode is bounded by the policy max, whatever was requested.
        let adaptive = ExecutionConfig {
            fee_mode: "adaptive".into(),
            ..ExecutionConfig::default()
        };
        let a = estimate_entry_fee(
            &FeePolicy::from_config(&adaptive),
            &adaptive,
            EntryRoute::PumpCurve,
            1,
        );
        assert_eq!(a.priority_micro_lamports, adaptive.fee_max_micro_lamports);
        // Overflow saturates instead of wrapping.
        let huge = FeePolicy {
            adaptive: false,
            min_micro_lamports: u64::MAX,
            max_micro_lamports: u64::MAX,
            emergency_max_micro_lamports: u64::MAX,
            escalation_pct: 0,
            ..FeePolicy::default()
        };
        let sat = estimate_entry_fee(&huge, &jito, EntryRoute::Jupiter, 5);
        assert_eq!(sat.total_lamports, u64::MAX);
    }

    #[test]
    fn fee_budget_check_is_off_at_zero_and_refuses_over_budget_or_policy() {
        let exec = ExecutionConfig::default();
        let policy = FeePolicy::from_config(&exec);
        // What `exec_policy_from_config` derives from `send_retries` (default 2).
        let attempts = exec.send_retries.clamp(1, 5);
        // 0 = no budget: only the policy veto can refuse.
        let est = check_fee_budget(&policy, &exec, EntryRoute::PumpCurve, attempts, 0).unwrap();
        assert_eq!(est.total_lamports, 155_000);
        // Budget exactly at the estimate passes; one lamport less refuses.
        assert!(check_fee_budget(&policy, &exec, EntryRoute::PumpCurve, attempts, 155_000).is_ok());
        let (reason, detail) =
            check_fee_budget(&policy, &exec, EntryRoute::PumpCurve, attempts, 154_999).unwrap_err();
        assert_eq!(reason, RejectReason::FeeLimit);
        assert!(detail.contains("155000 lamports"), "{detail}");
        assert!(detail.contains("max_entry_fee_lamports 154999"), "{detail}");
        // The same budget can pass on a cheaper route and fail on Jupiter.
        assert!(check_fee_budget(&policy, &exec, EntryRoute::PumpCurve, 1, 105_000).is_ok());
        assert_eq!(
            check_fee_budget(&policy, &exec, EntryRoute::Jupiter, 1, 105_000)
                .unwrap_err()
                .0,
            RejectReason::FeeLimit
        );
        // A policy that would refuse the configured fee is FEE_LIMIT even
        // with no budget (this is the executor's veto, reported early).
        let strict = FeePolicy {
            emergency_max_micro_lamports: exec.priority_fee_micro_lamports - 1,
            max_micro_lamports: exec.priority_fee_micro_lamports - 1,
            ..policy
        };
        let (reason, detail) =
            check_fee_budget(&strict, &exec, EntryRoute::PumpCurve, attempts, 0).unwrap_err();
        assert_eq!(reason, RejectReason::FeeLimit);
        assert!(detail.contains("emergency limit"), "{detail}");
    }

    #[test]
    fn risk_codes_map_onto_pipeline_reasons() {
        use RejectReason::*;
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::KillSwitch)),
            KillSwitch
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::ModuleDisabled)),
            StrategyDisabled
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::SniperEmergencyDisabled)),
            StrategyDisabled
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::SlippageCap)),
            SlippageLimit
        );
        for c in [
            RiskCode::MaxOpenPositions,
            RiskCode::ExposureCap,
            RiskCode::DailyLossLimit,
            RiskCode::SniperDailyLoss,
            RiskCode::SniperPendingCap,
        ] {
            assert_eq!(
                RejectReason::from_risk_code(Some(c)),
                ExposureLimit,
                "{c:?}"
            );
        }
        for c in [
            RiskCode::InvalidSize,
            RiskCode::DuplicateSymbol,
            RiskCode::ReentryCooldown,
            RiskCode::InsufficientBalance,
            RiskCode::VenueCheck,
            RiskCode::SniperTokenCooldown,
            RiskCode::SniperFailedEntryCooldown,
        ] {
            assert_eq!(RejectReason::from_risk_code(Some(c)), RiskRejected, "{c:?}");
        }
        assert_eq!(RejectReason::from_risk_code(None), RiskRejected);
    }

    #[test]
    fn transition_table_is_linear_with_terminal_exits() {
        use SniperStage::*;
        let forward = [
            (Detected, Validated),
            (Validated, RiskApproved),
            (RiskApproved, ExecutionReady),
            (ExecutionReady, Submitted),
            (Submitted, Confirmed),
        ];
        for (a, b) in forward {
            assert!(a.can_transition_to(b), "{a} -> {b}");
            assert!(!b.can_transition_to(a), "no going back {b} -> {a}");
        }
        // Skipping a stage is illegal.
        assert!(!Detected.can_transition_to(RiskApproved));
        assert!(!Validated.can_transition_to(ExecutionReady));
        assert!(!RiskApproved.can_transition_to(Submitted));
        assert!(!Detected.can_transition_to(Confirmed));
        // Rejection is legal before submission only.
        for s in [Detected, Validated, RiskApproved, ExecutionReady] {
            assert!(s.can_transition_to(Rejected), "{s}");
        }
        assert!(!Submitted.can_transition_to(Rejected));
        // Failure once something was built/sent.
        assert!(ExecutionReady.can_transition_to(Failed));
        assert!(Submitted.can_transition_to(Failed));
        assert!(!Validated.can_transition_to(Failed));
        // Terminal stages are sinks.
        for t in [Confirmed, Rejected, Failed] {
            assert!(t.is_terminal());
            for n in SniperStage::ALL {
                assert!(!t.can_transition_to(*n), "{t} -> {n}");
            }
        }
        assert_eq!(SniperStage::ALL.len(), 8);
        assert_eq!(
            serde_json::to_string(&RiskApproved).unwrap(),
            "\"RISK_APPROVED\""
        );
    }

    #[test]
    fn lifecycle_records_history_and_rejects_illegal_steps() {
        let t0 = Utc::now();
        let mut lc = Lifecycle::start(
            "evt_x",
            LaunchProtocol::PumpFun,
            t0 - Duration::milliseconds(40),
            t0 - Duration::milliseconds(10),
            t0,
        );
        assert_eq!(lc.stage, SniperStage::Detected);
        assert_eq!(lc.timeline.detection_ms(), Some(30));
        // Illegal skip → INVALID_STATE, state untouched.
        let err = lc.advance(SniperStage::RiskApproved, t0).unwrap_err();
        assert_eq!(err.reason, RejectReason::InvalidState);
        assert_eq!(err.stage, SniperStage::Detected);
        assert_eq!(lc.stage, SniperStage::Detected);
        assert!(err.to_string().contains("DETECTED -> RISK_APPROVED"));
        // Legal walk with timestamps.
        lc.advance(SniperStage::Validated, t0 + Duration::milliseconds(5))
            .unwrap();
        lc.advance(SniperStage::RiskApproved, t0 + Duration::milliseconds(25))
            .unwrap();
        lc.advance(SniperStage::ExecutionReady, t0 + Duration::milliseconds(45))
            .unwrap();
        lc.advance(SniperStage::Submitted, t0 + Duration::milliseconds(50))
            .unwrap();
        lc.advance(SniperStage::Confirmed, t0 + Duration::milliseconds(850))
            .unwrap();
        let tl = &lc.timeline;
        assert_eq!(tl.validation_ms(), Some(5));
        assert_eq!(tl.risk_ms(), Some(20));
        assert_eq!(tl.build_ms(), Some(20));
        assert_eq!(tl.submission_ms(), Some(5));
        assert_eq!(tl.confirmation_ms(), Some(800));
        assert_eq!(tl.total_ms(), Some(860));
        assert_eq!(
            lc.path(),
            "DETECTED>VALIDATED>RISK_APPROVED>EXECUTION_READY>SUBMITTED>CONFIRMED"
        );
        tl.record_metrics(LaunchProtocol::PumpFun);
        // Confirmed is terminal.
        assert!(lc.advance(SniperStage::Failed, t0).is_err());
    }

    #[test]
    fn lifecycle_reject_and_fail_pick_the_legal_terminal() {
        let t0 = Utc::now();
        let mut lc = Lifecycle::start("evt_y", LaunchProtocol::PumpSwap, t0, t0, t0);
        lc.advance(SniperStage::Validated, t0).unwrap();
        let r = lc.reject(RejectReason::StaleEvent, "too old", t0);
        assert_eq!(r.stage, SniperStage::Validated);
        assert_eq!(lc.stage, SniperStage::Rejected);
        assert_eq!(
            lc.rejection.as_ref().map(|r| r.reason),
            Some(RejectReason::StaleEvent)
        );
        assert_eq!(lc.timeline.total_ms(), Some(0));

        // After submission a "rejection" can only be a failure.
        let mut lc = Lifecycle::start("evt_z", LaunchProtocol::RaydiumAmmV4, t0, t0, t0);
        for s in [
            SniperStage::Validated,
            SniperStage::RiskApproved,
            SniperStage::ExecutionReady,
            SniperStage::Submitted,
        ] {
            lc.advance(s, t0).unwrap();
        }
        let r = lc.reject(RejectReason::KillSwitch, "mid-flight", t0);
        assert_eq!(r.stage, SniperStage::Submitted);
        assert_eq!(lc.stage, SniperStage::Failed);
        let r = lc.fail("already terminal", t0);
        assert_eq!(
            lc.stage,
            SniperStage::Failed,
            "fail on a terminal stage is a no-op"
        );
        assert_eq!(r.reason, RejectReason::ExecutionUnavailable);
    }

    #[test]
    fn route_selection_follows_protocol_and_config() {
        let mut cfg = SniperConfig::default();
        assert!(cfg.trade_pumpswap && cfg.trade_raydium && cfg.use_jupiter_fallback);
        assert_eq!(
            select_route(LaunchProtocol::PumpFun, &cfg, false),
            Ok(EntryRoute::PumpCurve)
        );
        assert_eq!(
            select_route(LaunchProtocol::PumpFun, &cfg, true),
            Ok(EntryRoute::PumpSwapDirect)
        );
        assert_eq!(
            select_route(LaunchProtocol::PumpSwap, &cfg, false),
            Ok(EntryRoute::PumpSwapDirect)
        );
        assert_eq!(
            select_route(LaunchProtocol::RaydiumAmmV4, &cfg, false),
            Ok(EntryRoute::RaydiumV4Direct)
        );

        cfg.trade_pumpswap = false;
        cfg.trade_raydium = false;
        assert_eq!(
            select_route(LaunchProtocol::PumpFun, &cfg, true),
            Ok(EntryRoute::Jupiter)
        );
        assert_eq!(
            select_route(LaunchProtocol::PumpSwap, &cfg, true),
            Ok(EntryRoute::Jupiter)
        );
        assert_eq!(
            select_route(LaunchProtocol::RaydiumAmmV4, &cfg, false),
            Ok(EntryRoute::Jupiter)
        );
        assert!(protocol_routable(LaunchProtocol::PumpSwap, &cfg).is_ok());

        cfg.use_jupiter_fallback = false;
        assert_eq!(
            select_route(LaunchProtocol::PumpFun, &cfg, false),
            Ok(EntryRoute::PumpCurve)
        );
        assert!(select_route(LaunchProtocol::PumpFun, &cfg, true).is_err());
        assert!(select_route(LaunchProtocol::PumpSwap, &cfg, false).is_err());
        assert!(select_route(LaunchProtocol::RaydiumAmmV4, &cfg, false).is_err());
        assert!(protocol_routable(LaunchProtocol::PumpFun, &cfg).is_ok());
        assert!(protocol_routable(LaunchProtocol::PumpSwap, &cfg).is_err());
        assert!(protocol_routable(LaunchProtocol::RaydiumAmmV4, &cfg).is_err());

        assert_eq!(
            EntryRoute::PumpSwapDirect.venue(),
            bot_core::models::Venue::PumpSwap
        );
        assert_eq!(
            EntryRoute::RaydiumV4Direct.venue(),
            bot_core::models::Venue::RaydiumAmmV4
        );
        assert_eq!(EntryRoute::Jupiter.to_string(), "jupiter");
        assert_eq!(EntryRoute::PumpCurve.as_str(), "curve");
    }

    #[test]
    fn precheck_orders_the_early_rejections() {
        use crate::event::LaunchEvent;
        use bot_core::models::{LaunchFeed, TokenLaunch};
        let now = Utc::now();
        let mint = solana_sdk::pubkey::Pubkey::new_unique().to_string();
        let launch = TokenLaunch {
            mint: mint.clone(),
            name: "Cat".into(),
            symbol: "CAT".into(),
            uri: None,
            creator: solana_sdk::pubkey::Pubkey::new_unique().to_string(),
            pool: "bonding-curve".into(),
            initial_buy_sol: 0.0,
            market_cap_sol: 30.0,
            market_cap_usd: None,
            total_supply: None,
            slot: Some(1),
            signature: None,
            tx_type: None,
            observed_at: now,
            feed: LaunchFeed::Manual,
            socials: None,
        };
        let event = LaunchEvent::from_token_launch(launch, 1, "raw".into());
        let cfg = SniperConfig::default();
        let ok = PrecheckContext {
            kill_switch: false,
            module_enabled: true,
            emergency_disable: false,
            symbol_gated: false,
        };
        assert_eq!(precheck(&event, &cfg, ok, now), Ok(()));

        let mut bad = event.clone();
        bad.event_id = "evt_forged".into();
        assert_eq!(
            precheck(&bad, &cfg, ok, now).unwrap_err().0,
            RejectReason::InvalidEvent
        );

        let mut no_route = cfg.clone();
        no_route.trade_raydium = false;
        no_route.use_jupiter_fallback = false;
        let mut ray = event.clone();
        ray.protocol = LaunchProtocol::RaydiumAmmV4;
        ray.pool = Some(solana_sdk::pubkey::Pubkey::new_unique().to_string());
        ray.event_id = ray.compute_event_id();
        assert_eq!(
            precheck(&ray, &no_route, ok, now).unwrap_err().0,
            RejectReason::InvalidRoute
        );

        assert_eq!(
            precheck(
                &event,
                &cfg,
                PrecheckContext {
                    kill_switch: true,
                    ..ok
                },
                now
            )
            .unwrap_err()
            .0,
            RejectReason::KillSwitch
        );
        assert_eq!(
            precheck(
                &event,
                &cfg,
                PrecheckContext {
                    module_enabled: false,
                    ..ok
                },
                now
            )
            .unwrap_err()
            .0,
            RejectReason::StrategyDisabled
        );
        assert_eq!(
            precheck(
                &event,
                &cfg,
                PrecheckContext {
                    emergency_disable: true,
                    ..ok
                },
                now
            )
            .unwrap_err()
            .0,
            RejectReason::StrategyDisabled
        );
        let later = now + Duration::seconds(cfg.max_launch_age_secs + 5);
        assert_eq!(
            precheck(&event, &cfg, ok, later).unwrap_err().0,
            RejectReason::StaleEvent
        );
        assert_eq!(
            precheck(
                &event,
                &cfg,
                PrecheckContext {
                    symbol_gated: true,
                    ..ok
                },
                now
            )
            .unwrap_err()
            .0,
            RejectReason::SymbolGated
        );
    }

    #[test]
    fn metric_helpers_do_not_panic_and_register_series() {
        let r = Rejection::new(RejectReason::DuplicateEvent, SniperStage::Detected, "seen");
        count_rejection(&r, LaunchProtocol::PumpFun);
        count_stage(SniperStage::Validated, LaunchProtocol::PumpSwap);
        count_event(LaunchProtocol::RaydiumAmmV4, "logsSubscribe");
        observe_slippage(1_500, "fixed");
        let text = metrics::global().encode();
        assert!(text.contains("sniper_rejections_total"));
        assert!(text.contains("sniper_stage_total"));
        assert!(text.contains("sniper_events_total"));
        assert!(text.contains("sniper_slippage_bps"));
        assert_eq!(r.to_string(), "DUPLICATE_EVENT at DETECTED: seen");
    }
}
