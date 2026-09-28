//! The canonical leader-trade event and the staged pipeline (TASK 3 §02).
//!
//! Every feed (`feeds.rs`: PumpPortal, `logs_poll`, `transaction_subscribe`)
//! decodes a leader's swap into a [`bot_core::models::WalletTrade`]. The
//! pipeline never works on that raw shape directly: it is lifted into a
//! [`LeaderTradeEvent`] which
//!
//! * carries a **deterministic identity** ([`LeaderTradeEvent::event_id`],
//!   derived from the leader, signature, mint and side) that is identical on
//!   every replica and across restarts — the durable journal, the dedup key
//!   and the audit trail all hang off it;
//! * records **where it came from** ([`EventSource`], per-source sequence) so
//!   ordering gaps and duplicate deliveries can be attributed to a feed;
//! * separates the **chain time** (`block_time`) from the **local time**
//!   (`observed_at`) so staleness is measured against the chain when the
//!   source provides it and detection lag can be metered;
//! * is **shape-validated** once ([`LeaderTradeEvent::validate`]) so no later
//!   stage has to defend against NaN sizes, empty mints or future timestamps.
//!
//! The event, the stage vocabulary ([`CopyStage`], 15 stages), the rejection
//! vocabulary ([`RejectReason`], 29 reasons) and the [`CopyOutcome`] are pure
//! data. The second half of this file is [`CopyBot::process_event`], the
//! staged pipeline that walks one event through them:
//!
//! ```text
//! RECEIVED → VALIDATED → LEADER_RESOLVED → DEDUPLICATED → ORDERED → POLICY_PASSED
//!          → SIZED → RISK_APPROVED → OWNERSHIP_CLAIMED → SUBMITTED → FILLED | AMBIGUOUS
//!                                                                    (or EXIT_MIRRORED)
//! ```
//!
//! Every stage either advances or ends the event with a machine-readable
//! [`Rejection`] (`REJECTED` / `FAILED`). The stages are thin: shape
//! validation is [`LeaderTradeEvent::validate`], the one authoritative dedup
//! is `event_dedup.rs`, ordering is `event_ordering.rs`, the mirror policy is
//! `policy.rs`, sizing is `sizing.rs`, intent identity is `intent.rs`, and
//! the one authoritative risk decision is `bot_core::risk::RiskEngine`
//! (`check_copy_coded` + `check_entry`). Execution is unchanged and lives in
//! `mirror.rs` (bonding-curve buy while the token is on the curve, otherwise
//! a Jupiter swap, both through the shared executor / execution ledger with
//! a deterministic intent id, the write-ahead intent journal and the
//! cross-replica ownership permit); leader **sells** are mirrored through
//! `exit.rs::sell_position` when `copy.mirror_exits` is on. The pre-TASK-3
//! door `CopyBot::mirror_trade` (`mirror.rs`) wraps the raw trade and runs
//! this same pipeline.

use bot_core::config::Config;
use bot_core::config::CopyWallet;
use bot_core::events::AppEvent;
use bot_core::models::{BotModule, Position, PositionSide, Venue, WalletTrade};
use bot_core::risk::EntryRequest;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::audit;
use crate::event_dedup::{self, DedupOutcome};
use crate::metrics::{self, LatencyTimeline};
use crate::policy::{self, ExitPlan, PolicyContext, PolicyVerdict};
use crate::sizing;
use crate::CopyBot;

/// Which feed produced an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// PumpPortal websocket trade stream (`copy.feed = "pumpportal"`).
    PumpPortal,
    /// `getSignaturesForAddress` polling (`copy.feed = "logs_poll"`).
    LogsPoll,
    /// Geyser / Helius `transactionSubscribe` (`copy.feed = "transaction_subscribe"`).
    TransactionSubscribe,
    /// Replayed from the durable journal or a fixture — never live.
    Replay,
    /// Injected by an operator or a test.
    Manual,
}

impl EventSource {
    /// Stable label (metric label, journal column).
    pub fn as_str(&self) -> &'static str {
        match self {
            EventSource::PumpPortal => "pumpportal",
            EventSource::LogsPoll => "logs_poll",
            EventSource::TransactionSubscribe => "transaction_subscribe",
            EventSource::Replay => "replay",
            EventSource::Manual => "manual",
        }
    }

    /// Map the `[copy].feed` setting onto a source. Unknown feed names are
    /// attributed to `Manual` so an unexpected value is visible in metrics
    /// instead of silently borrowing a real feed's label.
    pub fn from_feed(feed: &str) -> EventSource {
        match feed.trim().to_ascii_lowercase().as_str() {
            "pumpportal" => EventSource::PumpPortal,
            "logs_poll" | "logs" | "poll" => EventSource::LogsPoll,
            "transaction_subscribe" | "geyser" => EventSource::TransactionSubscribe,
            "replay" => EventSource::Replay,
            _ => EventSource::Manual,
        }
    }

    /// Whether events from this source may reach the executor. Replayed
    /// events are analysed, journaled and metered but NEVER traded.
    pub fn may_execute(&self) -> bool {
        !matches!(self, EventSource::Replay)
    }

    /// Inverse of [`EventSource::as_str`].
    pub fn parse(s: &str) -> Option<EventSource> {
        Some(match s {
            "pumpportal" => EventSource::PumpPortal,
            "logs_poll" => EventSource::LogsPoll,
            "transaction_subscribe" => EventSource::TransactionSubscribe,
            "replay" => EventSource::Replay,
            "manual" => EventSource::Manual,
            _ => return None,
        })
    }
}

/// Why an event failed shape validation. Each variant is a distinct,
/// machine-readable defect so the rejection metric can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventDefect {
    /// The leader address is empty.
    EmptyLeader,
    /// The transaction signature is empty.
    EmptySignature,
    /// The mint is empty.
    EmptyMint,
    /// The mint equals the leader address (a decoder mixed up accounts).
    MintIsLeader,
    /// `token_amount` or `sol_amount` is NaN / infinite.
    NonFiniteAmount,
    /// `token_amount` or `sol_amount` is negative.
    NegativeAmount,
    /// Both amounts are zero — nothing was traded.
    ZeroSize,
    /// `fee_sol` is NaN / infinite / negative.
    InvalidFee,
    /// `block_time` is more than one minute in the future.
    FutureTimestamp,
    /// The venue cannot carry a Solana token swap (Polymarket).
    ForeignVenue,
}

impl EventDefect {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventDefect::EmptyLeader => "empty_leader",
            EventDefect::EmptySignature => "empty_signature",
            EventDefect::EmptyMint => "empty_mint",
            EventDefect::MintIsLeader => "mint_is_leader",
            EventDefect::NonFiniteAmount => "non_finite_amount",
            EventDefect::NegativeAmount => "negative_amount",
            EventDefect::ZeroSize => "zero_size",
            EventDefect::InvalidFee => "invalid_fee",
            EventDefect::FutureTimestamp => "future_timestamp",
            EventDefect::ForeignVenue => "foreign_venue",
        }
    }
}

impl std::fmt::Display for EventDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tolerance for `block_time` ahead of the local clock before an event is
/// considered malformed (clock skew between the node and this host).
pub const FUTURE_SKEW_TOLERANCE_SECS: i64 = 60;

/// One decoded swap made by a followed leader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderTradeEvent {
    /// Deterministic id — see [`LeaderTradeEvent::compute_event_id`].
    pub event_id: String,
    /// Leader wallet (base58).
    pub leader: String,
    /// Transaction signature of the leader's swap.
    pub signature: String,
    /// Slot of the leader's transaction (`0` when the source had none).
    pub slot: u64,
    /// Chain time when the source provides it.
    pub block_time: Option<DateTime<Utc>>,
    /// `Long` = the leader bought `mint`, `Short` = the leader sold it.
    pub side: PositionSide,
    /// Token mint traded.
    pub mint: String,
    /// Token symbol when the decoder knew it.
    pub symbol: Option<String>,
    /// Venue the leader traded on.
    pub venue: Venue,
    /// Tokens the leader traded (UI units, best effort).
    pub token_amount: f64,
    /// SOL the leader spent / received.
    pub sol_amount: f64,
    /// Transaction fee the leader paid, in SOL.
    pub fee_sol: f64,
    /// Instruction discriminator the decoder matched, when known.
    pub discriminator: Option<String>,
    /// Feed that produced the event.
    pub source: EventSource,
    /// Monotonic per-source delivery counter assigned by the pipeline; `0`
    /// when the producer did not number its deliveries.
    pub source_sequence: u64,
    /// When THIS process first saw the event.
    pub observed_at: DateTime<Utc>,
}

impl LeaderTradeEvent {
    /// Lift a decoded feed trade into the canonical event.
    pub fn from_wallet_trade(
        trade: &WalletTrade,
        source: EventSource,
        source_sequence: u64,
    ) -> LeaderTradeEvent {
        let event_id =
            Self::compute_event_id(&trade.wallet, &trade.signature, &trade.mint, trade.side);
        LeaderTradeEvent {
            event_id,
            leader: trade.wallet.clone(),
            signature: trade.signature.clone(),
            slot: trade.slot,
            block_time: trade.block_time,
            side: trade.side,
            mint: trade.mint.clone(),
            symbol: trade.symbol.clone(),
            venue: trade.venue,
            token_amount: trade.token_amount,
            sol_amount: trade.sol_amount,
            fee_sol: trade.fee_sol,
            discriminator: trade.discriminator.clone(),
            source,
            source_sequence,
            observed_at: trade.observed_at,
        }
    }

    /// The raw shape the existing mirror / exit code and the risk engine's
    /// `check_copy` consume. Lossless for every field they read.
    pub fn to_wallet_trade(&self) -> WalletTrade {
        WalletTrade {
            wallet: self.leader.clone(),
            signature: self.signature.clone(),
            slot: self.slot,
            block_time: self.block_time,
            side: self.side,
            mint: self.mint.clone(),
            symbol: self.symbol.clone(),
            token_amount: self.token_amount,
            sol_amount: self.sol_amount,
            venue: self.venue,
            fee_sol: self.fee_sol,
            discriminator: self.discriminator.clone(),
            observed_at: self.observed_at,
        }
    }

    /// `cev_` + 32 hex chars of the versioned SHA-256 over
    /// `leader | signature | mint | side`. One leader swap that touches two
    /// mints (a token-for-token route) yields two events; a duplicate
    /// delivery of the same swap yields the same id.
    pub fn compute_event_id(
        leader: &str,
        signature: &str,
        mint: &str,
        side: PositionSide,
    ) -> String {
        let raw = bot_core::execution::intent_id(&[
            "copy-event",
            leader.trim(),
            signature.trim(),
            mint.trim(),
            side.as_str(),
        ]);
        format!("cev_{}", raw.trim_start_matches("int_"))
    }

    /// The one authoritative dedup key (`copy:{signature}:{leader}:{mint}:{side}`).
    /// Human-readable on purpose: it appears in logs and the durable journal.
    pub fn dedup_key(&self) -> String {
        format!(
            "copy:{}:{}:{}:{}",
            self.signature.trim(),
            self.leader.trim(),
            self.mint.trim(),
            self.side_str()
        )
    }

    /// `buy` for a leader purchase, `sell` for a leader sale.
    pub fn side_str(&self) -> &'static str {
        match self.side {
            PositionSide::Long => "buy",
            PositionSide::Short => "sell",
        }
    }

    /// Whether the leader bought (`Long`).
    pub fn is_buy(&self) -> bool {
        self.side == PositionSide::Long
    }

    /// Best available event time: chain time, else local observation.
    pub fn event_at(&self) -> DateTime<Utc> {
        self.block_time.unwrap_or(self.observed_at)
    }

    /// Seconds between the event time and `now` (negative = clock ahead).
    pub fn age_secs(&self, now: DateTime<Utc>) -> i64 {
        now.signed_duration_since(self.event_at()).num_seconds()
    }

    /// Milliseconds between chain time and local observation, when chain
    /// time is known — the detection lag of the feed.
    pub fn detection_lag_ms(&self) -> Option<i64> {
        self.block_time.map(|bt| {
            self.observed_at
                .signed_duration_since(bt)
                .num_milliseconds()
        })
    }

    /// Whether the event is older than `max_age_secs` (`0` = never stale).
    pub fn is_stale(&self, now: DateTime<Utc>, max_age_secs: i64) -> bool {
        max_age_secs > 0 && self.age_secs(now) > max_age_secs
    }

    /// Structural validation. Runs once at the top of the pipeline; every
    /// later stage may assume a valid shape.
    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), EventDefect> {
        if self.leader.trim().is_empty() {
            return Err(EventDefect::EmptyLeader);
        }
        if self.signature.trim().is_empty() {
            return Err(EventDefect::EmptySignature);
        }
        if self.mint.trim().is_empty() {
            return Err(EventDefect::EmptyMint);
        }
        if self.mint.trim() == self.leader.trim() {
            return Err(EventDefect::MintIsLeader);
        }
        if !self.token_amount.is_finite() || !self.sol_amount.is_finite() {
            return Err(EventDefect::NonFiniteAmount);
        }
        if self.token_amount < 0.0 || self.sol_amount < 0.0 {
            return Err(EventDefect::NegativeAmount);
        }
        if self.token_amount == 0.0 && self.sol_amount == 0.0 {
            return Err(EventDefect::ZeroSize);
        }
        if !self.fee_sol.is_finite() || self.fee_sol < 0.0 {
            return Err(EventDefect::InvalidFee);
        }
        if let Some(bt) = self.block_time {
            if bt > now + Duration::seconds(FUTURE_SKEW_TOLERANCE_SECS) {
                return Err(EventDefect::FutureTimestamp);
            }
        }
        if self.venue == Venue::PolymarketClob {
            return Err(EventDefect::ForeignVenue);
        }
        Ok(())
    }

    /// Short human label for logs: `whale…1234 buy MINT…5678 0.5 SOL`.
    pub fn describe(&self) -> String {
        format!(
            "{} {} {} {:.4} SOL via {} ({})",
            short(&self.leader),
            self.side_str(),
            short(&self.mint),
            self.sol_amount,
            self.venue.as_str(),
            self.source.as_str()
        )
    }
}

/// `abcd…wxyz` for long base58 strings, the string itself when short.
pub fn short(s: &str) -> String {
    if s.len() > 10 {
        format!("{}…{}", &s[..4], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Pipeline vocabulary: stages, rejection reasons, outcome
// ---------------------------------------------------------------------------

/// Where an event is in — or where it left — the copy pipeline. The order of
/// the variants is the order of the stages; terminal stages come last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CopyStage {
    /// Handed over by a feed.
    Received,
    /// Shape validation passed.
    Validated,
    /// The leader is followed (known to the registry).
    LeaderResolved,
    /// The authoritative dedup accepted the event as new.
    Deduplicated,
    /// Ordering check passed (or was advisory).
    Ordered,
    /// Copy policy allows mirroring.
    PolicyPassed,
    /// A positive mirror size was computed.
    Sized,
    /// Both risk gates approved.
    RiskApproved,
    /// This replica owns the entry.
    OwnershipClaimed,
    /// Handed to the execution engine.
    Submitted,
    /// Terminal: a mirrored position was booked.
    Filled,
    /// Terminal: a leader exit was mirrored onto our position.
    ExitMirrored,
    /// Terminal: refused by a pipeline rule (see [`RejectReason`]).
    Rejected,
    /// Terminal: an error before or during execution; nothing is known to be
    /// on chain.
    Failed,
    /// Terminal: broadcast happened but the landing is unproven — the
    /// position is booked and the ownership claim parks for reconciliation.
    Ambiguous,
}

impl CopyStage {
    /// Stable label for metrics, journal rows and the audit trail.
    pub fn as_str(&self) -> &'static str {
        match self {
            CopyStage::Received => "RECEIVED",
            CopyStage::Validated => "VALIDATED",
            CopyStage::LeaderResolved => "LEADER_RESOLVED",
            CopyStage::Deduplicated => "DEDUPLICATED",
            CopyStage::Ordered => "ORDERED",
            CopyStage::PolicyPassed => "POLICY_PASSED",
            CopyStage::Sized => "SIZED",
            CopyStage::RiskApproved => "RISK_APPROVED",
            CopyStage::OwnershipClaimed => "OWNERSHIP_CLAIMED",
            CopyStage::Submitted => "SUBMITTED",
            CopyStage::Filled => "FILLED",
            CopyStage::ExitMirrored => "EXIT_MIRRORED",
            CopyStage::Rejected => "REJECTED",
            CopyStage::Failed => "FAILED",
            CopyStage::Ambiguous => "AMBIGUOUS",
        }
    }

    /// Whether the stage ends the event's journey.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            CopyStage::Filled
                | CopyStage::ExitMirrored
                | CopyStage::Rejected
                | CopyStage::Failed
                | CopyStage::Ambiguous
        )
    }

    /// Terminal stages that mean "a position was (or may have been) opened".
    pub fn opened_exposure(&self) -> bool {
        matches!(self, CopyStage::Filled | CopyStage::Ambiguous)
    }

    /// Every stage, in pipeline order (dashboards, tests).
    pub const ALL: [CopyStage; 15] = [
        CopyStage::Received,
        CopyStage::Validated,
        CopyStage::LeaderResolved,
        CopyStage::Deduplicated,
        CopyStage::Ordered,
        CopyStage::PolicyPassed,
        CopyStage::Sized,
        CopyStage::RiskApproved,
        CopyStage::OwnershipClaimed,
        CopyStage::Submitted,
        CopyStage::Filled,
        CopyStage::ExitMirrored,
        CopyStage::Rejected,
        CopyStage::Failed,
        CopyStage::Ambiguous,
    ];
}

impl std::fmt::Display for CopyStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Machine-readable reason an event was refused. One reason per rule so
/// operators can see WHICH rule fires in `copy_rejections_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RejectReason {
    /// Shape validation failed (see the detail for the [`EventDefect`]).
    InvalidEvent,
    /// The authoritative dedup already saw this event.
    DuplicateEvent,
    /// `copy.strict_ordering` and the event is behind the leader's cursor.
    OutOfOrder,
    /// The leader is not in the registry.
    LeaderUnknown,
    /// The leader is paused by the operator.
    LeaderPaused,
    /// The leader was unfollowed.
    LeaderRemoved,
    /// The event came from a replay source — analysed, never traded.
    ReplayOnly,
    /// The wallet rule is `buys_only`; leader sells are not mirrored.
    SellsNotMirrored,
    /// `copy.mirror_exits` is off.
    MirrorExitsDisabled,
    /// The leader sold a mint we do not hold.
    NoPositionToExit,
    /// The venue decoder is disabled in `[copy]`.
    VenueDisabled,
    /// The leader's buy is below the wallet rule's `min_sol`.
    BelowLeaderMin,
    /// Older than the wallet rule or the global `max_event_age_secs` allows.
    StaleEvent,
    /// Entry refused while the symbol has unresolved reconciliation claims.
    SymbolGated,
    /// `copy.skip_if_sniper_holds` and the sniper holds the mint.
    SniperHolds,
    /// We already hold a copy position in this mint.
    AlreadyMirroring,
    /// The computed mirror size is not positive.
    ZeroSize,
    /// The computed mirror size is under `copy.min_mirror_sol`.
    DustSize,
    /// The spendable balance could not be read.
    BalanceUnavailable,
    /// Kill switch engaged.
    KillSwitch,
    /// Module disabled or `copy_emergency_disable`.
    StrategyDisabled,
    /// Per-leader-per-mint copy cooldown.
    CopyCooldown,
    /// Per-leader exposure / open-position cap.
    LeaderExposure,
    /// Any risk-engine exposure limit (positions, envelope, daily loss, pending cap).
    ExposureLimit,
    /// Slippage above the risk cap.
    SlippageLimit,
    /// Any other risk-engine rejection.
    RiskRejected,
    /// The ownership store was unavailable (fail closed).
    OwnershipUnavailable,
    /// Another replica owns the entry / exit.
    OwnershipLost,
    /// Build / quote / broadcast failed or the order did not fill.
    ExecutionFailed,
}

impl RejectReason {
    /// Stable SCREAMING_SNAKE label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectReason::InvalidEvent => "INVALID_EVENT",
            RejectReason::DuplicateEvent => "DUPLICATE_EVENT",
            RejectReason::OutOfOrder => "OUT_OF_ORDER",
            RejectReason::LeaderUnknown => "LEADER_UNKNOWN",
            RejectReason::LeaderPaused => "LEADER_PAUSED",
            RejectReason::LeaderRemoved => "LEADER_REMOVED",
            RejectReason::ReplayOnly => "REPLAY_ONLY",
            RejectReason::SellsNotMirrored => "SELLS_NOT_MIRRORED",
            RejectReason::MirrorExitsDisabled => "MIRROR_EXITS_DISABLED",
            RejectReason::NoPositionToExit => "NO_POSITION_TO_EXIT",
            RejectReason::VenueDisabled => "VENUE_DISABLED",
            RejectReason::BelowLeaderMin => "BELOW_LEADER_MIN",
            RejectReason::StaleEvent => "STALE_EVENT",
            RejectReason::SymbolGated => "SYMBOL_GATED",
            RejectReason::SniperHolds => "SNIPER_HOLDS",
            RejectReason::AlreadyMirroring => "ALREADY_MIRRORING",
            RejectReason::ZeroSize => "ZERO_SIZE",
            RejectReason::DustSize => "DUST_SIZE",
            RejectReason::BalanceUnavailable => "BALANCE_UNAVAILABLE",
            RejectReason::KillSwitch => "KILL_SWITCH",
            RejectReason::StrategyDisabled => "STRATEGY_DISABLED",
            RejectReason::CopyCooldown => "COPY_COOLDOWN",
            RejectReason::LeaderExposure => "LEADER_EXPOSURE",
            RejectReason::ExposureLimit => "EXPOSURE_LIMIT",
            RejectReason::SlippageLimit => "SLIPPAGE_LIMIT",
            RejectReason::RiskRejected => "RISK_REJECTED",
            RejectReason::OwnershipUnavailable => "OWNERSHIP_UNAVAILABLE",
            RejectReason::OwnershipLost => "OWNERSHIP_LOST",
            RejectReason::ExecutionFailed => "EXECUTION_FAILED",
        }
    }

    /// Map a risk-engine code onto the pipeline vocabulary. The risk engine
    /// remains the authority; this only chooses the label.
    pub fn from_risk_code(code: Option<bot_core::risk::RiskCode>) -> RejectReason {
        use bot_core::risk::RiskCode;
        match code {
            Some(RiskCode::KillSwitch) | Some(RiskCode::GlobalKillSwitch) => {
                RejectReason::KillSwitch
            }
            Some(RiskCode::ModuleDisabled) | Some(RiskCode::CopyEmergencyDisabled) => {
                RejectReason::StrategyDisabled
            }
            Some(RiskCode::SlippageCap) => RejectReason::SlippageLimit,
            Some(RiskCode::CopyCooldown) => RejectReason::CopyCooldown,
            Some(RiskCode::CopyLeaderExposure) => RejectReason::LeaderExposure,
            Some(RiskCode::StaleSignal) => RejectReason::StaleEvent,
            Some(c) if c.is_exposure_limit() => RejectReason::ExposureLimit,
            _ => RejectReason::RiskRejected,
        }
    }

    /// Whether the rejection is expected steady-state noise (duplicates,
    /// unknown leaders, policy skips) rather than something an operator
    /// should look at.
    pub fn is_routine(&self) -> bool {
        matches!(
            self,
            RejectReason::DuplicateEvent
                | RejectReason::LeaderUnknown
                | RejectReason::SellsNotMirrored
                | RejectReason::MirrorExitsDisabled
                | RejectReason::NoPositionToExit
                | RejectReason::BelowLeaderMin
                | RejectReason::AlreadyMirroring
                | RejectReason::CopyCooldown
                | RejectReason::OwnershipLost
                | RejectReason::ReplayOnly
        )
    }

    /// Every reason (dashboards, tests).
    pub const ALL: [RejectReason; 29] = [
        RejectReason::InvalidEvent,
        RejectReason::DuplicateEvent,
        RejectReason::OutOfOrder,
        RejectReason::LeaderUnknown,
        RejectReason::LeaderPaused,
        RejectReason::LeaderRemoved,
        RejectReason::ReplayOnly,
        RejectReason::SellsNotMirrored,
        RejectReason::MirrorExitsDisabled,
        RejectReason::NoPositionToExit,
        RejectReason::VenueDisabled,
        RejectReason::BelowLeaderMin,
        RejectReason::StaleEvent,
        RejectReason::SymbolGated,
        RejectReason::SniperHolds,
        RejectReason::AlreadyMirroring,
        RejectReason::ZeroSize,
        RejectReason::DustSize,
        RejectReason::BalanceUnavailable,
        RejectReason::KillSwitch,
        RejectReason::StrategyDisabled,
        RejectReason::CopyCooldown,
        RejectReason::LeaderExposure,
        RejectReason::ExposureLimit,
        RejectReason::SlippageLimit,
        RejectReason::RiskRejected,
        RejectReason::OwnershipUnavailable,
        RejectReason::OwnershipLost,
        RejectReason::ExecutionFailed,
    ];
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A rejection: the reason, the stage that raised it and a human detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// Machine-readable reason.
    pub reason: RejectReason,
    /// Stage that raised it.
    pub stage: CopyStage,
    /// Human explanation (never parsed).
    pub detail: String,
}

impl Rejection {
    /// Build a rejection.
    pub fn new(reason: RejectReason, stage: CopyStage, detail: impl Into<String>) -> Self {
        Rejection {
            reason,
            stage,
            detail: detail.into(),
        }
    }
}

/// What the pipeline did with one event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyOutcome {
    /// The event's deterministic id.
    pub event_id: String,
    /// Terminal stage reached.
    pub stage: CopyStage,
    /// Set when `stage` is `Rejected` / `Failed` (and for routine skips).
    pub rejection: Option<Rejection>,
    /// Execution-ledger intent id when the event reached the executor.
    pub intent_id: Option<String>,
    /// Position booked (entry) or acted on (exit).
    pub position_id: Option<String>,
    /// Mirror size the sizing stage computed, in SOL.
    pub requested_sol: Option<f64>,
    /// Size the risk engine approved, in SOL.
    pub sized_sol: Option<f64>,
    /// Broadcast signature when known.
    pub signature: Option<String>,
    /// Wall-clock milliseconds from `observed_at` to the terminal stage.
    pub total_ms: u64,
}

impl CopyOutcome {
    /// Outcome for an event that was refused.
    pub fn rejected(event_id: &str, rejection: Rejection, total_ms: u64) -> Self {
        let stage = if rejection.reason == RejectReason::ExecutionFailed
            || rejection.reason == RejectReason::BalanceUnavailable
            || rejection.reason == RejectReason::OwnershipUnavailable
        {
            CopyStage::Failed
        } else {
            CopyStage::Rejected
        };
        CopyOutcome {
            event_id: event_id.to_string(),
            stage,
            rejection: Some(rejection),
            intent_id: None,
            position_id: None,
            requested_sol: None,
            sized_sol: None,
            signature: None,
            total_ms,
        }
    }

    /// Whether the event opened (or may have opened) exposure.
    pub fn opened(&self) -> bool {
        self.stage.opened_exposure()
    }

    /// Reason label or the stage label when there is no rejection.
    pub fn reason_label(&self) -> &'static str {
        self.rejection
            .as_ref()
            .map(|r| r.reason.as_str())
            .unwrap_or_else(|| self.stage.as_str())
    }
}

// ---------------------------------------------------------------------------
// The staged pipeline: CopyBot::process_event
// ---------------------------------------------------------------------------

/// Short display form of an address for dashboard / Telegram events (first
/// 8 characters — the pre-TASK-3 `mirror::short` convention those consumers
/// already parse; distinct from [`short`], the log form).
fn display_short(addr: &str) -> String {
    crate::mirror::short(addr)
}

impl CopyBot {
    /// The staged copy pipeline for one leader-trade event.
    pub async fn process_event(&mut self, event: &LeaderTradeEvent, cfg: &Config) -> CopyOutcome {
        let mut tl = LatencyTimeline::start(event.observed_at);
        metrics::count_event(event.source, event.side_str());
        metrics::count_stage(CopyStage::Received);
        metrics::observe_detection_lag(event);

        // ---- 1. shape ------------------------------------------------------
        if let Err(defect) = event.validate(Utc::now()) {
            return self
                .finish_rejected(
                    event,
                    None,
                    Rejection::new(
                        RejectReason::InvalidEvent,
                        CopyStage::Received,
                        format!("malformed event: {defect}"),
                    ),
                    &mut tl,
                )
                .await;
        }
        tl.mark("validated");
        metrics::count_stage(CopyStage::Validated);

        // ---- 2. leader -----------------------------------------------------
        let leader = {
            let reg = self.leaders.read().await;
            reg.rule_for(&event.leader).map(|(r, s)| (r.clone(), s))
        };
        let Some((rule, status)) = leader else {
            debug!(wallet = %event.leader, "trade from untracked wallet, ignoring");
            return self
                .finish_rejected(
                    event,
                    None,
                    Rejection::new(
                        RejectReason::LeaderUnknown,
                        CopyStage::Validated,
                        format!("{} is not a followed leader", event.leader),
                    ),
                    &mut tl,
                )
                .await;
        };
        {
            let mut reg = self.leaders.write().await;
            reg.note_event(&event.leader, event.slot, event.observed_at);
        }
        let label = crate::leader::label_for(&rule);
        metrics::count_stage(CopyStage::LeaderResolved);

        // ---- 3. the one authoritative dedup --------------------------------
        if event_dedup::claim(&self.state, event).await == DedupOutcome::Duplicate {
            debug!(event = %event.event_id, source = event.source.as_str(), "duplicate leader event");
            return self
                .finish_rejected(
                    event,
                    Some(&rule),
                    Rejection::new(
                        RejectReason::DuplicateEvent,
                        CopyStage::LeaderResolved,
                        format!("already decided (dedup key {})", event.dedup_key()),
                    ),
                    &mut tl,
                )
                .await;
        }
        // Surface every tracked trade for the dashboard/telegram — once.
        self.state.events.publish(AppEvent::WalletTrade {
            ts: Utc::now(),
            trade: Box::new(event.to_wallet_trade()),
        });
        tl.mark("deduplicated");
        metrics::count_stage(CopyStage::Deduplicated);

        // ---- 4. ordering ---------------------------------------------------
        let report = self.ordering.observe(event);
        metrics::count_ordering(report.verdict.as_str());
        if report.sequence_gap > 0 {
            metrics::count_ordering("gap");
            warn!(
                leader = %label,
                source = event.source.as_str(),
                gap = report.sequence_gap,
                "leader feed skipped deliveries — reconciliation will compare leader activity"
            );
        }
        if report.reject {
            return self
                .finish_rejected(
                    event,
                    Some(&rule),
                    Rejection::new(
                        RejectReason::OutOfOrder,
                        CopyStage::Deduplicated,
                        format!(
                            "slot {} is behind the leader cursor ({})",
                            event.slot,
                            self.ordering
                                .cursor(&event.leader)
                                .map(|c| c.last_slot)
                                .unwrap_or(0)
                        ),
                    ),
                    &mut tl,
                )
                .await;
        }
        tl.mark("ordered");
        metrics::count_stage(CopyStage::Ordered);

        // ---- 5. policy -----------------------------------------------------
        let held = self.state.find_open(BotModule::Copy, &event.mint).await;
        let ctx = PolicyContext {
            now: Utc::now(),
            symbol_blocked: self.state.is_symbol_blocked(&event.mint).await,
            sniper_holds: self
                .state
                .find_open(BotModule::Sniper, &event.mint)
                .await
                .is_some(),
            holding: held.is_some(),
            held_qty: held.as_ref().map(|p| p.qty).unwrap_or(0.0),
        };
        let plan = match policy::evaluate(event, Some((&rule, status)), &cfg.copy, &ctx) {
            PolicyVerdict::Skip(rejection) => {
                if rejection.reason == RejectReason::SymbolGated {
                    bot_core::obs::metrics::global()
                        .counter(
                            "bot_symbol_gated_entries_total",
                            "Entries refused because the symbol is gated by unresolved reconciliation.",
                            &[("module", "copy")],
                        )
                        .inc();
                }
                debug!(wallet = %label, mint = %event.mint, reason = %rejection.reason, detail = %rejection.detail, "copy policy skip");
                return self
                    .finish_rejected(event, Some(&rule), rejection, &mut tl)
                    .await;
            }
            PolicyVerdict::Exit(plan) => {
                let Some(position) = held else {
                    return self
                        .finish_rejected(
                            event,
                            Some(&rule),
                            Rejection::new(
                                RejectReason::NoPositionToExit,
                                CopyStage::PolicyPassed,
                                format!("leader sold {} but we do not hold it", event.mint),
                            ),
                            &mut tl,
                        )
                        .await;
                };
                tl.mark("policy");
                metrics::count_stage(CopyStage::PolicyPassed);
                return self
                    .mirror_exit(event, &rule, &plan, position, &mut tl)
                    .await;
            }
            PolicyVerdict::Enter(plan) => plan,
        };
        tl.mark("policy");
        metrics::count_stage(CopyStage::PolicyPassed);

        // ---- 6. sizing -----------------------------------------------------
        let available = match self.available_sol().await {
            Ok(v) => v,
            Err(e) => {
                return self
                    .finish_rejected(
                        event,
                        Some(&rule),
                        Rejection::new(
                            RejectReason::BalanceUnavailable,
                            CopyStage::PolicyPassed,
                            format!("spendable balance unavailable: {e}"),
                        ),
                        &mut tl,
                    )
                    .await;
            }
        };
        let size = match sizing::size_mirror(&rule, &cfg.copy, event.sol_amount, Some(available)) {
            Ok(s) => s,
            Err(err) => {
                let reason = match err {
                    sizing::SizingError::Dust { .. } => RejectReason::DustSize,
                    _ => RejectReason::ZeroSize,
                };
                debug!(wallet = %label, %err, "computed mirror size unusable");
                return self
                    .finish_rejected(
                        event,
                        Some(&rule),
                        Rejection::new(reason, CopyStage::PolicyPassed, err.to_string()),
                        &mut tl,
                    )
                    .await;
            }
        };
        let requested = size.requested_sol;
        tl.mark("sized");
        metrics::count_stage(CopyStage::Sized);

        // ---- 7. risk gate 1: copy-specific (preflight, cooldown, staleness,
        //         leader exposure) — the risk engine decides, we label. ------
        let risk_cfg = cfg.risk.clone();
        let trade = event.to_wallet_trade();
        if let Err((code, reason)) = self
            .risk
            .check_copy_coded(
                &trade,
                &risk_cfg,
                requested,
                Some((rule.max_exposure_sol, rule.max_open_positions)),
            )
            .await
        {
            debug!(wallet = %label, mint = %event.mint, %reason, "copy gated");
            return self
                .finish_rejected(
                    event,
                    Some(&rule),
                    Rejection::new(
                        RejectReason::from_risk_code(Some(code)),
                        CopyStage::Sized,
                        reason,
                    ),
                    &mut tl,
                )
                .await;
        }

        // ---- 8. risk gate 2: sizing + hard limits ------------------------
        let slippage_pct = plan.slippage_pct;
        let slippage_bps = (slippage_pct * 100.0).round() as u64;
        let decision = self
            .risk
            .check_entry(&EntryRequest {
                module: BotModule::Copy,
                venue: event.venue,
                symbol: event.mint.clone(),
                symbol_display: event
                    .symbol
                    .clone()
                    .unwrap_or_else(|| display_short(&event.mint)),
                requested_quote: requested,
                available_quote: available,
                slippage_bps,
                price: None,
                fair_value: None,
                liquidity: None,
                // TASK 5 — attribution for the global layer: our wallet and
                // the leader-scoped strategy label (`copy:<leader>`).
                wallet: self.wallet.pubkey.to_string(),
                strategy: bot_core::global_risk::strategy_label(
                    BotModule::Copy,
                    Some(&event.leader),
                ),
            })
            .await;
        if !decision.allowed() {
            self.state.inc_risk_rejected(BotModule::Copy).await;
            self.state.events.publish(AppEvent::RiskRejected {
                ts: Utc::now(),
                module: BotModule::Copy,
                symbol: display_short(&event.mint),
                reason: decision.reason.clone(),
            });
            info!(mint = %event.mint, reason = %decision.reason, "copy entry rejected by risk");
            return self
                .finish_rejected(
                    event,
                    Some(&rule),
                    Rejection::new(
                        RejectReason::from_risk_code(decision.code),
                        CopyStage::Sized,
                        decision.reason.clone(),
                    ),
                    &mut tl,
                )
                .await;
        }
        let sized = decision.sized_quote;
        tl.mark("risk");
        metrics::count_stage(CopyStage::RiskApproved);
        metrics::observe_mirror_size(sized);

        self.state.inc_signals(BotModule::Copy).await;
        self.state.events.publish(AppEvent::Signal {
            ts: Utc::now(),
            module: BotModule::Copy,
            symbol: display_short(&event.mint),
            side: "buy".into(),
            reason: format!("mirror {} ({:.4} SOL)", label, event.sol_amount),
            strength: (requested / event.sol_amount.max(1e-9)).min(1.0),
        });
        info!(
            wallet = %label,
            mint = %event.mint,
            whale_sol = event.sol_amount,
            sized,
            mode = size.mode.as_str(),
            "mirroring whale buy"
        );

        // ---- 9. ownership --------------------------------------------------
        // Distributed ownership (Prompt 3 §F/§H): the whale trade arrives on
        // EVERY replica's feed connection — the claim on the logical identity
        // `copy:{wallet}:{mint}` elects exactly one executor. Losers skip
        // deterministically (§G); store failures fail closed (§K). The
        // process-local cooldown (mark_copied) stays as a rate guard, but the
        // claim is authoritative cross-replica.
        let mut permit = match bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("copy:{}:{}", event.leader, event.mint),
            "entry",
            "copy",
            "mirror",
            &event.mint,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                return self
                    .finish_rejected(
                        event,
                        Some(&rule),
                        Rejection::new(
                            RejectReason::OwnershipUnavailable,
                            CopyStage::RiskApproved,
                            format!("ownership store unavailable — failing closed: {e}"),
                        ),
                        &mut tl,
                    )
                    .await;
            }
        };
        if !permit.proceed() {
            debug!(wallet = %label, mint = %event.mint, "copy entry owned by another replica — skipping");
            return self
                .finish_rejected(
                    event,
                    Some(&rule),
                    Rejection::new(
                        RejectReason::OwnershipLost,
                        CopyStage::RiskApproved,
                        "entry owned by another replica".to_string(),
                    ),
                    &mut tl,
                )
                .await;
        }
        tl.mark("claimed");
        metrics::count_stage(CopyStage::OwnershipClaimed);

        // ---- 10. execute ---------------------------------------------------
        let res = self
            .buy(
                event,
                &rule,
                sized,
                slippage_pct,
                slippage_bps,
                &decision,
                &mut permit,
            )
            .await;
        match res {
            Err(e) => {
                // Pre-broadcast failure (quote/build/parse): nothing moved —
                // release so a redelivered event or later whale buy can proceed.
                permit.finish(false).await;
                self.state.note_failed_entry(&event.mint).await;
                self.finish_rejected(
                    event,
                    Some(&rule),
                    Rejection::new(
                        RejectReason::ExecutionFailed,
                        CopyStage::Submitted,
                        e.to_string(),
                    ),
                    &mut tl,
                )
                .await
            }
            Ok(report) if !report.filled => {
                self.state.note_failed_entry(&event.mint).await;
                let mut outcome = self
                    .finish_rejected(
                        event,
                        Some(&rule),
                        Rejection::new(
                            RejectReason::ExecutionFailed,
                            CopyStage::Submitted,
                            report.error.clone().unwrap_or_else(|| {
                                format!("copy order did not fill ({:?})", report.status)
                            }),
                        ),
                        &mut tl,
                    )
                    .await;
                outcome.intent_id = Some(report.intent_id);
                outcome.signature = report.signature;
                outcome
            }
            Ok(report) => {
                tl.mark("filled");
                let stage = if report.ambiguous {
                    CopyStage::Ambiguous
                } else {
                    CopyStage::Filled
                };
                metrics::count_stage(CopyStage::Submitted);
                metrics::count_stage(stage);
                let outcome = CopyOutcome {
                    event_id: event.event_id.clone(),
                    stage,
                    rejection: None,
                    intent_id: Some(report.intent_id.clone()),
                    position_id: report.position_id.clone(),
                    requested_sol: Some(requested),
                    sized_sol: Some(sized),
                    signature: report.signature.clone(),
                    total_ms: tl.total_ms(),
                };
                if let Some(pos_id) = &report.position_id {
                    let link = crate::recovery::link_for(
                        pos_id,
                        &event.leader,
                        &event.mint,
                        &event.event_id,
                        &event.signature,
                        Some(&report.intent_id),
                        event.token_amount,
                        report.qty,
                    );
                    if !self.store.upsert_link(link).await {
                        metrics::count_journal_error("upsert_link");
                    }
                }
                {
                    let mut reg = self.leaders.write().await;
                    reg.note_mirrored(&event.leader);
                }
                self.finish(event, Some(&rule), outcome, &tl, "entry").await
            }
        }
    }

    /// Mirror a leader's exit onto our position (`copy.mirror_exits`).
    pub(crate) async fn mirror_exit(
        &mut self,
        event: &LeaderTradeEvent,
        rule: &CopyWallet,
        plan: &ExitPlan,
        position: Position,
        tl: &mut LatencyTimeline,
    ) -> CopyOutcome {
        let label = crate::leader::label_for(rule);
        info!(
            wallet = %label,
            mint = %event.mint,
            fraction = plan.fraction,
            "mirroring whale exit"
        );
        // Distributed ownership (Prompt 3 §F/§H): every replica sees the
        // same whale exit on its own feed connection — exactly one may
        // sell our position. Fail closed on store errors: the position
        // stays under the sweeper's TP/SL/trailing management meanwhile.
        let mut permit = match bot_core::ownership::Permit::acquire(
            self.ownership.as_deref(),
            format!("exit:{}:mirror_exit", position.id),
            "exit",
            "copy",
            "mirror_exit",
            &position.symbol,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    position = %position.id,
                    error = %e,
                    "mirror-exit ownership unavailable — failing closed (sweeper still manages this position)"
                );
                return self
                    .finish_rejected(
                        event,
                        Some(rule),
                        Rejection::new(
                            RejectReason::OwnershipUnavailable,
                            CopyStage::PolicyPassed,
                            format!("ownership store unavailable — failing closed: {e}"),
                        ),
                        tl,
                    )
                    .await;
            }
        };
        if !permit.proceed() {
            debug!(position = %position.id, "mirror exit owned by another replica — skipping");
            return self
                .finish_rejected(
                    event,
                    Some(rule),
                    Rejection::new(
                        RejectReason::OwnershipLost,
                        CopyStage::PolicyPassed,
                        "exit owned by another replica".to_string(),
                    ),
                    tl,
                )
                .await;
        }
        tl.mark("claimed");
        metrics::count_stage(CopyStage::OwnershipClaimed);
        let res = crate::exit::sell_position(
            &self.state,
            &self.rpc,
            &self.wallet,
            &mut self.executor,
            &self.layouts,
            &self.risk,
            &position,
            plan.fraction,
            "whale exit (mirror)",
            None,
            self.intents.as_ref(),
            &mut permit,
        )
        .await;
        match res {
            Err(e) => {
                permit.finish(false).await;
                self.finish_rejected(
                    event,
                    Some(rule),
                    Rejection::new(
                        RejectReason::ExecutionFailed,
                        CopyStage::Submitted,
                        format!("mirrored exit failed: {e}"),
                    ),
                    tl,
                )
                .await
            }
            Ok(()) => {
                tl.mark("filled");
                metrics::count_stage(CopyStage::Submitted);
                metrics::count_stage(CopyStage::ExitMirrored);
                let still_open = self
                    .state
                    .find_open(BotModule::Copy, &event.mint)
                    .await
                    .is_some();
                if !still_open
                    && !self
                        .store
                        .close_link(
                            &position.id,
                            "closed",
                            Some(&event.event_id),
                            Some("leader exit mirrored"),
                        )
                        .await
                {
                    debug!(position = %position.id, "no open link to close for mirrored exit");
                }
                let outcome = CopyOutcome {
                    event_id: event.event_id.clone(),
                    stage: CopyStage::ExitMirrored,
                    rejection: None,
                    intent_id: None,
                    position_id: Some(position.id.clone()),
                    requested_sol: None,
                    sized_sol: None,
                    signature: None,
                    total_ms: tl.total_ms(),
                };
                self.finish(event, Some(rule), outcome, tl, "exit").await
            }
        }
    }

    /// Terminal bookkeeping for a refused / failed event.
    pub(crate) async fn finish_rejected(
        &mut self,
        event: &LeaderTradeEvent,
        rule: Option<&CopyWallet>,
        rejection: Rejection,
        tl: &mut LatencyTimeline,
    ) -> CopyOutcome {
        tl.mark("rejected");
        metrics::count_rejection(rejection.reason, rejection.stage);
        let outcome = CopyOutcome::rejected(&event.event_id, rejection, tl.total_ms());
        metrics::count_stage(outcome.stage);
        let kind = if event.is_buy() { "entry" } else { "exit" };
        self.finish(event, rule, outcome, tl, kind).await
    }

    /// Common terminal path: leader stats, durable journal, audit, latency.
    pub(crate) async fn finish(
        &mut self,
        event: &LeaderTradeEvent,
        rule: Option<&CopyWallet>,
        outcome: CopyOutcome,
        tl: &LatencyTimeline,
        kind: &str,
    ) -> CopyOutcome {
        let routine = outcome
            .rejection
            .as_ref()
            .map(|r| r.reason.is_routine())
            .unwrap_or(false);
        if rule.is_some() {
            if let Some(r) = &outcome.rejection {
                if r.reason != RejectReason::DuplicateEvent {
                    let mut reg = self.leaders.write().await;
                    reg.note_rejected(&event.leader, r.reason.as_str());
                }
            }
            // Duplicates and unknown leaders are not journaled: the first
            // delivery already is, and an unknown leader has no row.
            if outcome
                .rejection
                .as_ref()
                .map(|r| r.reason != RejectReason::DuplicateEvent)
                .unwrap_or(true)
            {
                let rec = crate::mirror::journal_record(event, &outcome);
                if !self.store.record_event(rec).await {
                    metrics::count_journal_error("record_event");
                }
                self.persist_leader(&event.leader).await;
            }
        }
        if rule.is_some() && !routine {
            match kind {
                "exit" => audit::publish_exit(&self.state, event, &outcome),
                _ => audit::publish_entry(&self.state, event, &outcome),
            }
        }
        tl.record();
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade() -> WalletTrade {
        WalletTrade {
            wallet: "LeaderWallet1111111111111111111111111111111".into(),
            signature: "5igNature111111111111111111111111111111111111111111111111111111".into(),
            slot: 42,
            block_time: Some(Utc::now() - Duration::seconds(2)),
            side: PositionSide::Long,
            mint: "Mint1111111111111111111111111111111111111111".into(),
            symbol: Some("TEST".into()),
            token_amount: 1_000.0,
            sol_amount: 0.5,
            venue: Venue::PumpFun,
            fee_sol: 0.00001,
            discriminator: None,
            observed_at: Utc::now(),
        }
    }

    #[test]
    fn event_id_is_deterministic_and_side_sensitive() {
        let t = trade();
        let a = LeaderTradeEvent::from_wallet_trade(&t, EventSource::PumpPortal, 1);
        let b = LeaderTradeEvent::from_wallet_trade(&t, EventSource::LogsPoll, 99);
        assert_eq!(
            a.event_id, b.event_id,
            "source/sequence must not change identity"
        );
        assert!(a.event_id.starts_with("cev_"));
        assert_eq!(a.event_id.len(), 4 + 32);
        let mut sell = t.clone();
        sell.side = PositionSide::Short;
        let c = LeaderTradeEvent::from_wallet_trade(&sell, EventSource::PumpPortal, 1);
        assert_ne!(a.event_id, c.event_id);
        assert_ne!(a.dedup_key(), c.dedup_key());
        assert!(a.dedup_key().starts_with("copy:5igNature"));
        assert!(a.dedup_key().ends_with(":buy"));
    }

    #[test]
    fn round_trips_to_wallet_trade() {
        let t = trade();
        let e = LeaderTradeEvent::from_wallet_trade(&t, EventSource::TransactionSubscribe, 7);
        let back = e.to_wallet_trade();
        assert_eq!(back.wallet, t.wallet);
        assert_eq!(back.signature, t.signature);
        assert_eq!(back.slot, t.slot);
        assert_eq!(back.mint, t.mint);
        assert_eq!(back.side, t.side);
        assert_eq!(back.sol_amount, t.sol_amount);
        assert_eq!(back.token_amount, t.token_amount);
        assert_eq!(back.venue, t.venue);
        assert_eq!(back.observed_at, t.observed_at);
        assert_eq!(back.block_time, t.block_time);
    }

    #[test]
    fn validation_names_each_defect() {
        let now = Utc::now();
        let base = LeaderTradeEvent::from_wallet_trade(&trade(), EventSource::Manual, 0);
        assert_eq!(base.validate(now), Ok(()));

        let mut e = base.clone();
        e.leader = "  ".into();
        assert_eq!(e.validate(now), Err(EventDefect::EmptyLeader));
        let mut e = base.clone();
        e.signature.clear();
        assert_eq!(e.validate(now), Err(EventDefect::EmptySignature));
        let mut e = base.clone();
        e.mint.clear();
        assert_eq!(e.validate(now), Err(EventDefect::EmptyMint));
        let mut e = base.clone();
        e.mint = e.leader.clone();
        assert_eq!(e.validate(now), Err(EventDefect::MintIsLeader));
        let mut e = base.clone();
        e.sol_amount = f64::NAN;
        assert_eq!(e.validate(now), Err(EventDefect::NonFiniteAmount));
        let mut e = base.clone();
        e.token_amount = f64::INFINITY;
        assert_eq!(e.validate(now), Err(EventDefect::NonFiniteAmount));
        let mut e = base.clone();
        e.sol_amount = -0.1;
        assert_eq!(e.validate(now), Err(EventDefect::NegativeAmount));
        let mut e = base.clone();
        e.sol_amount = 0.0;
        e.token_amount = 0.0;
        assert_eq!(e.validate(now), Err(EventDefect::ZeroSize));
        let mut e = base.clone();
        e.fee_sol = -1.0;
        assert_eq!(e.validate(now), Err(EventDefect::InvalidFee));
        let mut e = base.clone();
        e.block_time = Some(now + Duration::seconds(FUTURE_SKEW_TOLERANCE_SECS + 5));
        assert_eq!(e.validate(now), Err(EventDefect::FutureTimestamp));
        let mut e = base.clone();
        e.block_time = Some(now + Duration::seconds(FUTURE_SKEW_TOLERANCE_SECS - 5));
        assert_eq!(e.validate(now), Ok(()), "skew inside tolerance is fine");
        let mut e = base;
        e.venue = Venue::PolymarketClob;
        assert_eq!(e.validate(now), Err(EventDefect::ForeignVenue));
    }

    #[test]
    fn age_uses_chain_time_when_present() {
        let now = Utc::now();
        let mut e = LeaderTradeEvent::from_wallet_trade(&trade(), EventSource::Manual, 0);
        e.block_time = Some(now - Duration::seconds(40));
        e.observed_at = now - Duration::seconds(1);
        assert_eq!(e.age_secs(now), 40);
        assert!(e.is_stale(now, 30));
        assert!(!e.is_stale(now, 0), "0 disables the staleness check");
        assert_eq!(e.detection_lag_ms(), Some(39_000));
        e.block_time = None;
        assert_eq!(e.age_secs(now), 1);
        assert_eq!(e.detection_lag_ms(), None);
    }

    #[test]
    fn stage_and_reason_labels_are_unique_and_terminal_flags_hold() {
        let mut labels: Vec<&str> = CopyStage::ALL.iter().map(|s| s.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), CopyStage::ALL.len());
        let mut reasons: Vec<&str> = RejectReason::ALL.iter().map(|r| r.as_str()).collect();
        reasons.sort_unstable();
        reasons.dedup();
        assert_eq!(reasons.len(), RejectReason::ALL.len());
        assert!(CopyStage::Filled.is_terminal());
        assert!(CopyStage::Ambiguous.opened_exposure());
        assert!(!CopyStage::Rejected.opened_exposure());
        assert!(!CopyStage::Sized.is_terminal());
        let r = CopyOutcome::rejected(
            "cev_x",
            Rejection::new(RejectReason::ExecutionFailed, CopyStage::Submitted, "boom"),
            5,
        );
        assert_eq!(r.stage, CopyStage::Failed);
        let r = CopyOutcome::rejected(
            "cev_x",
            Rejection::new(RejectReason::StaleEvent, CopyStage::PolicyPassed, "old"),
            5,
        );
        assert_eq!(r.stage, CopyStage::Rejected);
        assert_eq!(r.reason_label(), "STALE_EVENT");
    }

    #[test]
    fn risk_codes_map_onto_pipeline_reasons() {
        use bot_core::risk::RiskCode;
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::KillSwitch)),
            RejectReason::KillSwitch
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::CopyEmergencyDisabled)),
            RejectReason::StrategyDisabled
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::CopyCooldown)),
            RejectReason::CopyCooldown
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::CopyLeaderExposure)),
            RejectReason::LeaderExposure
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::StaleSignal)),
            RejectReason::StaleEvent
        );
        for c in [
            RiskCode::MaxOpenPositions,
            RiskCode::ExposureCap,
            RiskCode::DailyLossLimit,
            RiskCode::CopyDailyLoss,
            RiskCode::CopyPendingCap,
        ] {
            assert_eq!(
                RejectReason::from_risk_code(Some(c)),
                RejectReason::ExposureLimit
            );
        }
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::SlippageCap)),
            RejectReason::SlippageLimit
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::InsufficientBalance)),
            RejectReason::RiskRejected
        );
        assert_eq!(
            RejectReason::from_risk_code(None),
            RejectReason::RiskRejected
        );
    }

    #[test]
    fn source_mapping_and_execution_gate() {
        assert_eq!(
            EventSource::from_feed("pumpportal"),
            EventSource::PumpPortal
        );
        assert_eq!(EventSource::from_feed("LOGS_POLL"), EventSource::LogsPoll);
        assert_eq!(
            EventSource::from_feed("transaction_subscribe"),
            EventSource::TransactionSubscribe
        );
        assert_eq!(
            EventSource::from_feed("something-else"),
            EventSource::Manual
        );
        assert!(!EventSource::Replay.may_execute());
        assert!(EventSource::PumpPortal.may_execute());
        for s in [
            EventSource::PumpPortal,
            EventSource::LogsPoll,
            EventSource::TransactionSubscribe,
            EventSource::Replay,
            EventSource::Manual,
        ] {
            assert_eq!(EventSource::parse(s.as_str()), Some(s));
        }
    }
}
